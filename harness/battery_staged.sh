#!/usr/bin/env bash
# battery_staged.sh — Q6 staged-window battery: window-size scaling of e2e wall.
# Boots ONE server, then forceloads fresh windows of increasing size at
# distinct virgin regions, capturing the honest-window line for each.
# Usage: bash /home/z/my-project/scripts/battery_staged.sh
set -uo pipefail
ROOT=/home/z/my-project/c-faster
SRV=$ROOT/server263
JAVA=/home/z/my-project/tools/jdk-25.0.5+8/bin/java
cd "$SRV"
RCON="python3 $ROOT/harness/rcon.py localhost 25576 crussty"

pgrep -f 'paper-26\.3|paper\.jar' >/dev/null && { echo "server already running"; exit 1; }

# fresh world for deterministic virgin regions
rm -rf world world_nether world_the_end
cp "$ROOT/plugin/CrusstyGen.jar" plugins/CrusstyGen.jar
rm -f run.in
mkfifo run.in
( sleep 28800 > run.in ) >/dev/null 2>&1 &
echo $! > fifo.pid

env CRUS_NATIVE_DECO=1 CRUS_NATIVE_CARVERS=1 CRUS_BIOME_GRID=1 CRUS_FILL_THREADS=2 \
  setsid $JAVA -Xms2G -Xmx3G -Ddisable.watchdog=true \
  -Djava.library.path="$ROOT/native/target/release" \
  -jar paper.jar nogui < run.in > run2.out 2>&1 &
echo $! > server.pid

for i in $(seq 1 120); do
  grep -q "Done (" run2.out 2>/dev/null && break
  sleep 2
done
grep -q "Done (" run2.out || { echo "BOOT FAILED"; tail -5 run2.out; exit 1; }
echo "BOOTED"

# warmup 8x8 (also JIT-warms the fill path) — virgin region
$RCON "forceload add $((14700*16)) $((14700*16)) $((14707*16+15)) $((14707*16+15))" >/dev/null
for i in $(seq 1 120); do
  s=$($RCON crusgenstats 2>/dev/null | grep -o "chunks=[0-9]*" | head -1 | cut -d= -f2)
  [ -n "$s" ] && [ "$s" -ge 64 ] && break
done
echo "warmup=$s"
$RCON "forceload remove all" >/dev/null

# battery: label n rx rz  (n => (n+1)^2 chunks; >256 chunks split into
# quadrant forceload commands — vanilla caps forceload at 256 chunks/cmd)
run_win() { # label n rx rz
  local label="$1" n="$2" rx="$3" rz="$4"
  $RCON "crusgenstats window $rx $rz $n" >/dev/null
  local before=$($RCON crusgenstats 2>/dev/null | grep -o "chunks=[0-9]*" | head -1 | cut -d= -f2)
  local t0=$(date +%s.%N)
  if [ "$n" -le 15 ]; then
    $RCON "forceload add $((rx*16)) $((rz*16)) $(((rx+n)*16+15)) $(((rz+n)*16+15))" >/dev/null
  else
    local midx=$(( (rx + n/2) * 16 )) midz=$(( (rz + n/2) * 16 ))
    $RCON "forceload add $((rx*16)) $((rz*16)) $((midx-1)) $((midz-1))" >/dev/null
    $RCON "forceload add $((midx)) $((rz*16)) $(((rx+n)*16+15)) $((midz-1))" >/dev/null
    $RCON "forceload add $((rx*16)) $((midz)) $((midx-1)) $(((rz+n)*16+15))" >/dev/null
    $RCON "forceload add $((midx)) $((midz)) $(((rx+n)*16+15)) $(((rz+n)*16+15))" >/dev/null
  fi
  local target=$(( (n+1)*(n+1) ))
  local target2=$(( before + target ))
  local i c
  for i in $(seq 1 400); do
    c=$($RCON crusgenstats 2>/dev/null | grep -o "chunks=[0-9]*" | head -1 | cut -d= -f2)
    [ -n "$c" ] && [ "$c" -ge "$target2" ] && break
    sleep 0.25
  done
  local t1=$(date +%s.%N)
  local nch=$(( (n+1)*(n+1) ))
  python3 -c "print(f'$label: {float('$t1')-float('$t0'):.2f}s / $nch chunks = {(float('$t1')-float('$t0'))*1000/$nch:.2f} ms/ch')"
  $RCON crusgenstats 2>/dev/null | grep -o "honest window.*" | cut -c1-400
  $RCON "forceload remove all" >/dev/null
}

# 4x4/8x8/12x12 measured earlier (probe+anchor)


run_win "24x24" 23 15400 15400
run_win "12x12b" 11 15200 15200

# graceful stop
echo stop > run.in
sleep 3
pkill -9 -f 'paper-26\.3' 2>/dev/null
rm -f run.in
echo "BATTERY COMPLETE"
