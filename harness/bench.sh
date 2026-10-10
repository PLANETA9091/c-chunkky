#!/usr/bin/env bash
# bench.sh <world> <tag>
#   Single-invocation benchmark session (ONE server start per measurement):
#     1. point server.properties level-name at <world>
#     2. server_up.sh (start once, FIFO stdin held open)
#     3. RCON: forceload add -64 -64 63 63  (8x8 chunk region, cx/cz -4..3)
#     4. poll `crusstatus` until 64/64 loaded, record wall time
#     5. settle, then crusstydumpregion -> reference/dumps/<tag>/
#     6. verify dumps, write reference/baseline_<tag>.txt
#     7. server_down.sh (EOF FIFO -> graceful stop)
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WORLD="$1"; TAG="$2"

rcon() { python3 "$ROOT/harness/rcon.py" localhost 25575 crussty "$1"; }

# --- point level-name at the target world ---------------------------------
sed -i "s/^level-name=.*/level-name=$WORLD/" "$ROOT/server/server.properties"

"$ROOT/harness/server_up.sh"

DUMPDIR="$ROOT/reference/dumps/$TAG"
mkdir -p "$DUMPDIR"
T0=$(date +%s%3N)
echo "[bench] $(date -u +%T) forceload start (t0=$T0)"
rcon "forceload add -64 -64 63 63" || true

# --- poll crusstatus until region complete ---------------------------------
N=""
for i in $(seq 1 900); do
  ST=$(rcon crusstatus 2>/dev/null || echo "")
  N=$(printf '%s\n' "$ST" | grep "^$WORLD:" | sed 's/.*region\[-4\.\.3,-4\.\.3\]=\([0-9]*\)\/64.*/\1/' || true)
  if [ "${N:-0}" = "64" ]; then
    T1=$(date +%s%3N)
    echo "[bench] $(date -u +%T) region complete (t1=$T1) after poll #$i"
    break
  fi
  sleep 2
done
if [ "${N:-0}" != "64" ]; then
  echo "[bench] TIMEOUT: region never reached 64/64 (last=$N)" >&2
  "$ROOT/harness/server_down.sh" || true
  exit 1
fi

WALL_MS=$((T1 - T0))
# settle: let light/tick-level propagation finish before snapshotting
sleep 5

echo "[bench] dumping region to $DUMPDIR"
DUMP_OUT=$(rcon "crusstydumpregion -4 -4 3 3 $DUMPDIR chunk $WORLD")
echo "[bench] $DUMP_OUT"

"$ROOT/harness/server_down.sh"

MS_PER_CHUNK=$((WALL_MS / 64))
DUMP_MS=$(printf '%s\n' "$DUMP_OUT" | sed 's/.*ms=\([0-9]*\).*/\1/')

{
  echo "baseline ($TAG) — vanilla Paper server chunk generation, region 8x8 chunks (cx,cz in [-4..3])"
  echo "seed=3257840388504953787  min_y=-64 height=384  sea_level=63"
  echo "method: single server invocation; wall-clock from 'forceload add -64 -64 63 63' until"
  echo "  crusstatus reports 64/64 chunks of the region loaded (coarse; includes settle/poll"
  echo "  granularity of 2s); wall_ms=$WALL_MS; ms/chunk=$MS_PER_CHUNK"
  echo "dump: crusstydumpregion of the same region took ${DUMP_MS:-?}ms (not part of baseline)"
  echo "note: chunks already present on disk (e.g. spawn overlap) load fast and lower the average"
  echo "date: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
} > "$ROOT/reference/baseline_$TAG.txt"
echo "[bench] wrote reference/baseline_$TAG.txt (ms/chunk=$MS_PER_CHUNK)"

python3 "$ROOT/harness/verify_dumps.py" "$DUMPDIR" 64
