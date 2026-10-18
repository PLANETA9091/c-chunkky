//! Features decoration layer (stage F1): level-dependent modifiers, block predicates,
//! biome-info noise, and the first native feature types. Bit-exact with vanilla 1.21.8.
//!
//! Contract: reference/features_contract.md §3-4. Level-free modifier core lives in
//! features_placement.rs (bit-pinned by ChainRef goldens); this module owns everything
//! that needs the 3x3 WorldGenRegion view. The pure variants are re-expanded here with
//! identical RNG order (re-pinned by re-running the same goldens through expand_deco).

use crate::features_placement::BlockPos;
use crate::features_rng::WorldgenRandom;
use crate::noise::ImprovedNoise;
use crate::rng::LegacyRandom;

pub use crate::features_placement::IntProvider;

// ---------------------------------------------------------------------------
// Level abstraction (the 3x3 WorldGenRegion view)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeightmapTy {
    WorldSurfaceWg,
    WorldSurface,
    OceanFloorWg,
    OceanFloor,
    MotionBlocking,
    MotionBlockingNoLeaves,
}

/// The WorldGenRegion view a feature chain sees during decoration.
/// All heights are already +1 (vanilla WorldGenRegion.getHeight semantics).
pub trait DecoLevel {
    fn min_y(&self) -> i32;
    fn max_y(&self) -> i32;
    fn height(&self, ty: HeightmapTy, x: i32, z: i32) -> i32;
    /// block state id at pos (0 = air); ids are opaque, resolved by the JVM bridge
    fn state(&self, pos: BlockPos) -> u32;
    fn fluid(&self, pos: BlockPos) -> u32;
    /// BiomeFilter: does the biome at pos include the placed feature (by uid)?
    fn biome_has_feature(&self, pos: BlockPos, feature: u32) -> bool;
    /// BlockState.canSurvive(level, pos) — resolved by the JVM bridge
    fn can_survive(&self, state: &str, pos: BlockPos) -> bool;
    /// BlockState.is(BlockTags.REPLACEABLE)
    fn is_replaceable(&self, pos: BlockPos) -> bool;
    fn is_solid(&self, pos: BlockPos) -> bool;
    /// BlockState.hasSturdyFace(direction) at pos (dir: 0..6 = down,up,north,south,west,east)
    fn has_sturdy_face(&self, pos: BlockPos, dir: u8) -> bool;
    fn is_outside_build_height(&self, y: i32) -> bool;
}

// ---------------------------------------------------------------------------
// Block predicates (blockpredicates package, subset by dptt census)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub enum BlockPredicate {
    True,
    AllOf(Vec<BlockPredicate>),
    AnyOf(Vec<BlockPredicate>),
    Not(Box<BlockPredicate>),
    /// block ids resolved from the state-string table at handshake time
    MatchingBlocks(Vec<String>),
    MatchingFluids(Vec<String>),
    Replaceable,
    Solid,
    HasSturdyFace { offset: (i32, i32, i32), dir: u8 },
    WouldSurvive { state: String },
    InsideWorldBounds,
    Unobstructed,
    /// decode kept the JSON but the engine cannot evaluate it yet
    Unsupported(&'static str),
}

pub fn pred_matches(
    pred: &BlockPredicate,
    level: &dyn DecoLevelExt,
    pos: BlockPos,
    state_at_pos: u32,
) -> Option<bool> {
    match pred {
        BlockPredicate::True => Some(true),
        BlockPredicate::AllOf(v) => {
            for p in v {
                if !pred_matches(p, level, pos, state_at_pos)? {
                    return Some(false);
                }
            }
            Some(true)
        }
        BlockPredicate::AnyOf(v) => {
            for p in v {
                if pred_matches(p, level, pos, state_at_pos)? {
                    return Some(true);
                }
            }
            Some(false)
        }
        BlockPredicate::Not(p) => pred_matches(p, level, pos, state_at_pos).map(|b| !b),
        BlockPredicate::MatchingBlocks(ids) => {
            // id resolution: ids are state STRINGS; equality via level.state -> String is
            // delegated: the bridge pre-resolves strings to a set of opaque ids.
            Some(level.state_is(pos, ids))
        }
        BlockPredicate::MatchingFluids(ids) => Some(level.fluid_is(pos, ids)),
        BlockPredicate::Replaceable => Some(level.is_replaceable(pos)),
        BlockPredicate::Solid => Some(level.is_solid(pos)),
        BlockPredicate::HasSturdyFace { offset, dir } => {
            let p = BlockPos::new(pos.x + offset.0, pos.y + offset.1, pos.z + offset.2);
            Some(level.has_sturdy_face(p, *dir))
        }
        BlockPredicate::WouldSurvive { state } => Some(level.can_survive(state, pos)),
        BlockPredicate::InsideWorldBounds => Some(true), // center-chunk origins always inside
        BlockPredicate::Unobstructed => Some(true),      // approximation: no entity blocks at gen time
        BlockPredicate::Unsupported(_) => None,
    }
}

