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

## 4. Changes + measured deltas — PENDING

## 5. cargo test + parity — PENDING

## 6. Not verified (handed to orchestrator) — PENDING

## 7. Artifacts — PENDING
