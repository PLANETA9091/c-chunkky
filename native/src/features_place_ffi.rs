//! Features FFI (stage F1): registry boot over JNI, decoStates/decoSetIds handshake,
//! decoRunFeature entry (one placed feature per call, exact seeding protocol).
//!
//! Java order: decoAddConfigured(name,json)* -> decoAddPlaced(name,json)* -> decoSeal
//! -> decoStates -> decoSetIds(short[]) -> per chunk: decoRunFeature(name, index, step)
//! -> apply returned ops with ensureCanWrite(3x3) semantics.

use crate::features_json::{decode_registry, FeatureRegistry};
use crate::features_place::{place_placed, DecoOp, PlaceCtx};
use crate::features_placement::BlockPos;
use crate::features_rng::WorldgenRandom;

pub struct DecoState {
    pub reg: FeatureRegistry,
    /// distinct state strings, first-appearance order (decoStates/decoSetIds handshake)
    pub states: Vec<String>,
    pub state_index: std::collections::HashMap<String, u16>,
    /// placed name -> uid (vanilla topFeature identity for BiomeFilter)
    pub uid_index: std::collections::HashMap<String, u32>,
}

impl DecoState {
    /// uid_order = the featuresPerStep flattened order (DecoCtx.uid_names):
    /// BiomeFilter uids must live in the SAME space the bridge maps back to
    /// names (JniLevel receives exactly this slice).
    pub fn seal(mut reg: FeatureRegistry, uid_order: &[String]) -> Self {
        let mut states = Vec::new();
        let mut idx = std::collections::HashMap::new();
        // deterministic order: sorted configured names (JSON maps are ordered in our parser)
        let mut cfg_names: Vec<&String> = reg.configured.keys().collect();
        cfg_names.sort();
        for name in cfg_names {
            index_states_cfg(&reg.configured[name], &mut states, &mut idx);
        }
        let mut uid_index = std::collections::HashMap::new();
        for (i, n) in uid_order.iter().enumerate() {
            uid_index.insert(n.clone(), i as u32);
        }
        DecoState { reg, states, state_index: idx, uid_index }
    }
}

fn note(s: &str, states: &mut Vec<String>, idx: &mut std::collections::HashMap<String, u16>) {
    if !s.starts_with("ROTATED\0") && !idx.contains_key(s) {
        idx.insert(s.to_string(), states.len() as u16);
        states.push(s.to_string());
    }
    if let Some(inner) = s.strip_prefix("ROTATED\0") {
        note(inner, states, idx);
    }
}

fn index_states_cfg(
    cfg: &crate::features_json::FeatureCfg,
    states: &mut Vec<String>,
    idx: &mut std::collections::HashMap<String, u16>,
) {
    use crate::features_json::{FeatureCfg as FC, StateSrc as SS};
    match cfg {
        FC::SimpleBlock { place, .. } => index_state_src(place, states, idx),
        FC::ReplaceBlock { targets } => {
            for (_, src) in targets {
                index_state_src(src, states, idx);
            }
        }
        FC::Ore { targets, .. } | FC::ScatteredOre { targets, .. } => {
            for (_, st) in targets {
                note(st, states, idx);
            }
        }
        FC::Disk { provider, .. } => index_state_src(provider, states, idx),
        FC::Tree(t) => index_tree_states(t, states, idx),
        FC::VegetationPatch(v) => index_state_src(&v.ground_state, states, idx),
        _ => {}
    }
}

