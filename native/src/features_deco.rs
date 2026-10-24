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
    /// BlockState.isAir() at pos
    fn is_air(&self, pos: BlockPos) -> bool;
    /// BlockState.is(BlockTags.REPLACEABLE)
    fn is_replaceable(&self, pos: BlockPos) -> bool;
    fn is_solid(&self, pos: BlockPos) -> bool;
    /// BlockState.hasSturdyFace(direction) at pos (dir: 0..6 = down,up,north,south,west,east)
    fn has_sturdy_face(&self, pos: BlockPos, dir: u8) -> bool;
    fn is_outside_build_height(&self, y: i32) -> bool;
}

// ---------------------------------------------------------------------------
// EnvScan column fastpath (cont-60): vertical EnvironmentScanPlacement on a
// snapshot-backed level. Vanilla steps one y at a time testing the target
// (and the allowed gate after each step); on the snapshot level each step
// was 1-3 full point queries (overlay probe + palette decode per predicate).
// The fastpath compiles the predicates to a flag/name tree and answers EVERY
// predicate of a step from ONE snapshot palette decode. Bit-exact by
// construction: the fastpath only engages when the chunk overlay has NO
// entries in the scan window (then the generic loop's per-predicate overlay
// checks all fall through to the same snapshot decode), falls back to the
// generic walk on ANY precondition miss (non-vertical, non-expressible
// predicate, overlay in the window, missing snapshot) BEFORE the first leaf
// fires (the scan is side-effect-free until the hit).
// ---------------------------------------------------------------------------

/// Compiled predicate over one snapshot palette entry (flag word + name).
#[derive(Clone, Debug)]
pub enum CFast {
    Const(bool),
    Flag { mask: u16, want: bool },
    /// state_is: any-of exact block names (the snapshot palette name)
    Name(Box<[Box<str>]>),
    /// fluid_is semantics: water/lava flag membership in the id list
    Fluid(Box<[Box<str>]>),
    All(Box<[CFast]>),
    Any(Box<[CFast]>),
    Not(Box<CFast>),
}

impl CFast {
    #[inline]
    pub fn eval(&self, flags: u16, name: Option<&str>) -> bool {
        match self {
            CFast::Const(b) => *b,
            CFast::Flag { mask, want } => (flags & mask != 0) == *want,
            CFast::Name(ids) => match name {
                Some(n) => ids.iter().any(|i| i.as_ref() == n),
                None => false,
            },
            CFast::Fluid(ids) => {
                if flags & crate::features_snap::flags::FLUID_WATER != 0 {
                    ids.iter().any(|i| i.as_ref() == "minecraft:water")
                } else if flags & crate::features_snap::flags::FLUID_LAVA != 0 {
                    ids.iter().any(|i| i.as_ref() == "minecraft:lava")
                } else {
                    false
                }
            }
            CFast::All(v) => v.iter().all(|p| p.eval(flags, name)),
            CFast::Any(v) => v.iter().any(|p| p.eval(flags, name)),
            CFast::Not(p) => !p.eval(flags, name),
        }
    }
}

/// Compile a vanilla predicate to the snapshot-expressible tree. None =
/// predicate touches another position (offset != 0) or needs the bridge
/// (WouldSurvive, Unsupported) — the generic step loop handles it.
pub fn compile_cfast(p: &BlockPredicate) -> Option<CFast> {
    match p {
        BlockPredicate::True => Some(CFast::Const(true)),
        BlockPredicate::AllOf(v) => {
            let mut out = Vec::with_capacity(v.len());
            for q in v {
                out.push(compile_cfast(q)?);
            }
            Some(CFast::All(out.into_boxed_slice()))
        }
        BlockPredicate::AnyOf(v) => {
            let mut out = Vec::with_capacity(v.len());
            for q in v {
                out.push(compile_cfast(q)?);
            }
            Some(CFast::Any(out.into_boxed_slice()))
        }
        BlockPredicate::Not(q) => Some(CFast::Not(Box::new(compile_cfast(q)?))),
        BlockPredicate::MatchingBlocks { ids, off } if *off == (0, 0, 0) => {
            let mut v: Vec<Box<str>> = ids.iter().map(|s| s.as_str().into()).collect();
            v.sort_unstable();
            v.dedup();
            Some(CFast::Name(v.into_boxed_slice()))
        }
        BlockPredicate::MatchingFluids { ids, off } if *off == (0, 0, 0) => {
            let mut v: Vec<Box<str>> = ids.iter().map(|s| s.as_str().into()).collect();
            v.sort_unstable();
            v.dedup();
            Some(CFast::Fluid(v.into_boxed_slice()))
        }
        BlockPredicate::Replaceable => Some(CFast::Flag {
            mask: crate::features_snap::flags::REPLACEABLE,
            want: true,
        }),
        BlockPredicate::Solid => Some(CFast::Flag {
            mask: crate::features_snap::flags::SOLID,
            want: true,
        }),
        BlockPredicate::HasSturdyFace { offset, dir } if *offset == (0, 0, 0) => {
            // ChunkLevel::has_sturdy_face dir mapping (DecoLevel order)
            let bit = match dir {
                0 => crate::features_snap::flags::STURDY_DOWN,
                1 => crate::features_snap::flags::STURDY_UP,
                2 => crate::features_snap::flags::STURDY_N,
                3 => crate::features_snap::flags::STURDY_S,
                4 => crate::features_snap::flags::STURDY_W,
                _ => crate::features_snap::flags::STURDY_E,
            };
            Some(CFast::Flag { mask: bit, want: true })
        }
        _ => None,
    }
}

