#!/usr/bin/env bash
# cont-54 full A/B (fix round 2: is_3d y-lazy + csl volume slab + fiddle).
# Region 16320..16575 = chunks 1020..1036. One run per leg, driven manually
# (background processes die between tool calls — this script is leg-by-leg).
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
RCON="python3 harness/rcon.py localhost 25576 crussty"
SRV="$ROOT/server263"

leg() {  # $1 = prefix, $2 = extra env
  local prefix="$1" extra="$2"
  bash harness/server263_down.sh >/dev/null 2>&1
  rm -rf "$SRV/world" "$SRV/world_nether" "$SRV/world_the_end"
  env "$extra" bash harness/server263_up.sh >/dev/null 2>&1
  for i in $(seq 1 90); do rg -q "Done \(" "$SRV/run.out" 2>/dev/null && break; sleep 5; done
  $RCON "forceload add 16320 16320 16575 16575" >/dev/null 2>&1
  sleep 150
  $RCON "blockdump ${prefix}.txt" >/dev/null 2>&1
  sleep 3
  $RCON "statdump ${prefix}_stat.txt" >/dev/null 2>&1
  sleep 2
  bash harness/server263_down.sh >/dev/null 2>&1
  echo "leg $prefix done: $(wc -l < "$SRV/${prefix}.txt" 2>/dev/null) chunks"
}

leg "ab54b_native" ""
leg "ab54b_vsurf" "CRUS_NO_NATIVE_SURFACE_263=1"
echo "ALL LEGS DONE"
