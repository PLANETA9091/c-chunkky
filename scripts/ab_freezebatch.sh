#!/usr/bin/env bash
# cont-157 paired A/B: freeze_top_layer batched JNI crossing
#   leg PERCOL = pre-batch jar (per-column lane, 256 crossings/run)
#   leg BATCH  = freezeChecksBatch jar (1 crossing/run)
# Same .so (relfast) both legs; 4 boots ABAB, same window rect, fresh world.
set -uo pipefail
SRV=/home/z/my-project/server263
REPO=/home/z/my-project/c-faster
JAVA=/home/z/tools/jdk-25.0.5.1+2/bin/java
RCON="python3 $REPO/harness/rcon.py localhost 25576 crussty"
WX=${WX:-3000}; WZ=${WZ:-3000}
TAGOUT=/home/z/my-project/scripts/logs/freeze_ab
mkdir -p "$TAGOUT"
JAR_PERCOL=/tmp/CrusstyGen_prebatch.jar
JAR_BATCH="$REPO/plugin/CrusstyGen.jar"
SO_DIR="$REPO/native/target/relfast"

run_boot() { # $1 = leg tag, $2 = jar path
  local tag="$1" jar="$2"
  cd "$SRV"
  pkill -f 'paper-26\.3' 2>/dev/null; sleep 1
  rm -rf world world_nether world_the_end
  mkdir -p world/datapacks
  cp -f "$REPO/server_datapacks/01_tectonic.zip" world/datapacks/01_tectonic.dpz
  cp -f "$REPO/server_datapacks/02_terralith.zip" world/datapacks/02_terralith.dpz
  cp -f "$jar" plugins/CrusstyGen.jar
  local WT=$(( $(nproc) + 1 ))
  sed -i -E "s/^([[:space:]]*)worker-threads:.*$/\1worker-threads: ${WT}/" config/paper-global.yml 2>/dev/null
  rm -f run.in; mkfifo run.in
  ( sleep 28800 > run.in ) >/dev/null 2>&1 &
  CRUS_NATIVE_CARVERS=1 CRUS_CARVE_IN_FILL=1 CRUS_NATIVE_DECO=1 CRUS_BIOME_GRID=1 CRUS_FILL_THREADS=2 \
    setsid $JAVA -Xms2G -Xmx3G -Djava.library.path="$SO_DIR" \
      -jar paper.jar nogui < run.in > "$TAGOUT/run_$tag.out" 2>&1 &
  echo $! > server.pid
  local DONE=0
  for i in $(seq 1 240); do
    grep -q "Done (" "$TAGOUT/run_$tag.out" 2>/dev/null && { DONE=1; break; }
    kill -0 "$(cat server.pid)" 2>/dev/null || { echo "$tag DIED"; tail -5 "$TAGOUT/run_$tag.out"; break; }
    sleep 1
  done
  [ "$DONE" = 1 ] || { echo "$tag: boot failed"; return 1; }
  # warm 12x12 + drain
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
  # honest window
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
  python3 -c "print(f'$tag window @${WX},${WZ}: {($t1-$t0)*1000/144:.2f} ms/ch (poll-wall)')"
  sleep 2
  $RCON crusgenstats 2>/dev/null | grep -o "honest window: [^|]*" | head -1 | sed "s/^/$tag /" | cut -c1-220
  $RCON crusgenstats 2>/dev/null | grep -o "native deco stage[^|]*" | head -1 | sed "s/^/$tag /" | cut -c1-160
  echo stop > run.in 2>/dev/null; sleep 4
  kill -9 "$(cat server.pid)" 2>/dev/null
  sleep 1
}

echo "=== ABAB: PERCOL / BATCH / BATCH / PERCOL ==="
run_boot A1_percol "$JAR_PERCOL"
run_boot B1_batch  "$JAR_BATCH"
run_boot B2_batch  "$JAR_BATCH"
run_boot A2_percol "$JAR_PERCOL"
echo "--- A/B complete ---"
