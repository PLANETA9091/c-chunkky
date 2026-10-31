#!/usr/bin/env bash
# CI run read-out: pull a completed run's logs (ZIP) and print the honest
# comparators: honest windows, stage lines, bench medians, JFR hot list,
# deco stats, judge verdicts. Usage: bash scripts/ci_readout.sh <run_id> [out_dir]
set -euo pipefail
RUN_ID="${1:?usage: ci_readout.sh <run_id> [out_dir]}"
OUT="${2:-/tmp/ciread_$RUN_ID}"
TOKEN="${CI_TOKEN:-$(git remote get-url origin | sed -n 's|https://PLANETA9091:\([^@]*\)@.*|\1|p')}"
if [ -z "$TOKEN" ]; then echo "no token" >&2; exit 1; fi
mkdir -p "$OUT"
if [ ! -f "$OUT/.unzipped" ]; then
  curl -sS -L -H "Authorization: token $TOKEN" \
    "https://api.github.com/repos/PLANETA9091/c-chunkky/actions/runs/$RUN_ID/logs" \
    -o "$OUT/logs.zip"
  rm -rf "$OUT/x" && mkdir -p "$OUT/x"
  unzip -o -q "$OUT/logs.zip" -d "$OUT/x"
  touch "$OUT/.unzipped"
fi
INT=$(ls "$OUT"/x/*integration*.txt 2>/dev/null | head -1)
NAT=$(ls "$OUT"/x/*native*.txt 2>/dev/null | head -1)
echo "=== honest windows ==="
rg -N "honest window:" "$INT" 2>/dev/null || true
echo "=== stage lines (surface/hood split) ==="
rg -N "stage\[terrain" "$INT" 2>/dev/null | head -6 || true
echo "=== deco stats ==="
rg -N "native deco stage:" "$INT" 2>/dev/null | head -6 || true
echo "=== surf call counters ==="
rg -N "surf_calls\[" "$INT" "$NAT" 2>/dev/null | head -6 || true
echo "=== gate lines ==="
rg -N "material-count gate|SIMD parity|divergent lines|parity fails" "$INT" "$NAT" 2>/dev/null | head -12 || true
echo "=== bench medians (ON run first, then CRUS_CORNER_CACHE=0 A/B) ==="
rg -N "server_fill median|fill_chunk  median" "$NAT" 2>/dev/null | head -8 || true
echo "=== corner-cache counters (cc[hits/miss/box], window totals) ==="
rg -N "cc\[hits" "$INT" 2>/dev/null | head -8 || true
echo "=== JFR hot (timed window) ==="
rg -N -A 24 "JFR hot methods" "$INT" 2>/dev/null | head -28 || true
echo "=== carver/deco WARNs ==="
rg -N "carver driver failed|native deco:" "$INT" 2>/dev/null | head -8 || true
