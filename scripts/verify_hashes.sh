#!/usr/bin/env bash
# Golden out_hash snapshot/verify: 8 chunks x 4 modes.
# Usage: verify_hashes.sh save   -> writes reference/local_golden.txt
#        verify_hashes.sh check  -> diffs against reference/local_golden.txt
set -euo pipefail
source "$HOME/.cargo/env" 2>/dev/null || export PATH="$HOME/.cargo/bin:$PATH"
cd "$(dirname "$0")/../native"
SEED=3257840388504953787
CX=(0 1 0 1 -2 3 -4 7)
CZ=(0 0 1 1 -2 1 2 -3)
MODES=("" "CRUS_FILL_THREADS=2" "CRUS_LANES=8" "CRUS_LANES=8 CRUS_FILL_THREADS=2")
cargo build --release 2>/dev/null
OUT=""
for i in 0 1 2 3 4 5 6 7; do
  cx=${CX[$i]}; cz=${CZ[$i]}
  for m in "${MODES[@]}"; do
    h=$(env $m ./target/release/bench fill $cx $cz 2>/dev/null | grep -o 'out_hash=[0-9a-f]*' | cut -d= -f2)
    OUT="${OUT}${h} chunk=(${cx},${cz}) mode=[${m}]
"
  done
done
if [ "${1:-check}" = "save" ]; then
  printf '%s' "$OUT" > ../reference/local_golden.txt
  echo "saved: $(echo "$OUT" | grep -c .) hashes"
else
  if printf '%s' "$OUT" | diff - ../reference/local_golden.txt >/dev/null; then
    echo "GOLDEN OK: 8 chunks x 4 modes bit-identical"
  else
    echo "GOLDEN MISMATCH:"
    printf '%s' "$OUT" | diff - ../reference/local_golden.txt || true
    exit 1
  fi
fi
