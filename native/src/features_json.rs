//! Features JSON decode (stage F1): placed_feature / configured_feature -> native structs.
//! Strict vanilla-loader semantics: unknown types -> Unsupported (element skipped by the
//! server's registry loader too — parity by construction). Coverage stats in tests.

use crate::features_deco::*;
use crate::features_placement::{HeightProvider as HP, IntProvider as IP, Modifier as PM};
use crate::json::{self, Json};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Config model
// ---------------------------------------------------------------------------

/// RuleTest (structure templatesystem) as used by ore targets.
#[derive(Clone, Debug)]
pub enum RuleTest {
    True,
    BlockMatch(String),
    TagMatch(String),
    /// RandomBlockMatchTest: state.is(block) && nextFloat() < probability
    RandomBlock { block: String, probability: f32 },
}

#[derive(Clone, Debug)]
pub enum StateSrc {
    Simple(String),
    Weighted(Vec<(u32, String)>),
    /// RuleBasedBlockStateProvider: first matching rule wins, else fallback.
    /// Rules are (if_true predicate, then provider); getState draws RNG only
    /// through the picked provider (weighted/rotated/noise leaves).
    RuleBased {
        fallback: Box<StateSrc>,
        rules: Vec<(BlockPredicate, StateSrc)>,
    },
    Unsupported(&'static str),
}

#[derive(Clone, Debug)]
pub enum FeatureCfg {
    NoOp,
    SimpleBlock {
        place: StateSrc,
        ticks: bool,
    },
    ReplaceBlock {
        targets: Vec<(BlockPredicate, StateSrc)>,
    },
    /// minecraft:ore — BulkSectionAccess writes (raw, no heightmap updates)
    Ore {
        discard: f32,
        size: i32,
        targets: Vec<(RuleTest, String)>,
    },
    /// minecraft:scattered_ore — region setBlock flags=2
    ScatteredOre {
        discard: f32,
        size: i32,
        targets: Vec<(RuleTest, String)>,
    },
    RandomSelector {
        entries: Vec<(f32, String)>,
        default: String,
    },
    SimpleRandomSelector {
        entries: Vec<String>,
    },
    RandomBoolSelector {
        chance: f32,
        if_true: String,
        if_false: String,
    },
    /// minecraft:random_patch — 2 draws per axis per try (X, Y, Z), inner
    /// placed feature placed with the SAME rng stream (no reseed, vanilla).
    /// `inner` = synthetic registry id registered at decode time.
    RandomPatch {
        tries: i32,
        xz_spread: i32,
        y_spread: i32,
        inner: String,
    },
    /// minecraft:disk — radius IntProvider sampled ONCE; x-outer/z-inner
    /// column scan with dx²+dz²<=r² mask; per-column top-down target test;
    /// RuleBased provider per placed block; markAboveForPostProcessing on
    /// the first hit of each consecutive run.
    Disk {
        provider: StateSrc,
        target: BlockPredicate,
        radius: IP,
        half_height: i32,
    },
    /// minecraft:tree (stage F5) — full trunk/foliage/root/decorator engine
    Tree(TreeCfg),
    /// minecraft:vegetation_patch / waterlogged_vegetation_patch
    VegetationPatch(VegetationPatchCfg),
    Unsupported(&'static str),
}

#[derive(Clone, Debug)]
pub struct PlacedDef {
    pub feature: String,
    pub placement: Vec<Mod>,
}

// ---------------------------------------------------------------------------
// Tree + vegetation-patch configs (stage F5)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrunkTy {
    Straight,
    Forking,
    Bending,
    Fancy,
    Giant,
    MegaJungle,
    DarkOak,
    UpwardsBranching,
    Cherry,
}

#[derive(Clone, Debug)]
pub struct TrunkCfg {
    pub ty: TrunkTy,
    pub base: i32,
    pub h_rand_a: i32,
    pub h_rand_b: i32,
    /// bending: min_height_for_leaves (default 1) + bend_length
    pub bend_min_leaves: i32,
    pub bend_length: IP,
    /// upwards_branching
    pub ub_extra_branch_steps: IP,
    pub ub_place_branch_prob: f32,
    pub ub_extra_branch_length: IP,
    pub ub_can_grow_through: GrowSet,
    /// cherry
    pub ch_branch_count: IP,
    pub ch_branch_hor_len: IP,
    pub ch_branch_start_off: IP,
    pub ch_branch_end_off: IP,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FoliageTy {
    Blob,
    Fancy,
    Bush,
    Spruce,
    Pine,
    MegaPine,
    Acacia,
    MegaJungle,
    DarkOak,
    RandomSpread,
    Cherry,
}

#[derive(Clone, Debug)]
pub struct FoliageCfg {
    pub ty: FoliageTy,
    pub radius: IP,
    pub offset: IP,
    /// blob/fancy/bush/megajungle: fixed int "height"
    pub fixed_height: i32,
    /// spruce: trunk_height; pine: height; megapine: crown_height;
    /// random_spread: foliage_height; cherry: height
    pub prov_height: IP,
    pub leaf_attempts: i32,
    /// cherry floats
    pub wide_bottom_hole: f32,
    pub corner_hole: f32,
    pub hanging_leaves: f32,
    pub hanging_ext: f32,
}

/// TwoLayers / ThreeLayers feature size + optional min_clipped_height.
#[derive(Clone, Copy, Debug)]
pub enum MinSize {
    TwoLayers { limit: i32, lower: i32, upper: i32 },
    ThreeLayers { limit: i32, lower: i32, middle: i32, upper: i32, upper_limit: i32 },
}

#[derive(Clone, Debug)]
pub enum GrowSet {
    Tag(String),
    Blocks(Vec<String>),
}

#[derive(Clone, Debug)]
pub struct RootCfg {
    pub trunk_offset_y: IP,
    pub root_provider: StateSrc,
    pub above_root: Option<(StateSrc, f32)>,
    pub max_root_width: i32,
    pub max_root_length: i32,
    pub random_skew_chance: f32,
    pub can_grow_through: GrowSet,
    pub muddy_roots_in: GrowSet,
    pub muddy_roots_provider: StateSrc,
}

#[derive(Clone, Debug)]
pub enum TreeDeco {
    LeaveVine(f32),
    TrunkVine,
    Beehive(f32),
    Cocoa(f32),
    AlterGround(StateSrc),
    AttachedToLogs { probability: f32, provider: StateSrc, dirs: Vec<u8> },
    AttachedToLeaves {
        probability: f32,
        excl_xz: i32,
        excl_y: i32,
        provider: StateSrc,
        req_empty: i32,
        dirs: Vec<u8>,
    },
    PlaceOnGround { tries: i32, radius: i32, height: i32, provider: StateSrc },
    PaleMoss { leaves: f32, trunk: f32, ground: f32 },
    CreakingHeart(f32),
}

#[derive(Clone, Debug)]
pub struct TreeCfg {
    pub trunk_provider: StateSrc,
    pub trunk_placer: TrunkCfg,
    pub foliage_provider: StateSrc,
    pub foliage_placer: FoliageCfg,
    pub root_placer: Option<RootCfg>,
    pub dirt_provider: StateSrc,
    pub minimum_size: MinSize,
    pub min_clipped: Option<i32>,
    pub decorators: Vec<TreeDeco>,
    pub ignore_vines: bool,
    pub force_dirt: bool,
}

#[derive(Clone, Debug)]
pub struct VegetationPatchCfg {
    /// replaceable block tag (without the '#')
    pub replaceable: String,
    pub ground_state: StateSrc,
    /// inner PLACED feature ref (name resolved in the registry)
    pub vegetation_feature: String,
    /// true = ceiling (dir UP), false = floor (dir DOWN)
    pub ceiling: bool,
    pub depth: IP,
    pub extra_bottom_chance: f32,
    pub vertical_range: i32,
    pub vegetation_chance: f32,
    pub xz_radius: IP,
    pub extra_edge_chance: f32,
    /// waterlogged_vegetation_patch variant
    pub waterlogged: bool,
}

pub struct FeatureRegistry {
    pub placed: HashMap<String, PlacedDef>,
    pub configured: HashMap<String, FeatureCfg>,
    pub unsupported_placed: usize,
    pub unsupported_configured: usize,
    pub total_placed: usize,
    pub total_configured: usize,
}

// ---------------------------------------------------------------------------
// Decoders
// ---------------------------------------------------------------------------

fn decode_int_provider(j: &Json) -> Option<IP> {
    if let Some(v) = j.as_i64() {
        return Some(IP::Constant(v as i32));
    }
    let obj = j.get("type")?.as_str()?;
    match obj {
        "minecraft:constant" => j.get("value").and_then(|v| v.as_i64()).map(|v| IP::Constant(v as i32)),
        "minecraft:uniform" => {
            let lo = j.get("min_inclusive").and_then(|v| v.as_i64())?;
            let hi = j.get("max_inclusive").and_then(|v| v.as_i64())?;
            Some(IP::Uniform { lo: lo as i32, hi: hi as i32 })
        }
        _ => None,
    }
}

fn decode_height(j: &Json) -> Option<HP> {
    if let Some(a) = j.get("absolute").and_then(|v| v.as_i64()) {
        return Some(HP::Abs(a as i32));
    }
    if let Some(a) = j.get("above_bottom").and_then(|v| v.as_i64()) {
        return Some(HP::AboveBottom(a as i32));
    }
    if let Some(a) = j.get("below_top").and_then(|v| v.as_i64()) {
        return Some(HP::BelowTop(a as i32));
    }
    let t = j.get("type")?.as_str()?;
    match t {
        "minecraft:uniform" => {
            let lo = decode_anchor_val(j.get("min_inclusive")?)?;
            let hi = decode_anchor_val(j.get("max_inclusive")?)?;
            Some(HP::Uniform { lo, hi })
        }
        "minecraft:trapezoid" => {
            let lo = decode_anchor_val(j.get("min_inclusive")?)?;
            let hi = decode_anchor_val(j.get("max_inclusive")?)?;
            // vanilla default plateau = 0 (triangle)
            let plateau = j.get("plateau").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
            Some(HP::Trapezoid { lo, hi, plateau })
        }
        _ => None,
    }
}

fn decode_anchor(j: &Json) -> Option<HP> {
    // anchor is itself a height-provider-shaped object or int
    if let Some(v) = j.as_i64() {
        return Some(HP::Abs(v as i32));
    }
    decode_height(j)
}

/// flat i32 anchor value (already absolute/above_bottom/below_top variants)
fn decode_anchor_val(j: &Json) -> Option<i32> {
    if let Some(v) = j.as_i64() {
        return Some(v as i32);
    }
    if let Some(v) = j.get("absolute").and_then(|v| v.as_i64()) {
        return Some(v as i32);
    }
    if let Some(v) = j.get("above_bottom").and_then(|v| v.as_i64()) {
        return Some(v as i32);
    }
    if let Some(v) = j.get("below_top").and_then(|v| v.as_i64()) {
        return Some(v as i32);
    }
    None
}

fn decode_state_provider(j: &Json) -> Option<StateSrc> {
    // RuleBasedBlockStateProvider wrapper: {"fallback": provider, "rules":
    // [{"if_true": predicate, "then": provider}]} (no "type" key). Only used
    // by disk features today; a wrapper with empty rules == its fallback.
    if j.get("fallback").is_some() || j.get("rules").is_some() {
        let fb = decode_state_provider(j.get("fallback")?)?;
        let mut rules = Vec::new();
        if let Some(arr) = j.get("rules").and_then(|v| v.as_arr()) {
            for e in arr {
                let pred = decode_predicate(e.get("if_true")?)?;
                let then = decode_state_provider(e.get("then")?)?;
                rules.push((pred, then));
            }
        }
        return Some(StateSrc::RuleBased { fallback: Box::new(fb), rules });
    }
    let t = j.get("type")?.as_str()?;
    match t {
        "minecraft:simple_state_provider" => {
            let s = j.get("state")?;
            Some(StateSrc::Simple(state_string(s)?))
        }
        "minecraft:weighted_state_provider" | "minecraft:weighted_list_state_provider" => {
            // WeightedStateProvider codec field = "entries" (1.21.8)
            let arr = j
                .get("entries")
                .or_else(|| j.get("distribution"))?
                .as_arr()?;
            let mut v = Vec::new();
            for e in arr {
                let w = e.get("weight").and_then(|x| x.as_i64())? as u32;
                let s = state_string(e.get("data")?)?;
                v.push((w, s));
            }
            Some(StateSrc::Weighted(v))
        }
        "minecraft:rotated_block_provider" | "minecraft:rotated_state_provider" => {
            // RotatedBlockProvider codec: BlockState.CODEC.fieldOf("state")
            let s = j.get("state")?;
            Some(StateSrc::Simple(format!("ROTATED\0{}", state_string(s)?)))
        }
        _ => Some(StateSrc::Unsupported("state_provider")),
    }
}

/// canonical "ns:block[k=v,...]" string from a vanilla state JSON
pub fn state_string(j: &Json) -> Option<String> {
    let name = j.get("Name").and_then(|v| v.as_str()).or_else(|| j.as_str())?;
    if let Some(props) = j.get("Properties") {
        let mut kv = Vec::new();
        if let Some(obj) = props.as_obj() {
            for (k, v) in obj {
                kv.push(format!("{}={}", k, v.as_str()?));
            }
        }
        if kv.is_empty() {
            return Some(name.to_string());
        }
        return Some(format!("{}[{}]", name, kv.join(",")));
    }
    Some(name.to_string())
}

/// RuleTest codec (ore targets + replace_single_block targets):
/// always_true / block_match / tag_match / random_block_match /
/// random_blockstate_match / blockstate_match.
fn decode_rule_test(j: &Json) -> Option<RuleTest> {
    let t = j.get("predicate_type").or_else(|| j.get("type"))?.as_str()?;
    Some(match t {
        "minecraft:always_true" => RuleTest::True,
        "minecraft:block_match" => RuleTest::BlockMatch(j.get("block")?.as_str()?.to_string()),
        "minecraft:tag_match" => RuleTest::TagMatch(j.get("tag")?.as_str()?.to_string()),
        "minecraft:random_block_match" => RuleTest::RandomBlock {
            block: j.get("block")?.as_str()?.to_string(),
            probability: j.get("probability")?.as_f64()? as f32,
        },
        _ => return None,
    })
}

fn decode_offset(j: &Json) -> (i32, i32, i32) {
    match j.get("offset").and_then(|v| v.as_arr()) {
        Some(a) if a.len() == 3 => (
            a[0].as_i64().unwrap_or(0) as i32,
            a[1].as_i64().unwrap_or(0) as i32,
            a[2].as_i64().unwrap_or(0) as i32,
        ),
        _ => (0, 0, 0),
    }
}

fn decode_predicate(j: &Json) -> Option<BlockPredicate> {
    // vanilla rule-test / block-predicate JSON key = "predicate_type" (the
    // blockpredicates codec field); "type" accepted for leniency
    let t = j.get("predicate_type").or_else(|| j.get("type"))?.as_str()?;
    Some(match t {
        "minecraft:true" => BlockPredicate::True,
        "minecraft:all_of" => {
            let mut v = Vec::new();
            for e in j.get("predicates")?.as_arr()? {
                v.push(decode_predicate(e)?);
            }
            BlockPredicate::AllOf(v)
        }
        "minecraft:any_of" => {
            let mut v = Vec::new();
            for e in j.get("predicates")?.as_arr()? {
                v.push(decode_predicate(e)?);
            }
            BlockPredicate::AnyOf(v)
        }
        "minecraft:not" => BlockPredicate::Not(Box::new(decode_predicate(j.get("predicate")?)?)),
        "minecraft:matching_blocks" => {
            // "blocks" may be a single string or a list (ExtraCodecs fast paths)
            let mut v = Vec::new();
            if let Some(s) = j.get("blocks").and_then(|x| x.as_str()) {
                v.push(s.to_string());
            } else {
                for e in j.get("blocks")?.as_arr()? {
                    v.push(state_string(e)?);
                }
            }
            BlockPredicate::MatchingBlocks { ids: v, off: decode_offset(j) }
        }
        "minecraft:matching_fluids" => {
            let mut v = Vec::new();
            if let Some(s) = j.get("fluids").and_then(|x| x.as_str()) {
                v.push(s.to_string());
            } else {
                for e in j.get("fluids")?.as_arr()? {
                    v.push(state_string(e)?);
                }
            }
            BlockPredicate::MatchingFluids { ids: v, off: decode_offset(j) }
        }
        "minecraft:replaceable" => BlockPredicate::Replaceable,
        "minecraft:solid" => BlockPredicate::Solid,
        "minecraft:has_sturdy_face" => {
            let dir = match j.get("direction").and_then(|v| v.as_str())? {
                "down" => 0,
                "up" => 1,
                "north" => 2,
                "south" => 3,
                "west" => 4,
                "east" => 5,
                _ => return None,
            };
            let off = j.get("offset").and_then(|v| v.as_arr());
            let (ox, oy, oz) = match off {
                Some(a) if a.len() == 3 => (
                    a[0].as_i64()? as i32,
                    a[1].as_i64()? as i32,
                    a[2].as_i64()? as i32,
                ),
                _ => (0, 0, 0),
            };
            BlockPredicate::HasSturdyFace { offset: (ox, oy, oz), dir }
        }
        "minecraft:would_survive" => {
            let s = state_string(j.get("state")?)?;
            BlockPredicate::WouldSurvive { state: s }
        }
        "minecraft:inside_world_bounds" => BlockPredicate::InsideWorldBounds,
        "minecraft:unobstructed" => BlockPredicate::Unobstructed,
        _ => BlockPredicate::Unsupported("predicate"),
    })
}

fn decode_heightmap_ty(s: &str) -> Option<HeightmapTy> {
    Some(match s {
        "WORLD_SURFACE_WG" => HeightmapTy::WorldSurfaceWg,
        "WORLD_SURFACE" => HeightmapTy::WorldSurface,
        "OCEAN_FLOOR_WG" => HeightmapTy::OceanFloorWg,
        "OCEAN_FLOOR" => HeightmapTy::OceanFloor,
        "MOTION_BLOCKING" => HeightmapTy::MotionBlocking,
        "MOTION_BLOCKING_NO_LEAVES" => HeightmapTy::MotionBlockingNoLeaves,
        _ => return None,
    })
}

pub fn decode_placement_modifier(j: &Json) -> Mod {
    let t = match j.get("type").and_then(|v| v.as_str()) {
        Some(t) => t.to_string(),
        None => return Mod::Unsupported("modifier:no-type"),
    };
    match t.as_str() {
        "minecraft:count" => match decode_int_provider(j.get("count").unwrap_or(&Json::Num(1.0))) {
            Some(p) => Mod::Pure(PM::Count(p)),
            None => Mod::Unsupported("count-provider"),
        },
        "minecraft:in_square" => Mod::Pure(PM::InSquare),
        "minecraft:rarity_filter" => match j.get("chance").and_then(|v| v.as_i64()) {
            Some(c) => Mod::Pure(PM::RarityFilter { chance: c as i32 }),
            None => Mod::Unsupported("rarity"),
        },
        "minecraft:random_offset" => {
            let xz = j.get("xz_spread").and_then(decode_int_provider);
            let y = j.get("y_spread").and_then(decode_int_provider);
            match (xz, y) {
                (Some(xz), Some(y)) => Mod::Pure(PM::RandomOffset { xz, y }),
                _ => Mod::Unsupported("random_offset-provider"),
            }
        }
        "minecraft:height_range" => match j.get("height").and_then(decode_height) {
            Some(h) => Mod::Pure(PM::HeightRange(h)),
            None => Mod::Unsupported("height_range"),
        },
        "minecraft:heightmap" => match j.get("heightmap").and_then(|v| v.as_str()).and_then(decode_heightmap_ty) {
            Some(ty) => Mod::Heightmap { ty },
            None => Mod::Unsupported("heightmap-ty"),
        },
        "minecraft:biome" => Mod::BiomeFilter { feature: 0 },
        "minecraft:block_predicate_filter" => match j.get("predicate").map(decode_predicate) {
            Some(Some(p)) => Mod::PredFilter(p),
            _ => Mod::Unsupported("predicate"),
        },
        "minecraft:surface_water_depth_filter" => match j.get("max_water_depth").and_then(|v| v.as_i64()) {
            Some(m) => Mod::SurfaceWaterDepth { max: m as i32 },
            None => Mod::Unsupported("water-depth"),
        },
        "minecraft:surface_relative_threshold_filter" => {
            let Some(ty) = j.get("heightmap").and_then(|v| v.as_str()).and_then(decode_heightmap_ty) else {
                return Mod::Unsupported("srt-heightmap");
            };
            let min = j.get("min_inclusive").and_then(|v| v.as_i64()).map(|v| v as i32).unwrap_or(i32::MIN);
            let max = j.get("max_inclusive").and_then(|v| v.as_i64()).map(|v| v as i32).unwrap_or(i32::MAX);
            Mod::SurfaceRelativeThreshold { ty, min, max }
        }
        "minecraft:environment_scan" => {
            let dir = match j.get("direction_of_search").and_then(|v| v.as_str()) {
                Some("up") => 1,
                Some("down") => 0,
                _ => return Mod::Unsupported("envscan-dir"),
            };
            let steps = j.get("max_steps").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
            let target = match j.get("target_condition").map(decode_predicate) {
                Some(Some(p)) => p,
                _ => return Mod::Unsupported("envscan-target"),
            };
            let allowed = match j.get("allowed_search_condition").map(decode_predicate) {
                Some(Some(p)) => Some(p),
                Some(None) => return Mod::Unsupported("envscan-allowed"),
                None => None,
            };
            Mod::EnvScan { dir, max_steps: steps, target, allowed }
        }
        "minecraft:noise_based_count" => {
            let ratio = j.get("noise_to_count_ratio").and_then(|v| v.as_i64());
            let factor = j.get("noise_factor").and_then(|v| v.as_f64());
            let offset = j.get("noise_offset").and_then(|v| v.as_f64()).unwrap_or(0.0);
            match (ratio, factor) {
                (Some(r), Some(f)) => Mod::NoiseBasedCount { ratio: r as i32, factor: f, offset },
                _ => Mod::Unsupported("noise-count"),
            }
        }
        _ => Mod::Unsupported("modifier"),
    }
}

pub fn decode_configured_feature(name: &str, j: &Json, reg: &mut FeatureRegistry) -> FeatureCfg {
    let t = match j.get("type").and_then(|v| v.as_str()) {
        Some(t) => t,
        None => return FeatureCfg::Unsupported("cf:no-type"),
    };
    let cfg = j.get("config").cloned().unwrap_or(Json::Obj(Vec::new()));
    match t {
        "minecraft:no_op" => FeatureCfg::NoOp,
        "minecraft:simple_block" => {
            let place = match cfg.get("to_place").map(decode_state_provider) {
                Some(Some(p)) => p,
                _ => return FeatureCfg::Unsupported("simple_block-place"),
            };
            let ticks = match cfg.get("schedule_tick") {
                Some(crate::json::Json::Bool(b)) => *b,
                Some(v) => v.as_i64().unwrap_or(0) != 0,
                None => false,
            };
            FeatureCfg::SimpleBlock { place, ticks }
        }
        "minecraft:replace_single_block" => {
            let mut targets = Vec::new();
            match cfg.get("targets").and_then(|v| v.as_arr()) {
                Some(arr) => {
                    for e in arr {
                        let pred = match e.get("target").map(decode_rule_test) {
                            Some(Some(rt)) => match rt {
                                RuleTest::True => BlockPredicate::True,
                                RuleTest::BlockMatch(b) => BlockPredicate::MatchingBlocks { ids: vec![b], off: (0, 0, 0) },
                                _ => return FeatureCfg::Unsupported("replace-target"),
                            },
                            _ => return FeatureCfg::Unsupported("replace-target"),
                        };
                        // vanilla ReplaceBlockFeature: "state" is a PLAIN
                        // BlockState (not a provider)
                        let st = match e.get("state").map(state_string) {
                            Some(Some(s)) => StateSrc::Simple(s),
                            _ => return FeatureCfg::Unsupported("replace-state"),
                        };
                        targets.push((pred, st));
                    }
                    FeatureCfg::ReplaceBlock { targets }
                }
                None => FeatureCfg::Unsupported("replace-targets"),
            }
        }
        "minecraft:random_selector" => {
            let default = match cfg.get("default").and_then(|v| v.as_str()) {
                Some(s) => s.to_string(),
                None => return FeatureCfg::Unsupported("rs-default"),
            };
            let mut entries = Vec::new();
            match cfg.get("features").and_then(|v| v.as_arr()) {
                Some(arr) => {
                    for e in arr {
                        let c = e.get("chance").and_then(|v| v.as_f64());
                        let f = e.get("feature").and_then(|v| v.as_str());
                        match (c, f) {
                            (Some(c), Some(f)) => entries.push((c as f32, f.to_string())),
                            _ => return FeatureCfg::Unsupported("rs-entry"),
                        }
                    }
                    FeatureCfg::RandomSelector { entries, default }
                }
                None => FeatureCfg::Unsupported("rs-features"),
            }
        }
        "minecraft:simple_random_selector" => {
            let mut entries = Vec::new();
            match cfg.get("features").and_then(|v| v.as_arr()) {
                Some(arr) => {
                    for e in arr {
                        match e.as_str().or_else(|| e.get("feature").and_then(|f| f.as_str())) {
                            Some(s) => entries.push(s.to_string()),
                            None => return FeatureCfg::Unsupported("srs-entry"),
                        }
                    }
                    FeatureCfg::SimpleRandomSelector { entries }
                }
                None => FeatureCfg::Unsupported("srs-features"),
            }
        }
        "minecraft:random_boolean_selector" => {
            // vanilla codec keys: feature_true / feature_false (if_true/if_false
            // were the decomposed-class field names, not the registry form)
            let if_true = cfg.get("feature_true").and_then(|v| v.as_str()).or_else(|| cfg.get("if_true").and_then(|v| v.as_str())).or_else(|| cfg.get("feature_true").and_then(|v| v.get("feature").and_then(|f| f.as_str())));
            let if_false = cfg.get("feature_false").and_then(|v| v.as_str()).or_else(|| cfg.get("if_false").and_then(|v| v.as_str())).or_else(|| cfg.get("feature_false").and_then(|v| v.get("feature").and_then(|f| f.as_str())));
            match (if_true, if_false) {
                (Some(t), Some(f)) => FeatureCfg::RandomBoolSelector { chance: 0.5, if_true: t.to_string(), if_false: f.to_string() },
                _ => FeatureCfg::Unsupported("rbs"),
            }
        }
        "minecraft:ore" | "minecraft:scattered_ore" => {
            let Some(size) = cfg.get("size").and_then(|v| v.as_i64()) else {
                return FeatureCfg::Unsupported("ore-size");
            };
            let discard = cfg
                .get("discard_chance_on_air_exposure")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0) as f32;
            let mut targets = Vec::new();
            let mut bad = false;
            if let Some(arr) = cfg.get("targets").and_then(|v| v.as_arr()) {
                for e in arr {
                    let rt = match e.get("target").map(decode_rule_test) {
                        Some(Some(rt)) => rt,
                        _ => {
                            bad = true;
                            break;
                        }
                    };
                    match e.get("state").map(state_string) {
                        Some(Some(st)) => targets.push((rt, st)),
                        _ => {
                            bad = true;
                            break;
                        }
                    }
                }
            } else {
                bad = true;
            }
            if bad {
                return FeatureCfg::Unsupported("ore-targets");
            }
            if t == "minecraft:ore" {
                FeatureCfg::Ore { discard, size: size as i32, targets }
            } else {
                FeatureCfg::ScatteredOre { discard, size: size as i32, targets }
            }
        }
        "minecraft:random_patch" => {
            let tries = cfg.get("tries").and_then(|v| v.as_i64()).unwrap_or(128) as i32;
            let xz_spread = cfg.get("xz_spread").and_then(|v| v.as_i64()).unwrap_or(7) as i32;
            let y_spread = cfg.get("y_spread").and_then(|v| v.as_i64()).unwrap_or(3) as i32;
            let Some(fj) = cfg.get("feature") else {
                return FeatureCfg::Unsupported("patch-feature");
            };
            // vanilla: the inner is a PLACED feature — a string ref or an
            // inline {feature, placement} object. A string ref resolves
            // through reg.placed at place time (place_placed by name); inline
            // objects register under synthetic $-names and are NOT in any
            // biome's feature list (no FeatureSorter uid, BiomeFilter would
            // fail in vanilla too — parity by construction).
            if let Some(s) = fj.as_str() {
                return FeatureCfg::RandomPatch { tries, xz_spread, y_spread, inner: s.to_string() };
            }
            let inner = format!("{name}$/inner");
            if !decode_inline_placed(&inner, fj, reg) {
                return FeatureCfg::Unsupported("patch-inner");
            }
            FeatureCfg::RandomPatch { tries, xz_spread, y_spread, inner }
        }
        "minecraft:disk" => {
            let Some(Some(radius)) = cfg.get("radius").map(decode_int_provider) else {
                return FeatureCfg::Unsupported("disk-radius");
            };
            let Some(half_height) = cfg.get("half_height").and_then(|v| v.as_i64()) else {
                return FeatureCfg::Unsupported("disk-half");
            };
            let Some(Some(target)) = cfg.get("target").map(decode_predicate) else {
                return FeatureCfg::Unsupported("disk-target");
            };
            if matches!(target, BlockPredicate::Unsupported(_)) {
                return FeatureCfg::Unsupported("disk-target-unsupported");
            }
            let Some(Some(provider)) = cfg.get("state_provider").map(decode_state_provider) else {
                return FeatureCfg::Unsupported("disk-provider");
            };
            FeatureCfg::Disk {
                provider,
                target,
                radius,
                half_height: half_height as i32,
            }
        }
        "minecraft:tree" => decode_tree_cfg(&cfg),
        "minecraft:vegetation_patch" | "minecraft:waterlogged_vegetation_patch" => {
            decode_vegetation_patch_cfg(&cfg, t.ends_with("waterlogged_vegetation_patch"))
        }
        _ => FeatureCfg::Unsupported("cf-type"),
    }
}

// ---------------------------------------------------------------------------
// Tree + vegetation-patch decoders
// ---------------------------------------------------------------------------

fn decode_grow_set(j: &Json) -> Option<GrowSet> {
    if let Some(s) = j.as_str() {
        return if let Some(tag) = s.strip_prefix('#') {
            Some(GrowSet::Tag(tag.to_string()))
        } else {
            Some(GrowSet::Blocks(vec![s.to_string()]))
        };
    }
    let arr = j.as_arr()?;
    let mut v = Vec::new();
    for e in arr {
        v.push(e.as_str()?.to_string());
    }
    Some(GrowSet::Blocks(v))
}

fn decode_dirs(j: &Json) -> Option<Vec<u8>> {
    // Direction 3D data order: 0=down 1=up 2=north 3=south 4=west 5=east
    let arr = j.as_arr()?;
    let mut v = Vec::new();
    for e in arr {
        v.push(match e.as_str()? {
            "down" => 0,
            "up" => 1,
            "north" => 2,
            "south" => 3,
            "west" => 4,
            "east" => 5,
            _ => return None,
        });
    }
    if v.is_empty() {
        return None; // vanilla nonEmptyList
    }
    Some(v)
}

fn ip(j: &Json) -> Option<IP> {
    decode_int_provider(j)
}

fn trunk_parts(c: &Json) -> Option<(i32, i32, i32)> {
    Some((
        c.get("base_height")?.as_i64()? as i32,
        c.get("height_rand_a")?.as_i64()? as i32,
        c.get("height_rand_b")?.as_i64()? as i32,
    ))
}

fn foliage_parts(c: &Json) -> Option<(IP, IP)> {
    Some((ip(c.get("radius")?)?, ip(c.get("offset")?)?))
}

fn decode_tree_cfg(c: &Json) -> FeatureCfg {
    let bad = |why: &'static str| FeatureCfg::Unsupported(why);
    let tp = match c.get("trunk_placer") {
        Some(p) => p,
        None => return bad("tree-trunk-placer"),
    };
    let ty = match tp.get("type").and_then(|v| v.as_str()).unwrap_or("") {
        "minecraft:straight_trunk_placer" => TrunkTy::Straight,
        "minecraft:forking_trunk_placer" => TrunkTy::Forking,
        "minecraft:bending_trunk_placer" => TrunkTy::Bending,
        "minecraft:fancy_trunk_placer" => TrunkTy::Fancy,
        "minecraft:giant_trunk_placer" => TrunkTy::Giant,
        "minecraft:mega_jungle_trunk_placer" => TrunkTy::MegaJungle,
        "minecraft:dark_oak_trunk_placer" => TrunkTy::DarkOak,
        "minecraft:upwards_branching_trunk_placer" => TrunkTy::UpwardsBranching,
        "minecraft:cherry_trunk_placer" => TrunkTy::Cherry,
        _ => return bad("tree-trunk-ty"),
    };
    let (base, ha, hb) = match trunk_parts(tp) {
        Some(v) => v,
        None => return bad("tree-trunk-parts"),
    };
    let mut trunk = TrunkCfg {
        ty,
        base,
        h_rand_a: ha,
        h_rand_b: hb,
        bend_min_leaves: tp.get("min_height_for_leaves").and_then(|v| v.as_i64()).unwrap_or(1) as i32,
        bend_length: IP::Constant(1),
        ub_extra_branch_steps: IP::Constant(1),
        ub_place_branch_prob: 0.0,
        ub_extra_branch_length: IP::Constant(0),
        ub_can_grow_through: GrowSet::Blocks(vec![]),
        ch_branch_count: IP::Constant(0),
        ch_branch_hor_len: IP::Constant(0),
        ch_branch_start_off: IP::Constant(0),
        ch_branch_end_off: IP::Constant(0),
    };
    match ty {
        TrunkTy::Bending => {
            match ip(tp.get("bend_length").unwrap_or(&Json::Null)) {
                Some(b) => trunk.bend_length = b,
                None => return bad("tree-bend-len"),
            }
        }
        TrunkTy::UpwardsBranching => {
            let Some(steps) = ip(tp.get("extra_branch_steps").unwrap_or(&Json::Null)) else {
                return bad("tree-ub-steps");
            };
            let Some(prob) = tp.get("place_branch_per_log_probability").and_then(|v| v.as_f64()) else {
                return bad("tree-ub-prob");
            };
            let Some(len) = ip(tp.get("extra_branch_length").unwrap_or(&Json::Null)) else {
                return bad("tree-ub-len");
            };
            let Some(grow) = tp.get("can_grow_through").and_then(decode_grow_set) else {
                return bad("tree-ub-grow");
            };
            trunk.ub_extra_branch_steps = steps;
            trunk.ub_place_branch_prob = prob as f32;
            trunk.ub_extra_branch_length = len;
            trunk.ub_can_grow_through = grow;
        }
        TrunkTy::Cherry => {
            let (Some(cnt), Some(hl), Some(so), Some(eo)) = (
                ip(tp.get("branch_count").unwrap_or(&Json::Null)),
                ip(tp.get("branch_horizontal_length").unwrap_or(&Json::Null)),
                ip(tp.get("branch_start_offset_from_top").unwrap_or(&Json::Null)),
                ip(tp.get("branch_end_offset_from_top").unwrap_or(&Json::Null)),
            ) else {
                return bad("tree-ch-fields");
            };
            // secondBranchStartOffsetFromTop = UniformInt.of(min, max-1) — the
            // engine derives it from the primary uniform's bounds
            if !matches!(so, IP::Uniform { .. }) {
                return bad("tree-ch-start-uniform");
            }
            trunk.ch_branch_count = cnt;
            trunk.ch_branch_hor_len = hl;
            trunk.ch_branch_start_off = so;
            trunk.ch_branch_end_off = eo;
        }
        _ => {}
    }
    let fp = match c.get("foliage_placer") {
        Some(p) => p,
        None => return bad("tree-foliage-placer"),
    };
    let fty = match fp.get("type").and_then(|v| v.as_str()).unwrap_or("") {
        "minecraft:blob_foliage_placer" => FoliageTy::Blob,
        "minecraft:fancy_foliage_placer" => FoliageTy::Fancy,
        "minecraft:bush_foliage_placer" => FoliageTy::Bush,
        "minecraft:spruce_foliage_placer" => FoliageTy::Spruce,
        "minecraft:pine_foliage_placer" => FoliageTy::Pine,
        "minecraft:mega_pine_foliage_placer" => FoliageTy::MegaPine,
        "minecraft:acacia_foliage_placer" => FoliageTy::Acacia,
        "minecraft:jungle_foliage_placer" | "minecraft:mega_jungle_foliage_placer" => FoliageTy::MegaJungle,
        "minecraft:dark_oak_foliage_placer" => FoliageTy::DarkOak,
        "minecraft:random_spread_foliage_placer" => FoliageTy::RandomSpread,
        "minecraft:cherry_foliage_placer" => FoliageTy::Cherry,
        _ => return bad("tree-foliage-ty"),
    };
    let (radius, offset) = match foliage_parts(fp) {
        Some(v) => v,
        None => return bad("tree-foliage-parts"),
    };
    let mut fol = FoliageCfg {
        ty: fty,
        radius,
        offset,
        fixed_height: fp.get("height").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
        prov_height: IP::Constant(0),
        leaf_attempts: fp.get("leaf_placement_attempts").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
        wide_bottom_hole: fp.get("wide_bottom_layer_hole_chance").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
        corner_hole: fp.get("corner_hole_chance").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
        hanging_leaves: fp.get("hanging_leaves_chance").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
        hanging_ext: fp.get("hanging_leaves_extension_chance").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
    };
    match fty {
        FoliageTy::Spruce | FoliageTy::Pine | FoliageTy::MegaPine | FoliageTy::RandomSpread | FoliageTy::Cherry => {
            let key = match fty {
                FoliageTy::Spruce => "trunk_height",
                FoliageTy::MegaPine => "crown_height",
                FoliageTy::RandomSpread => "foliage_height",
                _ => "height",
            };
            match ip(fp.get(key).unwrap_or(&Json::Null)) {
                Some(h) => fol.prov_height = h,
                None => return bad("tree-fol-height"),
            }
        }
        _ => {}
    }
    if matches!(fty, FoliageTy::RandomSpread) && fol.leaf_attempts <= 0 {
        return bad("tree-fol-attempts");
    }
    let Some(Some(trunk_provider)) = c.get("trunk_provider").map(decode_state_provider) else {
        return bad("tree-trunk-provider");
    };
    let Some(Some(foliage_provider)) = c.get("foliage_provider").map(decode_state_provider) else {
        return bad("tree-foliage-provider");
    };
    let Some(Some(dirt_provider)) = c.get("dirt_provider").map(decode_state_provider) else {
        return bad("tree-dirt-provider");
    };
    let ms = match c.get("minimum_size") {
        Some(m) => m,
        None => return bad("tree-minsize"),
    };
    let (minimum_size, min_clipped) = match ms.get("type").and_then(|v| v.as_str()).unwrap_or("") {
        "minecraft:two_layers_feature_size" => {
            let mc = ms.get("min_clipped_height").and_then(|v| v.as_i64()).map(|v| v as i32);
            (
                MinSize::TwoLayers {
                    limit: ms.get("limit").and_then(|v| v.as_i64()).unwrap_or(1) as i32,
                    lower: ms.get("lower_size").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
                    upper: ms.get("upper_size").and_then(|v| v.as_i64()).unwrap_or(1) as i32,
                },
                mc,
            )
        }
        "minecraft:three_layers_feature_size" => {
            let mc = ms.get("min_clipped_height").and_then(|v| v.as_i64()).map(|v| v as i32);
            (
                MinSize::ThreeLayers {
                    limit: ms.get("limit").and_then(|v| v.as_i64()).unwrap_or(1) as i32,
                    lower: ms.get("lower_size").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
                    middle: ms.get("middle_size").and_then(|v| v.as_i64()).unwrap_or(1) as i32,
                    upper: ms.get("upper_size").and_then(|v| v.as_i64()).unwrap_or(1) as i32,
                    upper_limit: ms.get("upper_limit").and_then(|v| v.as_i64()).unwrap_or(1) as i32,
                },
                mc,
            )
        }
        _ => return bad("tree-minsize-ty"),
    };
    let mut root_placer = None;
    if let Some(rp) = c.get("root_placer") {
        let rty = rp.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if rty != "minecraft:mangrove_root_placer" {
            return bad("tree-root-ty");
        }
        let Some(off) = ip(rp.get("trunk_offset_y").unwrap_or(&Json::Null)) else {
            return bad("tree-root-off");
        };
        let Some(root_provider) = decode_state_provider(rp.get("root_provider").unwrap_or(&Json::Null)) else {
            return bad("tree-root-provider");
        };
        let above_root = match rp.get("above_root_placement") {
            Some(a) => {
                let Some(p) = decode_state_provider(a.get("above_root_provider").unwrap_or(&Json::Null)) else {
                    return bad("tree-root-above-provider");
                };
                let Some(ch) = a.get("above_root_placement_chance").and_then(|v| v.as_f64()) else {
                    return bad("tree-root-above-chance");
                };
                Some((p, ch as f32))
            }
            None => None,
        };
        let mp = match rp.get("mangrove_root_placement") {
            Some(m) => m,
            None => return bad("tree-root-mangrove"),
        };
        let Some(can_grow_through) = mp.get("can_grow_through").and_then(decode_grow_set) else {
            return bad("tree-root-grow");
        };
        let Some(muddy_roots_in) = mp.get("muddy_roots_in").and_then(decode_grow_set) else {
            return bad("tree-root-muddy");
        };
        let Some(muddy_roots_provider) = decode_state_provider(mp.get("muddy_roots_provider").unwrap_or(&Json::Null))
        else {
            return bad("tree-root-muddy-provider");
        };
        root_placer = Some(RootCfg {
            trunk_offset_y: off,
            root_provider,
            above_root,
            max_root_width: mp.get("max_root_width").and_then(|v| v.as_i64()).unwrap_or(8) as i32,
            max_root_length: mp.get("max_root_length").and_then(|v| v.as_i64()).unwrap_or(15) as i32,
            random_skew_chance: mp.get("random_skew_chance").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
            can_grow_through,
            muddy_roots_in,
            muddy_roots_provider,
        });
    }
    let mut decorators = Vec::new();
    if let Some(arr) = c.get("decorators").and_then(|v| v.as_arr()) {
        for d in arr {
            let dt = d.get("type").and_then(|v| v.as_str()).unwrap_or("");
            let dec = match dt {
                "minecraft:leave_vine" => match d.get("probability").and_then(|v| v.as_f64()) {
                    Some(p) => TreeDeco::LeaveVine(p as f32),
                    None => return bad("tree-dec-leavevine"),
                },
                "minecraft:trunk_vine" => TreeDeco::TrunkVine,
                "minecraft:beehive" => match d.get("probability").and_then(|v| v.as_f64()) {
                    Some(p) => TreeDeco::Beehive(p as f32),
                    None => return bad("tree-dec-beehive"),
                },
                "minecraft:cocoa" => match d.get("probability").and_then(|v| v.as_f64()) {
                    Some(p) => TreeDeco::Cocoa(p as f32),
                    None => return bad("tree-dec-cocoa"),
                },
                "minecraft:alter_ground" => {
                    match decode_state_provider(d.get("provider").unwrap_or(&Json::Null)) {
                        Some(p) => TreeDeco::AlterGround(p),
                        None => return bad("tree-dec-alterground"),
                    }
                }
                "minecraft:attached_to_logs" => {
                    let (Some(prob), Some(provider), Some(dirs)) = (
                        d.get("probability").and_then(|v| v.as_f64()),
                        decode_state_provider(d.get("block_provider").unwrap_or(&Json::Null)),
                        d.get("directions").and_then(decode_dirs),
                    ) else {
                        return bad("tree-dec-atl");
                    };
                    TreeDeco::AttachedToLogs { probability: prob as f32, provider, dirs }
                }
                "minecraft:attached_to_leaves" => {
                    let (Some(prob), Some(excl_xz), Some(excl_y), Some(provider), Some(req_empty), Some(dirs)) = (
                        d.get("probability").and_then(|v| v.as_f64()),
                        d.get("exclusion_radius_xz").and_then(|v| v.as_i64()),
                        d.get("exclusion_radius_y").and_then(|v| v.as_i64()),
                        decode_state_provider(d.get("block_provider").unwrap_or(&Json::Null)),
                        d.get("required_empty_blocks").and_then(|v| v.as_i64()),
                        d.get("directions").and_then(decode_dirs),
                    ) else {
                        return bad("tree-dec-atlv");
                    };
                    TreeDeco::AttachedToLeaves {
                        probability: prob as f32,
                        excl_xz: excl_xz as i32,
                        excl_y: excl_y as i32,
                        provider,
                        req_empty: req_empty as i32,
                        dirs,
                    }
                }
                "minecraft:place_on_ground" => {
                    let (Some(tries), Some(radius), Some(height), Some(provider)) = (
                        d.get("tries").and_then(|v| v.as_i64()),
                        d.get("radius").and_then(|v| v.as_i64()),
                        d.get("height").and_then(|v| v.as_i64()),
                        decode_state_provider(d.get("block_state_provider").unwrap_or(&Json::Null)),
                    ) else {
                        return bad("tree-dec-pog");
                    };
                    TreeDeco::PlaceOnGround { tries: tries as i32, radius: radius as i32, height: height as i32, provider }
                }
                "minecraft:pale_moss" => {
                    let (Some(leaves), Some(trunk), Some(ground)) = (
                        d.get("leaves_probability").and_then(|v| v.as_f64()),
                        d.get("trunk_probability").and_then(|v| v.as_f64()),
                        d.get("ground_probability").and_then(|v| v.as_f64()),
                    ) else {
                        return bad("tree-dec-palemoss");
                    };
                    TreeDeco::PaleMoss { leaves: leaves as f32, trunk: trunk as f32, ground: ground as f32 }
                }
                "minecraft:creaking_heart" => match d.get("probability").and_then(|v| v.as_f64()) {
                    Some(p) => TreeDeco::CreakingHeart(p as f32),
                    None => return bad("tree-dec-creaking"),
                },
                _ => return bad("tree-dec-ty"),
            };
            decorators.push(dec);
        }
    }
    FeatureCfg::Tree(TreeCfg {
        trunk_provider,
        trunk_placer: trunk,
        foliage_provider,
        foliage_placer: fol,
        root_placer,
        dirt_provider,
        minimum_size,
        min_clipped,
        decorators,
        ignore_vines: c.get("ignore_vines").and_then(|v| v.as_bool()).unwrap_or(false),
        force_dirt: c.get("force_dirt").and_then(|v| v.as_bool()).unwrap_or(false),
    })
}

fn decode_vegetation_patch_cfg(c: &Json, waterlogged: bool) -> FeatureCfg {
    let Some(tag) = c.get("replaceable").and_then(|v| v.as_str()) else {
        return FeatureCfg::Unsupported("vp-replaceable");
    };
    let Some(tag) = tag.strip_prefix('#') else {
        return FeatureCfg::Unsupported("vp-replaceable-tag");
    };
    let Some(Some(ground_state)) = c.get("ground_state").map(decode_state_provider) else {
        return FeatureCfg::Unsupported("vp-ground");
    };
    // vegetation_feature: a PLACED feature ref (string or {"feature": ...})
    let vf = match c.get("vegetation_feature") {
        Some(v) => match v.as_str() {
            Some(s) => s.to_string(),
            None => match v.get("feature").and_then(|f| f.as_str()) {
                Some(s) => s.to_string(),
                None => return FeatureCfg::Unsupported("vp-veg"),
            },
        },
        None => return FeatureCfg::Unsupported("vp-veg"),
    };
    let ceiling = match c.get("surface").and_then(|v| v.as_str()) {
        Some("ceiling") => true,
        Some("floor") => false,
        _ => return FeatureCfg::Unsupported("vp-surface"),
    };
    let (Some(depth), Some(vr), Some(xz_radius)) = (
        c.get("depth").and_then(decode_int_provider),
        c.get("vertical_range").and_then(|v| v.as_i64()),
        c.get("xz_radius").and_then(decode_int_provider),
    ) else {
        return FeatureCfg::Unsupported("vp-fields");
    };
    FeatureCfg::VegetationPatch(VegetationPatchCfg {
        replaceable: tag.to_string(),
        ground_state,
        vegetation_feature: vf,
        ceiling,
        depth,
        extra_bottom_chance: c.get("extra_bottom_block_chance").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
        vertical_range: vr as i32,
        vegetation_chance: c.get("vegetation_chance").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
        xz_radius,
        extra_edge_chance: c.get("extra_edge_column_chance").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
        waterlogged,
    })
}

/// Decode an inline placed-feature JSON, registering it (and any nested inline
/// configured feature) into the registry under synthetic `$`-names. `fj` is a
/// placed feature: a plain string ref, or {"feature": ref|inline-cfg,
/// "placement": [...]}. Returns success.
pub fn decode_inline_placed(name: &str, fj: &Json, reg: &mut FeatureRegistry) -> bool {
    if let Some(s) = fj.as_str() {
        reg.placed.insert(name.to_string(), PlacedDef { feature: s.to_string(), placement: Vec::new() });
        return true;
    }
    let feature = match fj.get("feature") {
        Some(o) if o.as_str().is_some() => o.as_str().unwrap().to_string(),
        Some(o) if o.get("type").and_then(|v| v.as_str()).is_some() => {
            let cfg_name = format!("{name}$/cfg");
            let cfg = decode_configured_feature(&cfg_name, o, reg);
            if matches!(cfg, FeatureCfg::Unsupported(_)) {
                return false;
            }
            reg.configured.insert(cfg_name.clone(), cfg);
            cfg_name
        }
        _ => return false,
    };
    let mut placement = Vec::new();
    if let Some(arr) = fj.get("placement").and_then(|v| v.as_arr()) {
        for e in arr {
            let m = decode_placement_modifier(e);
            if matches!(m, Mod::Unsupported(_)) {
                return false;
            }
            placement.push(m);
        }
    }
    reg.placed.insert(name.to_string(), PlacedDef { feature, placement });
    true
}

pub fn decode_registry(
    configured: impl Iterator<Item = (String, String)>, // name -> json text
    placed: impl Iterator<Item = (String, String)>,
) -> FeatureRegistry {
    let mut reg = FeatureRegistry {
        placed: HashMap::new(),
        configured: HashMap::new(),
        unsupported_placed: 0,
        unsupported_configured: 0,
        total_placed: 0,
        total_configured: 0,
    };
    for (name, text) in configured {
        reg.total_configured += 1;
        match json::parse(&text) {
            Ok(j) => {
                let cfg = decode_configured_feature(&name, &j, &mut reg);
                if matches!(cfg, FeatureCfg::Unsupported(_)) {
                    reg.unsupported_configured += 1;
                }
                reg.configured.insert(name, cfg);
            }
            Err(_) => {
                reg.unsupported_configured += 1;
            }
        }
    }
    for (name, text) in placed {
        reg.total_placed += 1;
        match json::parse(&text) {
            Ok(j) => {
                let feature = match j.get("feature").and_then(|v| v.as_str()) {
                    Some(f) => f.to_string(),
                    None => {
                        reg.unsupported_placed += 1;
                        continue;
                    }
                };
                let mut placement = Vec::new();
                let mut bad = false;
                if let Some(arr) = j.get("placement").and_then(|v| v.as_arr()) {
                    for e in arr {
                        let m = decode_placement_modifier(e);
                        if matches!(m, Mod::Unsupported(_)) {
                            bad = true;
                        }
                        placement.push(m);
                    }
                }
                if bad {
                    reg.unsupported_placed += 1;
                }
                reg.placed.insert(name, PlacedDef { feature, placement });
            }
            Err(_) => {
                reg.unsupported_placed += 1;
            }
        }
    }
    reg
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Decode the REAL dptt packs (Tectonic+Terralith) and report vanilla-subset coverage.
    /// Packs extracted to /tmp/dptt_packs (see worklog); test skips gracefully if absent.
    #[test]
    fn real_pack_coverage() {
        let root = std::path::Path::new("/tmp/dptt_packs");
        if !root.exists() {
            eprintln!("packs not extracted — skipping");
            return;
        }
        let mut configured: Vec<(String, String)> = Vec::new();
        let mut placed: Vec<(String, String)> = Vec::new();
        for pack in ["tectonic", "terralith"] {
            for kind_dir in ["configured_feature", "placed_feature"] {
                let base = root.join(pack).join("data").join(pack).join("worldgen").join(kind_dir);
                if kind_dir == "configured_feature" {
                    collect_json(&base, &mut configured);
                } else {
                    collect_json(&base, &mut placed);
                }
            }
        }
        eprintln!("collected: {} configured, {} placed files", configured.len(), placed.len());
        assert!(configured.len() > 400, "too few configured files found");
        assert!(placed.len() > 500, "too few placed files found");
        let n_cfg = configured.len();
        let n_pl = placed.len();
        let reg = decode_registry(configured.into_iter(), placed.into_iter());
        eprintln!(
            "CONFIGURED: {}/{} decoded, {} unsupported",
            reg.total_configured - reg.unsupported_configured,
            reg.total_configured,
            reg.unsupported_configured
        );
        eprintln!(
            "PLACED: {}/{} fully supported ({} with some unsupported modifier)",
            reg.total_placed - reg.unsupported_placed,
            reg.total_placed,
            reg.unsupported_placed
        );
        // F1 scope: only 5 configured types are natively placeable; every FILE must still
        // classify (Unsupported is a valid classification). Assert supported F1 subset:
        let supported_cfg = reg.configured.values()
            .filter(|c| !matches!(c, FeatureCfg::Unsupported(_)))
            .count();
        eprintln!("F1 supported configured: {}", supported_cfg);
        assert!(supported_cfg >= 50, "F1 configured subset unexpectedly small: {}", supported_cfg);
        assert!(reg.total_placed == n_pl, "registry drop");
        assert!(reg.total_configured == n_cfg, "registry drop");
        // spot-check a known decoded feature
        let ice = reg.placed.get("tectonic:underground_river/ice");
        assert!(ice.is_some(), "tectonic:underground_river/ice must decode");
    }

    fn collect_json(dir: &std::path::Path, out: &mut Vec<(String, String)>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                collect_json(&p, out);
            } else if p.extension().map(|x| x == "json").unwrap_or(false) {
                if let Ok(text) = std::fs::read_to_string(&p) {
                    let rel = p.to_string_lossy();
                    let name = rel.rsplit_once("/worldgen/").map(|(_, r)| r)
                        .unwrap_or(&rel)
                        .trim_end_matches(".json")
                        .to_string();
                    let name = name
                        .trim_start_matches("configured_feature/")
                        .trim_start_matches("placed_feature/")
                        .to_string();
                    let name = format!("{}:{}", pack_ns(&rel), name);
                    out.push((name, text));
                }
            }
        }
    }

    fn pack_ns(rel: &str) -> String {
        // .../data/<ns>/worldgen/... -> <ns>
        let parts: Vec<&str> = rel.split('/').collect();
        if let Some(i) = parts.iter().position(|&p| p == "data") {
            return parts.get(i + 1).map(|s| s.to_string()).unwrap_or_default();
        }
        String::new()
    }
}
