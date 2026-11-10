#!/usr/bin/env bash
# run263_baseline.sh — boot server263, warmup, N e2e windows, stats, stop.
# MUST run inside a single tool call (sandbox kills background procs).
set -uo pipefail
ROOT=/home/z/my-project/c-faster
cd "$ROOT/server263"
export JAVA=/home/z/tools/jdk-25.0.5+8/bin/java

RX1=${RX1:-2000}; RZ1=${RZ1:-2000}
WX=${WX:-500}; WZ=${WZ:-500}
NW=${NW:-2}
FILL_THREADS=${FILL_THREADS:-2}
EXTRA_ENV=${EXTRA_ENV:-}
# WIPE=1: rm world dirs first — REQUIRED for same-region config A/B
# (fresh generation is deterministic per seed+coords, saved-chunk reload is not)
if [ "${WIPE:-0}" = "1" ]; then
  rm -rf world world_nether world_the_end
  echo "world wiped for clean A/B"
fi
# NOTE: every run MUST use fresh (never-generated) warmup+window regions:
# re-forceload of saved chunks produces 0 native fills and the chunk-count
# polls hang (E1 lesson, cont-136).

# deploy fresh plugin + native (deploy hygiene: md5 on record)
cp "$ROOT/plugin/CrusstyGen.jar" plugins/CrusstyGen.jar
{
  echo "# deployed263.md5 $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  md5sum plugins/CrusstyGen.jar "$ROOT/native/target/release/libcrussty_chunkgen.so"
} > deployed263.md5
cat deployed263.md5

rm -f run.in; mkfifo run.in
( sleep 28800 > run.in ) >/dev/null 2>&1 &
echo $! > fifo.pid

HEAP="-Xms2G -Xmx3G"
echo "booting with CRUS_NATIVE_DECO=1 CRUS_NATIVE_CARVERS=1 CRUS_FILL_THREADS=$FILL_THREADS $EXTRA_ENV"
env CRUS_NATIVE_DECO=1 CRUS_NATIVE_CARVERS=1 CRUS_FILL_THREADS="$FILL_THREADS" $EXTRA_ENV \
  setsid $JAVA $HEAP \
  -Djava.library.path="$ROOT/native/target/release" \
  -jar paper-26.3-222.jar nogui < run.in > run.out 2>&1 &
SPID=$!
echo $SPID > server.pid

RCON="python3 $ROOT/harness/rcon.py localhost 25576 crussty"
t0=$(date +%s)
up=0
for i in $(seq 1 120); do
  if grep -q 'Done (' run.out 2>/dev/null; then up=1; break; fi
  if ! kill -0 $SPID 2>/dev/null; then echo "SERVER DIED"; tail -20 run.out; exit 1; fi
  sleep 2
done
[ "$up" = "1" ] || { echo "BOOT TIMEOUT"; tail -20 run.out; exit 1; }
t1=$(date +%s)
echo "boot: $((t1-t0))s"

# native bridge + generator sanity (fail fast if plugin missing)
grep -m1 "native height service ready" run.out || { echo "NO NATIVE BRIDGE"; tail -5 run.out; }
grep -m2 "CrusstyGen" run.out | head -2 || { echo "GENERATOR NOT WIRED"; exit 1; }

# warmup 8x8 @ (WX,WZ) — fresh region
$RCON "forceload add $((WX*16)) $((WZ*16)) $(( (WX+7)*16+15 )) $(( (WZ+7)*16+15 ))" >/dev/null 2>&1
for i in $(seq 1 120); do
  s=$($RCON crusgenstats 2>/dev/null | grep -o "chunks=[0-9]*" | head -1 | cut -d= -f2)
  [ -n "$s" ] && [ "$s" -ge 64 ] && break
  sleep 1
done
echo "warmup done: chunks=$s"

# measured windows
$RCON "forceload remove all" >/dev/null 2>&1; sleep 2
for w in $(seq 0 $((NW-1))); do
  rx=$((RX1 + w*100)); rz=$RZ1
  bash "$ROOT/harness/e2e263.sh" "$rx" "$rz" 11 2>&1 | grep -E "window:|chunks="
done

# final stats
echo "--- crusgenstats ---"
$RCON crusgenstats 2>&1 | head -12

# graceful stop
$RCON "forceload remove all" >/dev/null 2>&1
echo stop > run.in
sleep 3
kill -0 $SPID 2>/dev/null && sleep 5
pkill -9 -f 'paper-26\.3' 2>/dev/null
rm -f run.in
echo "BASELINE RUN COMPLETE"
