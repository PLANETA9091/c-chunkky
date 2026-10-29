#!/usr/bin/env bash
# e2e measurement for the 26.3 server (server263/): forceload a fresh 12x12
# region, poll GenStats until done, report wall time + native fill p50.
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
HOST=localhost PORT=25576 PASS=crussty
RCON="python3 harness/rcon.py $HOST $PORT $PASS"

rx="${1:-1000}"   # region origin chunk
rz="${2:-1000}"
n="${3:-11}"      # 12x12 = (rx..rx+n) x (rz..rz+n)

stats_chunk_count() {
  $RCON crusgenstats 2>/dev/null | grep -o "chunks=[0-9]*" | head -1 | cut -d= -f2
}

before=$(stats_chunk_count)
t0=$(date +%s.%N)
$RCON "forceload add $((rx*16)) $((rz*16)) $(((rx+n)*16 + 15)) $(((rz+n)*16 + 15))" >/dev/null
# poll until count reaches before + (n+1)^2
target=$((before + (n + 1) * (n + 1)))
for i in $(seq 1 600); do
  c=$(stats_chunk_count)
  if [ -n "$c" ] && [ "$c" -ge "$target" ]; then
    break
  fi
  sleep 0.25
done
t1=$(date +%s.%N)
elapsed=$(python3 -c "print(f'{$t1-$t0:.2f}')")
nchunks=$(( (n + 1) * (n + 1) ))
python3 -c "print(f'window: {$elapsed}s / $nchunks chunks = {float('$elapsed')*1000/$nchunks:.1f} ms/chunk')"
$RCON crusgenstats 2>/dev/null | grep -o "chunks=[0-9]*.*" | head -1 | cut -c1-260