/// enumerate every state string a tree can write: providers + leaf distance
/// variants (1..7) + waterlogged variants + decorator fixed states
fn index_tree_states(
    t: &crate::features_json::TreeCfg,
    states: &mut Vec<String>,
    idx: &mut std::collections::HashMap<String, u16>,
) {
    use crate::features_json::StateSrc as SS;
    let mut all: Vec<(&SS, bool)> = vec![
        (&t.trunk_provider, false),
        (&t.foliage_provider, true),
        (&t.dirt_provider, false),
    ];
    if let Some(rp) = &t.root_placer {
        all.push((&rp.root_provider, true));
        all.push((&rp.muddy_roots_provider, true));
        if let Some((ap, _)) = &rp.above_root {
            all.push((ap, true));
        }
    }
    for d in &t.decorators {
        match d {
            crate::features_json::TreeDeco::AlterGround(p)
            | crate::features_json::TreeDeco::AttachedToLogs { provider: p, .. }
            | crate::features_json::TreeDeco::AttachedToLeaves { provider: p, .. }
            | crate::features_json::TreeDeco::PlaceOnGround { provider: p, .. } => {
                all.push((p, false));
            }
            _ => {}
        }
    }
    for (src, is_leaf) in all {
        let mut variants: Vec<String> = Vec::new();
        collect_state_src(src, &mut variants);
        for v in variants {
            note(&v, states, idx);
            if is_leaf {
                for d in 1..=7 {
                    note(&crate::features_tree::set_prop(&v, "distance", &d.to_string()), states, idx);
                }
            }
            // waterlogged=true write variant (root/leaf fluid override)
            note(
                &crate::features_tree::set_prop(&v, "waterlogged", "true"),
                states,
                idx,
            );
        }
    }
    // fixed decorator states
    for s in [
        "minecraft:vine[north=true,east=false,south=false,west=false,up=false]",
        "minecraft:vine[north=false,east=true,south=false,west=false,up=false]",
        "minecraft:vine[north=false,east=false,south=true,west=false,up=false]",
        "minecraft:vine[north=false,east=false,south=false,west=true,up=false]",
        "minecraft:vine[north=false,east=false,south=false,west=false,up=true]",
        "minecraft:bee_nest[facing=south,honey_level=0]",
        "minecraft:pale_hanging_moss[tip=false]",
        "minecraft:pale_hanging_moss[tip=true]",
        "minecraft:creaking_heart[creaking_heart_state=dormant,natural=true]",
    ] {
        note(s, states, idx);
    }
    for age in 0..3 {
        for facing in ["north", "east", "south", "west"] {
            note(
                &format!("minecraft:cocoa[age={},facing={}]", age, facing),
                states,
                idx,
            );
        }
    }
}

fn collect_state_src(
    src: &crate::features_json::StateSrc,
    out: &mut Vec<String>,
) {
    use crate::features_json::StateSrc as SS;
    match src {
        SS::Simple(s) => {
            match s.strip_prefix("ROTATED\0") {
                Some(inner) => out.push(inner.to_string()),
                None => out.push(s.clone()),
            }
        }
        SS::Weighted(list) => {
            for (_, s) in list {
                collect_state_src(&SS::Simple(s.clone()), out);
            }
        }
        SS::RuleBased { fallback, rules } => {
            collect_state_src(fallback, out);
            for (_, then) in rules {
                collect_state_src(then, out);
            }
        }
        SS::Unsupported(_) => {}
    }
}

fn index_state_src(
    src: &crate::features_json::StateSrc,
    states: &mut Vec<String>,
    idx: &mut std::collections::HashMap<String, u16>,
) {
    use crate::features_json::StateSrc as SS;
    match src {
        SS::Simple(s) => note(s, states, idx),
        SS::Weighted(list) => {
            for (_, s) in list {
                note(s, states, idx);
            }
        }
        SS::RuleBased { fallback, rules } => {
            index_state_src(fallback, states, idx);
            for (_, then) in rules {
                index_state_src(then, states, idx);
            }
        }
        SS::Unsupported(_) => {}
    }
}

/// ops -> caller buffer layout: 6 i32 per op [x, y, z, stateIdx, flags, aux]
pub fn run_feature(
    state: &DecoState,
    level: &dyn crate::features_deco::DecoLevelExt,
    seed: i64,
    chunk_block_x: i32,
    chunk_block_z: i32,
    name: &str,
    index: i32,
    step: i32,
    out: &mut [i32],
) -> usize {
    let mut rng = WorldgenRandom::new();
    let deco = rng.set_decoration_seed(seed, chunk_block_x, chunk_block_z);
    rng.set_feature_seed(deco, index, step);
    // overlay: intra-feature reads must see the feature's own writes (vanilla
    // setBlock is live; ops flush to the region after the feature returns)
    let overlay = crate::features_deco::OverlayLevel::new(level);
    let ctx = PlaceCtx {
        reg: &state.reg,
        level: &overlay,
        center_x: chunk_block_x,
        center_z: chunk_block_z,
        state_index: &state.state_index,
        uid_index: &state.uid_index,
    };
    let mut ops: Vec<DecoOp> = Vec::new();
    place_placed(&ctx, name, &mut rng, BlockPos::new(chunk_block_x, level.min_y(), chunk_block_z), &mut ops, 0);
    // resolve state strings -> table indices
    let mut n = 0;
    for op in &ops {
        if n * 6 + 5 >= out.len() {
            break;
        }
        out[n * 6] = op.x as i32;
        out[n * 6 + 1] = op.y as i32;
        out[n * 6 + 2] = op.z as i32;
        out[n * 6 + 3] = op.state as i32;
        out[n * 6 + 4] = op.flags as i32;
        out[n * 6 + 5] = op.aux as i32;
        n += 1;
    }
    n
}
