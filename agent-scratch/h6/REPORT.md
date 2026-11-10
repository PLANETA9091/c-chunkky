# H6 — native fill cost: slice-noise plane cache (cont139-H6)

Agent: Rust perf engineer, branch `cont139-h6`, worktree `/home/z/wt_h6b` (base `0091c8ea` = origin/main).
Parent pinned .so: `ae9d434bc37f5b6b18479c248f210e21` (verified md5 at /home/z/my-project/c-faster/native/target/release/libcrussty_chunkgen.so).
Lever: fill idle 5.7 -> ~3 ms/ch. Biggest native item: slice noise sampling ~2.9 ms (cont-132).

## 0. Adopt/reject of lost-session WIP (/home/z/wt_h6, base 64d2f119)

**DECISION: ADOPT (ported to wt_h6b, applied clean).** Rationale:
- `native/src/density.rs` (+130): diag-only counters (`CRUS_H6_DIAG=1`): per-noise-name ns/pts/calls
  wrapper `h6_named_noise`, spline point-value eval counters (`H6_SPLINE_PT_*` — this is the
  "blind spot" meter), Cache2d col hit/miss counters, blended counters. All behind a relaxed
  AtomicBool; zero behavioral change when off. Bit-parity-neutral (values untouched).
- `native/src/bin/dpperf.rs` (+22): CRUS_SPROF adds a prof263 pass (existing df263 counters),
  H6 diag pass after the timing loop (separate passes so the timing loop stays prof-free).
- `native/src/bin/h6probe.rs` (new): arena census — every node reachable from slot0 INCLUDING
  spline point values (the compile-time `compute_col_caches` blind spot: its `children()` skips
  spline point values), classified col-covered / 2D-UNCACHED / 3D, plus spline-with-noise census.
- Coherence: compiles against base fields (`noise_ids`, `col_idx`, `col_count` all exist);
  prof263 machinery already in df263.rs. Plan: keep diag for measurement rounds, STRIP the
  hot-path wrappers in the final optimization commit (atomic-load-per-call off-path cost).

## 1. Engine anatomy (what the 5.7ms fill / 2.9ms slice actually is)

- `dpperf` -> `ffi::fill` -> legacy engine (`ctx_new_assets` = v263:None) for the dpext
  (Terralith+Tectonic) pack set: `nochunk::ChunkGen::fill` -> 5 `fill_slice` x-plane calls
  (ccx=4 -> 5 planes/chunk), each 5 z-corners x 49 y-corners = 245 corners/slot.
- Slot 0 = final_density tree (the whale, ~88% of slice cost): `slice_slot0::<G>` batches
  G y-corners per call with `BatchMemo<G>` + col storage per plane. G = calibrated 4 (or 8).
  PROBLEM 1: the flat-p walk (`p = i*ny + y`) groups straddle z-rows every row (49 % G == 1
  for both G=4 and G=8) -> the batch Cache2d col-cache arm requires lane-uniform (x,z) ->
  ~1 in 6.125 (G=8) / 12.25 (G=4) groups loses ALL column caching.
- PROBLEM 2 (the blind spot): the batch `WKind::Spline` arm evaluates point VALUES per lane via
  `spline_apply(..., mem=None)` — scalar, NO stamps, NO column cache. The terrain offset/factor/
  jaggedness splines' point-value trees contain nested splines whose COORDINATES are 2D noises
  (continents/erosion/ridges/temperature/vegetation, Cache2d-wrapped) + ShiftA/ShiftB shift
  noises (y=0.0 literal, pure f(x,z)). Today: every lane x every y-group recomputes the whole
  nested chain from scratch — 8 lanes x 49 groups x (nested coordinate noises) per spline ref,
  zero reuse within the 245-corner plane.
- PROBLEM 3: `compute_col_caches` assigns col slots ONLY to Cache2d/FlatCache nodes; ShiftA/
  ShiftB are proven y-invariant (state=2) but get NO slot and NO cache lookup in either the
  batch or scalar arms. The 5 router ShiftedNoise users share the same shift_x/shift_z nodes;
  each is recomputed 8x per group on the batch path.
- Slots 1..7 (scalar path, MemoCtx, stamped + col-cached) are small; the y-tail (<=G-1 corners)
  evaluates with mem=None (minor).
- `base_height` (heightcmp gate) uses the SAME batch slot-0 lane path — batch-spline changes
  must hold bit-identity there too; heightcmp 1792/1792 is the verifier.

## 2. Plan (bit-parity-safe levers, in expected-value order)

- L1 batch-spline: rewrite the batch Spline arm — per-lane interval search (same binary search),
  collect DISTINCT point-value nodes needed by the group, evaluate each ONCE via
  `eval_batch_uncached` (batch memo: stamps dedupe shared refs; batch Cache2d arm col-caches the
  2D coordinate noises across the whole plane), then per-lane f32 spline arithmetic replicated
  exactly (i<0 / i==last / interior branches, same op order as `spline_apply`).
