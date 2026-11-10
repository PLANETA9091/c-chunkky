//! Features master loop (stage F1): FeatureSorter.buildFeaturesPerStep port +
//! per-chunk run plan + per-feature run protocol (Java drives the loop, native
//! expands exactly). Bit-exact with vanilla 1.21.8 (decomp src14 ChunkGenerator
//! addVanillaDecorations + biome/FeatureSorter).
//!
//! FeatureSorter semantics (decompiled src20):
//! - globalId: placed features numbered by first appearance across
//!   possibleBiomes in biome-source order.
//! - per-biome flat entry list (step-major, list order); each entry gets an
//!   edge to the NEXT entry of the SAME biome (dependency = "comes before").
//! - DFS over TreeMap<(step, gid)> keys ascending, post-order append, then
//!   reverse -> final order. Cycles impossible (edges strictly forward within
//!   a biome chain) but preserved as Err for parity.
//! - featuresPerStep[step] = final order filtered by step; indexMapping =
//!   position in that filtered list (Util.createIndexIdentityLookup).

use crate::features_place::{place_placed, DecoOp, PlaceCtx};
use crate::features_placement::BlockPos;
use crate::features_rng::WorldgenRandom;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

pub struct DecoPlan {
    /// placed feature names per step in final order (position = seed index i3)
    pub steps: Vec<Vec<String>>,
    /// placed name -> (step, index in step)
    pub index_of: HashMap<String, (usize, usize)>,
}

type Node = (i32, u32); // (step, globalId)

/// Everything the deco engine needs at run time: sealed registry + state
/// table, the featuresPerStep plan, and the per-biome feature lists of the
/// possible biomes (BiomeFilter + chunk_plan sources).
pub struct DecoCtx {
    pub state: crate::features_place_ffi::DecoState,
    pub plan: DecoPlan,
    /// biome name -> features[step] -> placed refs (possibleBiomes only)
    pub biome_features: HashMap<String, Vec<Vec<String>>>,
    /// step-major flattened plan order: uid -> placed name (BiomeFilter ids)
    pub uid_names: Vec<String>,
    /// chunk-scoped feature-write overlay (ALL of this chunk's feature
    /// writes; the ops flush to the region after each feature, but the
    /// snapshot + later features must see them live)
    pub chunk_ov: std::cell::RefCell<crate::features_snap::OvMap>,
    /// (x, z) -> ys of the chunk overlay entries in that column — the EnvScan
    /// column fastpath's window probe (maintained by write_overlay / the
    /// ops-dropped removal; cleared with chunk_ov)
    pub chunk_ov_cols:
        std::cell::RefCell<HashMap<(i32, i32), Vec<i32>>>,
    /// lazy 3x3 region section snapshot (perf pass — no per-query JNI)
    pub chunk_snap: std::cell::RefCell<crate::features_snap::SnapRegion>,
    /// WG heightmap snapshots per region chunk: (cx, cz) -> 512 i32 (WS_WG+OF_WG,
    /// +1 semantics). Cached across the whole chunk deco (ChunkLevel is
    /// re-created per feature); cleared in decoChunkBegin AND on every
    /// decoDelegateMark (vanilla delegate writes update the real region maps
    /// behind the cache — re-handshake at the next query). The inner
    /// RefCell allows in-place vanilla Heightmap.update bookkeeping on the
    /// overlay writes (update-on-write keeps later height() reads exact).
    pub chunk_hms:
        std::cell::RefCell<HashMap<(i32, i32), Option<std::sync::Arc<std::sync::Mutex<Vec<i32>>>>>>,
    /// BiomeManager.obfuscateSeed(world seed) — the biome zoom fiddle seed
    pub chunk_zoom_seed: std::cell::Cell<i64>,
    /// S1 sub-batch (cont139): ops of a feature that overflowed the pinned ops
    /// buffer mid-batch. They were already written into the chunk overlay
    /// (write_overlay at op production) and MUST be applied — the next
    /// decoRunBatch call drains them first. Empty outside a batch sequence;
    /// cleared by decoChunkBegin (a carry never outlives its chunk).
    pub batch_carry: std::cell::RefCell<Vec<DecoOp>>,
}