// extension hooks on DecoLevel used by predicates (string-set form)
pub trait DecoLevelExt: DecoLevel {
    fn state_is(&self, pos: BlockPos, ids: &[String]) -> bool;
    fn fluid_is(&self, pos: BlockPos, ids: &[String]) -> bool;
}

// ---------------------------------------------------------------------------
// Biome.BIOME_INFO_NOISE (PerlinNoise(LegacyRandomSource(-1698467423379765010), List.of(0)))
// ---------------------------------------------------------------------------

pub struct BiomeInfoNoise {
    imp: ImprovedNoise,
}

impl BiomeInfoNoise {
    pub fn new() -> Self {
        let mut lcg = LegacyRandom::new(-1698467423379765010);
        // useNewFactory ctor: forkPositional() -> LegacyPositionalRandomFactory(nextLong())
        let factory_seed = lcg.next_long();
        // factory.fromHashOf("octave_0") = LegacyRandomSource(name.hashCode() ^ seed)
        let seed = (java_string_hash("octave_0") as i64) ^ factory_seed;
        let mut srng = LegacyRandom::new(seed);
        BiomeInfoNoise {
            imp: ImprovedNoise::new_legacy(&mut srng),
        }
    }
    /// PerlinNoise.getValue(x, 0, z) with firstOctave=0, amplitudes=[1.0]:
    /// input_factor=1, value_factor=1, single octave -> improved.noise(wrap(x), wrap(y), wrap(z))
    #[inline]
    pub fn value(&self, x: f64, z: f64) -> f64 {
        self.imp.noise(wrap_coord(x), 0.0, wrap_coord(z))
    }
}

impl Default for BiomeInfoNoise {
    fn default() -> Self {
        Self::new()
    }
}

/// Global BIOME_INFO_NOISE (fixed vanilla seed — world-independent).
pub fn biome_info_noise() -> &'static BiomeInfoNoise {
    static NOISE: std::sync::OnceLock<BiomeInfoNoise> = std::sync::OnceLock::new();
    NOISE.get_or_init(BiomeInfoNoise::new)
}

#[inline]
fn wrap_coord(v: f64) -> f64 {
    v - (v / 3.3554432E7).floor() * 3.3554432E7
}

pub fn java_string_hash(s: &str) -> i32 {
    let mut h: i32 = 0;
    for c in s.encode_utf16() {
        h = h.wrapping_mul(31).wrapping_add(c as i32);
    }
    h
}

