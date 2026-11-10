#!/usr/bin/env bash
# cont139-H7 A/B + parity run script (executed after BUILD_OK).
# Usage: bash run_ab.sh <build|test|bench|all>
set -euo pipefail
export PATH="$HOME/.cargo/bin:$PATH"
W=/home/z/wt_h7b/native
T=/home/z/tgt_h7b
SCR=/home/z/my-project/c-faster/agent-scratch/h7
R1=/home/z/my-project/c-faster/workdir/dpext/01
R2=/home/z/my-project/c-faster/workdir/dpext/02
BENCH_ARGS="-- 64 2100 2000 --rounds 3"

build() {
  cd "$W"
  CARGO_TARGET_DIR="$T" cargo build --release 2>&1 | tail -5
}

test() {
  cd "$W"
  CARGO_TARGET_DIR="$T" cargo test --release 2>&1 | tail -15
}

bench() {
  mkdir -p "$SCR/runs"
  cd "$T/release"
  echo "=== OFF (parent semantics, CRUS_H7_OFF=1) ==="
  CRUS_H7_OFF=1 ./h7bench "$R1" "$R2" $BENCH_ARGS --dump "$SCR/runs/dump_off.bin" | tee "$SCR/runs/bench_off.txt"
  echo "=== FAST (H7 on) ==="
  ./h7bench "$R1" "$R2" $BENCH_ARGS --dump "$SCR/runs/dump_fast.bin" | tee "$SCR/runs/bench_fast.txt"
  echo "=== op-stream parity ==="
  python3 "$SCR/diff_dump.py" "$SCR/runs/dump_off.bin" "$SCR/runs/dump_fast.bin"
}

fillparity() {
  # fill-phase regression guard vs the pinned parent .so tree (fill untouched by H7)
  cd "$T/release"
  P=/home/z/my-project/c-faster/native/target/release
  for cx in 2100 2101; do
    ./filldump "$R1" "$R2" -- $cx 2000 3257840388504953787 "$SCR/runs/fill_mine_${cx}.txt"
    "$P/filldump" "$R1" "$R2" -- $cx 2000 3257840388504953787 "$SCR/runs/fill_parent_${cx}.txt"
    if cmp -s "$SCR/runs/fill_mine_${cx}.txt" "$SCR/runs/fill_parent_${cx}.txt"; then
      echo "fill parity cx=$cx: IDENTICAL"
    else
      echo "fill parity cx=$cx: DIVERGED"; exit 1
    fi
  done
}

case "${1:-all}" in
  build) build ;;
  test) test ;;
  bench) bench ;;
  fill) fillparity ;;
  all) build && test && bench && fillparity ;;
esac
