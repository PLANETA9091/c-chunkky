#!/usr/bin/env bash
# carver_probe.sh — cont-151: single ON-leg boot (native carvers) over the
# ab263 carvers-pair region (@16384, N=8) to capture the carver-phase
# counters (opsApplied/declined/applyFailed/fuse/uniform swaps) that the
# A/B gate dump does not carry. Diagnostic companion to ab263_ci.sh.
set -uo pipefail
ROOT=/home/z/my-project/c-faster
SRV=/home/z/my-project/server263
JAVA=/home/z/tools/jdk-25.0.5+8/bin/java
RCON="python3 $ROOT/harness/rcon.py localhost 25576 crussty"
RX=${RX:-16384}; RZ=${RZ:-16384}; N=${N:-8}

pgrep -f 'paper-26\.3|paper\.jar' >/dev/null && { echo "server already running"; exit 1; }

cd "$SRV"   # HARD RULE: java MUST start with CWD=$SRV (eula.txt read)
rm -rf world world_nether world_the_end
cp "$ROOT/plugin/CrusstyGen.jar" plugins/CrusstyGen.jar
cat > bukkit.yml <<'EOF'
worlds:
  world:
    generator: CrusstyGen
EOF
grep -q "eula=true" eula.txt 2>/dev/null || echo "eula=true" > eula.txt
rm -f run.in
mkfifo run.in
( sleep 3600 > run.in ) >/dev/null 2>&1 &

env CRUS_NATIVE_DECO=0 CRUS_NATIVE_CARVERS=1 \
  setsid $JAVA -Xms2G -Xmx3G \
  -Djava.library.path="$ROOT/native/target/release" \
  -jar paper.jar nogui < run.in > run_probe.out 2>&1 &

for i in $(seq 1 90); do
  grep -q "Done (" run_probe.out 2>/dev/null && break
  sleep 2
done
grep -q "Done (" run_probe.out || { echo "BOOT FAILED"; tail -10 run_probe.out; exit 1; }
grep -q "providing CrusstyChunkGenerator" run_probe.out || { echo "WIRING FAILED (vanilla world)"; exit 1; }
grep -q "native carvers active" run_probe.out || echo "WARN: native carvers flag line absent"
echo "BOOTED (native carvers ON, wired)"

$RCON "forceload add $RX $RZ $((RX + N * 16 - 1)) $((RZ + N * 16 - 1))" >/dev/null
sleep 45
$RCON crusgenstats > carver_probe_genstats.txt 2>/dev/null || true
echo "--- genstats (honest/carver lines) ---"
grep -oE "(honest window|carver-phases|bulk)[^|]*" carver_probe_genstats.txt | cut -c1-420
echo "--- boot sanity ---"
grep -E "bulk=|carver|swap|uniform" run_probe.out | head -6

echo stop > run.in
sleep 3
pkill -9 -f 'paper-26\.3|paper\.jar' 2>/dev/null
rm -f run.in
echo "PROBE COMPLETE"
