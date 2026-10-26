#!/usr/bin/env bash
# cont-54 diag: per-block chunkdumps of the worst sulfur-delta chunks in
# both surface modes, to localize the divergence (Y bands / materials).
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
RCON="python3 harness/rcon.py localhost 25576 crussty"
SRV="$ROOT/server263"

wipe_world() { rm -rf "$SRV/world" "$SRV/world_nether" "$SRV/world_the_end"; }

# target chunks (cx cz), one per line
TARGETS="1019 1037
1021 1034
1022 1022"

leg() {  # $1 = prefix, $2 = extra env
  local prefix="$1" extra="$2"
  bash harness/server263_down.sh >/dev/null 2>&1
  wipe_world
  env "$extra" bash harness/server263_up.sh >/dev/null 2>&1
  for i in $(seq 1 90); do rg -q "Done \(" "$SRV/run.out" 2>/dev/null && break; sleep 5; done
  # forceload a 3x3 region around each target (neighbors for fiddle parity)
  while read -r tcx tcz; do
    [ -z "$tcx" ] && continue
    local bx=$((tcx * 16)) bz=$((tcz * 16))
    $RCON "forceload add $((bx - 16)) $((bz - 16)) $((bx + 31)) $((bz + 31))" >/dev/null 2>&1
  done <<< "$TARGETS"
  sleep 90
  while read -r tcx tcz; do
    [ -z "$tcx" ] && continue
    $RCON "chunkdump $tcx $tcz ${prefix}_${tcx}_${tcz}.txt" >/dev/null 2>&1
  done <<< "$TARGETS"
  bash harness/server263_down.sh >/dev/null 2>&1
  echo "leg $prefix done: $(ls $SRV/${prefix}_*.txt 2>/dev/null | wc -l) dumps"
}

leg "d54_native" ""
leg "d54_vsurf" "CRUS_NO_NATIVE_SURFACE_263=1"
