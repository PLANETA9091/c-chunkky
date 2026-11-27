#!/usr/bin/env bash
# Paired A/B: CRUS_FILL_THREADS=1 vs 2, fresh world each boot, SAME window
# coords. Warmup + drain, then one measured 12x12 window. Honest window armed.
set -uo pipefail
SRV=/home/z/my-project/server263
JAVA=/home/z/tools/jdk-25.0.5.1+2/bin/java
RCON="python3 /home/z/my-project/c-faster/harness/rcon.py localhost 25576 crussty"
WX=${WX:-3500}; WZ=${WZ:-3500}

run_boot() {
  local ft=$1 tag=$2
  cd "$SRV"
  pkill -f 'paper-26\.3' 2>/dev/null; sleep 1
  rm -rf world world_nether world_the_end
  mkdir -p world/datapacks
  cp -f /home/z/my-project/c-faster/server_datapacks/01_tectonic.zip world/datapacks/01_tectonic.dpz
  cp -f /home/z/my-project/c-faster/server_datapacks/02_terralith.zip world/datapacks/02_terralith.dpz
  rm -f run.in; mkfifo run.in
  ( sleep 28800 > run.in ) >/dev/null 2>&1 &
  CRUS_NATIVE_CARVERS=1 CRUS_CARVE_IN_FILL=1 CRUS_NATIVE_DECO=1 CRUS_BIOME_GRID=1 \
CRUS_FILL_THREADS=$ft CRUS_LIGHT_DIVERT="${LD:-0}" \
    setsid $JAVA -Xms2G -Xmx3G -Djava.library.path=/home/z/my-project/c-faster/native/target/release \
      -jar paper.jar nogui < run.in > run_$tag.out 2>&1 &
  echo $! > server.pid
  for i in $(seq 1 240); do
    grep -q "Done (" run_$tag.out 2>/dev/null && break
    kill -0 "$(cat server.pid)" 2>/dev/null || { echo "$tag DIED"; break; }
    sleep 1
  done
  # warm 10x10 + drain
  $RCON "forceload add $(( (WX-40)*16 )) $(( (WZ-40)*16 )) $(( (WX-29)*16+15 )) $(( (WZ-29)*16+15 ))" >/dev/null
  local LAST=0 STABLE=0 c
  for i in $(seq 1 200); do
    sleep 0.5
    c=$($RCON crusgenstats 2>/dev/null | grep -o "chunks=[0-9]*" | head -1 | cut -d= -f2)
    if [ -n "$c" ] && [ "$c" = "$LAST" ]; then
      STABLE=$((STABLE+1)); [ "$STABLE" -ge 5 ] && break
    else STABLE=0; LAST=$c; fi
  done
  $RCON "forceload remove all" >/dev/null
  sleep 2
  # measured window
  $RCON "crusgenstats reset" >/dev/null
  $RCON "crusgenstats window $WX $WZ 11" >/dev/null
  local t0 t1
  t0=$(date +%s.%N)
  $RCON "forceload add $((WX*16)) $((WZ*16)) $(( (WX+11)*16+15 )) $(( (WZ+11)*16+15 ))" >/dev/null
  for i in $(seq 1 400); do
    c=$($RCON "crusgenstats wincount" 2>/dev/null | grep -o "window_inside=[0-9]*" | cut -d= -f2)
    [ -n "$c" ] && [ "$c" -ge 144 ] 2>/dev/null && break
    sleep 0.25
  done
  t1=$(date +%s.%N)
  python3 -c "print(f'FT=$ft window @${WX},${WZ}: {$t1-$t0:.3f}s / 144 = ({$t1-$t0})*1000/144 ms/ch')"
  sleep 2
  $RCON crusgenstats 2>/dev/null | grep -o "inside=144[^|]*" | head -1 | cut -c1-260
  echo stop > run.in 2>/dev/null; sleep 4
  kill -9 "$(cat server.pid)" 2>/dev/null
  sleep 1
}

run_boot 1 FT1_LD0
LD=1 run_boot 2 FT2_LD1
echo "--- A/B complete ---"
