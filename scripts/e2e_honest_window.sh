#!/usr/bin/env bash
# Profiled single-window cycle: boot fresh, warm up with a 12x12 window,
# DRAIN fully (stable fill count + forceload release + settle), then reset,
# arm the honest window, forceload the measured 12x12 and snapshot stats.
set -uo pipefail
REPO=/home/z/my-project/c-faster
SRV=/home/z/my-project/server263
JAVA=/home/z/tools/jdk-25.0.5.1+2/bin/java
RCON="python3 $REPO/harness/rcon.py localhost 25576 crussty"
W0X="${W0X:-3000}"; W0Z="${W0Z:-3000}"
# cont-156: SO_DIR selects the native build dir (release = fat-LTO prod,
# relfast = local iteration profile); both legs of an A/B must share one.
SO_DIR="${SO_DIR:-$REPO/native/target/release}"

cd "$SRV"
pkill -f 'paper-26\.3' 2>/dev/null && sleep 2 || true

cp "$REPO/plugin/CrusstyGen.jar" plugins/CrusstyGen.jar
if [ "${SKIP_SETUP:-0}" != "1" ]; then
  rm -rf world world_nether world_the_end
fi
mkdir -p world/datapacks
cp -f "$REPO/server_datapacks/01_tectonic.zip" world/datapacks/01_tectonic.dpz
cp -f "$REPO/server_datapacks/02_terralith.zip" world/datapacks/02_terralith.dpz

WT="${CRUS_WORKER_THREADS:-$(( $(nproc) + 1 ))}"
PGEN_YML=config/paper-global.yml
[ -f "$PGEN_YML" ] && sed -i -E "s/^([[:space:]]*)worker-threads:.*$/\1worker-threads: ${WT}/" "$PGEN_YML"

rm -f run.in; mkfifo run.in
( sleep 28800 > run.in ) >/dev/null 2>&1 &
echo $! > fifo.pid

# NOTE: env vars must stay on the SAME continuation block as setsid (a blank
# line after a backslash ends the command and the vars never reach java).
CRUS_NATIVE_CARVERS=1 CRUS_CARVE_IN_FILL=1 CRUS_NATIVE_DECO=1 CRUS_BIOME_GRID=1 \
CRUS_PROFILE="${PROF:-1}" CRUS_FILL_THREADS="${FT:-1}" CRUS_COL_CACHE="${CC:-}" \
CRUS_LANES="${LN:-}" CRUS_F32NOISE="${F32:-}" \
  setsid "$JAVA" -Xms2G -Xmx2G ${JVM_FLAGS_ADD:-} -Djava.library.path="$SO_DIR" \
    -jar "$SRV/paper.jar" nogui < run.in > run.out 2>&1 &
echo $! > server.pid

DONE=0
for i in $(seq 1 240); do
  grep -q "Done (" run.out 2>/dev/null && { DONE=1; break; }
  kill -0 "$(cat server.pid)" 2>/dev/null || { echo "SERVER DIED"; tail -20 run.out; exit 1; }
  sleep 1
done
echo "--- boot Done=$DONE after ${i}s ---"

chunk_count() {
  $RCON crusgenstats 2>/dev/null | grep -o "chunks=[0-9]*" | head -1 | cut -d= -f2
}

# warmup: 12x12 window far from the measured one
WX1=$((W0X - 500)); WZ1=$((W0Z - 500))
$RCON "forceload add $((WX1*16)) $((WZ1*16)) $(((WX1+11)*16+15)) $(((WZ1+11)*16+15))" >/dev/null
for i in $(seq 1 300); do
  c=$(chunk_count)
  [ -n "$c" ] && [ "$c" -ge 150 ] 2>/dev/null && break
  sleep 0.3
done
# drain: wait until the global fill count is stable for 6 polls (~3s)
STABLE=0; LAST=$(chunk_count)
for i in $(seq 1 200); do
  sleep 0.5
  c=$(chunk_count)
  if [ -n "$c" ] && [ "$c" = "$LAST" ]; then
    STABLE=$((STABLE + 1))
    [ "$STABLE" -ge 6 ] && break
  else
    STABLE=0
    LAST=$c
  fi
done
echo "--- drained after ${i} polls (count=$LAST stable=$STABLE) ---"
# release the warmup tickets + settle
$RCON "forceload remove all" >/dev/null
sleep 3

# reset + arm window, then forceload the measured window
$RCON "crusgenstats reset" >/dev/null
$RCON "crusgenstats window $W0X $W0Z 11" >/dev/null
t0=$(date +%s.%N)
$RCON "forceload add $((W0X*16)) $((W0Z*16)) $(((W0X+11)*16+15)) $(((W0Z+11)*16+15))" >/dev/null
# poll on the WINDOW counter (in-rect fills only — immune to backlog);
# "wincount" is the light poll command (window_inside=...)
for i in $(seq 1 600); do
  w=$($RCON "crusgenstats wincount" 2>/dev/null | grep -o "window_inside=[0-9]*" | cut -d= -f2)
  [ -n "$w" ] && [ "$w" -ge 144 ] 2>/dev/null && break
  sleep 0.25
done
t1=$(date +%s.%N)
python3 -c "print(f'window wall: {$t1-$t0:.3f}s = ({$t1-$t0})*1000/144 ms/ch')"
sleep 3  # let deco drain
echo "--- stats (window armed) ---"
$RCON crusgenstats 2>/dev/null | head -4
echo stop > run.in 2>/dev/null; sleep 5; kill -9 "$(cat server.pid)" 2>/dev/null
echo "--- cycle complete ---"
