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
        _ => {}
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

/// ops -> caller buffer layout: 5 i32 per op [x, y, z, stateIdx, flags]; returns op count
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
        if n * 5 + 4 >= out.len() {
            break;
        }
        out[n * 5] = op.x as i32;
        out[n * 5 + 1] = op.y as i32;
        out[n * 5 + 2] = op.z as i32;
        out[n * 5 + 3] = op.state as i32; // per-op state resolution lands with op-state store
        out[n * 5 + 4] = op.flags as i32;
        n += 1;
    }
    n
}
