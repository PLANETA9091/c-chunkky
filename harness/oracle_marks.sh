#!/usr/bin/env bash
# Vanilla aquifer-marks oracle: boot a PURE VANILLA Paper server with ONLY the
# oracle plugin; it runs the real vanilla doFill loop for the requested chunks
# and dumps (a) every fluid/flagged block with shouldScheduleFluidUpdate,
# (b) the full aquifer status cache (source locations + FluidStatus per cell).
#
# Output: oracle_marks_out/chunk_X_Y_marks.txt (flags) + aqcache section at EOF.
#
# Env:
#   ORACLE_CHUNKLIST   "cx:cz,cx:cz,..." (default "2:-2,2:-3,3:-2,3:-3")
#   ORACLE_LEVEL       level-name (default oraclemarks)
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
LEVEL="${ORACLE_LEVEL:-oraclemarks}"
CHUNKLIST="${ORACLE_CHUNKLIST:-2:-2,2:-3,3:-2,3:-3}"

# --- build oracle ------------------------------------------------------------
chmod +x oracle/build.sh
bash oracle/build.sh

# --- deploy: oracle ONLY (pure vanilla generation) ---------------------------
mkdir -p server/plugins
rm -f server/plugins/CrusstyGen.jar server/plugins/CrusstyProbe.jar
cp oracle/CrusstyOracle.jar server/plugins/
rm -f server/plugins/*.jar.disabled

# --- config ------------------------------------------------------------------
: > server/bukkit.yml
sed -i "s/^level-name=.*/level-name=$LEVEL/; s/^level-seed=.*/level-seed=3257840388504953787/; s/^allow-nether=.*/allow-nether=false/" server/server.properties
grep -qE "^allow-nether=false" server/server.properties || echo "allow-nether=false" >> server/server.properties
rm -rf "server/$LEVEL" "server/${LEVEL}_nether" "server/${LEVEL}_the_end"

# --- boot (custom launch: -Doraclemarks + chunklist) --------------------------
cd server
if pgrep -f 'java .*-jar .*paper\.jar' >/dev/null 2>&1; then
  echo "ERROR: paper already running" >&2; exit 1
fi
rm -f run.in
mkfifo run.in
( sleep 28800 > run.in ) &
echo $! > fifo.pid
java -Xms512M -Xmx1536M \
  -Doraclemarks=true \
  -Doraclemarks.chunklist="$CHUNKLIST" \
  -jar paper.jar nogui < run.in > run.out 2>&1 &
echo $! > server.pid
cd "$ROOT"
echo "oracle server started pid=$(cat server/server.pid)"

# --- wait for oracle completion ----------------------------------------------
UP=0
for i in $(seq 1 300); do
  if ! kill -0 "$(cat server/server.pid)" 2>/dev/null; then
    echo "SERVER DIED during startup; last log lines:" >&2
    tail -40 server/run.out >&2
    exit 1
  fi
  if grep -q 'OracleNoiseStage done' server/run.out; then UP=1; break; fi
  sleep 2
done
if [ "$UP" != "1" ]; then
  echo "TIMEOUT waiting for oracle" >&2; tail -40 server/run.out >&2
  bash harness/server_down.sh || true
  exit 1
fi
grep -E "noise stage dumped|OracleNoiseStage done" server/run.out | tail -8

# --- shutdown + collect -------------------------------------------------------
bash harness/server_down.sh || true
mkdir -p oracle_marks_out
cp server/oracle_noisestage/*_marks.txt oracle_marks_out/ 2>/dev/null || true
ls -la oracle_marks_out/
echo "=== oracle marks summary ==="
for f in oracle_marks_out/*_marks.txt; do
  total=$(grep -vc '^AQ' "$f" || true)
  marked=$(grep -v '^AQ' "$f" | grep -c ' true$' || true)
  aq=$(grep -c '^AQ' "$f" || true)
  echo "$f: fluid_blocks=$total marked_true=$marked aqcache=$aq"
done