impl DecoCtx {
    pub fn uid_of(&self, placed: &str) -> Option<u32> {
        self.plan
            .index_of
            .get(placed)
            .map(|(s, i)| (self.steps_before(*s) + i) as u32)
    }
    pub fn steps_before(&self, step: usize) -> usize {
        self.plan.steps.iter().take(step).map(|v| v.len()).sum()
    }
}

/// Build a DecoCtx from datapack assets (paths "<ns>/worldgen/..."):
/// - biome source order from dimension/overworld.json (= possibleBiomes order)
/// - per-biome features[step] lists from worldgen/biome/*.json
/// - registry from worldgen/configured_feature + placed_feature/*.json
/// Returns None when the assets carry no data-driven multi_noise dimension.
pub fn build_deco_ctx(
    assets: &crate::worldgen::Assets,
) -> Option<DecoCtx> {
    let Some(src) = crate::biomes::BiomeSourceData::from_assets(assets) else {
        crate::ffi::log_warn("deco: pack biome source data missing (build_deco_ctx)");
        return None;
    };
    build_deco_ctx_src(assets, &src)
}

/// build_deco_ctx with an EXTERNAL biome-source (embedded-vanilla 26.3: the
/// v263 ctx already owns BiomeState263.src from params263_overworld.json —
/// re-parsing from synthetic assets would risk a divergent biome order).
pub fn build_deco_ctx_src(
    assets: &crate::worldgen::Assets,
    src: &crate::biomes::BiomeSourceData,
) -> Option<DecoCtx> {
    let mut biome_features: HashMap<String, Vec<Vec<String>>> = HashMap::new();
    for name in &src.biome_names {
        let Some((ns, file)) = name.split_once(':') else {
            crate::ffi::log_warn(&format!("deco: biome name without namespace: {name}"));
            return None;
        };
        let key = format!("{ns}/worldgen/biome/{file}.json");
        let mut steps: Vec<Vec<String>> = Vec::new();
        if let Some(bytes) = assets.get(&key) {
            if let Ok(text) = std::str::from_utf8(bytes) {
                if let Ok(j) = crate::json::parse(text) {
                    if let Some(arr) = j.get("features").and_then(|v| v.as_arr()) {
                        for step in arr {
                            let mut list = Vec::new();
                            if let Some(hs) = step.as_arr() {
                                for e in hs {
                                    if let Some(s) = e.as_str() {
                                        list.push(s.to_string());
                                    }
                                }
                            }
                            steps.push(list);
                        }
                    }
                }
            }
        }
        biome_features.insert(name.clone(), steps);
    }
    // possibleBiomes order = biome-source parameter order
    let ordered: Vec<(String, Vec<Vec<String>>)> = src
        .biome_names
        .iter()
        .map(|n| {
            let empty = Vec::new();
            (n.clone(), biome_features.get(n).unwrap_or(&empty).clone())
        })
        .collect();
    let plan = match build_plan(&ordered) {
        Ok(p) => p,
        Err(e) => {
            crate::ffi::log_warn(&format!("deco: build_plan failed ({} biomes): {}", ordered.len(), e));
            return None;
        }
    };
    let uid_names: Vec<String> = plan.steps.iter().flatten().cloned().collect();

    // feature registry from assets
    let mut configured: Vec<(String, String)> = Vec::new();
    let mut placed: Vec<(String, String)> = Vec::new();
    for p in assets.paths() {
        let Some((kind, rest)) = p.split_once("/worldgen/") else { continue };
        let ns = kind.to_string(); // "<ns>" (data/ already stripped)
        let Some(bytes) = assets.get(p) else { continue };
        let Ok(json_text) = std::str::from_utf8(bytes) else { continue };
        if let Some(f) = rest.strip_prefix("configured_feature/") {
            if let Some(f) = f.strip_suffix(".json") {
                configured.push((format!("{ns}:{f}"), json_text.to_string()));
            }
        } else if let Some(f) = rest.strip_prefix("placed_feature/") {
            if let Some(f) = f.strip_suffix(".json") {
                placed.push((format!("{ns}:{f}"), json_text.to_string()));
            }
        }
    }
    // vanilla fallback: pack features WIN (pack names inserted first), the
    // built-in 1.21.8 set fills the gaps (minecraft:fancy_oak selectors,
    // minecraft:pale_moss_patch nested places, vanilla vegetation inners)
    let pack_cfg: std::collections::HashSet<String> =
        configured.iter().map(|(n, _)| n.clone()).collect();
    let pack_pl: std::collections::HashSet<String> =
        placed.iter().map(|(n, _)| n.clone()).collect();
    for (n, t) in crate::features_vanilla_data::VANILLA_CONFIGURED {
        if !pack_cfg.contains(*n) {
            configured.push(((*n).to_string(), (*t).to_string()));
        }
    }
    for (n, t) in crate::features_vanilla_data::VANILLA_PLACED {
        if !pack_pl.contains(*n) {
            placed.push(((*n).to_string(), (*t).to_string()));
        }
    }
    let reg = crate::features_json::decode_registry(
        configured.into_iter(),
        placed.into_iter(),
    );
    if reg.placed.is_empty() || reg.configured.is_empty() {
        crate::ffi::log_warn(&format!(
            "deco: decoded registry nearly empty (configured={} placed={} unsup_cfg={} unsup_pl={})",
            reg.configured.len(),
            reg.placed.len(),
            reg.unsupported_configured,
            reg.unsupported_placed
        ));
    }
    // extend the uid space: inner placed refs (vanilla trees behind pack
    // selectors, pale_moss_patch, vegetation inners) resolve their OWN
    // BiomeFilter uid — without a mapping the bridge would return false and
    // silently drop every inner placement
    let mut uid_names = uid_names;
    let known: std::collections::HashSet<&String> = uid_names.iter().collect();
    let mut extra: Vec<&String> = reg
        .placed
        .keys()
        .filter(|n| !known.contains(*n))
        .collect();
    extra.sort();
    for n in extra {
        uid_names.push(n.clone());
    }
    // Seed the global BiomeFilter map from the decoded biome features when
    // nothing was shipped yet. The Java setupDeco handshake only ships a map
    // when a datapack is present (PackData) — for the embedded-vanilla 26.3
    // ctx an EMPTY overwrite would make every native BiomeFilter answer
    // false and silently drop all features (biome_has_feature reads ONLY
    // BIOME_FULL). First seed wins; pack worlds overwrite via the handshake.
    {
        let mut bf = crate::features_snap::BIOME_FULL.write().unwrap();
        if bf.is_empty() {
            for (b, steps) in &biome_features {
                bf.insert(
                    b.clone(),
                    steps.iter().flatten().cloned().collect::<Vec<String>>(),
                );
            }
        }
    }
    Some(DecoCtx {
        state: crate::features_place_ffi::DecoState::seal(reg, &uid_names),
        plan,
        biome_features,
        uid_names,
        chunk_ov: std::cell::RefCell::new(crate::features_snap::OvMap::with_hasher(
            std::hash::BuildHasherDefault::<crate::features_snap::OvHasher>::default(),
        )),
        chunk_ov_cols: std::cell::RefCell::new(HashMap::new()),
        chunk_snap: std::cell::RefCell::new(crate::features_snap::SnapRegion::new(0, 0, -1024, 26)),
        chunk_hms: std::cell::RefCell::new(HashMap::new()),
        chunk_zoom_seed: std::cell::Cell::new(0),
        batch_carry: std::cell::RefCell::new(Vec::new()),
    })
}

