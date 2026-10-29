#!/usr/bin/env bash
# Rigorous 26.3 native-surface A/B: fresh world per run, one dump per run.
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
RCON="python3 harness/rcon.py localhost 25576 crussty"
SRV="$ROOT/server263"

wipe_world() { rm -rf "$SRV/world" "$SRV/world_nether" "$SRV/world_the_end"; }

run_one() {  # $1 = dump name, $2 = extra env ("" or CRUS_NO_NATIVE_SURFACE_263=1)
  bash harness/server263_down.sh >/dev/null 2>&1
  wipe_world
  env $2 bash harness/server263_up.sh >/dev/null 2>&1
  for i in $(seq 1 60); do
    rg -q "Done \(" "$SRV/run.out" 2>/dev/null && break
    sleep 5
  done
  $RCON "forceload add 16384 16384 16527 16527" >/dev/null 2>&1
  sleep 40
  $RCON "blockdump $1" >/dev/null 2>&1
  sleep 2
  bash harness/server263_down.sh >/dev/null 2>&1
  echo "$1: $(wc -l < "$SRV/$1" 2>/dev/null || echo MISSING) chunks"
}

run_one "ab_native_on.txt" ""
run_one "ab_vanilla_off.txt" "CRUS_NO_NATIVE_SURFACE_263=1"

echo "=== diff ==="
diff "$SRV/ab_native_on.txt" "$SRV/ab_vanilla_off.txt" > /tmp/ab.diff && echo "IDENTICAL" || {
  echo "divergent chunks: $(rg -c '^[<>]' /tmp/ab.diff)"
  head -6 /tmp/ab.diff
}
