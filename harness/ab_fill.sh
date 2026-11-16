#!/usr/bin/env bash
# Paired A/B: CRUS_COVER_STAGE=0 vs 1 — margin staging lever on current defaults.
# Each leg: wipe, boot, warmup 8x8 @14700, 2x 12x12 windows @16000/16100.
set -uo pipefail
ROOT=/home/z/my-project/c-faster
SRV=$ROOT/server263
JAVA=/home/z/my-project/tools/jdk-25.0.5+8/bin/java
cd "$SRV"
RCON="python3 $ROOT/harness/rcon.py localhost 25576 crussty"
leg() {
  local stage="$1"
  pgrep -f 'paper-26\.3|paper\.jar' >/dev/null && { pkill -9 -f paper; sleep 2; }
  rm -rf world world_nether world_the_end
  cp "$ROOT/plugin/CrusstyGen.jar" plugins/CrusstyGen.jar
  rm -f run.in; mkfifo run.in
  ( sleep 28800 > run.in ) >/dev/null 2>&1 & echo $! > fifo.pid
  env CRUS_NATIVE_DECO=1 CRUS_NATIVE_CARVERS=1 CRUS_BIOME_GRID=1 CRUS_FILL_THREADS=2 \
      CRUS_COVER_STAGE="$stage" CRUS_FILL_THREADS="$stage" \
    setsid $JAVA -Xms2G -Xmx3G -Ddisable.watchdog=true \
      -Djava.library.path="$ROOT/native/target/release" \
      -jar paper.jar nogui < run.in > run_ab.out 2>&1 & echo $! > server.pid
  for i in $(seq 1 60); do grep -q "Done (" run_ab.out 2>/dev/null && break; sleep 2; done
  grep -q "Done (" run_ab.out || { echo "BOOT FAILED stage=$stage"; return 1; }
  $RCON "forceload add $((14000*16)) $((14000*16)) $((14007*16+15)) $((14007*16+15))" >/dev/null
  for i in $(seq 1 120); do s=$($RCON crusgenstats 2>/dev/null | grep -o "chunks=[0-9]*" | head -1 | cut -d= -f2); [ -n "$s" ] && [ "$s" -ge 64 ] && break; sleep 0.5; done
  $RCON "forceload remove all" >/dev/null
  for w in 0 1; do
    local rx=$((14600 + w*100))
    $RCON "crusgenstats window $rx 14600 11" >/dev/null
    local before=$($RCON crusgenstats 2>/dev/null | grep -o "chunks=[0-9]*" | head -1 | cut -d= -f2)
    local t0=$(date +%s.%N)
    $RCON "forceload add $((rx*16)) $((14600*16)) $(((rx+11)*16+15)) $((14611*16+15))" >/dev/null
    local target2=$(( before + 144 ))
    local i c
    for i in $(seq 1 300); do
      c=$($RCON crusgenstats 2>/dev/null | grep -o "chunks=[0-9]*" | head -1 | cut -d= -f2)
      [ -n "$c" ] && [ "$c" -ge "$target2" ] && break
      sleep 0.25
    done
    local t1=$(date +%s.%N)
    python3 -c "print(f'stage=$stage w$w: {float('$t1')-float('$t0'):.2f}s = {(float('$t1')-float('$t0'))*1000/144:.2f} ms/ch')"
    $RCON crusgenstats 2>/dev/null | grep -o "e2e=[0-9.]+ms/chunk[^ ]* carver=[0-9.]*\|margin_fills=[0-9]* margin_fill_cpu=[0-9.]*\|busy=[0-9.]*ms/chunk cover=[0-9.]*" | head -3
    $RCON "forceload remove all" >/dev/null
  done
  echo stop > run.in; sleep 2; pkill -9 -f 'paper-26\.3' 2>/dev/null; rm -f run.in
}
STAGE="${1:?stage 0|1}"
leg "F${STAGE}"
echo "AB LEG COMPLETE"