/// FeatureSorter port. `possible_biomes` in biome-source order:
/// (name, features[step] -> placed refs).
pub fn build_plan(
    possible_biomes: &[(String, Vec<Vec<String>>)],
) -> Result<DecoPlan, String> {
    let mut gid_of: HashMap<&str, u32> = HashMap::new();
    let mut next_gid: u32 = 0;
    // nodes: (step, gid) -> edges ( TreeSet by (step, gid) )
    let mut graph: BTreeMap<Node, BTreeSet<Node>> = BTreeMap::new();
    let mut max_len: usize = 0;
    let mut name_of_gid: HashMap<u32, String> = HashMap::new();

    for (biome, feats) in possible_biomes {
        let _ = biome;
        max_len = max_len.max(feats.len());
        // per-biome flat entry list
        let mut entries: Vec<Node> = Vec::new();
        for (step, holderset) in feats.iter().enumerate() {
            for r in holderset {
                let gid = match gid_of.get(r.as_str()) {
                    Some(g) => *g,
                    None => {
                        let g = next_gid;
                        next_gid += 1;
                        gid_of.insert(r, g);
                        name_of_gid.insert(g, r.clone());
                        g
                    }
                };
                entries.push((step as i32, gid));
            }
        }
        // edges: entry[i] -> entry[i+1]
        for w in entries.windows(2) {
            graph.entry(w[0]).or_default().insert(w[1]);
        }
        // ensure every entry is a graph key (vanilla: map1.computeIfAbsent for all)
        for e in &entries {
            graph.entry(*e).or_default();
        }
    }

    // DFS over keys ascending; post-order append; reverse. (Acyclic by
    // construction; Err mirrors the vanilla cycle crash path.)
    let mut visited: HashSet<Node> = HashSet::new();
    let mut path: HashSet<Node> = HashSet::new();
    let mut post: Vec<Node> = Vec::new();
    for &k in graph.keys() {
        dfs(&graph, k, &mut visited, &mut path, &mut post)?;
    }
    post.reverse();

    let mut steps: Vec<Vec<String>> = vec![Vec::new(); max_len];
    let mut index_of: HashMap<String, (usize, usize)> = HashMap::new();
    for (step, gid) in &post {
        let s = *step as usize;
        if s >= steps.len() {
            // entry beyond every biome's len is impossible; guard anyway
            steps.resize(s + 1, Vec::new());
        }
        let idx = steps[s].len();
        let name = name_of_gid.get(gid).cloned().unwrap_or_default();
        steps[s].push(name.clone());
        index_of.insert(name, (s, idx));
    }
    Ok(DecoPlan { steps, index_of })
}

