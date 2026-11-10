#!/usr/bin/env bash
# Start the Paper 26.3 server (server263/) with the fresh native fill engine.
# 26.3 requires JDK 25 (Temurin 25.0.5+8).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SRV="$ROOT/server263"
JAVA=/home/z/tools/jdk-25.0.5+8/bin/java
cd "$SRV"

if pgrep -f 'java .*paper-26\.3.*\.jar' >/dev/null 2>&1; then
  echo "ERROR: a 26.3 server instance is already running" >&2
  exit 1
fi

# cont-103: production default — pin chunk-system worker-threads. Paper auto
# = 1 thread at <=3 PHYSICAL cores (2-vCPU/4-thread cloud runners included),
# which serializes the gen pipeline (maxconc=1; measured +9.3ms/ch e2e on
# 4 vCPU vs pinned-4, same runner CI 41588638543). Pin to all logical cores;
# override with CRUS_WORKER_THREADS (= -1 restores Paper auto).
WT="${CRUS_WORKER_THREADS:-$(nproc)}"
PGEN_YML="config/paper-global.yml"
if [ -f "$PGEN_YML" ] && ! grep -qE "^[[:space:]]*worker-threads:[[:space:]]*${WT}[[:space:]]*$" "$PGEN_YML"; then
  sed -i -E "s/^([[:space:]]*)worker-threads:.*$/\1worker-threads: ${WT}/" "$PGEN_YML"
  echo "chunk-system.worker-threads=${WT} (patched $PGEN_YML; CRUS_WORKER_THREADS=-1 restores auto)"
fi

# deploy fresh plugin + native visibility
cp "$ROOT/plugin/CrusstyGen.jar" plugins/CrusstyGen.jar
NATIVE_SO="$ROOT/native/target/release/libcrussty_chunkgen.so"
[ -f "$NATIVE_SO" ] || { echo "missing $NATIVE_SO" >&2; exit 1; }
{
  echo "# deployed263.md5 $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  md5sum plugins/CrusstyGen.jar "$NATIVE_SO"
} > deployed263.md5
echo "--- deployed263.md5 ---"; cat deployed263.md5

# FIFO stdin holder (Paper exits on stdin EOF)
rm -f run.in
mkfifo run.in
( sleep 28800 > run.in ) >/dev/null 2>&1 &
echo $! > fifo.pid

HEAP_ARGS="-Xms2G -Xmx3G"
# cont-134 (paper-levers audit): PGEN_FLAG (-Dpaper.chunk-system.gen-parallelism)
# REMOVED — the key was dropped in Paper 26.3 (string survives only in
# RemovedConfigurations; nothing reads the property). Was a dead no-op.

# opt-in native carvers pass-through
CRUS_NATIVE_CARVERS="${CRUS_NATIVE_CARVERS:-0}" CRUS_NATIVE_DECO="${CRUS_NATIVE_DECO:-0}" \
echo "booting Paper 26.3 (JDK 25)..."
CRUS_NATIVE_CARVERS="${CRUS_NATIVE_CARVERS:-0}" CRUS_NATIVE_DECO="${CRUS_NATIVE_DECO:-0}" \
setsid $JAVA $HEAP_ARGS \
  -XX:FlightRecorderOptions=stackdepth=128 \
  -Djava.library.path="$ROOT/native/target/release" \
  -jar paper-26.3-222.jar nogui < run.in > run.out 2>&1 &
echo $! > server.pid
echo "server263 pid: $(cat server.pid)"