// ---------------------------------------------------------------------------
// Full modifier set
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub enum Mod {
    /// level-free core (InSquare/Count/Rarity/RandomOffset/HeightRange) — same RNG order
    Pure(crate::features_placement::Modifier),
    Heightmap {
        ty: HeightmapTy,
    },
    BiomeFilter {
        feature: u32,
    },
    PredFilter(BlockPredicate),
    SurfaceWaterDepth {
        max: i32,
    },
    EnvScan {
        /// 0=down 1=up (only directions used by dptt packs); max_steps; target; allowed
        dir: u8,
        max_steps: i32,
        target: BlockPredicate,
        allowed: Option<BlockPredicate>,
    },
    NoiseBasedCount {
        ratio: i32,
        factor: f64,
        offset: f64,
    },
    Unsupported(&'static str),
}

fn pure_expand_one(
    m: &crate::features_placement::Modifier,
    rng: &mut WorldgenRandom,
    pos: BlockPos,
    out: &mut Vec<BlockPos>,
    min_y: i32,
    gen_depth: i32,
) {
    // mirrors features_placement::expand_rec single-variant bodies (bit-pinned there)
    match *m {
        crate::features_placement::Modifier::InSquare => {
            let x = rng.next_int_bound(16) + pos.x;
            let z = rng.next_int_bound(16) + pos.z;
            out.push(BlockPos::new(x, pos.y, z));
        }
        crate::features_placement::Modifier::Count(p) => {
            let n = p.sample(rng).max(0);
            for _ in 0..n {
                out.push(pos);
            }
        }
        crate::features_placement::Modifier::RarityFilter { chance } => {
            if rng.next_f32() < 1.0 / (chance as f32) {
                out.push(pos);
            }
        }
        crate::features_placement::Modifier::RandomOffset { xz, y } => {
            let x = pos.x + xz.sample(rng);
            let ny = pos.y + y.sample(rng);
            let z = pos.z + xz.sample(rng);
            out.push(BlockPos::new(x, ny, z));
        }
        crate::features_placement::Modifier::HeightRange(hp) => {
            let h = hp.sample(rng, min_y, gen_depth);
            out.push(BlockPos::new(pos.x, h, pos.z));
        }
    }
}

/// Depth-first expansion over the full modifier set (lazy flatMap semantics):
/// each position runs the WHOLE modifier chain and the LEAF action (the
/// configured feature body — which draws RNG) BEFORE the next position is
/// pulled. Vanilla: stream.forEach(pos -> configuredFeature.place(..)) with
/// lazy flatMap pulls. `top_uid` = uid of the placed feature being placed
/// (vanilla topFeature).
pub fn expand_deco(
    mods: &[Mod],
    idx: usize,
    level: &dyn DecoLevelExt,
    rng: &mut WorldgenRandom,
    pos: BlockPos,
    leaf: &mut dyn FnMut(BlockPos, &mut WorldgenRandom) -> bool,
    top_uid: u32,
) {
    if idx == mods.len() {
        leaf(pos, rng);
        return;
    }
    match &mods[idx] {
        Mod::Pure(m) => {
            let gen_depth = level.max_y() - level.min_y() + 1;
            let mut next = Vec::new();
            pure_expand_one(m, rng, pos, &mut next, level.min_y(), gen_depth);
            for p in next {
                expand_deco(mods, idx + 1, level, rng, p, leaf, top_uid);
            }
        }
        Mod::Heightmap { ty } => {
            let h = level.height(*ty, pos.x, pos.z);
            if h > level.min_y() {
                expand_deco(mods, idx + 1, level, rng, BlockPos::new(pos.x, h, pos.z), leaf, top_uid);
            }
        }
        Mod::BiomeFilter { .. } => {
            // vanilla BiomeFilter reads PlacementContext.topFeature — the placed
            // feature currently being placed (passed through the expansion)
            if level.biome_has_feature(pos, top_uid) {
                expand_deco(mods, idx + 1, level, rng, pos, leaf, top_uid);
            }
        }
        Mod::PredFilter(p) => {
            let st = level.state(pos);
            if pred_matches(p, level, pos, st) == Some(true) {
                expand_deco(mods, idx + 1, level, rng, pos, leaf, top_uid);
            }
        }
        Mod::SurfaceWaterDepth { max } => {
            let of = level.height(HeightmapTy::OceanFloor, pos.x, pos.z);
            let ws = level.height(HeightmapTy::WorldSurface, pos.x, pos.z);
            if ws - of <= *max {
                expand_deco(mods, idx + 1, level, rng, pos, leaf, top_uid);
            }
        }
        Mod::EnvScan { dir, max_steps, target, allowed } => {
            // vanilla EnvironmentScanPlacement: allowed(origin) gate, then step loop
            let allowed_ok = |level: &dyn DecoLevelExt, p: BlockPos| -> bool {
                match allowed {
                    Some(a) => {
                        let st = level.state(p);
                        pred_matches(a, level, p, st) == Some(true)
                    }
                    None => true,
                }
            };
            if !allowed_ok(level, pos) {
                return;
            }
            let step = match dir {
                1 => (0, 1),
                _ => (0, -1),
            };
            let mut cur = pos;
            for _ in 0..*max_steps {
                let st = level.state(cur);
                if pred_matches(target, level, cur, st) == Some(true) {
                    leaf(cur, rng);
                    expand_deco(mods, idx + 1, level, rng, cur, leaf, top_uid);
                    return;
                }
                cur = BlockPos::new(cur.x, cur.y + step.1, cur.z);
                if level.is_outside_build_height(cur.y) {
                    return;
                }
                if !allowed_ok(level, cur) {
                    break;
                }
            }
            let st = level.state(cur);
            if pred_matches(target, level, cur, st) == Some(true) {
                leaf(cur, rng);
                expand_deco(mods, idx + 1, level, rng, cur, leaf, top_uid);
            }
        }
        Mod::NoiseBasedCount { ratio, factor, offset } => {
            // RepeatingPlacement: n = ceil((BIOME_INFO_NOISE(x/factor, z/factor)+offset)*ratio)
            let v = biome_info_noise().value(pos.x as f64 / factor, pos.z as f64 / factor);
            let n = ((v + offset) * (*ratio as f64)).ceil() as i64;
            for _ in 0..n.max(0) {
                expand_deco(mods, idx + 1, level, rng, pos, leaf, top_uid);
            }
        }
        Mod::Unsupported(_) => {}
    }
}