fn dfs(
    graph: &BTreeMap<Node, BTreeSet<Node>>,
    cur: Node,
    visited: &mut HashSet<Node>,
    path: &mut HashSet<Node>,
    post: &mut Vec<Node>,
) -> Result<(), String> {
    if visited.contains(&cur) {
        return Ok(());
    }
    if path.contains(&cur) {
        return Err("feature order cycle found".into());
    }
    path.insert(cur);
    if let Some(nexts) = graph.get(&cur) {
        for &n in nexts {
            dfs(graph, n, visited, path, post)?;
        }
    }
    path.remove(&cur);
    visited.insert(cur);
    post.push(cur);
    Ok(())
}

impl DecoPlan {
    /// (step, index) for a placed feature ref, or None when the ref is absent
    /// from the possible biomes (cannot be decorated).
    pub fn run_of(&self, placed: &str) -> Option<(usize, usize)> {
        self.index_of.get(placed).copied()
    }
}

/// Per-chunk run plan: union over `region_biomes` (the 3x3 biome set filtered
/// to possibleBiomes by the caller) of each biome's step lists, mapped to
/// (step, index), sorted per step by index ascending. Returned pairs are
/// step-major, index-ascending within a step (vanilla IntArraySet + sort).
pub fn chunk_plan(
    plan: &DecoPlan,
    biome_features: &HashMap<String, Vec<Vec<String>>>,
    region_biomes: &[String],
) -> Vec<(usize, usize)> {
    let mut set: HashSet<(usize, usize)> = HashSet::new();
    for b in region_biomes {
        let Some(steps) = biome_features.get(b) else { continue };
        for (step, holderset) in steps.iter().enumerate() {
            for r in holderset {
                if let Some(run) = plan.index_of.get(r.as_str()) {
                    if run.0 == step {
                        set.insert(*run);
                    }
                }
            }
        }
    }
    let mut runs: Vec<(usize, usize)> = set.into_iter().collect();
    runs.sort_unstable();
    runs
}

/// One master-loop feature run, STAGED: the exact seeding + placement body of
/// run_feature_master, returning the ops instead of serializing them. The S1
/// batch core reuses this so the batch path and the per-feature path share ONE
/// expansion implementation (bit-parity by construction).
pub fn run_feature_staged(
    state: &crate::features_place_ffi::DecoState,
    level: &dyn crate::features_deco::DecoLevelExt,
    deco_seed: i64,
    paper_seed: Option<i64>,
    origin: BlockPos,
    name: &str,
    step: usize,
    index: usize,
) -> Vec<DecoOp> {
    let mut rng = WorldgenRandom::new();
    let pop_seed = match paper_seed {
        Some(cfg) => rng.set_decoration_seed(cfg, origin.x, origin.z),
        None => deco_seed,
    };
    rng.set_feature_seed(pop_seed, index as i32, step as i32);
    let ctx = PlaceCtx {
        reg: &state.reg,
        level,
        center_x: origin.x,
        center_z: origin.z,
        state_index: &state.state_index,
        uid_index: &state.uid_index,
    };
    let mut ops: Vec<DecoOp> = Vec::new();
    place_placed(&ctx, name, &mut rng, origin, &mut ops, 0);
    ops
}

