#!/usr/bin/env bash
# Status-walk on OUR generator (crussty plugin deployed): same dumps as
# oracle_walk.sh but with our noise stage — for mark-list diffing.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
LEVEL="${ORACLE_LEVEL:-oraclewalk}"
CHUNKLIST="${ORACLE_CHUNKLIST:-2:-2,2:-3,3:-2,3:-3}"

# native + plugin must be built
[ -f native/target/release/libcrussty_chunkgen.so ] || { echo "no .so" >&2; exit 1; }

mkdir -p server/plugins
cp plugin/CrusstyGen.jar server/plugins/
cp native/target/release/libcrussty_chunkgen.so server/plugins/ 2>/dev/null || true

python3 - "$LEVEL" <<'PY'
import sys
world = sys.argv[1]
open("server/bukkit.yml", "w").write(f"worlds:\n  {world}:\n    generator: CrusstyGen\n")
PY
sed -i "s/^level-name=.*/level-name=$LEVEL/; s/^level-seed=.*/level-seed=3257840388504953787/; s/^allow-nether=.*/allow-nether=false/" server/server.properties
rm -rf "server/$LEVEL" "server/${LEVEL}_nether" "server/${LEVEL}_the_end"

cd server
if pgrep -f 'java .*-jar .*paper\.jar' >/dev/null 2>&1; then
  echo "ERROR: paper already running" >&2; exit 1
fi
rm -f run.in
mkfifo run.in
( sleep 28800 > run.in ) &
echo $! > fifo.pid
NATIVE_DIR="$ROOT/native/target/release"
java -Xms512M -Xmx1536M \
  -Djava.library.path="$NATIVE_DIR" \
  -Doraclewalk=true \
  -Doraclemarks.chunklist="$CHUNKLIST" \
  -jar paper.jar nogui < run.in > run.out 2>&1 &
echo $! > server.pid
cd "$ROOT"
echo "our-world statuswalk started pid=$(cat server/server.pid)"

UP=0
for i in $(seq 1 300); do
  if ! kill -0 "$(cat server/server.pid)" 2>/dev/null; then
    echo "SERVER DIED; last log:" >&2; tail -40 server/run.out >&2; exit 1
  fi
  if grep -q 'OracleNoiseStage done' server/run.out; then UP=1; break; fi
  sleep 2
done
if [ "$UP" != "1" ]; then echo "TIMEOUT" >&2; tail -40 server/run.out >&2; bash harness/server_down.sh || true; exit 1; fi
grep -E "walk .* status=" server/run.out | tail -6
grep -E "beardifier bridge|marks" server/run.out | grep -i crussty | tail -3
bash harness/server_down.sh || true
mkdir -p oracle_walk_ours
rm -rf oracle_walk_ours/*
cp server/oracle_statuswalk/*.txt oracle_walk_ours/ 2>/dev/null || true
echo "=== ours vs vanilla walk ==="
for f in oracle_walk_ours/walk_*.txt; do
  echo "--- $(basename $f) (ours)"; grep '^STATUS' "$f"
done
