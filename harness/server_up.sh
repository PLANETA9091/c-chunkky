#!/usr/bin/env bash
# Start the Paper server exactly once per measurement session.
# Keeps stdin open via a FIFO (Paper exits on stdin EOF).
# Deploy hygiene: logs md5 of every deployed artifact (jar/.so) into server/deployed.md5.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SRV="$ROOT/server"
cd "$SRV"

# Refuse to start if another paper instance is alive (2 cores / 3GB rule).
if pgrep -f 'java .*-jar .*paper\.jar' >/dev/null 2>&1; then
  echo "ERROR: a paper server instance is already running" >&2
  exit 1
fi

# --- deploy hygiene -------------------------------------------------------
{
  echo "# deployed.md5 $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  [ -f paper.jar ] && md5sum paper.jar
  for f in plugins/*.jar; do [ -e "$f" ] && md5sum "$f"; done
  for f in plugins/**/*.so; do [ -e "$f" ] && md5sum "$f"; done || true
} > deployed.md5
echo "--- deployed.md5 ---"; cat deployed.md5

# --- FIFO stdin holder ----------------------------------------------------
rm -f run.in
mkfifo run.in
( sleep 28800 > run.in ) &
echo $! > fifo.pid

# --- launch ---------------------------------------------------------------
# Native lib visibility for plugins doing System.loadLibrary("crussty_chunkgen").
JAVA_LIB_PATH_ARGS=""
NATIVE_SO="$ROOT/native/target/release/libcrussty_chunkgen.so"
if [ -f "$NATIVE_SO" ]; then
  JAVA_LIB_PATH_ARGS="-Djava.library.path=$ROOT/native/target/release"
fi
java -Xms512M -Xmx1536M $JAVA_LIB_PATH_ARGS -jar paper.jar nogui < run.in > run.out 2>&1 &
echo $! > server.pid
echo "paper started pid=$(cat server.pid) fifo_holder=$(cat fifo.pid)"

# --- wait for Done --------------------------------------------------------
for i in $(seq 1 600); do
  if ! kill -0 "$(cat server.pid)" 2>/dev/null; then
    echo "SERVER DIED during startup; last log lines:" >&2
    tail -40 run.out >&2
    exit 1
  fi
  if grep -q 'Done (' run.out; then
    grep 'Done (' run.out | tail -1
    echo "SERVER UP"
    exit 0
  fi
  sleep 2
done
echo "TIMEOUT waiting for server startup" >&2
tail -40 run.out >&2
exit 1