/// Serialize staged ops into `out` starting at op slot `base` with the exact
/// run_feature_master bounds guard (n*6+5 >= out.len()). Returns ops written.
#[inline]
fn serialize_ops(ops: &[DecoOp], out: &mut [i32], base: usize) -> usize {
    let mut n = 0;
    for op in ops {
        if (base + n) * 6 + 5 >= out.len() {
            break;
        }
        out[(base + n) * 6] = op.x as i32;
        out[(base + n) * 6 + 1] = op.y as i32;
        out[(base + n) * 6 + 2] = op.z as i32;
        out[(base + n) * 6 + 3] = op.state as i32;
        out[(base + n) * 6 + 4] = op.flags as i32;
        out[(base + n) * 6 + 5] = op.aux as i32;
        n += 1;
    }
    n
}

/// One master-loop feature run (vanilla body of the inner i2 loop):
/// Paper featureSeeds override -> setFeatureSeed -> placeWithBiomeCheck.
/// `out` layout: 6 i32 per op [x, y(abs), z, stateIdx, flags, aux]. Returns ops.
pub fn run_feature_master(
    state: &crate::features_place_ffi::DecoState,
    level: &dyn crate::features_deco::DecoLevelExt,
    deco_seed: i64,
    paper_seed: Option<i64>,
    origin: BlockPos,
    name: &str,
    step: usize,
    index: usize,
    out: &mut [i32],
) -> usize {
    let ops = run_feature_staged(state, level, deco_seed, paper_seed, origin, name, step, index);
    serialize_ops(&ops, out, 0)
}

// ---------------------------------------------------------------------------
// S1 sub-batch (cont139, H4): expand CONSECUTIVE (step, gid) runs in one JNI
// crossing. Legal because the chunk overlay (write_overlay at op production)
// + the WG heightmap emulation already make the native read path independent
// of the Java apply timing; the only live-region readers are the freezeChecks
// upcall (native freeze_top_layer) and the vanilla delegates — both are flush
// boundaries here. Bit-parity: same per-feature seeding/placement body
// (run_feature_staged), same op order, same overlay evolution.
// ---------------------------------------------------------------------------

/// Batch stop reason reported to Java (meta[1]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BatchBoundary {
    /// all runs expanded
    End = 0,
    /// run `start + completed` is a vanilla delegate (Java runs it + marks)
    Delegate = 1,
    /// run `start + completed` is freeze_top_layer with ops pending — flush
    /// first; the feature itself runs at the next call start (written == 0),
    /// so its live-region freezeChecks upcalls see the flushed state
    Freeze = 2,
    /// the ops buffer cannot hold the next feature's ops — they carry over in
    /// DecoCtx.batch_carry and are drained first on the next call
    Full = 3,
}

/// Result of one batch expansion.
pub struct BatchResult {
    /// runs whose ops are in the output buffer (a drained carry counts as 1)
    pub completed: usize,
    /// total ops written to the output buffer this call
    pub ops_written: usize,
    /// why the batch stopped
    pub boundary: BatchBoundary,
    /// (ops, nanos) per completed run — Java stats + native PROF_FEAT feed
    pub per_run: Vec<(usize, u64)>,
}

/// freeze_top_layer is the only native feature whose placement READS/WRITES
/// the live region through the bridge (freezeChecks upcall per 16x16 column:
/// region.getHeight(MOTION_BLOCKING) + region.setBlock ice/snow). It must run
/// at a flush boundary so the live region matches the old per-feature timing.
#[inline]
pub fn is_freeze_like(name: &str) -> bool {
    name == "minecraft:freeze_top_layer"
}