/// The snapshot-side half of the fastpath (implemented by ChunkLevel —
/// the only level with a palette snapshot + the overlay column index).
pub trait EnvColScan {
    /// true when the chunk overlay has at least one entry in the (x, z)
    /// column within [y0, y1] — the fastpath must bail (overlay writes
    /// change predicate verdicts vs the snapshot).
    fn ov_window_has(&self, x: i32, z: i32, y0: i32, y1: i32) -> bool;
    /// All-compiled-predicate verdict at pos from ONE palette decode.
    /// None = the position is not snapshotted (bail to the generic walk).
    fn step_eval(&self, x: i32, y: i32, z: i32, fast: &CFast) -> Option<bool>;
    fn outside(&self, y: i32) -> bool;
}

/// Run the vertical EnvScan through the snapshot; returns:
/// None            — preconditions failed, run the generic loop instead
/// Some(None)      — scan exhausted without a hit (no leaf)
/// Some(Some(pos)) — hit (the CALLER runs leaf + the modifier chain)
pub fn env_scan_column_fast(
    level: &dyn DecoLevelExt,
    pos: BlockPos,
    dir: u8,
    max_steps: i32,
    target: &BlockPredicate,
    allowed: &Option<BlockPredicate>,
) -> Option<Option<BlockPos>> {
    use std::sync::OnceLock;
    static OFF: OnceLock<bool> = OnceLock::new();
    if *OFF.get_or_init(|| std::env::var("CRUS_DECO_NOENVFAST").ok().as_deref() == Some("1")) {
        return None;
    }
    let cols = level.env_col_scan()?;
    let step = match dir {
        1 => 1,
        _ => -1,
    };
    let ft = compile_cfast(target)?;
    let fa = match allowed {
        Some(a) => compile_cfast(a)?,
        None => CFast::Const(true),
    };
    // overlay window: origin + every stepped position (max_steps steps past
    // the origin; the final check position is origin + step*max_steps)
    let y_end = pos.y + step * max_steps;
    let (y0, y1) = if step > 0 {
        (pos.y, y_end)
    } else {
        (y_end, pos.y)
    };
    if cols.ov_window_has(pos.x, pos.z, y0, y1) {
        return None;
    }
    // origin allowed gate
    if !cols.step_eval(pos.x, pos.y, pos.z, &fa)? {
        return Some(None);
    }
    let mut cur = pos;
    let mut blocked = false;
    for _ in 0..max_steps.max(0) {
        if cols.step_eval(cur.x, cur.y, cur.z, &ft)? {
            return Some(Some(cur));
        }
        cur = BlockPos::new(cur.x, cur.y + step, cur.z);
        if cols.outside(cur.y) {
            return Some(None);
        }
        if !cols.step_eval(cur.x, cur.y, cur.z, &fa)? {
            // vanilla breaks to the FINAL check at cur
            blocked = true;
            break;
        }
    }
    let _ = blocked;
    // loop exhausted OR allowed-break: the final check at cur
    if cols.step_eval(cur.x, cur.y, cur.z, &ft)? {
        Some(Some(cur))
    } else {
        Some(None)
    }
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
    /// block ids resolved from the state-string table at handshake time;
    /// off = predicate offset (vanilla MatchingBlocksTest/MatchingFluidsTest)
    MatchingBlocks { ids: Vec<String>, off: (i32, i32, i32) },
    MatchingFluids { ids: Vec<String>, off: (i32, i32, i32) },
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
        BlockPredicate::MatchingBlocks { ids, off } => {
            // id resolution: ids are state STRINGS; equality via level.state -> String is
            // delegated: the bridge pre-resolves strings to a set of opaque ids.
            let p = BlockPos::new(pos.x + off.0, pos.y + off.1, pos.z + off.2);
            Some(level.state_is(p, ids))
        }
        BlockPredicate::MatchingFluids { ids, off } => {
            let p = BlockPos::new(pos.x + off.0, pos.y + off.1, pos.z + off.2);
            Some(level.fluid_is(p, ids))
        }
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
    /// RuleTest.tag_match — state.is(TagKey.create(BLOCK, tag)) at pos
    fn tag_is(&self, pos: BlockPos, tag: &str) -> bool;
    /// solidity of a block NAME (no position): used by OverlayLevel to answer
    /// is_solid for states written earlier in the same feature run (vanilla
    /// reads its own writes; our ops flush after the feature).
    fn name_is_solid(&self, name: &str) -> bool {
        let _ = name;
        false
    }
    /// record a state the engine will write at pos (OverlayLevel hook; no-op
    /// on raw levels)
    fn write_overlay(&self, pos: BlockPos, state: &str) {
        let _ = (pos, state);
    }
    /// pending written state at pos, if any
    fn read_overlay(&self, pos: BlockPos) -> Option<String> {
        let _ = pos;
        None
    }
    /// Debug: block registry key at pos (empty on mocks)
    fn state_name(&self, pos: BlockPos) -> String {
        let _ = pos;
        String::new()
    }
    /// FULL state string "ns:block[k=v,...]" at pos (empty when unknown).
    /// Overlay entries win over the world read (own writes are live).
    fn state_full(&self, pos: BlockPos) -> String {
        if let Some(s) = self.read_overlay(pos) {
            return s;
        }
        String::new()
    }
    /// BlockStateBase.isSolidRender at pos (PlaceOnGround decorator)
    fn is_solid_render(&self, pos: BlockPos) -> bool {
        let _ = pos;
        false
    }
    /// The EnvScan column fastpath backend (ChunkLevel only — the snapshot
    /// palette + the overlay column index). Default None = generic loop.
    fn env_col_scan(&self) -> Option<&dyn EnvColScan> {
        None
    }
}

