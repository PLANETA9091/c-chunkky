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
PGEN_FLAG="-Dpaper.chunk-system.gen-parallelism=false"

# opt-in native carvers pass-through
CRUS_NATIVE_CARVERS="${CRUS_NATIVE_CARVERS:-0}" CRUS_NATIVE_DECO="${CRUS_NATIVE_DECO:-0}" \
echo "booting Paper 26.3 (JDK 25)..."
CRUS_NATIVE_CARVERS="${CRUS_NATIVE_CARVERS:-0}" CRUS_NATIVE_DECO="${CRUS_NATIVE_DECO:-0}" \
$JAVA $HEAP_ARGS $PGEN_FLAG \
  -XX:FlightRecorderOptions=stackdepth=128 \
  -Djava.library.path="$ROOT/native/target/release" \
  -jar paper-26.3-218.jar nogui < run.in > run.out 2>&1 &
echo $! > server.pid
echo "server263 pid: $(cat server.pid)"