/// S1 batch core: expand runs[start..] until a boundary. `carry` holds the ops
/// of the feature that overflowed the PREVIOUS call (drained first, attributed
/// to run `start`); on a Full boundary it receives the current feature's ops.
/// `per_run_out[k]` is written as each run completes (a panic mid-batch leaves
/// the written prefix consistent with `progress`). `clock` timestamps each
/// staged feature (native per-run wall for PROF_FEAT; pass a no-op clock in
/// tests). `progress` is updated after every completed feature so a panic
/// mid-batch still reports the valid prefix.
pub fn run_batch_core(
    state: &crate::features_place_ffi::DecoState,
    level: &dyn crate::features_deco::DecoLevelExt,
    deco_seed: i64,
    paper_seed: Option<i64>,
    origin: BlockPos,
    plan: &DecoPlan,
    runs: &[(usize, usize)],
    start: usize,
    out: &mut [i32],
    per_run_out: &mut [i32],
    carry: &mut Vec<DecoOp>,
    clock: &dyn Fn() -> u64,
    progress: &std::cell::Cell<(usize, usize)>,
) -> BatchResult {
    let mut completed = 0usize;
    let mut written = 0usize;
    let mut per_run: Vec<(usize, u64)> = Vec::new();
    let mut boundary = BatchBoundary::End;
    let mut r = start;

    // 1. drain the carry: ops of run `start` staged by the previous call (the
    // buffer was full then). They are already in the overlay — apply as-is.
    // A carry larger than the whole buffer (pathological >cap feature) drains
    // across several calls: report Full with completed=0 until it is empty.
    if !carry.is_empty() {
        let drained = serialize_ops(carry, out, written);
        written += drained;
        if 0 < per_run_out.len() {
            per_run_out[0] = drained as i32;
        }
        per_run.push((drained, 0));
        carry.drain(..drained);
        if carry.is_empty() {
            completed = 1;
            r = start + 1;
        } else {
            progress.set((completed, written));
            return BatchResult {
                completed,
                ops_written: written,
                boundary: BatchBoundary::Full,
                per_run,
            };
        }
        progress.set((completed, written));
    }

    while r < runs.len() {
        let (step, gid) = runs[r];
        let Some(name) = plan.steps.get(step).and_then(|v| v.get(gid)) else {
            boundary = BatchBoundary::End;
            break;
        };
        // delegate gate BEFORE running (same check + order as decoRunFeature)
        if !crate::features_place::placed_fully_supported(&state.reg, name, 0) {
            boundary = BatchBoundary::Delegate;
            break;
        }
        // freeze gate: the feature itself still runs at the NEXT call start
        // (written == 0 there), seeing the flushed live region like the old
        // per-feature protocol
        if is_freeze_like(name) && written > 0 {
            boundary = BatchBoundary::Freeze;
            break;
        }
        let t0 = clock();
        let staged = run_feature_staged(state, level, deco_seed, paper_seed, origin, name, step, gid);
        let dt = clock().saturating_sub(t0);
        // serialize with the exact run_feature_master bounds guard; a partial
        // serialize means the buffer filled mid-feature:
        //   written > 0 — the tail carries over (it is already in the overlay
        //                 and MUST be applied — never dropped); this call ends
        //                 with a Full boundary, the next one drains first.
        //   written == 0 — the feature alone exceeds the buffer (>65535 ops,
        //                 pathological): the tail is DROPPED — the exact
        //                 pre-cont139 truncation semantics (old serialized
        //                 with the same guard into the same buffer).
        let n = serialize_ops(&staged, out, written);
        if n < staged.len() {
            if written > 0 {
                carry.extend_from_slice(&staged[n..]);
            }
            written += n;
            per_run.push((n, dt));
            let k = completed;
            if k < per_run_out.len() {
                per_run_out[k] = n as i32;
            }
            completed += 1;
            r += 1;
            progress.set((completed, written));
            boundary = BatchBoundary::Full;
            break;
        }
        written += n;
        let k = completed;
        if k < per_run_out.len() {
            per_run_out[k] = n as i32;
        }
        per_run.push((n, dt));
        completed += 1;
        r += 1;
        progress.set((completed, written));
    }

    BatchResult {
        completed,
        ops_written: written,
        boundary,
        per_run,
    }
}

