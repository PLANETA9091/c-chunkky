# H7: ore-family native compute — cont139-H7 report

Agent: Rust perf engineer, branch `cont139-h7`, worktree `/home/z/wt_h7b` (base origin/main = 0091c8ea).
Hypothesis H7: ore-family native compute ~5.5ms/ch (8 types) + dripstone ~0.9ms/ch — per-cell
overlay query cost inside deco run phase (8.5ms loaded, honest worker=2).

## STATUS: IN PROGRESS (flushed after every step)

## Step 0 — WIP evaluation (DONE)

WIP found in `/home/z/wt_h7` (branch h7-ore-b, base 64d2f119, uncommitted changes to
features_deco.rs / features_place.rs / features_snap.rs). Saved verbatim to
`agent-scratch/h7/wip_diff_from_wt_h7.patch` (971 lines).

Diff content (from static review):
- **H7a — per-section ore target classification**: `OrePlan`/`RCls` compiled rng-free
  target ruleset (RandomBlockMatch REJECTED -> plan None -> exact path, RNG draw order
  preserved by construction); `SecCls` per-section verdict table (palette entry x rule ->
  VD_FALSE/VD_TRUE/VD_MIXED); `OreClassScan` trait (default None; ChunkLevel impl);
  `place_ore` consults `cell_verdicts` per cell: overlay hit / unsnapshotted / stale epoch /
  Mixed section / dead plan all bail to the exact per-cell chain. HeightMatch tri-state
  per section; conservative: ANY Mixed verdict in the cell -> exact path.
- **H7c — fused `is_air_or_water`** dripstone predicate (speleothem, large_dripstone,
  speleothem_cluster): verbatim composition on bridge, single snapshot decode
  (IS_AIR|FLUID_WATER) on ChunkLevel; overlay branch keeps is_air air-name set semantics.
- **Kill-switch `CRUS_H7_OFF=1`** (+ test/bench override `H7_FORCE` atomic evaluated first).
- **3 new tests**: `h7_ore_op_stream_identical_killswitch`, `h7_cell_verdict_table`,
  `h7_dripstone_op_stream_identical_killswitch` (op-stream referee ON vs kill-switch OFF).

Decision: **ADOPT** (coherent, parity-minded design: rng-free plans only, overlay-first bail,
epoch-aware section cache, conservative Mixed fallback, kill-switch default OFF, semantic
referee tests). Verified `64d2f119` is an ancestor of origin/main and none of the 3 newer
commits (docs x2 + rings-warmup paper-side) touch native/src -> patch applies clean onto
`0091c8ea` (verified: `git apply --check` clean, applied to /home/z/wt_h7b).

Compile/parity verdict still PENDING: cargo build+test blocked by CPU GATE (BUILD_OK not
present yet).

## Step 1 — static verification of WIP against real signatures (IN PROGRESS)

Checklist while waiting for BUILD_OK:
- [ ] features_json::RuleTest variant shapes (BlockMatch/TagMatch inner type)
- [ ] ChunkLevel::new signature, SnapSection fields, unpack() signature, flags consts
- [ ] place_ore / place_speleothem* signatures vs test call sites
- [ ] tag_matches_name / tag_registry_is helpers
- [ ] ore_prof counters (S_NODES/S_TAG) untouched

## Step 2 — parent SPROF profile (BLOCKED on BUILD_OK)

## Step 3 — build+test (BLOCKED on BUILD_OK)

## Step 4 — AFTER profile + bit-parity dumps (BLOCKED)

## Artifacts
- branch: cont139-h7 (commit TBD)
- .so md5: TBD
