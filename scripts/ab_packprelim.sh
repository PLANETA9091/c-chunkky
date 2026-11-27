#!/usr/bin/env bash
# cont-156 paired A/B: pack preliminary_surface_level binding
#   leg SCAN = CRUS_PACK_PRELIM=0 (legacy top-down scan)
#   leg DF   = default (pack-shipped find_top_surface DF)
# 4 boots in ABBA order, same window rect, fresh world per boot,
# warmup 12x12 + drain, honest window armed (reset + wincount poll).
set -uo pipefail
SRV=/home/z/my-project/server263
REPO=/home/z/my-project/c-faster
JAVA=/home/z/tools/jdk-25.0.5.1+2/bin/java
RCON="python3 $REPO/harness/rcon.py localhost 25576 crussty"
WX=${WX:-3000}; WZ=${WZ:-3000}
TAGOUT=/home/z/my-project/scripts/logs/packprelim_ab
mkdir -p "$TAGOUT"

run_boot() { # $1 = leg tag, $2 = extra env "K=V" or ""
  local tag="$1" extra="$2"
  cd "$SRV"
  pkill -f 'paper-26\.3' 2>/dev/null; sleep 1
  rm -rf world world_nether world_the_end
  mkdir -p world/datapacks
  cp -f "$REPO/server_datapacks/01_tectonic.zip" world/datapacks/01_tectonic.dpz
  cp -f "$REPO/server_datapacks/02_terralith.zip" world/datapacks/02_terralith.dpz
  # WT = nproc+1 (cont-155 champion), FT2
  local WT=$(( $(nproc) + 1 ))
  sed -i -E "s/^([[:space:]]*)worker-threads:.*$/\1worker-threads: ${WT}/" config/paper-global.yml 2>/dev/null
  rm -f run.in; mkfifo run.in
  ( sleep 28800 > run.in ) >/dev/null 2>&1 &
  # $extra propagates as env prefix on the java line (CRUS_PACK_PRELIM=...)
  CRUS_NATIVE_CARVERS=1 CRUS_CARVE_IN_FILL=1 CRUS_NATIVE_DECO=1 CRUS_BIOME_GRID=1 CRUS_FILL_THREADS=2 $extra \
    setsid $JAVA -Xms2G -Xmx3G -Djava.library.path=$REPO/native/target/release \
      -jar paper.jar nogui < run.in > "$TAGOUT/run_$tag.out" 2>&1 &
  echo $! > server.pid
  local DONE=0
  for i in $(seq 1 240); do
    grep -q "Done (" "$TAGOUT/run_$tag.out" 2>/dev/null && { DONE=1; break; }
    kill -0 "$(cat server.pid)" 2>/dev/null || { echo "$tag DIED"; tail -5 "$TAGOUT/run_$tag.out"; break; }
    sleep 1
  done
  [ "$DONE" = 1 ] || { echo "$tag: boot failed"; return 1; }
  # warm 12x12 @ (WX-40, WZ-40) + drain
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
  python3 -c "print(f'$tag window @${WX},${WZ}: {$t1-$t0:.3f}s / 144 = {($t1-$t0)*1000/144:.2f} ms/ch')"
  sleep 2
  $RCON crusgenstats 2>/dev/null | head -4 | sed "s/^/$tag stats: /" | cut -c1-240
  echo stop > run.in 2>/dev/null; sleep 4
  kill -9 "$(cat server.pid)" 2>/dev/null
  sleep 1
}

echo "=== ABBA: SCAN(0) / DF(1) / DF(1) / SCAN(0) ==="
run_boot A0_scan "CRUS_PACK_PRELIM=0" && echo "A0 done" >> "$TAGOUT/progress.txt"
run_boot B1_df "" && echo "B1 done" >> "$TAGOUT/progress.txt"
run_boot B2_df "" && echo "B2 done" >> "$TAGOUT/progress.txt"
run_boot A2_scan "CRUS_PACK_PRELIM=0" && echo "A2 done" >> "$TAGOUT/progress.txt"
echo "--- A/B complete ---"
