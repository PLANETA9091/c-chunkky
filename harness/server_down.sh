#!/usr/bin/env bash
# Shut down the paper server started by server_up.sh.
# 1) close the stdin FIFO (EOF -> Paper graceful stop)
# 2) fallback: RCON stop
# 3) last resort: SIGTERM/SIGKILL
set -u
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SRV="$ROOT/server"
cd "$SRV"

[ -f server.pid ] || { echo "no server.pid; nothing to stop"; exit 0; }
PID=$(cat server.pid)

if kill -0 "$PID" 2>/dev/null; then
  if [ -f fifo.pid ]; then
    kill "$(cat fifo.pid)" 2>/dev/null || true
    rm -f fifo.pid run.in
  fi
  for i in $(seq 1 60); do
    kill -0 "$PID" 2>/dev/null || { echo "SERVER DOWN (stdin EOF)"; exit 0; }
    sleep 2
  done
  # fallback: RCON stop
  python3 "$ROOT/harness/rcon.py" localhost 25575 crussty stop >/dev/null 2>&1 || true
  for i in $(seq 1 60); do
    kill -0 "$PID" 2>/dev/null || { echo "SERVER DOWN (rcon stop)"; exit 0; }
    sleep 2
  done
  echo "forcing SIGTERM" >&2
  kill "$PID" 2>/dev/null || true
  for i in $(seq 1 15); do
    kill -0 "$PID" 2>/dev/null || { echo "SERVER DOWN (sigterm)"; exit 0; }
    sleep 2
  done
  kill -9 "$PID" 2>/dev/null || true
  echo "SERVER DOWN (sigkill)"
  exit 0
fi
echo "SERVER ALREADY DOWN"
rm -f fifo.pid run.in
exit 0
