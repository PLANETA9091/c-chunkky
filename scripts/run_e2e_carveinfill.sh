#!/usr/bin/env bash
# ONE-CALL E2E cycle for the carve-in-fill lane (worklog rule: the whole
# server cycle lives in a single tool call — the sandbox reaps background
# processes at call end).
#   boot (fresh world + datapacks) -> Done -> 3 honest 12x12 windows
#   -> crusgenstats -> stop.
# Env: W0X/W0Z (window origin, default 2000/2000), SKIP_SETUP=1 to keep world.
set -uo pipefail
REPO=/home/z/my-project/c-faster
SRV=/home/z/my-project/server263
JAVA=/home/z/tools/jdk-25.0.5+8/bin/java
RCON="python3 $REPO/harness/rcon.py localhost 25576 crussty"
W0X="${W0X:-2000}"; W0Z="${W0Z:-2000}"

cd "$SRV"

# 0) no leftover server
pkill -f 'paper-26\.3' 2>/dev/null && sleep 2 || true

# 1) deploy the fresh plugin + native visibility
cp "$REPO/plugin/CrusstyGen.jar" plugins/CrusstyGen.jar
echo "--- deployed md5 ---"
md5sum plugins/CrusstyGen.jar "$REPO/native/target/release/libcrussty_chunkgen.so"

# 2) fresh world + datapacks — named .dpz: the VANILLA server ignores unknown
#    extensions (26.3 cannot parse the 1.21.x pack registries — auto-enable
#    kills the boot); the PLUGIN reads .dpz (cont-152).
if [ "${SKIP_SETUP:-0}" != "1" ]; then
  rm -rf world world_nether world_the_end
fi
mkdir -p world/datapacks
if [ "${PACKS:-1}" = "1" ]; then
  cp -f "$REPO/server_datapacks/01_tectonic.zip" world/datapacks/01_tectonic.dpz
  cp -f "$REPO/server_datapacks/02_terralith.zip" world/datapacks/02_terralith.dpz
fi
ls -la world/datapacks/

# 3) chunk-system worker threads = logical cores (cont-103 pin)
WT="$(nproc)"
PGEN_YML=config/paper-global.yml
if [ -f "$PGEN_YML" ]; then
  sed -i -E "s/^([[:space:]]*)worker-threads:.*$/\1worker-threads: ${WT}/" "$PGEN_YML"
fi

# 4) FIFO stdin holder (Paper exits on stdin EOF)
rm -f run.in
mkfifo run.in
( sleep 28800 > run.in ) >/dev/null 2>&1 &
echo $! > fifo.pid

# 5) boot
echo "booting Paper 26.3 (JDK 25, packs + carve-in-fill + native deco)..."
CRUS_NATIVE_CARVERS=1 CRUS_CARVE_IN_FILL=${CARVE_FILL:-1} CRUS_NATIVE_DECO=${DECO_ON:-1} CRUS_BIOME_GRID=1 \
setsid "$JAVA" -Xms2G -Xmx3G \
  -XX:FlightRecorderOptions=stackdepth=128 \
  -Djava.library.path="$REPO/native/target/release" \
  -jar "$SRV/paper.jar" nogui < run.in > run.out 2>&1 &
echo $! > server.pid

# 6) wait for Done (240s cap — pack registry parse on 2 cores)
DONE=0
for i in $(seq 1 240); do
  if grep -q "Done (" run.out 2>/dev/null; then DONE=1; break; fi
  if ! kill -0 "$(cat server.pid)" 2>/dev/null; then echo "SERVER DIED"; break; fi
  sleep 1
done
echo "--- boot evidence (Done=$DONE after ${i}s) ---"
grep -E "providing CrusstyChunkGenerator|datapack worldgen assets|carver: registry|ctx pool|CrusstyGen enabled|surface263" run.out | head -12
if [ "$DONE" != "1" ]; then
  tail -30 run.out
  echo stop > run.in 2>/dev/null; sleep 6; kill -9 "$(cat server.pid)" 2>/dev/null
  exit 1
fi

# 7) honest 12x12 windows (fresh regions, spaced 60 chunks apart)
window() {
  local rx=$1 rz=$2
  local before target c t0 t1 elapsed
  before=$($RCON crusgenstats 2>/dev/null | grep -o "chunks=[0-9]*" | head -1 | cut -d= -f2)
  [ -z "$before" ] && before=0
  t0=$(date +%s.%N)
  $RCON "forceload add $((rx*16)) $((rz*16)) $(((rx+11)*16 + 15)) $(((rz+11)*16 + 15))" >/dev/null
  target=$((before + 144))
  for i in $(seq 1 600); do
    c=$($RCON crusgenstats 2>/dev/null | grep -o "chunks=[0-9]*" | head -1 | cut -d= -f2)
    [ -n "$c" ] && [ "$c" -ge "$target" ] 2>/dev/null && break
    sleep 0.25
  done
  t1=$(date +%s.%N)
  elapsed=$(python3 -c "print(f'{$t1-$t0:.3f}')")
  python3 -c "print(f'window @${rx},${rz}: {$elapsed}s / 144 chunks = {float('$elapsed')*1000/144:.2f} ms/ch')"
}
window "$W0X" "$W0Z"
window "$W0X" "$((W0Z + 60))"
window "$((W0X + 60))" "$W0Z"

# 8) stats snapshot
echo "--- crusgenstats ---"
$RCON crusgenstats 2>/dev/null | head -6
echo "--- carver log lines ---"
grep -E "carver|carve" run.out | grep -v "at \[" | head -8

# 9) clean stop
echo stop > run.in 2>/dev/null
sleep 6
kill -9 "$(cat server.pid)" 2>/dev/null
echo "--- cycle complete ---"
