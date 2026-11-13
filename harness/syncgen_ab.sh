#!/usr/bin/env bash
# syncgen_ab.sh (cont-139): A/B the cont-138 SyncGen inline status-chain path.
#   A: pipeline honest window (forceload 12x12 @ 2100,2000) via e2e263.sh
#   B: /crussyncgen 2100 2000 11 (main-thread inline chain) + 2nd fresh rect
# Same rect across phases, WIPE before each boot, worker=2 FILL=2 no-watchdog
# (final config from PROGRESS.json E7 matrix), pinned md5s logged.
set -uo pipefail
ROOT=/home/z/my-project/c-faster
SRV="$ROOT/server263"
PH="${1:?usage: syncgen_ab.sh A|B}"
RX=2100; RZ=2000; N=11
export JAVA=/home/z/tools/jdk-25.0.5+8/bin/java
cd "$SRV"

pgrep -f 'java .*paper-26\.3.*\.jar' >/dev/null 2>&1 && { echo "ERROR: server already running"; exit 1; }

sed -i -E "s/^([[:space:]]*)worker-threads:.*$/\1worker-threads: 2/" config/paper-global.yml
rm -rf world world_nether world_the_end
cp "$ROOT/plugin/CrusstyGen.jar" plugins/CrusstyGen.jar
{ echo "# deployed263.md5 $(date -u +%Y-%m-%dT%H:%M:%SZ) phase=$PH"
  md5sum plugins/CrusstyGen.jar "$ROOT/native/target/release/libcrussty_chunkgen.so"; } > deployed263.md5
cat deployed263.md5

rm -f run.in; mkfifo run.in
( sleep 28800 > run.in ) >/dev/null 2>&1 & echo $! > fifo.pid
env CRUS_NATIVE_DECO=1 CRUS_NATIVE_CARVERS=1 CRUS_FILL_THREADS=2 ${EXTRA_ENV:-} \
  setsid $JAVA -Xms2G -Xmx3G -Ddisable.watchdog=true \
  -Djava.library.path="$ROOT/native/target/release" \
  -jar paper.jar nogui < run.in > run.out 2>&1 &
SPID=$!; echo $SPID > server.pid
RCON="python3 $ROOT/harness/rcon.py localhost 25576 crussty"

up=0
for i in $(seq 1 120); do
  grep -q 'Done (' run.out 2>/dev/null && { up=1; break; }
  kill -0 $SPID 2>/dev/null || { echo "SERVER DIED"; tail -20 run.out; exit 1; }
  sleep 2
done
[ "$up" = 1 ] || { echo "BOOT TIMEOUT"; tail -20 run.out; exit 1; }
grep -m1 "native height service ready" run.out || echo "WARN: no native bridge line"

# warmup 8x8 @ (500,500) — fresh region
$RCON "forceload add $((500*16)) $((500*16)) $((507*16+15)) $((507*16+15))" >/dev/null 2>&1
s=""
for i in $(seq 1 120); do
  s=$($RCON crusgenstats 2>/dev/null | grep -o "chunks=[0-9]*" | head -1 | cut -d= -f2)
  [ -n "$s" ] && [ "$s" -ge 64 ] && break
  sleep 1
done
echo "warmup: chunks=${s:-0}"
$RCON "forceload remove all" >/dev/null 2>&1; sleep 2

if [ "$PH" = "A" ]; then
  $RCON "crusgenstats window $RX $RZ $N" >/dev/null 2>&1
  bash "$ROOT/harness/e2e263.sh" "$RX" "$RZ" "$N" 2>&1 | grep -E "window:|chunks="
  echo "--- window line ---"
  $RCON crusgenstats 2>&1 | grep -Ei 'window' | head -3
else
  $RCON "crusgenstats reset" >/dev/null 2>&1
  echo "--- syncgen rect1 ($RX,$RZ) 12x12 ---"
  $RCON "crussyncgen $RX $RZ $N" 2>&1 | tail -2
  sleep 2; grep '\[syncgen\]' run.out | tail -2
  echo "--- syncgen rect2 (2200,2000) 12x12 ---"
  $RCON "crussyncgen 2200 2000 11" 2>&1 | tail -2
  sleep 2; grep '\[syncgen\]' run.out | tail -2
  echo "--- genstats ---"
  $RCON crusgenstats 2>&1 | head -6
fi

$RCON "forceload remove all" >/dev/null 2>&1
echo stop > run.in; sleep 3
kill -0 $SPID 2>/dev/null && sleep 5
pkill -9 -f 'paper-26\.3' 2>/dev/null || true
rm -f run.in
echo "PHASE $PH COMPLETE"
