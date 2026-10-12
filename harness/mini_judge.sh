#!/usr/bin/env bash
# Mini judge: boot Paper with CrusstyGen, forceload ONLY the 4 pocket chunks
# (judge-style batch), settle 15s, dump the region, stop. Isolates the
# batch-generation fluid-sim effect vs the sequential oracle walk.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
LEVEL="${ORACLE_LEVEL:-minijudge}"

mkdir -p server/plugins
rm -f server/plugins/CrusstyOracle.jar
cp plugin/CrusstyGen.jar server/plugins/
cp probe/CrusstyProbe.jar server/plugins/ 2>/dev/null || true
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
java -Xms512M -Xmx1536M \
  -Djava.library.path="$ROOT/native/target/release" \
  -jar paper.jar nogui < run.in > run.out 2>&1 &
echo $! > server.pid
cd "$ROOT"
echo "mini-judge started pid=$(cat server/server.pid)"

for i in $(seq 1 300); do
  if grep -q 'Done (' server/run.out 2>/dev/null; then break; fi
  if ! kill -0 "$(cat server/server.pid)" 2>/dev/null; then
    echo "SERVER DIED" >&2; tail -30 server/run.out >&2; exit 1
  fi
  sleep 2
done
echo "server up"

rcon() { python3 harness/rcon.py 127.0.0.1 25575 crussty "$1"; }
# start rcon listener? rcon is enabled via bootstrap props
rcon "forceload add 32 -48 63 -17" || echo "forceload issued"
# settle like the judge
sleep 15
rcon "crusgenstats" | tail -2 || true
mkdir -p minijudge_out
rm -rf minijudge_out/*
rcon "crusstydumpregion 2 -3 3 -2 $ROOT/minijudge_out chunk" || true
ls minijudge_out/ | head
bash harness/server_down.sh || true
echo "=== mini-judge done ==="
