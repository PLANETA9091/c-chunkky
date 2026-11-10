# cont140-slice — slice-phase per-WKind decomposition + optimization

## Status log (flush constantly)

- [t0] Worktree /home/z/wt_slice2 (branch cont140-slice @ origin/main c468c999). Parent .so md5 ef411ab46ee948f9390d9d0861c00647 verified.
- [t1] INSTRUMENTATION DONE, commit 9a7460b9: `native/src/sliceprof.rs` + guards in every WKind arm of
  density.rs eval_batch_recurse (batch) AND eval_uncached (scalar) + eval()/eval_batch_uncached memo-hit probes +
  SpScope (slice-phase marker) in fill_slice / parallel worker closures / eval_plane_into + SpWall buckets
  (fill_slice_total / slot0_batch / slots1_7_scalar / eval_plane). Gate: CRUS_SLICE_PROF=1 (default OFF —
  one Relaxed load per arm visit when off). rdtsc-based SELF time (TLS descend subtraction), phase-tagged
  buckets (slice vs other), per-thread flush at thread exit (scope worker threads).
- NOTE (important): the "failed" MultiEdit #2 partially applied (edits 1-4); python pass completed the rest.
  All 29 kinds instrumented in both paths (49 SpGuard sites in density.rs).
- [t2] dpperf updated: CRUS_SLICE_PROF=1 prints TWO 8-fill decomposition passes (cz=-2, cz=+77).

## DELIVERABLE #1 — per-WKind slice decomposition (dpext 01+02, CRUS_NO_PIPELINE=1 so all slice work is synchronous)

Campaign reference reproduced on parent-eq binary: `jni_prof wall=4.30ms slice=2.61 cellfill=0.25 material=0.98 select=0.09`
(pipeline ON, 24 fills @ (2,-2)). With CRUS_NO_PIPELINE=1: slice wall = 4.68 ms/ch (TRUE serial slice CPU;
pipeline hides ~2ms under cellfill on the worker thread).

Profiler run (8-fill passes; profiler inflation ≈ +3.2ms wall over ~67k guarded visits ≈ 45-50ns/visit —
deflate buckets by ~n_calls x 45ns for real values):

### Pass 1 (z=-2, 8 chunks) — slice-phase buckets
| arm | calls/ch | self ms/ch (inflated) | ~real (deflated) |
|---|---|---|---|
| Noise (batch perlin kernels) | 12154 | 2.40 | ~1.8 |
| Blend (old_blended_noise batch8col) | 247.5 | 1.54 | ~1.5 |
| RangeChoice (branch machinery) | 15764 | 1.07 | ~0.35 |
| Ap2Add+Mul+Min+Max | 16587 | 1.26 | ~0.45 |
| Gradient | 14597 | 0.26 | ~0 |
| Const | 10344 | 0.21 | ~0 |
| CachePassthrough | 3692 | 0.20 | ~0.03 |
| WeirdScaledSampler | 495 | 0.16 | ~0.13 |
| Spline | 757 | 0.06 | ~0.03 |
| ShiftA/ShiftB/ShiftedNoise | 37/37/37 | 0.15 | ~0.14 |
| Cache2dHit/Miss | 1642/309 | 0.06 | ~0 |
| MemoHit (count-only) | 7025 | — | — |

Walls: fill_slice_total=7.90ms(infl) = slot0_batch 5.12 + slots1_7_scalar 2.78(infl, ~1.2-1.5 real) + scatter/misc.

### Pass 2 (z=+77): same structure — Noise 2.91, Blend 1.42, RangeChoice 1.17, Shift* 1.57 (Tectonic biomes active), Spline 0.11.

### Pass 2 for root 02-heavy biome region (Tectonic): ShiftedNoise/ShiftA/ShiftB n=1819/ch ≈ 1.6ms inflated.

## WHALE VERDICT (hypothesis check)
- BlendedNoise: CONFIRMED ~1.4-1.5ms/ch (247.5 batch8 calls/ch x ~5.9us = blendmicro figure ✓). NOT vanilla-only —
  present in dpext trees via minecraft:old_blended_noise x ~27 nodes.
- Spline interpolation: REFUTED as whale — 0.06-0.17ms/ch here. H6 spline_pt 0.83ms/8fills = 0.104ms/ch ✓ consistent.
- "Named 2D noise ~280us/ch" (H6 note): NOT reproduced — h6 named table on these roots sums ≈ 2.06ms/ch; my
  slice:Noise arm = 2.4-2.9ms (incl. 3D noises: spaghetti/cave/megacave — they dominate the named table).
  The kernels are ALREADY AVX2/AVX512 + column-memoized (get_value_batch_col, compute_batch8_col512 via
  avx512_selected() probe, noise_5_batch_avx2, blend_main_loop_avx2) → Candidate A is already upstream; little left there.
- NEW WHALE FOUND: **slots-1-7 scalar corner loops** (wall_slots1_7_scalar 2.78-2.85ms inflated / ~1.2-2.0ms real):
  7350 per-corner scalar evals/chunk through MemoCtx — biome-independent. This is "tree-walk overhead" the H6 entry
  pointed at. Batch path exists and is production-proven bit-identical: eval_plane_batched (used by plane worker +
  prefetch) evaluates ALL slots in G-wide groups. PLAN: make serial fill_slice use the all-slots batched path.
- RangeChoice self ~0.35-0.5ms real + unattributed wall remainder (~1-2ms: BatchMemo::new per plane = arena.len() x 72B
  zeroing x9, stamp checks, scatter) = Candidate C territory if more is needed.

## NEXT
1. Batch the slots-1-7 scalar loops in fill_slice (reuse eval_plane_batched / eval_plane_into on the serial path).
2. filldump byte-parity + cargo test --release --lib + dpperf A/B interleaved.
