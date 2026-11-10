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

## Step 1 — static verification of WIP against real signatures (DONE — all PASS)

- [x] `features_json::RuleTest` variants: BlockMatch(String)/TagMatch(String)/RandomBlock/HeightMatch{lo,hi}/Not/AnyOf/AllOf/True — match `compile_rcls` + test constructors
- [x] `ChunkLevel::new(inner, ov, ov_cols, snap, loader, hm_loader, uid_names, zoom_seed, st_flags)` — test call matches; loader gets ABSOLUTE (cx,cz,sec_y)
- [x] `SnapSection` fields (sec_y, epoch, bits, names, pal, raw, bio_*) match; `unpack(&bits,&raw,idx)` matches; flags consts exist (IS_AIR=1, FLUID_WATER=1024, SOLID=32...)
- [x] `place_ore(ctx, discard, size, targets, rng, origin, ops, scattered)` matches test call
- [x] `place_speleothem`/`place_large_dripstone`/`place_speleothem_cluster` signatures match test call sites
- [x] `tag_matches_name` + `tag_registry_is` live in features_snap.rs (classify_section is in the same module; tag_registry_is pub)
- [x] ore_prof counters S_TAG/S_NODES/S_HM/S_AIR untouched (drain consumer in features_jni prof_chunk_tick)
- [x] DecoLevel/DecoLevelExt vs SnapMock: all required methods covered; sea_level+ext defaults fine
- [x] tests module: `use super::*` covers HeightmapTy/RuleTest/etc.; build_reg/EMPTY_U32/EMPTY_IDX in scope; emit in place_ore is a local closure (place_ore defines `let emit = |ctx,...|`) — mystery resolved, compiles
- [x] RNG parity: `rule_test_matches` draws ONLY via RandomBlock (short-circuit on state match); plans REJECT RandomBlock → rng-free; fast path swaps only the target test, discard draw + isAdjacentToAir + emit order untouched; scattered path keeps exact chain (plan gated on `!scattered`)
- [x] Tag chain equivalence: per-cell `tag_is` snapshot leg = matches-name -> registry -> bridge; classification tag_ans = matches-name -> registry, and DIES (plan.dead -> exact path) when the bridge would be needed. For any verdict served, answer == per-cell chain answer
- [x] HeightMatch tri-state per section [sec*16, sec*16+15] vs per-cell `lo <= pos.y <= hi` (absolute y) — consistent; conservative Mixed fallback
- [x] 26.3 tag ground truth (reference/vanilla263/data/minecraft/tags/block/): height_specific_ore_replaceables=[tuff], deepslate_ore_replaceables=[deepslate], stone_ore_replaceables=[stone,granite,diorite,andesite], dripstone_replaceable_blocks=[#base_stone_overworld]. tag_matches_name's tuff-in-deepslate_ore_replaceables answer is unreachable in rule2 (gated by Not(hspec), tuff∈hspec) — pre-existing parent behavior, not touched by H7
- [x] embed check: full vanilla 26.3 configured+placed features (all 8 ore families with real chains, dripstone_cluster/large_dripstone/pointed_dripstone) are embedded in features_vanilla_data263.rs (VANILLA_CONFIGURED_263 / VANILLA_PLACED_263) → headless replay is possible
- [x] dpperf CRUS_SPROF = surface-FILL kinds only (SR_*), NOT deco → for H7 I need a deco-replay bench: dpperf covers fill; the lost COMPUTE_TOP was live-server [run-prof-top] (per-16-ch window, server-only). Since Paper runs are forbidden to me, I built `h7bench` (see Step 2)

Decision recap: **ADOPT** WIP. Kill-switch convention = CRUS_H7_OFF=1 restores pre-H7 path
(same convention as CRUS_DELEG_NOGEN: new behavior default-ON, env restores parent semantics;
A/B is then one binary, two env settings — and the kill-switch path IS the parent by construction).

## Step 2 — h7bench (headless deco compute bench + parity dump) (IN PROGRESS)

No live server allowed (rule 5) and dpperf profiles only the fill phase, so COMPUTE_TOP
is re-gathered headlessly: `native/src/bin/h7bench.rs` replays the REAL placed features
(real pack + embedded vanilla 26.3 ore/dripstone chains) through the real place engine
(`run_feature` → place_placed → expand_deco → place_ore/speleothem...) against a
ChunkLevel snapshot built from native fill() of the 3x3 region (real terrain mix:
stone/deepslate/granite... + air caves + water/lava). Per-feature ns+ops per chunk,
3+ rounds, p50. `--dump FILE` writes the op stream → byte-diff fast vs CRUS_H7_OFF=1.

## Artifacts
- branch: cont139-h7 (commit TBD)
- .so md5: TBD

## Step 2b — h7bench design notes (flushed pre-build)

- Entry = `run_feature_master` (the PRODUCTION features_jni decoRunFeature call), NOT
  run_feature: run_feature wraps the level in `OverlayLevel`, and OverlayLevel does NOT
  forward `ore_class()`/`env_col_scan()` (trait defaults None) — through that wrapper the
  H7 fast path would never engage. Production deco (decoRunFeature) and the ore hook
  (oreRunConfigured) both construct the ChunkLevel and pass it UNWRAPPED, so ore_class()
  engages live. (Discovered latent cont-60 hazard for any future run_feature user — noted,
  not fixed: prod path unaffected.)
- Level = ChunkLevel over a 3x3-region snapshot built from native fill() (frozen 22-id
  palette; air caves/water/lava + base stones = the real per-cell mix for ore targets).
  `SnapSection` gained #[derive(Clone)] (additive, plain data) so the bench loader can
  serve cached sections.
- BiomeFilter: one dummy biome passes everything (upper-bound dripstone counts; identical
  for both A/B paths — parity unaffected; live ratio comes from honest-window numbers).
- Bridge mock supplies the REAL 26.3 tag member sets (hspec=[tuff], ds_rep=[deepslate],
  stone_rep=4 stones, base_stone_overworld=6, dripstone_replaceable_blocks=6).
- Parity protocol: same binary, two env settings (default=FAST vs CRUS_H7_OFF=1=parent
  semantics), `--dump` framed op streams byte-compared via agent-scratch/h7/diff_dump.py.
- A/B commands (after BUILD_OK):
  `cd /home/z/wt_h7b/native && CARGO_TARGET_DIR=/home/z/tgt_h7b cargo build --release && CARGO_TARGET_DIR=/home/z/tgt_h7b cargo test --release`
  `R1=/home/z/my-project/c-faster/workdir/dpext/01; R2=/home/z/my-project/c-faster/workdir/dpext/02`
  `CRUS_H7_OFF=1 ./h7bench $R1 $R2 -- 64 2100 2000 --rounds 3 --dump off.bin`
  `./h7bench $R1 $R2 -- 64 2100 2000 --rounds 3 --dump fast.bin`
  `python3 .../diff_dump.py off.bin fast.bin`
  fill parity vs pinned parent .so: `filldump $R1 $R2 -- cx cz seed out` parent-vs-mine,
  byte-compare (fill untouched by H7 — regression guard).