/// Decoration seed `l` for the chunk (vanilla line:
/// worldgenRandom.setDecorationSeed(level.getSeed(), origin.x, origin.z)).
pub fn decoration_seed(seed: i64, origin_x: i32, origin_z: i32) -> i64 {
    let mut rng = WorldgenRandom::new();
    rng.set_decoration_seed(seed, origin_x, origin_z)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bf(pairs: &[(&str, Vec<Vec<&str>>)]) -> Vec<(String, Vec<Vec<String>>)> {
        pairs
            .iter()
            .map(|(n, v)| {
                (
                    n.to_string(),
                    v.iter()
                        .map(|s| s.iter().map(|x| x.to_string()).collect())
                        .collect(),
                )
            })
            .collect()
    }

    /// Order within one biome: chain preserves flat list order per step.
    #[test]
    fn single_biome_chain() {
        let biomes = bf(&[("a", vec![vec!["f1"], vec!["f2", "f3"], vec!["f4"]])]);
        let plan = build_plan(&biomes).unwrap();
        assert_eq!(plan.steps.len(), 3);
        assert_eq!(plan.steps[0], vec!["f1"]);
        assert_eq!(plan.steps[1], vec!["f2", "f3"]);
        assert_eq!(plan.steps[2], vec!["f4"]);
        assert_eq!(plan.run_of("f3"), Some((1, 1)));
    }

    /// Two biomes: interleaving resolved by DFS over (step, gid) ascending —
    /// gid order = first appearance across biomes in source order.
    #[test]
    fn two_biome_interleave() {
        let biomes = bf(&[
            ("a", vec![vec!["x1", "x2"]]),
            ("b", vec![vec!["y1", "x1", "y2"]]),
        ]);
        let plan = build_plan(&biomes).unwrap();
        // gids: x1=0, x2=1 (biome a), y1=2, y2=3 (biome b)
        // edges a: (0,x1)->(0,x2); b: (0,y1)->(0,x1), (0,x1)->(0,y2)
        // keys ascending: (0,0) (0,1) (0,2) (0,3)
        // DFS from (0,0): nexts {(0,1),(0,3)}; post [1,3,0]; then (0,2): post [1,3,0,2]
        // reversed: [2,0,3,1] -> step0 = [y1, x1, y2, x2]
        assert_eq!(plan.steps[0], vec!["y1", "x1", "y2", "x2"]);
    }

    /// chunk_plan unions the region biomes' features and sorts by index.
    #[test]
    fn chunk_union_sorted() {
        let biomes = bf(&[
            ("a", vec![vec!["x1", "x2"]]),
            ("b", vec![vec!["y1", "x1", "y2"]]),
        ]);
        let plan = build_plan(&biomes).unwrap();
        let map: HashMap<String, Vec<Vec<String>>> =
            biomes.iter().cloned().collect();
        let runs = chunk_plan(&plan, &map, &["b".to_string()]);
        // b has y1,x1,y2 -> indexes from plan.steps[0] order [y1,x1,y2,x2]:
        // y1=0, x1=1, y2=2 -> sorted [0,1,2]
        assert_eq!(runs, vec![(0, 0), (0, 1), (0, 2)]);
        let runs2 = chunk_plan(&plan, &map, &["a".to_string(), "b".to_string()]);
        assert_eq!(runs2, vec![(0, 0), (0, 1), (0, 2), (0, 3)]);
    }

    /// Same feature in two biomes at different steps keeps both entries.
    #[test]
    fn cross_step_duplicate() {
        let biomes = bf(&[
            ("a", vec![vec!["f"], vec!["f"]]),
            ("b", vec![vec![], vec!["f", "g"]]),
        ]);
        let plan = build_plan(&biomes).unwrap();
        assert_eq!(plan.steps[0], vec!["f"]);
        // step1 chain: a: (1,f) -> b's (1,f) is the SAME node (same gid);
        // b edges: (1,f)->(1,g)
        assert_eq!(plan.steps[1], vec!["f", "g"]);
    }

    /// decoration_seed matches the RNG-golden pins (features_rng DECO cases).
    #[test]
    fn deco_seed_golden() {
        assert_eq!(decoration_seed(1234567890, 0, 0), 1234567890);
        assert_eq!(decoration_seed(1234567890, 16, -16), -8357490867922322894);
    }
}