- L2 row-local grouping in `slice_slot0`: batch within a (z-row, y-run) so every group is
  lane-uniform in (x,z) -> Cache2d col cache hits on 100% of groups. Pure function of corners ->
  out-layout-preserving, bit-exact.
- L3 col slots for ShiftA/ShiftB (and any y-invariant Noise w/o Cache2d if the census shows
  them): add cache lookup/store in batch + scalar arms keyed (x,z). ShiftA/ShiftB are literal
  y=0.0 vanilla semantics — value is a pure function of (x,z); caching is bit-identical.
- All memoization = pure-function value reuse; same doubles, no RNG in the density path
  (perlin deterministic from coords); draw order untouched.

## 3. Fill profile decomposition (SPROF) — PENDING BUILD_OK

## 4. Changes + measured deltas

Patch written (not yet compiled — CPU gate). Worktree /home/z/wt_h6b, files: native/src/density.rs,
native/src/nochunk.rs, native/src/bin/dpperf.rs (WIP diag), +2 bins (h6probe.rs WIP, h6heights.rs new).

- **L1 batch-spline** (density.rs, batch `WKind::Spline` arm): the point VALUES are no longer
  evaluated per-lane via `spline_apply(..., mem=None)`; instead each DISTINCT point-value node
  needed by the group (<=2 per lane, deduped) is sampled ONCE via `eval_batch_uncached` over all
  N lanes, then the per-lane f32 arithmetic replicates spline_apply op-for-op (same binary-search
  interval, same f32 expression order incl. the i<0 / i==last linear-extend branches). Effect:
  nested spline coordinate subtrees (the terrain offset/factor/jaggedness splines over
  continents/erosion/ridges/temperature/vegetation) now flow through the batch memo -> stamps
  dedupe shared refs within the group AND the Cache2d column cache serves the 2D coordinate
  noises across the whole 245-corner plane (previously: 8 lanes x 49 y-groups, zero reuse).
- **L2 row-local grouping** (nochunk.rs `slice_slot0`): groups are composed WITHIN a z-row
  (y-run) instead of over the flat p index, so every batch is lane-uniform in (x,z). The old
  flat walk straddled a z-row restart every row (49 % G == 1 for G=4 and G=8) and those groups
  lost ALL column caching. Out layout + values bit-identical (pure corner functions, same flat
  p indexing; tail lanes still duplicate the last real corner; the all-lane y-const gate kept).
- **L3 plane/column cache extension**:
  - compile (density.rs `compute_col_caches`): ShiftA/ShiftB earn column slots (they pass y=0.0
    LITERALLY — pure f(x,z), caching bit-identical); y-invariant Noise(y_scale==0) and
    ShiftedNoise(y_scale==0 + invariant shifts) earn slots too (sign-of-zero equivalence class —
    the same one the production Cache2d column cache relies on; gates verify on real trees).
  - batch arms: Noise/ShiftedNoise (2D), ShiftA/ShiftB get per-plane per-(x,z) column-cache
    hit/store in `BatchMemo.col` (uniform-lane check kept — straddling groups fall back).
  - scalar arms (MemoCtx-with-cols paths): same four leaves get 16-way `col_lookup4/col_store4`
    caching; `ChunkGen.self.memo` and the parallel fill_slice per-thread memos are now col-armed
    (slots 1..N scalar path picks up the caches). base_height's slots memo left col-less (its
    slot-0 batch path gets the caches; allocs per 1-column query not worth it).
- Parity reasoning: all memoization = pure-function value reuse; same doubles, same f32 order;
  no RNG in the density path; draw order untouched. base_height shares slice_slot0 (covered by
  the new heights dump gate below).

## 5. cargo test + parity — PENDING

## 6. Not verified (handed to orchestrator) — PENDING

## 7. Artifacts — PENDING

## 8. Parity battery (planned)

1. `filldump` (dpext 01+02 roots, dptt region chunks) parent binary vs my binary — byte-diff.
2. heights: new `h6heights` bin (dump "x z world ocean" over a grid; format compatible with
   ProbePlugin.crusbase/baseref) — parent build (throwaway worktree /home/z/wt_h6base at
   0091c8ea + untracked bin, CARGO_TARGET_DIR=/home/z/tgt_h6base) vs my build — byte-diff.
   Replaces the lost heightcmp 1792/1792 gate.
3. `planecmp` (fill_slice vs eval_plane_into self-consistency) on my build.
4. cargo test --release (88/88 expected).
5. Orchestrator: CI judge on push (bit gate of record).

