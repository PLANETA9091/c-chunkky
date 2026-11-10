#!/usr/bin/env bash
# cont139-H6 A/B + parity driver (runs ONLY after BUILD_OK).
# Parent binaries: pinned tree build (md5 .so ae9d434bc37f5b6b18479c248f210e21).
# Mine: /home/z/tgt_h6b/release (branch cont139-h6).
set -u
PDIR=/home/z/my-project/c-faster
P=$PDIR/native/target/release
M=/home/z/tgt_h6b/release
R1=$PDIR/workdir/dpext/01
R2=$PDIR/workdir/dpext/02
OUT=$PDIR/agent-scratch/h6
mkdir -p "$OUT"
export PATH="$HOME/.cargo/bin:$PATH"

mode=${1:?usage: h6_ab.sh profile|parity|ab|all}

if [ "$mode" = profile ] || [ "$mode" = all ]; then
  echo "=== SPROF + census (WIP-diag build = parent fill code) ==="
  W=/home/z/tgt_h6wip/release
  for R in "$R1" "$R2"; do
    echo "--- root $(basename "$R") prof263 ---"
    CRUS_SPROF=1 "$W/dpperf" "$R" -- 8 -4 -4 | grep -E "prof263|native fill"
    echo "--- root $(basename "$R") h6 census ---"
    CRUS_H6_DIAG=1 "$W/dpperf" "$R" -- 8 -4 -4 | grep -E "^h6|native fill"
    "$W/h6probe" "$R" | head -40
  done
  echo "=== SPROF after (my build) ==="
  for R in "$R1" "$R2"; do
    CRUS_SPROF=1 "$M/dpperf" "$R" -- 8 -4 -4 | grep -E "prof263|native fill"
    CRUS_H6_DIAG=1 "$M/dpperf" "$R" -- 8 -4 -4 | grep -E "^h6|native fill"
  done
fi

if [ "$mode" = parity ] || [ "$mode" = all ]; then
  echo "=== filldump parity: parent vs mine (dptt 8x8 region corners + center) ==="
  for c in "-4 -4" "3 3" "0 0" "-1 2"; do
    set -- $c
    cx=$1; cz=$2
    for R in "$R1" "$R2"; do
      tag="$(basename "$R")_${cx}_${cz}"
      "$P/filldump" "$R" -- $cx $cz > "$OUT/fd_p_$tag.txt"
      "$M/filldump" "$R" -- $cx $cz > "$OUT/fd_m_$tag.txt"
      if cmp -s "$OUT/fd_p_$tag.txt" "$OUT/fd_m_$tag.txt"; then
        echo "filldump $tag: IDENTICAL ($(wc -l < "$OUT/fd_p_$tag.txt") lines)"
      else
        echo "filldump $tag: DIVERGED"; diff "$OUT/fd_p_$tag.txt" "$OUT/fd_m_$tag.txt" | head -8
      fi
    done
  done
  echo "=== heights parity: parent-build h6heights vs mine ==="
  HB=/home/z/tgt_h6base/release
  for R in "$R1" "$R2"; do
    tag="$(basename "$R")"
    "$HB/h6heights" "$R" -- -64 -64 63 63 2 > "$OUT/hh_p_$tag.txt"
    "$M/h6heights" "$R" -- -64 -64 63 63 2 > "$OUT/hh_m_$tag.txt"
    if cmp -s "$OUT/hh_p_$tag.txt" "$OUT/hh_m_$tag.txt"; then
      echo "h6heights $tag: IDENTICAL ($(grep -c DONE "$OUT/hh_p_$tag.txt") ok, $(($(wc -l < "$OUT/hh_p_$tag.txt")-1)) cols)"
    else
      echo "h6heights $tag: DIVERGED"; diff "$OUT/hh_p_$tag.txt" "$OUT/hh_m_$tag.txt" | head -8
    fi
  done
  echo "=== planecmp self-consistency (mine, vanilla embedded seed 3257840388504953787) ==="
  "$M/planecmp" 3 -2 3257840388504953787 | tail -3 || true
fi

if [ "$mode" = ab ] || [ "$mode" = all ]; then
  echo "=== dpperf A/B n=64 x3 rounds (serial), min/p50 per (bin,root) ==="
  for R in "$R1" "$R2"; do
    for B in P M; do
      BIN="$P/dpperf"; [ "$B" = M ] && BIN="$M/dpperf"
      echo "--- $B root $(basename "$R") ---"
      for i in 1 2 3; do
        "$BIN" "$R" -- 64 -4 -4
      done
    done
  done
fi
echo "h6_ab.sh: mode $mode done"
