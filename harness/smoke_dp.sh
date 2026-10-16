#!/usr/bin/env bash
# Quick smoke: boot Paper with CrusstyGen on a datapack world, force far
# chunks, verify the native datapack ctx was built (no CHIMERA fallback).
# Usage: timeout 280 bash harness/smoke_dp.sh <world> [n_chunks_per_axis]
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WORLD="${1:-dptt}"
N="${2:-2}"
cd "$ROOT"

bash harness/server_down.sh >/dev/null 2>&1 || true
pkill -9 -f 'paper\.jar' 2>/dev/null || true
sleep 1

mkdir -p server/plugins
rm -f server/plugins/CrusstyOracle.jar
cp plugin/CrusstyGen.jar server/plugins/
cp probe/CrusstyProbe.jar server/plugins/ 2>/dev/null || true
cp native/target/release/libcrussty_chunkgen.so server/plugins/ 2>/dev/null || true

python3 - "$WORLD" <<'PY'
import sys
world = sys.argv[1]
open("server/bukkit.yml", "w").write(f"worlds:\n  {world}:\n    generator: CrusstyGen\n")
PY
sed -i "s/^level-name=.*/level-name=$WORLD/; s/^allow-nether=.*/allow-nether=false/" server/server.properties

cd server
rm -f run.in
mkfifo run.in
( sleep 2400 > run.in ) >/dev/null 2>&1 &
echo $! > fifo.pid
java -Xms512M -Xmx1536M \
  -Djava.library.path="$ROOT/native/target/release" \
  -jar paper.jar nogui < run.in > run.out 2>&1 &
echo $! > server.pid
cd "$ROOT"

UP=0
for i in $(seq 1 75); do
  if grep -q 'Done (' server/run.out 2>/dev/null; then UP=1; break; fi
  if ! kill -0 "$(cat server/server.pid)" 2>/dev/null; then
    echo "SMOKE: SERVER DIED during boot"; tail -20 server/run.out; exit 1
  fi
  sleep 2
done
[ "$UP" = "1" ] || { echo "SMOKE: boot timeout"; tail -10 server/run.out; exit 1; }
echo "SMOKE: server up"

rcon() { python3 harness/rcon.py 127.0.0.1 25575 crussty "$1" 2>/dev/null; }

# force N x N chunks FAR from spawn (fresh generation, never on disk)
FB=$((1000 * 16))
rcon "forceload add $FB $FB $((FB + N * 16 - 1)) $((FB + N * 16 - 1))" >/dev/null || true
for i in $(seq 1 40); do
  ST="$(rcon "crusgenstats" | head -1)"
  echo "$ST" | grep -qE "chunks=[0-9]+" && break
  sleep 2
done
echo "--- crusgenstats ---"
rcon "crusgenstats" || true

echo "--- ctx/pack logs ---"
grep -E "ctx pool|datapack worldgen|CHIMERA|native datapack" server/run.out | tail -5 || echo "(no pack logs!)"

bash harness/server_down.sh >/dev/null 2>&1 || true
echo "SMOKE DONE"