/// level name of a state string ("ns:block[props]" -> "ns:block")
#[inline]
pub fn state_block_name(state: &str) -> &str {
    match state.split_once('[') {
        Some((n, _)) => n,
        None => state,
    }
}

/// OverlayLevel: wraps a DecoLevelExt and serves states written DURING the
/// current feature run (vanilla setBlock is live; our ops flush to the region
/// only after the feature returns — without the overlay, intra-feature reads
/// of own writes see stale blocks and diverge, e.g. weighted patch inners
/// re-tested by later tries).
pub struct OverlayLevel<'a> {
    inner: &'a dyn DecoLevelExt,
    ov: std::cell::RefCell<std::collections::HashMap<(i32, i32, i32), String>>,
}

impl<'a> OverlayLevel<'a> {
    pub fn new(inner: &'a dyn DecoLevelExt) -> Self {
        OverlayLevel { inner, ov: std::cell::RefCell::new(std::collections::HashMap::new()) }
    }
}

#[inline]
fn is_air_name(name: &str) -> bool {
    name == "minecraft:air" || name == "minecraft:cave_air" || name == "minecraft:void_air"
}

/// blocksMotion() for the states our features write (vanilla heightmap
/// MOTION_BLOCKING predicate is blocksMotion || fluid).
#[inline]
fn blocks_motion(name: &str) -> bool {
    !matches!(
        name,
        "minecraft:poppy"
            | "minecraft:oxeye_daisy"
            | "minecraft:cornflower"
            | "minecraft:lily_of_the_valley"
            | "minecraft:dandelion"
            | "minecraft:allium"
            | "minecraft:azure_bluet"
            | "minecraft:short_grass"
            | "minecraft:fern"
            | "minecraft:air"
            | "minecraft:cave_air"
            | "minecraft:void_air"
    )
}

#[inline]
fn is_fluid_name(name: &str) -> bool {
    name == "minecraft:water" || name == "minecraft:lava"
}

