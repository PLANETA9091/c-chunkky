#!/usr/bin/env bash
# cont-156 offline probe: pack preliminary_surface_level DF binding
#   leg 0 = CRUS_PACK_PRELIM=0 (legacy scan)   -> dumps + aqdbg
#   leg 1 = default (pack DF bound)            -> dumps + aqdbg
#   leg 2 = CRUS_PACK_PRELIM_VERIFY=1          -> divergence counters
# Byte-compare leg0 vs leg1 per chunk (bit-equality gate).
set -uo pipefail
FD=/home/z/my-project/c-faster/native/target/relfast/filldump
R1=/home/z/my-project/c-faster/workdir/dpext/01
R2=/home/z/my-project/c-faster/workdir/dpext/02
OUT=/home/z/my-project/scripts/logs/packprelim
mkdir -p "$OUT"
SEED=3257840388504953787
CHUNKS=("3000 3000" "3001 3000" "2999 2999" "2500 2500" "14600 14600")

run() { # $1=leg tag  $2..=env
  local tag="$1"; shift
  for c in "${CHUNKS[@]}"; do
    local cx=${c% *}; local cz=${c#* }
    local f="$OUT/${tag}_${cx}_${cz}.txt"
    env "$@" CRUS_AQDBG=1 "$FD" "$R1" "$R2" -- "$cx" "$cz" "$SEED" "$f" 2> "$OUT/${tag}_${cx}_${cz}.err"
    grep -E "^\[aqdbg\]" "$OUT/${tag}_${cx}_${cz}.err" | sed "s/^/[$tag @$cx,$cz] /"
  done
}

echo "=== leg0: legacy scan (CRUS_PACK_PRELIM=0) ==="
run leg0 CRUS_PACK_PRELIM=0
echo "=== leg1: pack DF bound (default) ==="
run leg1 CRUS_PACK_PRELIM_UNSET=1
echo "=== leg2: DF bound + verify (scan re-run per column) ==="
run leg2 CRUS_PACK_PRELIM_VERIFY=1

echo "=== byte-compare leg0 vs leg1 ==="
fail=0
for c in "${CHUNKS[@]}"; do
  cx=${c% *}; cz=${c#* }
  if cmp -s "$OUT/leg0_${cx}_${cz}.txt" "$OUT/leg1_${cx}_${cz}.txt"; then
    echo "  @$cx,$cz: BYTE-EQUAL ($(wc -l < "$OUT/leg0_${cx}_${cz}.txt") lines)"
  else
    echo "  @$cx,$cz: DIFF"; fail=1
  fi
done
echo "=== verify counters (leg2, summed) ==="
python3 - <<EOF
import glob, re
tot = div = 0
for f in glob.glob("$OUT/leg2_*.err"):
    t = open(f, errors="ignore").read()
    m = re.search(r"pack_prelim_total=(\d+) pack_prelim_diverge=(\d+)", t)
    if m:
        tot += int(m.group(1)); div += int(m.group(2))
print(f"  columns verified: {tot}, divergences: {div} -> {'BIT-EQUAL' if div==0 and tot>0 else 'MISMATCH'}")
EOF
echo "probe: $([ $fail = 0 ] && echo ALL-BYTE-EQUAL || echo HAS-DIFF)"
