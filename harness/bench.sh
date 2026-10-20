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

# --- optional fresh-world wipe (deterministic parity runs) ------------------
if [ "${CRUS_FRESH:-0}" = "1" ]; then
  echo "[bench] wiping world dirs ($WORLD)"
  rm -rf "$ROOT/server/$WORLD" "$ROOT/server/${WORLD}_nether" "$ROOT/server/${WORLD}_the_end"
fi

# --- optional datapack wiring (world datapacks, enabled automatically on a
# fresh world). DATAPACKS_DIR=*: wipe + copy pack zips/dirs into the world.
if [ "${DATAPACKS_DIR:-}" != "" ]; then
  DP="$ROOT/server/$WORLD/datapacks"
  rm -rf "$DP"
  mkdir -p "$DP"
  cp -r "$DATAPACKS_DIR"/"." "$DP"/
  echo "[bench] datapacks installed from $DATAPACKS_DIR:"; ls "$DP"
fi

# --- point level-name at the target world ---------------------------------
sed -i "s/^level-name=.*/level-name=$WORLD/" "$ROOT/server/server.properties"

"$ROOT/harness/server_up.sh"

# minimise spawn-chunk pregen overlap with the measured region (vanilla gamerule, 1.20.5+)
rcon "gamerule spawnChunkRadius 0" || true
# freeze random ticks (kelp growth, amethyst buds, crops): which chunks get
# random-ticked depends on the world random draw sequence = WHEN chunks
# loaded — a timing artifact, not generation output. Both capture (this
# script) and judge (plugin_bench.sh) run randomTickSpeed=0.
rcon "gamerule randomTickSpeed 0" || true
echo "[bench] gamerule randomTickSpeed=0 (random-tick freeze)"

DUMPDIR="$ROOT/reference/dumps/$TAG"
mkdir -p "$DUMPDIR"
T0=$(date +%s%3N)
echo "[bench] $(date -u +%T) sequential forceload start (t0=$T0)"
# Sequential per-chunk forceload (cx asc, cz asc): identical to
# plugin_bench.sh. Ore/blob features write into the 3x3 WorldGenRegion, so
# the final block at contested boundary positions depends on the chunk
# DECORATION ORDER — both sides must decorate in the same order for a
# deterministic judge comparison.
EXPECT=0
for fcz in $(seq -4 3); do
  for fcx in $(seq -4 3); do
    EXPECT=$((EXPECT + 1))
    rcon "forceload add $((fcx * 16)) $((fcz * 16)) $((fcx * 16 + 15)) $((fcz * 16 + 15))" >/dev/null || true
    for i in $(seq 1 150); do
      ST=$(rcon "crusstatus -4 -4 3 3" 2>/dev/null || echo "")
      N=$(printf '%s\n' "$ST" | grep "^$WORLD:" | sed 's/.*region\[-4\.\.3,-4\.\.3\]=\([0-9]*\)\/.*/\1/' || true)
      if [ "${N:-0}" -ge "$EXPECT" ]; then break; fi
      sleep 1
    done
    # per-chunk dump (the judge dataset — see plugin_bench.sh)
    rcon "crusstydump $fcx $fcz $DUMPDIR/chunk_${fcx}_${fcz}.crsf $WORLD" >/dev/null || true
  done
done

# --- final poll: region complete --------------------------------------------
N=""
for i in $(seq 1 300); do
  ST=$(rcon "crusstatus -4 -4 3 3" 2>/dev/null || echo "")
  N=$(printf '%s\n' "$ST" | grep "^$WORLD:" | sed 's/.*region\[-4\.\.3,-4\.\.3\]=\([0-9]*\)\/.*/\1/' || true)
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
# settle: the per-chunk dumps below freeze the compared state at each chunk's
# generation — no region-wide settle (it would deepen the simulation after
# the dumps; the judge compares the per-chunk dataset).
sleep 2

echo "[bench] per-chunk dumps already written (the judge dataset)"
DUMP_OUT=$(ls "$DUMPDIR" | wc -l)
echo "[bench] dump files: $DUMP_OUT"

"$ROOT/harness/server_down.sh"

MS_PER_CHUNK=$((WALL_MS / 64))
DUMP_MS=$(printf '%s\n' "$DUMP_OUT" | sed 's/.*ms=\([0-9]*\).*/\1/')

{
  echo "baseline ($TAG) — vanilla Paper server chunk generation, region 8x8 chunks (cx,cz in [-4..3])"
  echo "seed=3257840388504953787  min_y=-64 height=384  sea_level=63"
  echo "datapacks: ${DATAPACKS_DIR:-none}"
  echo "method: single server invocation; wall-clock from 'forceload add -64 -64 63 63' until"
  echo "  crusstatus reports 64/64 chunks of the region loaded (coarse; includes settle/poll"
  echo "  granularity of 2s); wall_ms=$WALL_MS; ms/chunk=$MS_PER_CHUNK"
  echo "dump: crusstydumpregion of the same region took ${DUMP_MS:-?}ms (not part of baseline)"
  echo "note: chunks already present on disk (e.g. spawn overlap) load fast and lower the average"
  echo "date: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
} > "$ROOT/reference/baseline_$TAG.txt"
echo "[bench] wrote reference/baseline_$TAG.txt (ms/chunk=$MS_PER_CHUNK)"

python3 "$ROOT/harness/verify_dumps.py" "$DUMPDIR" 64