impl DecoLevel for OverlayLevel<'_> {
    fn min_y(&self) -> i32 { self.inner.min_y() }
    fn max_y(&self) -> i32 { self.inner.max_y() }
    fn height(&self, ty: HeightmapTy, x: i32, z: i32) -> i32 {
        // Paper 1.21.8: ChunkStatus CARVERS/FEATURES register FINAL_HEIGHTMAPS
        // (OCEAN_FLOOR/WORLD_SURFACE/MOTION_BLOCKING/MB_NO_LEAVES) for setBlock
        // updates. The WG maps (WORLD_SURFACE_WG/OCEAN_FLOOR_WG) are NOT
        // updated by decoration writes — they stay frozen at their pre-
        // decoration (surface) values (vanilla trace: WS_WG(1616,1750)=102
        // AFTER a cornflower@102, whose update r=true only hit the FINAL
        // WORLD_SURFACE map). So overlay writes must NOT reach WG reads:
        // vanilla placements reading WS_WG/OCEAN_FLOOR_WG see the frozen map.
        let base = self.inner.height(ty, x, z);
        match ty {
            HeightmapTy::WorldSurfaceWg | HeightmapTy::OceanFloorWg => return base,
            HeightmapTy::WorldSurface
            | HeightmapTy::OceanFloor
            | HeightmapTy::MotionBlocking
            | HeightmapTy::MotionBlockingNoLeaves => {}
        }
        let mut h = base;
        for ((ox, oy, oz), st) in self.ov.borrow().iter() {
            if *ox != x || *oz != z {
                continue;
            }
            let name = state_block_name(st);
            let blocking = match ty {
                HeightmapTy::WorldSurfaceWg | HeightmapTy::WorldSurface => {
                    !is_air_name(name)
                }
                HeightmapTy::OceanFloorWg | HeightmapTy::OceanFloor => {
                    !is_air_name(name) && !is_fluid_name(name)
                }
                HeightmapTy::MotionBlocking | HeightmapTy::MotionBlockingNoLeaves => {
                    blocks_motion(name) || is_fluid_name(name)
                }
            };
            if blocking && oy + 1 > h {
                h = oy + 1;
            }
        }
        h
    }
    fn state(&self, pos: BlockPos) -> u32 { self.inner.state(pos) }
    fn fluid(&self, pos: BlockPos) -> u32 { self.inner.fluid(pos) }
    fn biome_has_feature(&self, pos: BlockPos, feature: u32) -> bool {
        self.inner.biome_has_feature(pos, feature)
    }
    fn can_survive(&self, state: &str, pos: BlockPos) -> bool {
        self.inner.can_survive(state, pos)
    }
    fn is_air(&self, pos: BlockPos) -> bool {
        if let Some(st) = self.ov.borrow().get(&(pos.x, pos.y, pos.z)) {
            return is_air_name(state_block_name(st));
        }
        self.inner.is_air(pos)
    }
    fn is_replaceable(&self, pos: BlockPos) -> bool {
        if let Some(st) = self.ov.borrow().get(&(pos.x, pos.y, pos.z)) {
            // BlockTags.REPLACEABLE for the states our features write: the
            // small flowers ARE replaceable (a later patch try may overwrite
            // an earlier flower), everything else (ore/bone/grass_block/
            // andesite/tuff/disk material) is not.
            let name = state_block_name(st);
            return matches!(
                name,
                "poppy"
                    | "oxeye_daisy"
                    | "cornflower"
                    | "lily_of_the_valley"
                    | "dandelion"
                    | "allium"
                    | "azure_bluet"
                    | "short_grass"
                    | "fern"
            );
        }
        self.inner.is_replaceable(pos)
    }
    fn is_solid(&self, pos: BlockPos) -> bool {
        if let Some(st) = self.ov.borrow().get(&(pos.x, pos.y, pos.z)) {
            return self.inner.name_is_solid(state_block_name(st));
        }
        self.inner.is_solid(pos)
    }
    fn has_sturdy_face(&self, pos: BlockPos, dir: u8) -> bool {
        self.inner.has_sturdy_face(pos, dir)
    }
    fn is_outside_build_height(&self, y: i32) -> bool {
        self.inner.is_outside_build_height(y)
    }
}

