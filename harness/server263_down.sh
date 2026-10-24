#!/usr/bin/env bash
# Stop the Paper 26.3 server (server263/) cleanly via the FIFO stdin.
set -uo pipefail
SRV="/home/z/my-project/c-faster/server263"
cd "$SRV"
if [ -f run.in ]; then
  echo "stop" > run.in 2>/dev/null || true
  sleep 6
fi
if [ -f server.pid ]; then
  PID="$(cat server.pid)"
  kill "$PID" 2>/dev/null || true
  for i in $(seq 1 10); do
    kill -0 "$PID" 2>/dev/null || break
    sleep 1
  done
  kill -9 "$PID" 2>/dev/null || true
fi
pkill -f 'paper-26\.3' 2>/dev/null || true
echo "server263 stopped"
