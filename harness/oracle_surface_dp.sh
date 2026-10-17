#!/usr/bin/env bash
# Oracle SURFACE-stage capture WITH datapacks (Terralith+Tectonic, pack content
# untouched): boots a PURE VANILLA Paper server with ONLY the oracle plugin,
# walks the requested chunks through NOISE->SURFACE (statusWalk) and dumps the
# SURFACEBLOCKS section — the ground truth for the native surface engine
# (native/src/bin/sfcmp.rs).
#
# Env:
#   SURFACE_CHUNKLIST  "cx:cz,..." (default "0:0,-1:-1")
#   SURFACE_LEVEL      level-name (default dpttoracle)
#   SURFACE_DP_DIR     datapacks to wire into the fresh world
#                      (default server_datapacks; empty = vanilla)
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
LEVEL="${SURFACE_LEVEL:-dpttoracle}"
CHUNKLIST="${SURFACE_CHUNKLIST:-0:0,-1:-1}"
DPDIR="${SURFACE_DP_DIR:-server_datapacks}"

chmod +x oracle/build.sh
bash oracle/build.sh

mkdir -p server/plugins
if [ "${KEEP_CRUSSTY:-0}" = "1" ]; then
  # co-boot: CrusstyGen stays installed (A/B probes of OUR pipeline through
  # the oracle statuswalk); bukkit.yml keeps its worlds->generator mapping.
  cp plugin/CrusstyGen.jar server/plugins/CrusstyGen.jar
  printf 'worlds:\n  %s:\n    generator: CrusstyGen\n' "$LEVEL" > server/bukkit.yml
else
  rm -f server/plugins/CrusstyGen.jar server/plugins/CrusstyProbe.jar
  : > server/bukkit.yml
fi
cp oracle/CrusstyOracle.jar server/plugins/
sed -i "s/^level-name=.*/level-name=$LEVEL/; s/^level-seed=.*/level-seed=3257840388504953787/; s/^allow-nether=.*/allow-nether=false/" server/server.properties
grep -qE "^allow-nether=false" server/server.properties || echo "allow-nether=false" >> server/server.properties
rm -rf "server/$LEVEL" "server/${LEVEL}_nether" "server/${LEVEL}_the_end"
rm -rf server/oracle_statuswalk server/oracle_noisestage
mkdir -p server/oracle_statuswalk server/oracle_noisestage
# ORACLE_MODE=statuswalk (default, SURFACEBLOCKS dump) | run (noise blocks + -Doraclebiomes)
ORACLE_MODE="${ORACLE_MODE:-statuswalk}"

if [ "$DPDIR" != "" ]; then
  mkdir -p "server/$LEVEL/datapacks"
  cp -r "$DPDIR"/"." "server/$LEVEL/datapacks/"
  echo "datapacks wired:"
  ls "server/$LEVEL/datapacks"
fi

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
  -Doraclewalk=$([ "$ORACLE_MODE" = statuswalk ] && echo true || echo false) \
  -Doraclemarks=$([ "$ORACLE_MODE" = statuswalk ] && echo false || echo true) \
  -Doraclebiomes=$([ "$ORACLE_MODE" = run ] && echo true || echo false) \
  -Doraclemarks.chunklist="$CHUNKLIST" \
  -Doracleprobe.col="${ORACLEPROBE_COL:-}" \
  -jar paper.jar nogui < run.in > run.out 2>&1 &
echo $! > server.pid
cd "$ROOT"
echo "surface oracle server started pid=$(cat server/server.pid)"

UP=0
for i in $(seq 1 300); do
  if ! kill -0 "$(cat server/server.pid)" 2>/dev/null; then
    echo "SERVER DIED; last log:" >&2; tail -40 server/run.out >&2; exit 1
  fi
  if grep -q 'OracleNoiseStage done' server/run.out; then UP=1; break; fi
  sleep 2
done
if [ "$UP" != "1" ]; then echo "TIMEOUT" >&2; tail -40 server/run.out >&2; bash harness/server_down.sh || true; exit 1; fi

grep -E "walk .* (status|surface blocks)" server/run.out 2>/dev/null | tail -12 || true
bash harness/server_down.sh || true
if [ "$ORACLE_MODE" = statuswalk ]; then
  mkdir -p oracle_surface_out
  rm -rf oracle_surface_out/*
  cp server/oracle_statuswalk/walk_*.txt oracle_surface_out/ 2>/dev/null || true
  echo "collected: $(ls oracle_surface_out)"
  exit 0
fi
mkdir -p oracle_biome_out
rm -rf oracle_biome_out/*
cp server/oracle_noisestage/biomes_*.txt oracle_biome_out/ 2>/dev/null || true
cp server/oracle_noisestage/chunk_*.txt oracle_biome_out/ 2>/dev/null || true
echo "collected: $(ls oracle_biome_out)"
exit 0