impl DecoLevelExt for OverlayLevel<'_> {
    fn state_is(&self, pos: BlockPos, ids: &[String]) -> bool {
        if let Some(st) = self.ov.borrow().get(&(pos.x, pos.y, pos.z)) {
            // BlockState.is(Block) ignores properties
            let name = state_block_name(st);
            return ids.iter().any(|i| i == name);
        }
        self.inner.state_is(pos, ids)
    }
    fn fluid_is(&self, pos: BlockPos, ids: &[String]) -> bool {
        if self.ov.borrow().contains_key(&(pos.x, pos.y, pos.z)) {
            return false; // feature writes are never fluids
        }
        self.inner.fluid_is(pos, ids)
    }
    fn tag_is(&self, pos: BlockPos, tag: &str) -> bool {
        // Overlay entries MUST be tag-tested against their own name, not the
        // stale world block: ore_replace writes andesite/tuff, which DO match
        // stone/deepslate_ore_replaceables, while our ore/flower/bone writes
        // never match any queried tag. Delegating shifted the ore_gold
        // discard-draw stream wherever an earlier write covered a candidate.
        if let Some(st) = self.ov.borrow().get(&(pos.x, pos.y, pos.z)) {
            let name = state_block_name(st);
            return match tag {
                "minecraft:stone_ore_replaceables" => {
                    matches!(name, "stone" | "granite" | "diorite" | "andesite")
                }
                "minecraft:deepslate_ore_replaceables" => matches!(name, "deepslate" | "tuff"),
                _ => false,
            };
        }
        self.inner.tag_is(pos, tag)
    }
    fn name_is_solid(&self, name: &str) -> bool { self.inner.name_is_solid(name) }
    fn write_overlay(&self, pos: BlockPos, state: &str) {
        self.ov.borrow_mut().insert((pos.x, pos.y, pos.z), state.to_string());
    }
    fn read_overlay(&self, pos: BlockPos) -> Option<String> {
        self.ov.borrow().get(&(pos.x, pos.y, pos.z)).cloned()
    }
    fn state_name(&self, pos: BlockPos) -> String {
        if let Some(st) = self.ov.borrow().get(&(pos.x, pos.y, pos.z)) {
            return st.clone();
        }
        self.inner.state_name(pos)
    }
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
    /// SurfaceRelativeThresholdFilter: height(type) within [min, max] (no RNG).
    /// min/max are absolute; i32::MIN/MAX = unbounded.
    SurfaceRelativeThreshold {
        ty: HeightmapTy,
        min: i32,
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
            if crate::features_place::draw_trace_on() {
                eprintln!("[off] {} {} {} -> {} {} {}", pos.x, pos.y, pos.z, x, ny, z);
            }
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
        if crate::features_place::draw_trace_on() {
            eprintln!("[leaf] uid={top_uid} {} {} {}", pos.x, pos.y, pos.z);
        }
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
            if crate::features_place::draw_trace_on() {
                eprintln!("[hm] {} {} {} ty={ty:?} h={h} -> {}", pos.x, pos.y, pos.z, h > level.min_y());
            }
            if h > level.min_y() {
                expand_deco(mods, idx + 1, level, rng, BlockPos::new(pos.x, h, pos.z), leaf, top_uid);
            }
        }
        Mod::BiomeFilter { .. } => {
            // vanilla BiomeFilter reads PlacementContext.topFeature — the placed
            // feature currently being placed (passed through the expansion)
            let ok = level.biome_has_feature(pos, top_uid);
            if crate::features_place::draw_trace_on() {
                eprintln!("[biomef] {} {} {} uid={top_uid} -> {ok}", pos.x, pos.y, pos.z);
            }
            if ok {
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
            if crate::features_place::draw_trace_on() {
                eprintln!("[swd] {} {} {} of={of} ws={ws} max={max} -> {}", pos.x, pos.y, pos.z, ws - of <= *max);
            }
            if ws - of <= *max {
                expand_deco(mods, idx + 1, level, rng, pos, leaf, top_uid);
            }
        }
        Mod::SurfaceRelativeThreshold { ty, min, max } => {
            let h = level.height(*ty, pos.x, pos.z);
            if h >= *min && h <= *max {
                expand_deco(mods, idx + 1, level, rng, pos, leaf, top_uid);
            }
        }
        Mod::EnvScan { dir, max_steps, target, allowed } => {
            // Column fastpath: vertical scans on the snapshot level with
            // flag/name-expressible predicates and no overlay entries in the
            // scan window. Bit-exact fallback to the generic loop on any
            // precondition miss (before the first leaf fires).
            if let Some(hit) =
                env_scan_column_fast(level, pos, *dir, *max_steps, target, allowed)
            {
                if let Some(hp) = hit {
                    leaf(hp, rng);
                    expand_deco(mods, idx + 1, level, rng, hp, leaf, top_uid);
                }
                return;
            }
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
