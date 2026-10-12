#!/usr/bin/env bash
# Status-walk oracle: walk the 4 pocket chunks through NOISE/SURFACE/CARVERS/
# FEATURES/FULL on a PURE VANILLA server, dumping cumulative ProtoChunk
# postProcessing lists at each status.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
LEVEL="${ORACLE_LEVEL:-oraclewalk}"
CHUNKLIST="${ORACLE_CHUNKLIST:-2:-2,2:-3,3:-2,3:-3}"

chmod +x oracle/build.sh
bash oracle/build.sh

mkdir -p server/plugins
rm -f server/plugins/CrusstyGen.jar server/plugins/CrusstyProbe.jar
cp oracle/CrusstyOracle.jar server/plugins/

: > server/bukkit.yml
sed -i "s/^level-name=.*/level-name=$LEVEL/; s/^level-seed=.*/level-seed=3257840388504953787/; s/^allow-nether=.*/allow-nether=false/" server/server.properties
grep -qE "^allow-nether=false" server/server.properties || echo "allow-nether=false" >> server/server.properties
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
  -Doraclewalk=true \
  -Doraclemarks.chunklist="$CHUNKLIST" \
  -jar paper.jar nogui < run.in > run.out 2>&1 &
echo $! > server.pid
cd "$ROOT"
echo "statuswalk server started pid=$(cat server/server.pid)"

UP=0
for i in $(seq 1 300); do
  if ! kill -0 "$(cat server/server.pid)" 2>/dev/null; then
    echo "SERVER DIED; last log:" >&2; tail -40 server/run.out >&2; exit 1
  fi
  if grep -q 'OracleNoiseStage done' server/run.out; then UP=1; break; fi
  sleep 2
done
if [ "$UP" != "1" ]; then echo "TIMEOUT" >&2; tail -40 server/run.out >&2; bash harness/server_down.sh || true; exit 1; fi

grep -E "walk .* status=" server/run.out | tail -12
bash harness/server_down.sh || true
mkdir -p oracle_walk_out
rm -rf oracle_walk_out/*
cp server/oracle_statuswalk/*.txt oracle_walk_out/ 2>/dev/null || true
ls -la oracle_walk_out/
echo "=== per-status cumulative marks ==="
for f in oracle_walk_out/walk_*.txt; do
  echo "--- $(basename $f)"
  grep '^STATUS' "$f"
done
