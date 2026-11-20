#!/usr/bin/env bash
# b2_measure.sh — cont-148 follow-up: remeasure the carver apply lever in the
# anchor zone (@14600 / @14900, the cont-147 region) on THIS box.
# Boots ONE server (defaults cont-147: F2/deco/carvers/biome-grid), then
# forceloads two virgin 12x12 windows, capturing the honest-window line +
# carver-phases for each. Java dies with the call — one shot.
set -uo pipefail
ROOT=/home/z/my-project/c-faster
SRV=/home/z/my-project/server263
JAVA=/home/z/tools/jdk-25.0.5+8/bin/java
cd "$SRV"   # HARD RULE: java MUST start with CWD=$SRV263 (eula.txt read)
RCON="python3 $ROOT/harness/rcon.py localhost 25576 crussty"

pgrep -f 'paper-26\.3|paper\.jar' >/dev/null && { echo "server already running"; exit 1; }

# fresh world for deterministic virgin regions
rm -rf world world_nether world_the_end
cp "$ROOT/plugin/CrusstyGen.jar" plugins/CrusstyGen.jar
rm -f run.in run2.out
mkfifo run.in
( sleep 28800 > run.in ) >/dev/null 2>&1 &
echo $! > fifo.pid

env CRUS_NATIVE_DECO=1 CRUS_NATIVE_CARVERS=1 CRUS_BIOME_GRID=1 CRUS_FILL_THREADS=2 \
  setsid $JAVA -Xms768M -Xmx1536M -Ddisable.watchdog=true \
  -Djava.library.path="$ROOT/native/target/release" \
  -jar paper.jar nogui < run.in > run2.out 2>&1 &
echo $! > server.pid

for i in $(seq 1 120); do
  grep -q "Done (" run2.out 2>/dev/null && break
  sleep 2
done
grep -q "Done (" run2.out || { echo "BOOT FAILED"; tail -8 run2.out; exit 1; }
echo "BOOTED"
# plugin sanity: bulk/carver lanes alive
grep -E "bulk=|carver" run2.out | head -4

run_win() { # label n rx rz
  local label="$1" n="$2" rx="$3" rz="$4"
  $RCON "crusgenstats window $rx $rz $n" >/dev/null
  sleep 1
  local before=$($RCON wincount 2>/dev/null | grep -oE "window_inside=[0-9]+" | grep -oE "[0-9]+")
  [ -z "$before" ] && before=0
  local t0=$(date +%s.%N)
  local midx=$(( (rx + n/2) * 16 )) midz=$(( (rz + n/2) * 16 ))
  $RCON "forceload add $((rx*16)) $((rz*16)) $((midx-1)) $((midz-1))" >/dev/null
  $RCON "forceload add $((midx)) $((rz*16)) $(((rx+n)*16+15)) $((midz-1))" >/dev/null
  $RCON "forceload add $((rx*16)) $((midz)) $((midx-1)) $(((rz+n)*16+15))" >/dev/null
  $RCON "forceload add $((midx)) $((midz)) $(((rx+n)*16+15)) $(((rz+n)*16+15))" >/dev/null
  local target=$(( (n+1)*(n+1) ))
  local target2=$(( before + target ))
  python3 "$ROOT/harness/winpoll.py" localhost 25576 crussty "$target2" 150 || true
  local t1=$(date +%s.%N)
  local nch=$(( (n+1)*(n+1) ))
  python3 -c "print(f'$label: {float('$t1')-float('$t0'):.2f}s / $nch chunks = {(float('$t1')-float('$t0'))*1000/$nch:.2f} ms/ch wall (ramp incl)')"
  $RCON crusgenstats 2>/dev/null | grep -oE "(honest window|carver-phases)[^|]*" | cut -c1-460
  $RCON "forceload remove all" >/dev/null
  sleep 2
}

# warmup 8x8 (JIT) — virgin
$RCON "forceload add $((14700*16)) $((14700*16)) $((14707*16+15)) $((14707*16+15))" >/dev/null
for i in $(seq 1 120); do
  c=$($RCON wincount 2>/dev/null | grep -oE "window_inside=[0-9]+" | grep -oE "[0-9]+")
  [ -n "$c" ] && [ "$c" -ge 64 ] && break
  sleep 0.5
done
echo "warmup done"
$RCON "forceload remove all" >/dev/null
sleep 2

run_win "8x8@14600" 7 14594 14594
run_win "8x8@14900" 7 14894 14894

echo stop > run.in
sleep 3
pkill -9 -f 'paper-26\.3' 2>/dev/null
rm -f run.in
echo "B2 MEASURE COMPLETE"
