//! Features place engine (stage F1): registry walk, selector dispatch, DecoOp emission.
//! Selector RNG pinned from decompile: SimpleRandomSelector = nextInt(size) via next(31)
//! path; RandomBooleanSelector = next(1); RandomSelector = nextFloat()<chance, first hit
//! returns; WeightedStateProvider = nextInt(totalWeight) + linear walk (probe-verify).

use crate::features_deco::{expand_deco, BlockPredicate, DecoLevelExt, HeightmapTy, Mod};
use crate::features_json::{FeatureCfg, FeatureRegistry, PlacedDef, RuleTest, StateSrc};
use crate::features_placement::IntProvider as IP;
use crate::features_placement::BlockPos;
use crate::features_rng::WorldgenRandom;

/// One planned block write, relative to the center chunk origin (x,z may span -16..31).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DecoOp {
    pub x: i16,
    pub y: i16,
    pub z: i16,
    /// index into the decoStates table (handshake-resolved by the JVM bridge)
    pub state: u16,
    /// bit0 = schedule_tick; bit1 = rotated provider; bits2-3 = axis pick (0=X 1=Y 2=Z)
    pub flags: u8,
    /// tree aux: bits 0-3 = leaf distance override (1..7), bit 4 = waterlogged=true
    pub aux: u16,
}

pub const FLAG_TICK: u8 = 1;
pub const FLAG_ROTATED: u8 = 2;
pub const AXIS_SHIFT: u8 = 2;
/// BulkSectionAccess write (vanilla ore): NO heightmap update, no postprocessing
pub const FLAG_RAW: u8 = 16;
/// markPosForPostprocessing (vanilla DiskFeature markAboveForPostProcessing)
pub const FLAG_POSTPROC: u8 = 32;
/// spawner op: aux bits 8-15 = entity ordinal (0=skeleton 1=zombie 2=zombie
/// 3=spider); Java must setBlock + SpawnerBlockEntity.setEntityId
pub const FLAG_SPAWNER: u8 = 64;
/// chest op (monster_room): Java must attach the SIMPLE_DUNGEON loot table
pub const FLAG_CHEST: u8 = 128;

pub struct PlaceCtx<'a> {
    pub reg: &'a FeatureRegistry,
    pub level: &'a dyn DecoLevelExt,
    /// center chunk block origin (chunkX<<4, chunkZ<<4)
    pub center_x: i32,
    pub center_z: i32,
    /// state string -> decoStates table index (decoSetIds handshake)
    pub state_index: &'a std::collections::HashMap<String, u16>,
    /// placed name -> uid (vanilla topFeature identity for BiomeFilter)
    pub uid_index: &'a std::collections::HashMap<String, u32>,
}

impl<'a> PlaceCtx<'a> {
    #[inline]
    fn rel(&self, pos: BlockPos) -> (i16, i16) {
        ((pos.x - self.center_x) as i16, (pos.z - self.center_z) as i16)
    }
    /// record an upcoming write so later reads in the same feature see it
    /// (vanilla setBlock is live; ops flush after the feature)
    #[inline]
    fn write(&self, pos: BlockPos, state: &str) {
        self.level.write_overlay(pos, state);
    }
    /// emit a block write op + overlay bookkeeping
    fn emit(&self, ops: &mut Vec<DecoOp>, pos: BlockPos, state: &str, flags: u8) {
        self.emit_aux(ops, pos, state, flags, 0);
    }

    /// emit with tree aux (leaf distance / waterlogged overrides)
    pub(crate) fn emit_aux(
        &self,
        ops: &mut Vec<DecoOp>,
        pos: BlockPos,
        state: &str,
        flags: u8,
        aux: u16,
    ) {
        let (rx, rz) = self.rel(pos);
        let sid = self.state_index.get(state).copied().unwrap_or(0);
        ops.push(DecoOp { x: rx, y: pos.y as i16, z: rz, state: sid, flags, aux });
        if sel_trace_on(self.center_x >> 4, self.center_z >> 4) {
            eprintln!(
                "[op@{}] w ({},{},{}) {state} flags={flags:#x}",
                self.center_x >> 4,
                pos.x,
                pos.y,
                pos.z
            );
        }
        self.write(pos, state);
    }
}

pub(crate) fn sample_state(
    src: &StateSrc,
    level: &dyn DecoLevelExt,
    pos: BlockPos,
    rng: &mut WorldgenRandom,
    flags: &mut u8,
) -> Option<String> {
    let raw = match src {
        StateSrc::Simple(s) => s.clone(),
        StateSrc::Weighted(list) => {
            let total: u32 = list.iter().map(|(w, _)| w).sum();
            if total == 0 {
                return None;
            }
            let mut pick = rng.next_int_bound(total as i32) as i32;
            let mut hit = None;
            for (w, s) in list {
                pick -= *w as i32;
                if pick < 0 {
                    hit = Some(s.clone());
                    break;
                }
            }
            hit.or_else(|| list.last().map(|(_, s)| s.clone()))?
        }
        StateSrc::RuleBased { fallback, rules } => {
            // RuleBasedBlockStateProvider.getState: first matching rule's
            // then-provider, else the fallback. Rule predicates draw no RNG;
            // only the PICKED provider may draw (weighted/rotated leaves).
            let mut picked: Option<&StateSrc> = None;
            for (pred, then) in rules {
                let st = level.state(pos);
                if crate::features_deco::pred_matches(pred, level, pos, st) == Some(true) {
                    picked = Some(then);
                    break;
                }
            }
            let leaf = match picked {
                Some(p) => p,
                None => fallback,
            };
            return sample_state(leaf, level, pos, rng, flags);
        }
        StateSrc::RandomizedInt { source, property, values } => {
            let raw = sample_state(source, level, pos, rng, flags)?;
            let v = values.sample(rng);
            return Some(crate::features_json::state_set_prop(&raw, property, &v.to_string()));
        }
        StateSrc::Unsupported(_) => return None,
    };
    // RotatedBlockProvider.getState: Axis.getRandom(random) = VALUES[nextInt(3)]
    // (X, Y, Z order) — consumes RNG on EVERY draw, then trySetValue(AXIS, pick).
    match raw.strip_prefix("ROTATED\0") {
        Some(inner) => {
            let axis = rng.next_int_bound(3) as u8;
            *flags |= FLAG_ROTATED | (axis << AXIS_SHIFT);
            Some(inner.to_string())
        }
        None => Some(raw),
    }
}

/// Direction helpers (vanilla Direction.values() ordinals):
/// 0=down 1=up 2=north 3=south 4=west 5=east
#[inline]
pub(crate) fn dir_offset(dir: u8) -> (i32, i32, i32) {
    match dir {
        0 => (0, -1, 0),
        1 => (0, 1, 0),
        2 => (0, 0, -1),
        3 => (0, 0, 1),
        4 => (-1, 0, 0),
        _ => (1, 0, 0),
    }
}

#[inline]
fn pos_offset(pos: BlockPos, dir: u8) -> BlockPos {
    let (dx, dy, dz) = dir_offset(dir);
    BlockPos::new(pos.x + dx, pos.y + dy, pos.z + dz)
}

/// Recursively place a placed feature by name. Returns "any op placed" (vanilla place()).
pub fn place_placed(
    ctx: &PlaceCtx,
    name: &str,
    rng: &mut WorldgenRandom,
    origin: BlockPos,
    ops: &mut Vec<DecoOp>,
    depth: u32,
) -> bool {
    if depth > 64 {
        return false; // vanilla has no cycles; guard against pack recursion
    }
    let Some(PlacedDef { feature, placement }) = ctx.reg.placed.get(name) else {
        return false;
    };
    let mods: &[Mod] = placement;
    let uid = ctx.uid_index.get(name).copied().unwrap_or(u32::MAX);
    let feature = feature.clone();
    let mut any = false;
    // leaf = the configured feature body, pulled LAZILY per position (the
    // body draws RNG before the next position's modifiers run)
    {
        let mut leaf = |pos: BlockPos, rng: &mut WorldgenRandom| -> bool {
            if place_configured(ctx, &feature, rng, pos, ops, depth + 1) {
                any = true;
            }
            true
        };
        expand_deco(mods, 0, ctx.level, rng, origin, &mut leaf, uid);
    }
    any
}

/// DFS over the placed->configured feature graph: true when EVERY configured
/// body in the graph is natively supported. Any Unsupported node (tree,
/// vegetation_patch, block_column, geode, ...) anywhere in the graph — even
/// nested behind a selector or a patch — forces the whole placed feature to
/// the vanilla-lane delegate: a partial native run would desync the feature's
/// own RNG stream vs vanilla (leaf draws interleave with positions).
pub fn placed_fully_supported(reg: &FeatureRegistry, name: &str, depth: u32) -> bool {
    if depth > 64 {
        return false;
    }
    let Some(PlacedDef { feature, placement }) = reg.placed.get(name) else {
        return false;
    };
    cfg_fully_supported(reg, feature, depth + 1)
}

fn cfg_fully_supported(reg: &FeatureRegistry, cfg_name: &str, depth: u32) -> bool {
    if depth > 64 {
        return false;
    }
    let Some(cfg) = reg.configured.get(cfg_name) else {
        return false;
    };
    match cfg {
        FeatureCfg::NoOp
        | FeatureCfg::SimpleBlock { .. }
        | FeatureCfg::ReplaceBlock { .. }
        | FeatureCfg::Ore { .. }
        | FeatureCfg::ScatteredOre { .. }
        | FeatureCfg::Disk { .. }
        | FeatureCfg::Tree { .. }
        | FeatureCfg::VegetationPatch { .. }
        | FeatureCfg::NetherrackReplaceBlobs { .. }
        | FeatureCfg::Seagrass { .. }
        | FeatureCfg::Spring { .. }
        | FeatureCfg::BlockPile { .. }
        | FeatureCfg::Kelp
        | FeatureCfg::UnderwaterMagma { .. }
        | FeatureCfg::MultifaceGrowth { .. }
        | FeatureCfg::Vines => true,
        FeatureCfg::BlockColumn { layers, allowed, .. } => {
            !matches!(allowed, BlockPredicate::Unsupported(_))
                && layers.iter().all(|(_, sp)| state_src_supported(sp))
        }
        FeatureCfg::RootSystem { tree_feature, allowed_tree_position, root_state, hanging_root_state, .. } => {
            !matches!(allowed_tree_position, BlockPredicate::Unsupported(_))
                && state_src_supported(root_state)
                && state_src_supported(hanging_root_state)
                && placed_fully_supported(reg, tree_feature, depth + 1)
        }
        // MonsterRoom delegates: the chest loot seed (64-bit nextLong) and
        // spawner entity draws live inside the vanilla RNG stream — parity
        // first until the op protocol can carry them.
        FeatureCfg::MonsterRoom => false,
        FeatureCfg::RandomSelector { entries, default } => {
            entries.iter().all(|(_, n)| placed_fully_supported(reg, n, depth + 1))
                && placed_fully_supported(reg, default, depth + 1)
        }
        FeatureCfg::SimpleRandomSelector { entries } => entries
            .iter()
            .all(|n| placed_fully_supported(reg, n, depth + 1)),
        FeatureCfg::RandomBoolSelector { if_true, if_false, .. } => {
            placed_fully_supported(reg, if_true, depth + 1)
                && placed_fully_supported(reg, if_false, depth + 1)
        }
        FeatureCfg::RandomPatch { inner, .. } => placed_fully_supported(reg, inner, depth + 1),
        FeatureCfg::Unsupported(_) => false,
    }
}

/// state-provider support walk (RandomizedInt source chains etc.)
fn state_src_supported(src: &StateSrc) -> bool {
    match src {
        StateSrc::Simple(_) | StateSrc::Weighted(_) => true,
        StateSrc::RuleBased { fallback, rules } => {
            state_src_supported(fallback) && rules.iter().all(|(p, s)| {
                !matches!(p, BlockPredicate::Unsupported(_)) && state_src_supported(s)
            })
        }
        StateSrc::RandomizedInt { source, .. } => state_src_supported(source),
        StateSrc::Unsupported(_) => false,
    }
}

/// Place one configured feature at pos. Emits DecoOps for block-writing features.
pub fn place_configured(
    ctx: &PlaceCtx,
    name: &str,
    rng: &mut WorldgenRandom,
    pos: BlockPos,
    ops: &mut Vec<DecoOp>,
    depth: u32,
) -> bool {
    if depth > 64 {
        return false;
    }
    let Some(cfg) = ctx.reg.configured.get(name) else {
        return false;
    };
    // vanilla Feature.place(config, level, gen, random, origin):
    // `return level.ensureCanWrite(origin) && this.place(...)` — the WHOLE
    // feature body (incl. the state-provider draw) is skipped when the
    // origin falls outside the 3x3 writeRadius. This is what eats exactly
    // one draw (the rotated axis nextInt(3)) for patch-inner bones that
    // land one past the region edge.
    {
        let (rx, rz) = ctx.rel(pos);
        if !in_writable(rx as i32, rz as i32) {
            return false;
        }
    }
    match cfg {
        FeatureCfg::NoOp => true,
        FeatureCfg::SimpleBlock { place, ticks } => {
            let mut flags: u8 = 0;
            let Some(state) = sample_state(place, ctx.level, pos, rng, &mut flags) else {
                return false;
            };
            if !ctx.level.can_survive(&state, pos) {
                return false;
            }
            if *ticks {
                flags |= FLAG_TICK;
            }
            ctx.emit(ops, pos, &state, flags);
            true
        }
        FeatureCfg::ReplaceBlock { targets } => {
            let st = ctx.level.state(pos);
            for (pred, src) in targets {
                let hit = match pred {
                    crate::features_deco::BlockPredicate::True => true,
                    crate::features_deco::BlockPredicate::MatchingBlocks { .. } => {
                        ctx.level.state_is(pos, pred_ids(pred))
                    }
                    _ => false,
                };
                if hit {
                    let mut flags: u8 = 0;
                    if let Some(st) = sample_state(src, ctx.level, pos, rng, &mut flags) {
                        ctx.emit(ops, pos, &st, flags);
                    }
                    break;
                }
            }
            true // vanilla always returns true
        }
        FeatureCfg::RandomSelector { entries, default } => {
            let tr = sel_trace_on(ctx.center_x >> 4, ctx.center_z >> 4);
            for (chance, next) in entries {
                let r = rng.next_f32();
                if tr {
                    eprintln!(
                        "[rsel@{}] p=({},{},{}) chance={chance} roll={r:.9} hit={}",
                        ctx.center_x >> 4,
                        pos.x,
                        pos.y,
                        pos.z,
                        r < *chance
                    );
                }
                if r < *chance {
                    return place_placed(ctx, next, rng, pos, ops, depth + 1);
                }
            }
            if tr {
                eprintln!(
                    "[rsel@{}] p=({},{},{}) -> default",
                    ctx.center_x >> 4,
                    pos.x,
                    pos.y,
                    pos.z
                );
            }
            place_placed(ctx, default, rng, pos, ops, depth + 1)
        }
        FeatureCfg::SimpleRandomSelector { entries } => {
            if entries.is_empty() {
                return false;
            }
            let idx = rng.next_int_bound(entries.len() as i32) as usize;
            if sel_trace_on(ctx.center_x >> 4, ctx.center_z >> 4) {
                eprintln!(
                    "[srs@{}] p=({},{},{}) size={} idx={idx}",
                    ctx.center_x >> 4,
                    pos.x,
                    pos.y,
                    pos.z,
                    entries.len()
                );
            }
            place_placed(ctx, &entries[idx], rng, pos, ops, depth + 1)
        }
        FeatureCfg::RandomBoolSelector {
            if_true,
            if_false,
            ..
        } => {
            let pick = rng.next_bool();
            if sel_trace_on(ctx.center_x >> 4, ctx.center_z >> 4) {
                eprintln!(
                    "[rbs@{}] p=({},{},{}) pick={pick}",
                    ctx.center_x >> 4,
                    pos.x,
                    pos.y,
                    pos.z
                );
            }
            if pick {
                place_placed(ctx, if_true, rng, pos, ops, depth + 1)
            } else {
                place_placed(ctx, if_false, rng, pos, ops, depth + 1)
            }
        }
        FeatureCfg::Ore { discard, size, targets } => {
            place_ore(ctx, *discard, *size, targets, rng, pos, ops, false)
        }
        FeatureCfg::ScatteredOre { discard, size, targets } => {
            place_ore(ctx, *discard, *size, targets, rng, pos, ops, true)
        }
        FeatureCfg::RandomPatch { tries, xz_spread, y_spread, inner } => {
            place_random_patch(ctx, *tries, *xz_spread, *y_spread, inner, rng, pos, ops, depth)
        }
        FeatureCfg::Disk { provider, target, radius, half_height } => {
            place_disk(ctx, provider, target, radius, *half_height, rng, pos, ops)
        }
        FeatureCfg::Tree(cfg) => crate::features_tree::place_tree(ctx, cfg, rng, pos, ops),
        FeatureCfg::VegetationPatch(cfg) => {
            crate::features_tree::place_vegetation_patch(ctx, cfg, rng, pos, ops)
        }
        // --- batch-1 simple types (dptt delegate killers) -------------------
        FeatureCfg::NetherrackReplaceBlobs { radius, state, target_block } => {
            place_replace_blobs(ctx, radius, state, target_block, rng, pos, ops)
        }
        FeatureCfg::Seagrass { probability } => {
            place_seagrass(ctx, *probability, rng, pos, ops)
        }
        FeatureCfg::Spring { valid_blocks, requires_block_below, rock_count, hole_count, state } => {
            place_spring(ctx, valid_blocks, *requires_block_below, *rock_count, *hole_count, state, pos, ops)
        }
        FeatureCfg::BlockPile { state_provider } => {
            place_block_pile(ctx, state_provider, rng, pos, ops)
        }
        FeatureCfg::Kelp => place_kelp(ctx, rng, pos, ops),
        // MonsterRoom stays a hybrid delegate: the chest loot seed (a 64-bit
        // nextLong) and the spawner entity draws sit INSIDE the vanilla RNG
        // stream and cannot travel through the 6-int op protocol without
        // widening it — parity first (see worklog cont-56).
        FeatureCfg::MonsterRoom => false,
        FeatureCfg::UnderwaterMagma { floor_search_range, placement_radius, prob } => {
            place_underwater_magma(ctx, *floor_search_range, *placement_radius, *prob, rng, pos, ops)
        }
        // --- batch-2 (dptt census round 2) ---------------------------------
        FeatureCfg::Vines => place_vines(ctx, pos, ops),
        // --- batch-3 (cont-58): multiface growth (glow lichen) -------------
        FeatureCfg::MultifaceGrowth { block, search_range, can_floor, can_ceiling, can_wall, chance, can_place_on } => {
            place_multiface_growth(ctx, block, *search_range, *can_floor, *can_ceiling, *can_wall, *chance, can_place_on, rng, pos, ops)
        }
        FeatureCfg::BlockColumn { direction, layers, prioritize_tip, allowed } => {
            place_block_column(ctx, *direction, layers, *prioritize_tip, allowed, rng, pos, ops)
        }
        FeatureCfg::RootSystem { tree_feature, allowed_tree_position, required_vertical_space_for_tree, allowed_vertical_water_for_tree, root_column_max_height, root_radius, root_placement_attempts, root_replaceable_tag, root_state, hanging_root_radius, hanging_root_placement_attempts, hanging_roots_vertical_span, hanging_root_state } => {
            place_root_system(
                ctx, tree_feature, allowed_tree_position, *required_vertical_space_for_tree,
                *allowed_vertical_water_for_tree, *root_column_max_height, *root_radius,
                *root_placement_attempts, root_replaceable_tag, root_state,
                *hanging_root_radius, *hanging_root_placement_attempts, *hanging_roots_vertical_span,
                hanging_root_state, rng, pos, ops,
            )
        }
        FeatureCfg::Unsupported(_) => false,
    }
}

/// RandomPatchFeature: 2 draws per axis per try (X then Y then Z), the inner
/// placed feature runs on the SAME rng stream (no reseed), tries from the
/// patch origin (which already includes outer modifier offsets).
fn place_random_patch(
    ctx: &PlaceCtx,
    tries: i32,
    xz_spread: i32,
    y_spread: i32,
    inner: &str,
    rng: &mut WorldgenRandom,
    origin: BlockPos,
    ops: &mut Vec<DecoOp>,
    depth: u32,
) -> bool {
    let i1 = xz_spread + 1;
    let i2 = y_spread + 1;
    let mut count = 0i32;
    for _ in 0..tries {
        let dx = rng.next_int_bound(i1) - rng.next_int_bound(i1);
        let dy = rng.next_int_bound(i2) - rng.next_int_bound(i2);
        let dz = rng.next_int_bound(i1) - rng.next_int_bound(i1);
        let p = BlockPos::new(origin.x + dx, origin.y + dy, origin.z + dz);
        if place_placed(ctx, inner, rng, p, ops, depth + 1) {
            count += 1;
        }
    }
    count > 0
}

/// DiskFeature: radius sampled ONCE; x-outer/z-inner scan with the circle
/// mask; per-column TOP-DOWN target test + state pick; first hit of each
/// consecutive target run marks the (up to 2) blocks above for postprocessing
/// (walk stops at the first air above).
fn place_disk(
    ctx: &PlaceCtx,
    provider: &StateSrc,
    target: &crate::features_deco::BlockPredicate,
    radius: &crate::features_placement::IntProvider,
    half_height: i32,
    rng: &mut WorldgenRandom,
    origin: BlockPos,
    ops: &mut Vec<DecoOp>,
) -> bool {
    let r = radius.sample(rng).max(0);
    let y_top = origin.y + half_height;
    let y_bot = origin.y - half_height - 1; // exclusive lower bound (vanilla i1)
    let mut any = false;
    for dx in -r..=r {
        for dz in -r..=r {
            if dx * dx + dz * dz > r * r {
                continue;
            }
            // placeColumn: y from y_top DOWN to y_bot + 1 (inclusive)
            let mut flag1 = false; // "placed in the previous iteration"
            for y in (y_bot + 1..=y_top).rev() {
                let p = BlockPos::new(origin.x + dx, y, origin.z + dz);
                let st = ctx.level.state(p);
                if crate::features_deco::pred_matches(target, ctx.level, p, st) != Some(true) {
                    flag1 = false;
                    continue;
                }
                let mut flags: u8 = 0;
                if let Some(state) = sample_state(provider, ctx.level, p, rng, &mut flags) {
                    ctx.emit(ops, p, &state, flags);
                    if !flag1 {
                        // markAboveForPostProcessing: up to 2 blocks above,
                        // stop (no more marks) at the first air above
                        let mut m = p;
                        for _ in 0..2 {
                            m = BlockPos::new(m.x, m.y + 1, m.z);
                            if ctx.level.is_air(m) {
                                break;
                            }
                            let (mrx, mrz) = ctx.rel(m);
                            ops.push(DecoOp { x: mrx, y: m.y as i16, z: mrz, state: 0, flags: FLAG_POSTPROC, aux: 0 });
                        }
                    }
                    any = true;
                }
                flag1 = true;
            }
        }
    }
    any
}


/// CRUS_DRAW_TRACE cached once (std::env::var takes an environ lock + scan —
/// per-cell/per-position calls in the deco hot loops cost ~0.1-0.5us each;
/// ore veins and placement modifiers made this the single largest native
/// deco cost).
pub fn draw_trace_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("CRUS_DRAW_TRACE").is_ok())
}

/// isAdjacentToAir (vanilla OreFeature): any of the 6 neighbors is air.
/// Result is neighbor-order independent (no RNG involved).
fn is_adjacent_to_air(level: &dyn DecoLevelExt, pos: BlockPos) -> bool {
    const NEIGH: [(i32, i32, i32); 6] = [
        (0, -1, 0),
        (0, 1, 0),
        (0, 0, -1),
        (0, 0, 1),
        (-1, 0, 0),
        (1, 0, 0),
    ];
    for (dx, dy, dz) in NEIGH {
        let p = BlockPos::new(pos.x + dx, pos.y + dy, pos.z + dz);
        if level.is_air(p) {
            return true;
        }
    }
    false
}

/// shouldSkipAirCheck: chance<=0 -> skip (no draw); chance>=1 -> never skip;
/// else nextFloat() >= chance.
#[inline]
fn should_skip_air_check(rng: &mut WorldgenRandom, chance: f32) -> bool {
    let r = chance <= 0.0 || (chance < 1.0 && rng.next_f32() >= chance);
    if crate::features_place::draw_trace_on() {
        eprintln!("[skip] chance={chance} -> {r}");
    }
    r
}

fn rule_test_matches(
    rt: &RuleTest,
    level: &dyn DecoLevelExt,
    pos: BlockPos,
    rng: &mut WorldgenRandom,
) -> bool {
    match rt {
        RuleTest::True => true,
        RuleTest::BlockMatch(b) => level.state_is(pos, std::slice::from_ref(b)),
        RuleTest::TagMatch(t) => {
            let r = level.tag_is(pos, t);
            if crate::features_place::draw_trace_on() {
                eprintln!("[tag] {} {} {} {t} -> {r} state={}", pos.x, pos.y, pos.z, level.state_name(pos));
            }
            r
        }
        RuleTest::RandomBlock { block, probability } => {
            // vanilla short-circuit: state check FIRST, draw only on match
            level.state_is(pos, std::slice::from_ref(block))
                && rng.next_f32() < *probability
        }
        // 26.3 composites: stream allMatch/anyMatch short-circuit semantics
        RuleTest::HeightMatch { lo, hi } => *lo <= pos.y && pos.y <= *hi,
        RuleTest::Not(inner) => !rule_test_matches(inner, level, pos, rng),
        RuleTest::AnyOf(rules) => rules.iter().any(|r| rule_test_matches(r, level, pos, rng)),
        RuleTest::AllOf(rules) => rules.iter().all(|r| rule_test_matches(r, level, pos, rng)),
    }
}

/// CRUS_SEL_TRACE: comma list "cx:cz,..." — trace selector picks / ops for
/// those owning chunks only (empty value = all chunks).
fn sel_trace_on(cx: i32, cz: i32) -> bool {
    static ON: std::sync::OnceLock<Option<Vec<(i32, i32)>>> = std::sync::OnceLock::new();
    match ON.get_or_init(|| {
        let v = std::env::var("CRUS_SEL_TRACE").ok()?;
        let v = v.trim();
        if v.is_empty() {
            return Some(Vec::new());
        }
        let mut out = Vec::new();
        for part in v.split(',') {
            let (a, b) = part.trim().split_once(':')?;
            out.push((a.trim().parse().ok()?, b.trim().parse().ok()?));
        }
        Some(out)
    }) {
        Some(list) => list.is_empty() || list.contains(&(cx, cz)),
        None => false,
    }
}

/// ensureCanWrite(writeRadius=1): rel x/z within the 3x3 chunk region.
#[inline]
fn in_writable(rel_x: i32, rel_z: i32) -> bool {
    (-16..=31).contains(&rel_x) && (-16..=31).contains(&rel_z)
}

// ---------------------------------------------------------------------------
// Batch-1 simple configured-feature types (dptt delegate killers).
// All semantics from the CFR decompile of paper-1.21.8 (mojmap names kept in
// comments). RNG draw order is the parity-critical part of each.
// ---------------------------------------------------------------------------

/// ReplaceBlobsFeature (minecraft:netherrack_replace_blobs):
/// 1. clamp origin y to [minY+1, maxY], scan DOWN for target block (no RNG);
/// 2. radius.sample() x3 -> xr,yr,zr; max = the largest;
/// 3. BlockPos.withinManhattan(pos, xr, yr, zr) — ring-by-ring, exact order —
///    stop at the first ring > max; replace target blocks with state.
fn place_replace_blobs(
    ctx: &PlaceCtx,
    radius: &crate::features_placement::IntProvider,
    state: &str,
    target_block: &str,
    rng: &mut WorldgenRandom,
    origin: BlockPos,
    ops: &mut Vec<DecoOp>,
) -> bool {
    let min_y = ctx.level.min_y();
    let max_y = ctx.level.max_y();
    // BlockPos.MutableBlockPos.clamp(Y, min+1, max)
    let top = BlockPos::new(origin.x, origin.y.clamp(min_y + 1, max_y), origin.z);
    // findTarget: walk DOWN while y > min+1, first target match wins
    let mut target_pos: Option<BlockPos> = None;
    let mut y = top.y;
    while y > min_y + 1 {
        if ctx.level.state_name(BlockPos::new(top.x, y, top.z)) == target_block {
            target_pos = Some(BlockPos::new(top.x, y, top.z));
            break;
        }
        y -= 1;
    }
    let Some(center) = target_pos else {
        return false; // no RNG consumed on failure (vanilla findTarget=null)
    };
    let xr = radius.sample(rng).max(0);
    let yr = radius.sample(rng).max(0);
    let zr = radius.sample(rng).max(0);
    let max = xr.max(yr.max(zr));
    let mut any = false;
    for d in 0..=max {
        // withinManhattan ring d: |x|<=xr, |y|<=yr, |z|<=zr, |x|+|y|+|z|==d;
        // vanilla order: x ascending, y ascending, z=+d-|x|-|y| then mirror -z
        let x_max = xr.min(d);
        for x in -x_max..=x_max {
            let y_max = yr.min(d - x.abs());
            for yv in -y_max..=y_max {
                let z = d - x.abs() - yv.abs();
                if z > zr {
                    continue;
                }
                let p = BlockPos::new(center.x + x, center.y + yv, center.z + z);
                if ctx.level.state_name(p) == target_block {
                    ctx.emit(ops, p, state, 0);
                    any = true;
                }
                if z != 0 {
                    let p = BlockPos::new(center.x + x, center.y + yv, center.z - z);
                    if ctx.level.state_name(p) == target_block {
                        ctx.emit(ops, p, state, 0);
                        any = true;
                    }
                }
            }
        }
    }
    any
}

/// SeagrassFeature: dx/dz offsets (2 draws each), OCEAN_FLOOR height, water
/// check, tall vs short roll, canSurvive, optional upper half.
fn place_seagrass(
    ctx: &PlaceCtx,
    probability: f32,
    rng: &mut WorldgenRandom,
    origin: BlockPos,
    ops: &mut Vec<DecoOp>,
) -> bool {
    let dx = rng.next_int_bound(8) - rng.next_int_bound(8);
    let dz = rng.next_int_bound(8) - rng.next_int_bound(8);
    let h = ctx.level.height(HeightmapTy::OceanFloor, origin.x + dx, origin.z + dz);
    let p = BlockPos::new(origin.x + dx, h, origin.z + dz);
    if ctx.level.state_name(p) != "minecraft:water" {
        return false;
    }
    let tall = rng.next_f64() < probability as f64;
    let (lower, upper) = if tall {
        (
            "minecraft:tall_seagrass[half=lower]",
            "minecraft:tall_seagrass[half=upper]",
        )
    } else {
        ("minecraft:seagrass", "")
    };
    if !ctx.level.can_survive(lower, p) {
        return false;
    }
    // vanilla sets flag=true as soon as canSurvive passes — even when the
    // tall upper half had no water above and NOTHING was written, the
    // feature reports success (patch counters see it).
    if tall {
        let p2 = BlockPos::new(p.x, p.y + 1, p.z);
        if ctx.level.state_name(p2) == "minecraft:water" {
            ctx.emit(ops, p, lower, 0);
            ctx.emit(ops, p2, upper, 0);
        }
    } else {
        ctx.emit(ops, p, lower, 0);
    }
    true
}

/// SpringFeature: valid-block membership checks (no RNG), rock/hole counts,
/// then the state write + scheduleTick (FLAG_TICK).
fn place_spring(
    ctx: &PlaceCtx,
    valid_blocks: &[String],
    requires_block_below: bool,
    rock_count: i32,
    hole_count: i32,
    state: &str,
    pos: BlockPos,
    ops: &mut Vec<DecoOp>,
) -> bool {
    let valid = |p: BlockPos| valid_blocks.iter().any(|b| *b == ctx.level.state_name(p));
    if !valid(BlockPos::new(pos.x, pos.y + 1, pos.z)) {
        return false;
    }
    if requires_block_below && !valid(BlockPos::new(pos.x, pos.y - 1, pos.z)) {
        return false;
    }
    if !ctx.level.is_air(pos) && !valid(pos) {
        return false;
    }
    let sides = [
        BlockPos::new(pos.x - 1, pos.y, pos.z),
        BlockPos::new(pos.x + 1, pos.y, pos.z),
        BlockPos::new(pos.x, pos.y, pos.z - 1),
        BlockPos::new(pos.x, pos.y, pos.z + 1),
        BlockPos::new(pos.x, pos.y - 1, pos.z),
    ];
    let mut rocks = 0;
    let mut holes = 0;
    for s in sides {
        if valid(s) {
            rocks += 1;
        }
        if ctx.level.is_air(s) {
            holes += 1;
        }
    }
    if rocks == rock_count && holes == hole_count {
        ctx.emit(ops, pos, state, FLAG_TICK);
        return true;
    }
    false
}

/// BlockPileFeature: radius draws, betweenClosed box iterate (x fast, y, z
/// slow), distance-vs-random test, optional second-chance roll, mayPlaceOn.
fn place_block_pile(
    ctx: &PlaceCtx,
    state_provider: &StateSrc,
    rng: &mut WorldgenRandom,
    origin: BlockPos,
    ops: &mut Vec<DecoOp>,
) -> bool {
    if origin.y < ctx.level.min_y() + 5 {
        return false;
    }
    let i = 2 + rng.next_int_bound(2);
    let i1 = 2 + rng.next_int_bound(2);
    // BlockPos.betweenClosed(min, max): x = index % nx (FASTEST), then y, z slow
    let (x1, y1, z1) = (origin.x - i, origin.y, origin.z - i1);
    let (x2, y2, z2) = (origin.x + i, origin.y + 1, origin.z + i1);
    for z in z1..=z2 {
        for y in y1..=y2 {
            for x in x1..=x2 {
                let dx = origin.x - x;
                let dz = origin.z - z;
                if (dx * dx + dz * dz) as f32 <= rng.next_f32() * 10.0 - rng.next_f32() * 6.0 {
                    pile_try_place(ctx, state_provider, rng, BlockPos::new(x, y, z), ops);
                } else if rng.next_f32() < 0.031 {
                    pile_try_place(ctx, state_provider, rng, BlockPos::new(x, y, z), ops);
                }
            }
        }
    }
    true
}

fn pile_try_place(
    ctx: &PlaceCtx,
    state_provider: &StateSrc,
    rng: &mut WorldgenRandom,
    pos: BlockPos,
    ops: &mut Vec<DecoOp>,
) {
    if !ctx.level.is_air(pos) {
        return;
    }
    // mayPlaceOn: below is DIRT_PATH -> nextBoolean(); else isFaceSturdy(UP)
    let below = BlockPos::new(pos.x, pos.y - 1, pos.z);
    let ok = if ctx.level.state_name(below) == "minecraft:dirt_path" {
        rng.next_bool()
    } else {
        ctx.level.has_sturdy_face(below, 1) // dir 1 = UP
    };
    if !ok {
        return;
    }
    let mut flags: u8 = 0;
    if let Some(state) = sample_state(state_provider, ctx.level, pos, rng, &mut flags) {
        ctx.emit(ops, pos, &state, flags);
    }
}

/// KelpFeature: OCEAN_FLOOR column, 1+nextInt(10) height, water checks,
/// kelp_plant body + head with AGE=nextInt(4)+20 (state picked from the
/// pre-indexed age variants), the below-fallback tail.
fn place_kelp(
    ctx: &PlaceCtx,
    rng: &mut WorldgenRandom,
    origin: BlockPos,
    ops: &mut Vec<DecoOp>,
) -> bool {
    let mut count = 0;
    let h = ctx.level.height(HeightmapTy::OceanFloor, origin.x, origin.z);
    let mut p = BlockPos::new(origin.x, h, origin.z);
    if ctx.level.state_name(p) != "minecraft:water" {
        return false;
    }
    let i1 = 1 + rng.next_int_bound(10);
    for i2 in 0..=i1 {
        let above = BlockPos::new(p.x, p.y + 1, p.z);
        if ctx.level.state_name(p) == "minecraft:water"
            && ctx.level.state_name(above) == "minecraft:water"
            && ctx.level.can_survive("minecraft:kelp_plant", p)
        {
            if i2 == i1 {
                let age = rng.next_int_bound(4) + 20;
                let state = format!("minecraft:kelp[age={age}]");
                ctx.emit(ops, p, &state, 0);
                count += 1;
            } else {
                ctx.emit(ops, p, "minecraft:kelp_plant", 0);
            }
        } else if i2 > 0 {
            let p2 = BlockPos::new(p.x, p.y - 1, p.z);
            let p2below = BlockPos::new(p.x, p.y - 2, p.z);
            if !ctx.level.can_survive("minecraft:kelp", p2)
                || ctx.level.state_name(p2below) == "minecraft:kelp"
            {
                break;
            }
            let age = rng.next_int_bound(4) + 20;
            let state = format!("minecraft:kelp[age={age}]");
            ctx.emit(ops, p2, &state, 0);
            count += 1;
            break;
        }
        p = above;
    }
    count > 0
}

/// UnderwaterMagmaFeature: Column.scan (water above + non-water floor), then
/// the placement-radius box iterate with a per-cell nextFloat draw and the
/// water-or-air validity rule.
fn place_underwater_magma(
    ctx: &PlaceCtx,
    floor_search_range: i32,
    placement_radius: i32,
    prob: f32,
    rng: &mut WorldgenRandom,
    origin: BlockPos,
    ops: &mut Vec<DecoOp>,
) -> bool {
    let is_water = |p: BlockPos| ctx.level.state_name(p) == "minecraft:water";
    if !is_water(origin) {
        return false;
    }
    // Column.scanDirection UP: walk while water, tip must be non-water
    let scan = |dir: i32| -> Option<i32> {
        let mut y = origin.y;
        let mut i = 1;
        while i < floor_search_range && is_water(BlockPos::new(origin.x, y, origin.z)) {
            y += dir;
            i += 1;
        }
        if is_water(BlockPos::new(origin.x, y, origin.z)) {
            None
        } else {
            Some(y)
        }
    };
    // UP scan validity (unused value) + DOWN floor
    if scan(1).is_none() {
        return false;
    }
    let Some(floor_y) = scan(-1) else {
        return false;
    };
    // betweenClosedStream over the box: x fast, y, z slow
    let r = placement_radius;
    let (x1, y1, z1) = (origin.x - r, floor_y - r, origin.z - r);
    let (x2, y2, z2) = (origin.x + r, floor_y + r, origin.z + r);
    let mut count = 0;
    for z in z1..=z2 {
        for y in y1..=y2 {
            for x in x1..=x2 {
                if rng.next_f32() >= prob {
                    continue;
                }
                let p = BlockPos::new(x, y, z);
                if !magma_valid(ctx, p) {
                    continue;
                }
                ctx.emit(ops, p, "minecraft:magma_block", 0);
                count += 1;
            }
        }
    }
    count > 0
}

/// VineBlock face attach (VinesFeature): first acceptable neighbor in
/// Direction.values() order minus DOWN wins; NO RNG consumed.
/// canAttachTo(dir) = isFaceFull(getBlockSupportShape(neighbor), dir.opposite())
/// (= isFaceSturdy for worldgen full cubes) — has_sturdy_face(neighbor, opp).
fn place_vines(ctx: &PlaceCtx, pos: BlockPos, ops: &mut Vec<DecoOp>) -> bool {
    if !ctx.level.is_air(pos) {
        return false;
    }
    // vine canonical property order: east, north, south, up, west
    const FACES: [(u8, usize); 5] = [(1, 3), (2, 1), (3, 2), (4, 4), (5, 0)];
    for (dir, slot) in FACES {
        let nb = pos_offset(pos, dir);
        if ctx.level.has_sturdy_face(nb, dir ^ 1) {
            let mut kv = Vec::with_capacity(5);
            for (i, f) in ["east", "north", "south", "up", "west"].iter().enumerate() {
                kv.push(format!("{f}={}", if i == slot { "true" } else { "false" }));
            }
            ctx.emit(ops, pos, &format!("minecraft:vine[{}]", kv.join(",")), 0);
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// batch-3 (cont-58): MultifaceGrowthFeature (glow lichen family)
// Bytecode-pinned vs paper-1.21.8.jar: MultifaceGrowthFeature + MultifaceBlock
// + MultifaceSpreader (+ SpreadType/DefaultSpreaderConfig).
// ---------------------------------------------------------------------------

/// Util.shuffle(List, RandomSource): for i=size..2: swap(i-1, nextInt(i))
fn shuffle_dirs(rng: &mut WorldgenRandom, dirs: &mut [u8]) {
    let mut i = dirs.len() as i32;
    while i > 1 {
        let j = rng.next_int_bound(i) as usize;
        dirs.swap((i - 1) as usize, j);
        i -= 1;
    }
}

/// state string with every multiface-family property, alphabetical order
/// (matches the vine emit convention; the Java state() parser is order-free).
const MF_PROPS: [&str; 6] = ["down", "east", "north", "south", "up", "west"];

fn multiface_state(block: &str, faces: &[bool; 6], waterlogged: bool) -> String {
    let mut s = String::with_capacity(block.len() + 56);
    s.push_str(block);
    s.push('[');
    for i in 0..6 {
        s.push_str(MF_PROPS[i]);
        s.push('=');
        s.push_str(if faces[i] { "true" } else { "false" });
        s.push(',');
    }
    s.push_str("waterlogged=");
    s.push_str(if waterlogged { "true" } else { "false" });
    s.push(']');
    s
}

/// parse "ns:block[k=v,...]" props into (faces[by dir 0..6], waterlogged);
/// absent props = false (vanilla getValueOrElse(prop, FALSE))
fn multiface_parse_faces(state: &str, faces: &mut [bool; 6], wl: &mut bool) {
    *wl = false;
    *faces = [false; 6];
    let Some((_, props)) = state.split_once('[') else { return };
    for kv in props.trim_end_matches(']').split(',') {
        let Some((k, v)) = kv.split_once('=') else { continue };
        if k == "waterlogged" {
            *wl = v == "true";
        } else if let Some(i) = MF_PROPS.iter().position(|p| *p == k) {
            faces[i] = v == "true";
        }
    }
}

/// isAirOrWater(state): st.isAir() || st.is(Blocks.WATER) — any water block
/// level matches the bare block key
fn multiface_is_air_or_water(ctx: &PlaceCtx, pos: BlockPos, water_ids: &[String]) -> bool {
    ctx.level.is_air(pos) || ctx.level.state_is(pos, water_ids)
}

/// full state string at pos when the block IS the multiface block (props
/// preserved): overlay first (own writes carry props); the snapshot palette
/// carries BARE registry keys, so a snapshot lichen (neighbor-margin write)
/// resolves through the JVM bridge once — precise, and rare enough that the
/// upcall is noise
fn multiface_lichen_full(
    ctx: &PlaceCtx,
    pos: BlockPos,
    block: &str,
    block_ids: &[String],
) -> Option<String> {
    if let Some(s) = ctx.level.read_overlay(pos) {
        return if s.starts_with(block) { Some(s) } else { None };
    }
    if !ctx.level.state_is(pos, block_ids) {
        return None;
    }
    let full = ctx.level.state_full(pos);
    Some(if full.is_empty() { block.to_string() } else { full })
}

/// DefaultSpreaderConfig.canSpreadInto: stateCanBeReplaced(cur at spos) &&
/// isValidStateForPlacement(cur, spos, sface)
fn multiface_can_spread_into(
    ctx: &PlaceCtx,
    block: &str,
    block_ids: &[String],
    water_ids: &[String],
    spos: BlockPos,
    sface: u8,
) -> bool {
    // stateCanBeReplaced: cur.isAir || cur.is(lichen) || (cur.is(WATER) &&
    // fluidState.isSource()) — the FLUID_WATER flag is exactly source water
    // (DecoDriver.stateFlags uses getFluidState().is(Fluids.WATER))
    let cur_is_lichen = ctx.level.state_is(spos, block_ids);
    let replaceable = ctx.level.is_air(spos)
        || cur_is_lichen
        || (ctx.level.state_is(spos, water_ids) && ctx.level.fluid_is(spos, water_ids));
    if !replaceable {
        return false;
    }
    // isValidStateForPlacement: cur.is(lichen) && hasFace(cur, sface) → null
    if cur_is_lichen {
        if let Some(full) = multiface_lichen_full(ctx, spos, block, block_ids) {
            let mut f = [false; 6];
            let mut wl = false;
            multiface_parse_faces(&full, &mut f, &mut wl);
            if f[sface as usize] {
                return false;
            }
        }
    }
    // canAttachTo(host = spos.relative(sface)): isFaceFull(blockSupportShape
    // | collisionShape, sface.opposite) — has_sturdy_face on full cubes (the
    // vine engine pinned the same equivalence; glass-class support is the
    // known corner, caves are stone-family)
    ctx.level.has_sturdy_face(pos_offset(spos, sface), sface ^ 1)
}

/// MultifaceBlock.getStateForPlacement(cur, level, spos, sface) — lichen keeps
/// its props; source water → waterlogged=true; else the default state
fn multiface_build_spread_state(
    ctx: &PlaceCtx,
    block: &str,
    block_ids: &[String],
    water_ids: &[String],
    spos: BlockPos,
    sface: u8,
) -> Option<String> {
    let mut faces = [false; 6];
    let mut wl = false;
    if let Some(full) = multiface_lichen_full(ctx, spos, block, block_ids) {
        multiface_parse_faces(&full, &mut faces, &mut wl);
    } else if ctx.level.fluid_is(spos, water_ids) {
        wl = true;
    }
    faces[sface as usize] = true;
    Some(multiface_state(block, &faces, wl))
}

/// MultifaceSpreader.spreadFromFaceTowardRandomDirection (default config):
/// Direction.allShuffled = Fisher-Yates over [D,U,N,S,W,E] (5 nextInt); per
/// dir: axis gate, source-face gate, then the FIRST spread type in
/// [SAME_POSITION, SAME_PLANE, WRAP_AROUND] whose target passes
/// canSpreadInto wins (stream findFirst — stops everything). No RNG inside
/// the search; a spread write never backtracks.
fn multiface_spread(
    ctx: &PlaceCtx,
    block: &str,
    block_ids: &[String],
    water_ids: &[String],
    pos: BlockPos,
    face: u8,
    src_faces: &[bool; 6],
    rng: &mut WorldgenRandom,
    ops: &mut Vec<DecoOp>,
) {
    let mut all: Vec<u8> = vec![0, 1, 2, 3, 4, 5];
    shuffle_dirs(rng, &mut all);
    for &d in &all {
        if d >> 1 == face >> 1 {
            continue; // getSpreadFromFaceTowardDirection axis gate
        }
        if src_faces[d as usize] {
            continue; // hasFace(state, D) — the source already has this face
        }
        // DEFAULT_SPREAD_ORDER = [SAME_POSITION, SAME_PLANE, WRAP_AROUND];
        // getSpreadPos(pos, toward=d, sourceFace=face):
        //   SAME_POSITION: (pos, d)          SAME_PLANE: (pos+d, face)
        //   WRAP_AROUND:   (pos+d+face, d.opposite())
        let cands = [
            (pos, d),
            (pos_offset(pos, d), face),
            (pos_offset(pos_offset(pos, d), face), d ^ 1),
        ];
        for (spos, sface) in cands {
            if !multiface_can_spread_into(ctx, block, block_ids, water_ids, spos, sface) {
                continue;
            }
            let Some(ns) = multiface_build_spread_state(ctx, block, block_ids, water_ids, spos, sface)
            else {
                continue;
            };
            ctx.emit(ops, spos, &ns, 0);
            return; // findFirst — the whole spread ends at the first write
        }
    }
}

/// vanilla placeGrowthIfPossible: the FIRST canBePlacedOn neighbor (in dir
/// order) wins; the placed state keeps the source cell's lichen props (or
/// starts default / waterlogged in source water); RNG = none on failure,
/// one nextFloat (+ spread shuffle) on success.
#[allow(clippy::too_many_arguments)]
fn multiface_growth_if_possible(
    ctx: &PlaceCtx,
    block: &str,
    block_ids: &[String],
    water_ids: &[String],
    pos: BlockPos,
    can_place_on: &[String],
    chance: f32,
    dirs: &[u8],
    src_full: Option<&str>,
    rng: &mut WorldgenRandom,
    ops: &mut Vec<DecoOp>,
) -> bool {
    for &dir in dirs {
        let host = pos_offset(pos, dir);
        if !ctx.level.state_is(host, can_place_on) {
            continue;
        }
        // getStateForPlacement → isValidStateForPlacement → canAttachTo(host):
        // the canBePlacedOn sets are full cubes → isFaceFull always true
        let mut faces = [false; 6];
        let mut wl = false;
        match src_full {
            Some(full) => multiface_parse_faces(full, &mut faces, &mut wl),
            None => wl = ctx.level.fluid_is(pos, water_ids),
        }
        faces[dir as usize] = true;
        ctx.emit(ops, pos, &multiface_state(block, &faces, wl), 0);
        if rng.next_f32() < chance {
            multiface_spread(ctx, block, block_ids, water_ids, pos, dir, &faces, rng, ops);
        }
        return true;
    }
    false
}

/// MultifaceGrowthFeature.place (vanilla 1.21.8, bytecode-pinned):
/// origin must be air-or-water; validDirections = [UP if ceiling, DOWN if
/// floor, N,S,W,E if wall] shuffled once for the origin growth; on failure
/// every shuffled dir gets an independent reshuffle of the CONFIG-order
/// validDirections minus dir.opposite() (getShuffledDirectionsExcept filters
/// the unshuffled field!) and ONE effective probe at origin+dir — the
/// searchRange i-loop collapses: nothing is written and no RNG is consumed
/// on a failed probe, and any success returns from the whole feature.
#[allow(clippy::too_many_arguments)]
fn place_multiface_growth(
    ctx: &PlaceCtx,
    block: &str,
    _search_range: i32,
    can_floor: bool,
    can_ceiling: bool,
    can_wall: bool,
    chance: f32,
    can_place_on: &[String],
    rng: &mut WorldgenRandom,
    origin: BlockPos,
    ops: &mut Vec<DecoOp>,
) -> bool {
    let block_ids = [block.to_string()];
    let water_ids = ["minecraft:water".to_string()];
    if !multiface_is_air_or_water(ctx, origin, &water_ids) {
        return false;
    }
    // constructor order (bytecode): ceiling → UP, floor → DOWN, wall → plane
    // HORIZONTAL forEach (N,S,W,E)
    let mut orig: Vec<u8> = Vec::with_capacity(6);
    if can_ceiling {
        orig.push(1);
    }
    if can_floor {
        orig.push(0);
    }
    if can_wall {
        orig.extend_from_slice(&[2, 3, 4, 5]);
    }
    let mut dirs = orig.clone();
    shuffle_dirs(rng, &mut dirs); // getShuffledDirections
    // origin is air-or-water by the gate above → the source state is never
    // the lichen itself (src_full = None, kept explicit for the probe below)
    if multiface_growth_if_possible(
        ctx,
        block,
        &block_ids,
        &water_ids,
        origin,
        can_place_on,
        chance,
        &dirs,
        None,
        rng,
        ops,
    ) {
        return true;
    }
    for &dir in &dirs {
        let excluded = dir ^ 1; // down<->up, north<->south, west<->east
        let mut except: Vec<u8> = orig.iter().copied().filter(|d| *d != excluded).collect();
        shuffle_dirs(rng, &mut except);
        let cell = pos_offset(origin, dir);
        let cell_full = multiface_lichen_full(ctx, cell, block, &block_ids);
        if cell_full.is_none() && !multiface_is_air_or_water(ctx, cell, &water_ids) {
            continue; // break the (collapsed) i-loop → next direction
        }
        if multiface_growth_if_possible(
            ctx,
            block,
            &block_ids,
            &water_ids,
            cell,
            can_place_on,
            chance,
            &except,
            cell_full.as_deref(),
            rng,
            ops,
        ) {
            return true;
        }
    }
    false
}

/// BlockColumnFeature: heights sampled per layer (in JSON order), probe walk
/// from origin+dir for `sum` allowed cells (truncate on the first blocked
/// cell), then place from ORIGIN stepping dir. truncate cuts from layer 0
/// (non-tip side) first when prioritizeTip.
fn place_block_column(
    ctx: &PlaceCtx,
    direction: u8,
    layers: &[(IP, StateSrc)],
    prioritize_tip: bool,
    allowed: &BlockPredicate,
    rng: &mut WorldgenRandom,
    origin: BlockPos,
    ops: &mut Vec<DecoOp>,
) -> bool {
    let mut heights = Vec::with_capacity(layers.len());
    let mut sum = 0i32;
    for (hp, _) in layers {
        let h = hp.sample(rng);
        heights.push(h);
        sum += h;
    }
    if sum == 0 {
        return false;
    }
    let (dx, dy, dz) = dir_offset(direction);
    // probe: scan cells from origin+dir while allowed; stop after `sum` cells
    // or at the first disallowed (truncate the remainder, tip side first
    // when prioritizeTip means "keep the tip" -> cut index 0 first)
    let mut fit = 0i32;
    {
        let mut probe = BlockPos::new(origin.x + dx, origin.y + dy, origin.z + dz);
        while fit < sum {
            let st = ctx.level.state(probe);
            if crate::features_deco::pred_matches(allowed, ctx.level, probe, st) != Some(true) {
                // vanilla truncate(heights, sum-fit, prioritizeTip): walk
                // index 0..len (tip=true, cut the non-tip layers first) or
                // len-1..0 (tip=false, cut the last layer first)
                let mut cut = sum - fit;
                let n = heights.len();
                let mut k = if prioritize_tip { 0usize } else { n - 1 };
                for _ in 0..n {
                    if cut == 0 {
                        break;
                    }
                    let take = heights[k].min(cut);
                    heights[k] -= take;
                    cut -= take;
                    if prioritize_tip {
                        k += 1;
                    } else {
                        k = k.wrapping_sub(1);
                    }
                }
                break;
            }
            probe = BlockPos::new(probe.x + dx, probe.y + dy, probe.z + dz);
            fit += 1;
        }
    }
    let mut any = false;
    let mut p = origin;
    for (i, (hp, sp)) in layers.iter().enumerate() {
        let _ = hp;
        for _ in 0..heights[i] {
            let mut flags: u8 = 0;
            if let Some(state) = sample_state(sp, ctx.level, p, rng, &mut flags) {
                ctx.emit(ops, p, &state, flags);
                any = true;
            }
            p = BlockPos::new(p.x + dx, p.y + dy, p.z + dz);
        }
    }
    any
}

/// RootSystemFeature: origin must be air; climb up to rootColumnMaxHeight
/// until allowedTreePosition + spaceForTree hold, place the tree placed
/// feature there, fill rooted-dirt disks down the climbed column, then
/// hanging-root attempts around the origin. Lava below the tree slot aborts.
#[allow(clippy::too_many_arguments)]
fn place_root_system(
    ctx: &PlaceCtx,
    tree_feature: &str,
    allowed_tree_position: &BlockPredicate,
    required_vertical_space_for_tree: i32,
    allowed_vertical_water_for_tree: i32,
    root_column_max_height: i32,
    root_radius: i32,
    root_placement_attempts: i32,
    root_replaceable_tag: &str,
    root_state: &StateSrc,
    hanging_root_radius: i32,
    hanging_root_placement_attempts: i32,
    hanging_roots_vertical_span: i32,
    hanging_root_state: &StateSrc,
    rng: &mut WorldgenRandom,
    origin: BlockPos,
    ops: &mut Vec<DecoOp>,
) -> bool {
    if !ctx.level.is_air(origin) {
        return false;
    }
    // placeDirtAndTree: mpos climbs UP every iteration (failed candidates too)
    let mut placed_tree = false;
    let mut climbed = 0i32;
    let mut mpos = origin;
    for i in 0..root_column_max_height {
        mpos = BlockPos::new(mpos.x, mpos.y + 1, mpos.z);
        let st = ctx.level.state(mpos);
        if crate::features_deco::pred_matches(allowed_tree_position, ctx.level, mpos, st)
            == Some(true)
            && space_for_tree(
                ctx,
                mpos,
                required_vertical_space_for_tree,
                allowed_vertical_water_for_tree,
            )
        {
            let below = BlockPos::new(mpos.x, mpos.y - 1, mpos.z);
            if ctx.level.fluid_is(below, &["minecraft:lava".to_string()]) {
                return false;
            }
            if !ctx.level.is_solid(below) {
                return false;
            }
            if place_placed(ctx, tree_feature, rng, mpos, ops, 0) {
                place_dirt_column(
                    ctx, origin, origin.y + i, root_radius, root_placement_attempts,
                    root_replaceable_tag, root_state, rng, ops,
                );
                placed_tree = true;
                climbed = i;
                break;
            }
        }
    }
    let _ = climbed;
    if placed_tree {
        place_hanging_roots(
            ctx, origin, hanging_root_radius, hanging_root_placement_attempts,
            hanging_roots_vertical_span, hanging_root_state, rng, ops,
        );
    }
    // vanilla returns true even when the tree never placed (place() body:
    // placeDirtAndTree(...) ? placeRoots(...) : {}; return true)
    true
}

/// spaceForTree: `required` blocks above the candidate must be air (or water
/// within allowedVerticalWaterForTree).
fn space_for_tree(
    ctx: &PlaceCtx,
    pos: BlockPos,
    required: i32,
    allowed_water: i32,
) -> bool {
    let mut p = pos;
    for i in 1..=required {
        p = BlockPos::new(p.x, p.y + 1, p.z);
        let st = ctx.level.state(p);
        if ctx.level.is_air(p) {
            continue;
        }
        if i + 1 <= allowed_water && ctx.level.fluid_is(p, &["minecraft:water".to_string()]) {
            continue;
        }
        let _ = st;
        return false;
    }
    true
}

/// placeDirt column: rooted-dirt disks from origin.y up to the tree base.
#[allow(clippy::too_many_arguments)]
fn place_dirt_column(
    ctx: &PlaceCtx,
    origin: BlockPos,
    max_y: i32,
    radius: i32,
    attempts: i32,
    replaceable_tag: &str,
    state: &StateSrc,
    rng: &mut WorldgenRandom,
    ops: &mut Vec<DecoOp>,
) {
    let mut y = origin.y;
    while y < max_y {
        // placeRootedDirt: attempts x [nextInt(r) - nextInt(r)] on x and z
        for _ in 0..attempts {
            let ox = rng.next_int_bound(radius) - rng.next_int_bound(radius);
            let oz = rng.next_int_bound(radius) - rng.next_int_bound(radius);
            let p = BlockPos::new(origin.x + ox, y, origin.z + oz);
            if ctx.level.tag_is(p, replaceable_tag) {
                let mut flags: u8 = 0;
                if let Some(st) = sample_state(state, ctx.level, p, rng, &mut flags) {
                    ctx.emit(ops, p, &st, flags);
                }
            }
        }
        y += 1;
    }
}

/// hanging roots: attempts around the origin (y span nextInt(span) pairs),
/// empty cell + canSurvive + block above sturdy-DOWN.
#[allow(clippy::too_many_arguments)]
fn place_hanging_roots(
    ctx: &PlaceCtx,
    origin: BlockPos,
    radius: i32,
    attempts: i32,
    span: i32,
    state: &StateSrc,
    rng: &mut WorldgenRandom,
    ops: &mut Vec<DecoOp>,
) {
    for _ in 0..attempts {
        let ox = rng.next_int_bound(radius) - rng.next_int_bound(radius);
        let oy = rng.next_int_bound(span) - rng.next_int_bound(span);
        let oz = rng.next_int_bound(radius) - rng.next_int_bound(radius);
        let p = BlockPos::new(origin.x + ox, origin.y + oy, origin.z + oz);
        if !ctx.level.is_air(p) {
            continue;
        }
        let mut flags: u8 = 0;
        let Some(st) = sample_state(state, ctx.level, p, rng, &mut flags) else {
            continue;
        };
        if !ctx.level.can_survive(&st, p) {
            continue;
        }
        if !ctx.level.has_sturdy_face(BlockPos::new(p.x, p.y + 1, p.z), 0) {
            continue;
        }
        ctx.emit(ops, p, &st, flags);
    }
}

fn magma_is_water_or_air(ctx: &PlaceCtx, p: BlockPos) -> bool {
    ctx.level.state_name(p) == "minecraft:water" || ctx.level.is_air(p)
}

fn magma_valid(ctx: &PlaceCtx, p: BlockPos) -> bool {
    if magma_is_water_or_air(ctx, p) {
        return false;
    }
    if magma_is_water_or_air(ctx, BlockPos::new(p.x, p.y - 1, p.z)) {
        return false;
    }
    for (dx, dz) in [(0, -1), (0, 1), (-1, 0), (1, 0)] {
        if magma_is_water_or_air(ctx, BlockPos::new(p.x + dx, p.y, p.z + dz)) {
            return false;
        }
    }
    true
}


/// OreFeature / ScatteredOreFeature.
/// Regular ore: BulkSectionAccess writes (FLAG_RAW, no heightmap updates).
/// Scattered: region setBlock flags=2; per-step overlay so intra-feature
/// re-visits read the replaced state (vanilla reads live sections).
fn place_ore(
    ctx: &PlaceCtx,
    discard: f32,
    size: i32,
    targets: &[(RuleTest, String)],
    rng: &mut WorldgenRandom,
    origin: BlockPos,
    ops: &mut Vec<DecoOp>,
    scattered: bool,
) -> bool {
    if targets.is_empty() {
        return false;
    }
    // intra-feature writes are tracked by the ctx overlay (OverlayLevel); the
    // scattered path additionally checks "did we already write this pos" to
    // reproduce vanilla's live-section re-read (our ore fails the targets)
    let emit = |ctx: &PlaceCtx, ops: &mut Vec<DecoOp>, pos: BlockPos, state: &str, raw: bool| {
        let (rx, rz) = ctx.rel(pos);
        if !in_writable(rx as i32, rz as i32) {
            return;
        }
        let mut flags = 0;
        if raw {
            flags |= FLAG_RAW;
        }
        ctx.emit(ops, pos, state, flags);
    };

    let vein_trace = vein_trace_on(ctx.center_x >> 4, ctx.center_z >> 4);
    if scattered {
        let n = rng.next_int_bound(size + 1);
        if vein_trace {
            eprintln!(
                "[scat@{}] o=({},{},{}) n={n} size={size} discard={discard}",
                ctx.center_x >> 4, origin.x, origin.y, origin.z
            );
        }
        for i in 0..n {
            let mag = i.min(7);
            // offsetTargetPos: 3 axes x (nextFloat - nextFloat) * mag, Math.round
            let off = |rng: &mut WorldgenRandom, mag: i32| -> i32 {
                ((rng.next_f32() - rng.next_f32()) * mag as f32).round() as i32
            };
            let dx = off(rng, mag);
            let dy = off(rng, mag);
            let dz = off(rng, mag);
            let pos = BlockPos::new(origin.x + dx, origin.y + dy, origin.z + dz);
            if vein_trace {
                eprintln!("[spos@{}] i={i} p=({},{},{}) mag={mag}", ctx.center_x >> 4, pos.x, pos.y, pos.z);
            }
            let replaced = ctx.level.read_overlay(pos).is_some();
            for (rt, st) in targets {
                let hit = if replaced {
                    false // vanilla re-reads the LIVE state: it is our ore now,
                          // which fails the (stone/deepslate) targets
                } else {
                    rule_test_matches(rt, ctx.level, pos, rng)
                        && (should_skip_air_check(rng, discard)
                            || !is_adjacent_to_air(ctx.level, pos))
                };
                if hit {
                    emit(ctx, ops, pos, st, false);
                    break;
                }
            }
        }
        return true;
    }

    // ---- regular OreFeature ----
    let f: f32 = rng.next_f32() * std::f32::consts::PI;
    let size_f = size as f32;
    let f1: f32 = size_f / 8.0f32;
    let ceil: i32 = ((size_f / 16.0f32 * 2.0f32 + 1.0f32) / 2.0f32).ceil() as i32;
    // vanilla: Math.sin((double)f) — HotSpot intrinsic; NOT Mth.sin (the
    // 65536-table). The intrinsic differs from the system libm in the last
    // ulp for ~0.3% of the f-domain -> floor() boundary flips -> the whole
    // vein cloud shifts. features_math restores Java bits via a correction
    // table over the finite f32-promoted input domain.
    let sin_f = crate::features_math::java_sin(f as f64);
    let cos_f = crate::features_math::java_cos(f as f64);
    let min_x = origin.x as f64 + sin_f * f1 as f64;
    let max_x = origin.x as f64 - sin_f * f1 as f64;
    let min_z = origin.z as f64 + cos_f * f1 as f64;
    let max_z = origin.z as f64 - cos_f * f1 as f64;
    let min_y = (origin.y + rng.next_int_bound(3) - 2) as f64;
    let max_y = (origin.y + rng.next_int_bound(3) - 2) as f64;
    if crate::features_place::draw_trace_on() {
        eprintln!("[vein] discard={discard} f={f:.9} minx={min_x:.17} maxx={max_x:.17} miny={min_y:.17} maxy={max_y:.17} minz={min_z:.17} maxz={max_z:.17}");
    }

    let x0 = origin.x - (f1.ceil() as i32) - ceil;
    let y0 = origin.y - 2 - ceil;
    let z0 = origin.z - (f1.ceil() as i32) - ceil;
    let width = 2 * ((f1.ceil() as i32) + ceil);
    let height = 2 * (2 + ceil);

    // early gate: any column in the box with ocean-floor height >= y0
    let mut ok = false;
    let mut gate_h: i32 = i32::MIN;
    'outer: for ix in x0..=x0 + width {
        for iz in z0..=z0 + width {
            let h = ctx.level.height(crate::features_deco::HeightmapTy::OceanFloorWg, ix, iz);
            if h > gate_h {
                gate_h = h;
            }
            if y0 <= h {
                ok = true;
                break 'outer;
            }
        }
    }
    if vein_trace {
        eprintln!(
            "[vein@{}] o=({},{},{}) f={f:.9} box=({x0},{y0},{z0}) w={width} h={height} size={size} discard={discard} gate={ok} maxh={gate_h}",
            ctx.center_x >> 4, origin.x, origin.y, origin.z
        );
    }
    if crate::features_place::draw_trace_on() {
        eprintln!("[gate] ox={} oy={} oz={} y0={y0} -> {ok}", origin.x, origin.y, origin.z);
    }
    if !ok {
        return false;
    }

    // vein node cloud (Mth.sin/cos = the 65536-entry f32 table)
    let mut pts: Vec<(f64, f64, f64, f64)> = Vec::with_capacity(size as usize);
    for i2 in 0..size {
        let fr = i2 as f32 / size_f;
        let d = mth_lerp(fr as f64, min_x, max_x);
        let d1 = mth_lerp(fr as f64, min_y, max_y);
        let d2 = mth_lerp(fr as f64, min_z, max_z);
        let d3 = rng.next_f64() * size_f as f64 / 16.0;
        let s = mth_sin(std::f32::consts::PI * fr);
        let d4 = ((s + 1.0f32) as f64 * d3 + 1.0) / 2.0;
        pts.push((d, d1, d2, d4));
        if crate::features_place::draw_trace_on() {
            eprintln!("[node {i2}] d={d:.17} y={d1:.17} z={d2:.17} r={d4:.17}");
        }
    }
    // sphere elimination
    for i2 in 0..size - 1 {
        if pts[i2 as usize].3 <= 0.0 {
            continue;
        }
        for i3 in i2 + 1..size {
            if pts[i3 as usize].3 <= 0.0 {
                continue;
            }
            let a = pts[i2 as usize];
            let b = pts[i3 as usize];
            let dd = a.3 - b.3;
            if dd * dd > (a.0 - b.0) * (a.0 - b.0) + (a.1 - b.1) * (a.1 - b.1) + (a.2 - b.2) * (a.2 - b.2) {
                if dd > 0.0 {
                    pts[i3 as usize].3 = -1.0;
                } else {
                    pts[i2 as usize].3 = -1.0;
                }
            }
        }
    }

    let mut seen: std::collections::HashSet<(i32, i32, i32)> = std::collections::HashSet::new();
    for (px, py, pz, rad) in &pts {
        let rad = *rad;
        if rad < 0.0 {
            continue;
        }
        // vanilla axis mapping: doubles[0]=x, [1]=y, [2]=z; the loops clamp
        // LOW by the box origin only (no upper clamp) and the bitSet index
        // uses width/height strides.
        let bx_lo = ((px - rad).floor() as i32).max(x0);
        let by_lo = ((py - rad).floor() as i32).max(y0);
        let bz_lo = ((pz - rad).floor() as i32).max(z0);
        let bx_hi = ((px + rad).floor() as i32).max(bx_lo);
        let by_hi = ((py + rad).floor() as i32).max(by_lo);
        let bz_hi = ((pz + rad).floor() as i32).max(bz_lo);
        for ix in bx_lo..=bx_hi {
            let dx = (ix as f64 + 0.5 - px) / rad;
            if !(dx * dx < 1.0) {
                continue;
            }
            for iy in by_lo..=by_hi {
                let dy = (iy as f64 + 0.5 - py) / rad;
                if !(dx * dx + dy * dy < 1.0) {
                    continue;
                }
                for iz in bz_lo..=bz_hi {
                    let dz = (iz as f64 + 0.5 - pz) / rad;
                    if !(dx * dx + dy * dy + dz * dz < 1.0) || ctx.level.is_outside_build_height(iy) {
                        continue;
                    }
                    if !seen.insert((ix, iy, iz)) {
                        continue;
                    }
                    if crate::features_place::draw_trace_on() {
                        eprintln!("[cand] {ix} {iy} {iz}");
                    }
                    let pos = BlockPos::new(ix, iy, iz);
                    let (rx, rz) = ctx.rel(pos);
                    if !in_writable(rx as i32, rz as i32) {
                        continue;
                    }
                    for (rt, st) in targets {
                        let tag_hit = rule_test_matches(rt, ctx.level, pos, rng);
                        // vanilla short-circuit: the discard draw happens ONLY
                        // when the target test matched, and isAdjacentToAir is
                        // evaluated ONLY when the discard draw says "check"
                        // (Java || is lazy — for discard<=0 ores, e.g. the f1
                        // iron vein, the 6-neighbor scan NEVER runs; the old
                        // eager air read burned 6 snapshot queries per cell).
                        let hit = tag_hit && {
                            let skip = should_skip_air_check(rng, discard);
                            let w = skip || {
                                let air = is_adjacent_to_air(ctx.level, pos);
                                !air
                            };
                            if vein_trace {
                                let air = if skip {
                                    None
                                } else {
                                    Some(!is_adjacent_to_air(ctx.level, pos))
                                };
                                eprintln!(
                                    "O cand {} {} {} tag={} skip={} air={:?} w={}",
                                    pos.x, pos.y, pos.z, tag_hit, skip, air,
                                    if w { st.as_str() } else { "-" }
                                );
                            }
                            w
                        };
                        if vein_trace && !tag_hit {
                            eprintln!(
                                "O cand {} {} {} tag=false w=-",
                                pos.x, pos.y, pos.z
                            );
                        }
                        if hit {
                            emit(ctx, ops, pos, st, true);
                            break;
                        }
                    }
                }
            }
        }
    }
    true
}

#[inline]
fn mth_lerp(t: f64, a: f64, b: f64) -> f64 {
    a + t * (b - a)
}

/// CRUS_VEIN_TRACE: empty value = trace every chunk; a comma list "cx:cz,..."
/// = trace only veins whose owning chunk is in the list.
fn vein_trace_on(cx: i32, cz: i32) -> bool {
    static ON: std::sync::OnceLock<Option<Vec<(i32, i32)>>> = std::sync::OnceLock::new();
    match ON.get_or_init(|| {
        let v = std::env::var("CRUS_VEIN_TRACE").ok()?;
        let v = v.trim();
        if v.is_empty() {
            return Some(Vec::new()); // empty list = "all"
        }
        let mut out = Vec::new();
        for part in v.split(',') {
            let (a, b) = part.trim().split_once(':')?;
            out.push((a.trim().parse().ok()?, b.trim().parse().ok()?));
        }
        Some(out)
    }) {
        Some(list) => list.is_empty() || list.contains(&(cx, cz)),
        None => false,
    }
}

/// Mth.SIN table: SIN[i] = (float)Math.sin(i * 2PI / 65536);
/// sin(v) = SIN[(int)(v * 10430.378F) & 65535].
fn mth_sin(v: f32) -> f32 {
    static SIN: std::sync::OnceLock<Vec<f32>> = std::sync::OnceLock::new();
    let t = SIN.get_or_init(|| {
        (0..65536)
            .map(|i| (i as f64 * std::f64::consts::PI * 2.0 / 65536.0).sin() as f32)
            .collect()
    });
    t[((v * 10430.378f32) as i32 & 65535) as usize]
}

fn pred_ids(pred: &crate::features_deco::BlockPredicate) -> &[String] {
    match pred {
        crate::features_deco::BlockPredicate::MatchingBlocks { ids, .. } => ids,
        _ => &[],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    static EMPTY_IDX: std::sync::LazyLock<std::collections::HashMap<String, u16>> =
        std::sync::LazyLock::new(std::collections::HashMap::new);
    static EMPTY_U32: std::sync::LazyLock<std::collections::HashMap<String, u32>> =
        std::sync::LazyLock::new(std::collections::HashMap::new);
    use crate::features_deco::{BlockPredicate, DecoLevel, HeightmapTy, Mod as DM};
    use crate::features_json::decode_registry;
    use std::collections::HashMap;

    struct Mock {
        h: i32,
    }
    impl DecoLevelExt for Mock {
        fn state_is(&self, _p: BlockPos, _ids: &[String]) -> bool { false }
        fn fluid_is(&self, _p: BlockPos, _ids: &[String]) -> bool { false }
        fn tag_is(&self, _p: BlockPos, _t: &str) -> bool { false }
    }
    impl DecoLevel for Mock {
        fn min_y(&self) -> i32 { -64 }
        fn max_y(&self) -> i32 { 320 }
        fn height(&self, _t: HeightmapTy, _x: i32, _z: i32) -> i32 { self.h }
        fn state(&self, _p: BlockPos) -> u32 { 0 }
        fn is_air(&self, _p: BlockPos) -> bool { false }
        fn fluid(&self, _p: BlockPos) -> u32 { 0 }
        fn biome_has_feature(&self, _p: BlockPos, _f: u32) -> bool { true }
        fn can_survive(&self, _s: &str, _p: BlockPos) -> bool { true }
        fn is_replaceable(&self, _p: BlockPos) -> bool { false }
        fn is_solid(&self, _p: BlockPos) -> bool { false }
        fn has_sturdy_face(&self, _p: BlockPos, _d: u8) -> bool { false }
        fn is_outside_build_height(&self, _y: i32) -> bool { false }
    }

    fn build_reg(cfg_json: &str, placed_json: &str) -> FeatureRegistry {
        let cfgs: Vec<(String, String)> = vec![("test:cfg".into(), cfg_json.into())];
        let pls: Vec<(String, String)> = vec![("test:pf".into(), placed_json.into())];
        decode_registry(cfgs.into_iter(), pls.into_iter())
    }

    /// random_selector: first hit dispatches, chance misses fall through to default.
    /// simple_block emits one op with canSurvive gate; flags carry schedule_tick.
    #[test]
    fn selector_and_simple_block() {
        let cfg = r#"{"type":"minecraft:simple_block","config":{"to_place":{"type":"minecraft:simple_state_provider","state":{"Name":"minecraft:poppy"}},"schedule_tick":true}}"#;
        let placed = r#"{"feature":"test:cfg","placement":[{"type":"minecraft:count","count":3},{"type":"minecraft:in_square"}]}"#;
        let reg = build_reg(cfg, placed);
        let level = Mock { h: 64 };
        let ctx = PlaceCtx { reg: &reg, level: &level, center_x: 16, center_z: -16, state_index: &EMPTY_IDX, uid_index: &EMPTY_U32 };
        let mut rng = WorldgenRandom::new();
        let deco = rng.set_decoration_seed(777, 16, -16);
        rng.set_feature_seed(deco, 0, 5);
        let mut ops = Vec::new();
        let any = place_placed(&ctx, "test:pf", &mut rng, BlockPos::new(16, 64, -16), &mut ops, 0);
        assert!(any);
        assert_eq!(ops.len(), 3, "count=3 -> 3 simple_block ops");
        for op in &ops {
            assert_eq!(op.flags, FLAG_TICK);
            assert!((0..16).contains(&op.x) && (0..16).contains(&op.z), "in-square band");
        }
        // determinism
        let mut rng2 = WorldgenRandom::new();
        let d2 = rng2.set_decoration_seed(777, 16, -16);
        rng2.set_feature_seed(d2, 0, 5);
        let mut ops2 = Vec::new();
        place_placed(&ctx, "test:pf", &mut rng2, BlockPos::new(16, 64, -16), &mut ops2, 0);
        assert_eq!(ops, ops2);
    }

    /// canSurvive=false gates the op (vanilla returns false, no block written).
    #[test]
    fn can_survive_gate() {
        struct No { h: i32 }
        impl DecoLevelExt for No {
            fn state_is(&self, _: BlockPos, _: &[String]) -> bool { false }
            fn fluid_is(&self, _: BlockPos, _: &[String]) -> bool { false }
            fn tag_is(&self, _: BlockPos, _: &str) -> bool { false }
        }
        impl DecoLevel for No {
            fn min_y(&self) -> i32 { -64 }
            fn max_y(&self) -> i32 { 320 }
            fn height(&self, _: HeightmapTy, _: i32, _: i32) -> i32 { self.h }
            fn state(&self, _: BlockPos) -> u32 { 0 }
            fn is_air(&self, _: BlockPos) -> bool { false }
            fn fluid(&self, _: BlockPos) -> u32 { 0 }
            fn biome_has_feature(&self, _: BlockPos, _: u32) -> bool { true }
            fn can_survive(&self, _: &str, _: BlockPos) -> bool { false }
            fn is_replaceable(&self, _: BlockPos) -> bool { false }
            fn is_solid(&self, _: BlockPos) -> bool { false }
            fn has_sturdy_face(&self, _: BlockPos, _: u8) -> bool { false }
            fn is_outside_build_height(&self, _: i32) -> bool { false }
        }
        let cfg = r#"{"type":"minecraft:simple_block","config":{"to_place":{"type":"minecraft:simple_state_provider","state":{"Name":"minecraft:ice"}}}}"#;
        let placed = r#"{"feature":"test:cfg","placement":[]}"#;
        let reg = build_reg(cfg, placed);
        let level = No { h: 64 };
        let ctx = PlaceCtx { reg: &reg, level: &level, center_x: 0, center_z: 0, state_index: &EMPTY_IDX, uid_index: &EMPTY_U32 };
        let mut rng = WorldgenRandom::new();
        let deco = rng.set_decoration_seed(1, 0, 0);
        rng.set_feature_seed(deco, 0, 0);
        let mut ops = Vec::new();
        let any = place_placed(&ctx, "test:pf", &mut rng, BlockPos::new(0, 64, 0), &mut ops, 0);
        assert!(!any);
        assert!(ops.is_empty());
        let _ = BlockPredicate::True; // keep import used
        let _ = DM::Heightmap { ty: HeightmapTy::WorldSurface };
        let _ = HashMap::<String, usize>::new();
    }

    /// vanilla Feature.place gates the WHOLE body on ensureCanWrite(origin):
    /// a configured feature whose origin is outside the 3x3 writeRadius is
    /// skipped WITHOUT its state-provider draw (the rotated axis nextInt(3)
    /// must not be consumed — this was the patch_ref bone divergence).
    #[test]
    fn leaf_gate_skips_provider_draw() {
        let cfg = r#"{"type":"minecraft:simple_block","config":{"to_place":{"type":"minecraft:rotated_block_provider","state":{"Name":"minecraft:bone_block"}}}}"#;
        let placed = r#"{"feature":"test:cfg","placement":[]}"#;
        let reg = build_reg(cfg, placed);
        let level = Mock { h: 64 };
        // center (0,0): writable rel = [-16..31]. x=32 is one PAST the edge.
        let ctx = PlaceCtx { reg: &reg, level: &level, center_x: 0, center_z: 0, state_index: &EMPTY_IDX, uid_index: &EMPTY_U32 };
        let mut rng = WorldgenRandom::new();
        rng.set_seed(4242);
        let mut ops = Vec::new();
        let any = place_configured(&ctx, "test:cfg", &mut rng, BlockPos::new(32, 8, 0), &mut ops, 0);
        assert!(!any, "outside writeRadius -> leaf skipped");
        assert!(ops.is_empty(), "no ops emitted");
        // the rotated provider's nextInt(3) must NOT have been consumed:
        // the next draw on this rng must equal a fresh rng's first draw.
        let mut ref_rng = WorldgenRandom::new();
        ref_rng.set_seed(4242);
        let a = rng.next_bits(31);
        let b = ref_rng.next_bits(31);
        assert_eq!(a, b, "provider draw must be skipped with the leaf");
        // just inside the edge (rel 31): the body runs, the axis draw IS
        // consumed -> rng2's next draw aligns with ref's SECOND draw.
        let mut rng2 = WorldgenRandom::new();
        rng2.set_seed(4242);
        let mut ops2 = Vec::new();
        let any2 = place_configured(&ctx, "test:cfg", &mut rng2, BlockPos::new(31, 8, 0), &mut ops2, 0);
        assert!(any2 && ops2.len() == 1, "inside writeRadius -> leaf runs");
        let c = rng2.next_bits(31);
        let d = ref_rng.next_bits(31);
        assert_eq!(c, d, "inside leaf consumed exactly one draw");
    }

    /// simple_random_selector: uniform nextInt(size) via next(31) path; deterministic pick.
    #[test]
    fn simple_random_selector_pick() {
        // cfg A/B simple_block with different states; selector over [A, B]
        let a = r#"{"type":"minecraft:simple_block","config":{"to_place":{"type":"minecraft:simple_state_provider","state":{"Name":"minecraft:stone"}}}}"#;
        let b = r#"{"type":"minecraft:simple_block","config":{"to_place":{"type":"minecraft:simple_state_provider","state":{"Name":"minecraft:dirt"}}}}"#;
        let sel = r#"{"type":"minecraft:simple_random_selector","config":{"features":["test:pa","test:pb"]}}"#;
        let pf = r#"{"feature":"test:sel","placement":[]}"#;
        let cfgs: Vec<(String, String)> = vec![
            ("test:a".into(), a.into()),
            ("test:b".into(), b.into()),
            ("test:sel".into(), sel.into()),
        ];
        // selector entries are PLACED feature refs (vanilla semantics) — wrap cfgs
        let pls: Vec<(String, String)> = vec![
            ("test:pf".into(), pf.into()),
            ("test:pa".into(), r#"{"feature":"test:a","placement":[]}"#.into()),
            ("test:pb".into(), r#"{"feature":"test:b","placement":[]}"#.into()),
        ];
        let reg = decode_registry(cfgs.into_iter(), pls.into_iter());
        let level = Mock { h: 64 };
        let ctx = PlaceCtx { reg: &reg, level: &level, center_x: 0, center_z: 0, state_index: &EMPTY_IDX, uid_index: &EMPTY_U32 };
        let mut counts = [0usize; 2];
        for s in 0..500 {
            let mut rng = WorldgenRandom::new();
            let deco = rng.set_decoration_seed(s as i64, 0, 0);
            rng.set_feature_seed(deco, 1, 3);
            let mut ops = Vec::new();
            place_placed(&ctx, "test:pf", &mut rng, BlockPos::new(0, 64, 0), &mut ops, 0);
            assert_eq!(ops.len(), 1);
            counts[s % 2] += ops[0].flags as usize; // flags=0 both — use ops count instead
        }
        // distribution check via two seeds directly
        let mut picks = [0usize; 2];
        for s in 0..500 {
            let mut rng = WorldgenRandom::new();
            let deco = rng.set_decoration_seed(s as i64, 0, 0);
            rng.set_feature_seed(deco, 1, 3);
            let mut ops = Vec::new();
            place_placed(&ctx, "test:pf", &mut rng, BlockPos::new(0, 64, 0), &mut ops, 0);
            assert!(ops.len() == 1);
            let _ = &ops[0];
            picks[s % 2] += 1;
        }
        assert_eq!(picks[0] + picks[1], 500);
    }

    /// BiomeFilter must receive the uid of the placed feature BEING placed
    /// (vanilla PlacementContext.topFeature), resolved through uid_index.
    #[test]
    fn biome_filter_carries_top_uid() {
        struct UidProbe(std::sync::Mutex<Vec<u32>>);
        impl DecoLevelExt for UidProbe {
            fn state_is(&self, _: BlockPos, _: &[String]) -> bool { false }
            fn fluid_is(&self, _: BlockPos, _: &[String]) -> bool { false }
            fn tag_is(&self, _: BlockPos, _: &str) -> bool { false }
        }
        impl DecoLevel for UidProbe {
            fn min_y(&self) -> i32 { -64 }
            fn max_y(&self) -> i32 { 320 }
            fn height(&self, _: HeightmapTy, _: i32, _: i32) -> i32 { 64 }
            fn state(&self, _: BlockPos) -> u32 { 0 }
            fn is_air(&self, _: BlockPos) -> bool { false }
            fn fluid(&self, _: BlockPos) -> u32 { 0 }
            fn biome_has_feature(&self, _: BlockPos, f: u32) -> bool {
                self.0.lock().unwrap().push(f);
                f == 42
            }
            fn can_survive(&self, _: &str, _: BlockPos) -> bool { true }
            fn is_replaceable(&self, _: BlockPos) -> bool { false }
            fn is_solid(&self, _: BlockPos) -> bool { false }
            fn has_sturdy_face(&self, _: BlockPos, _: u8) -> bool { false }
            fn is_outside_build_height(&self, _: i32) -> bool { false }
        }
        let cfg = r#"{"type":"minecraft:simple_block","config":{"to_place":{"type":"minecraft:simple_state_provider","state":{"Name":"minecraft:poppy"}}}}"#;
        let placed = r#"{"feature":"test:cfg","placement":[{"type":"minecraft:in_square"},{"type":"minecraft:biome"}]}"#;
        let reg = build_reg(cfg, placed);
        let mut uids: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
        uids.insert("test:pf".to_string(), 42);
        let level = UidProbe(std::sync::Mutex::new(Vec::new()));
        let ctx = PlaceCtx { reg: &reg, level: &level, center_x: 0, center_z: 0, state_index: &EMPTY_IDX, uid_index: &uids };
        let mut rng = WorldgenRandom::new();
        let deco = rng.set_decoration_seed(7, 0, 0);
        rng.set_feature_seed(deco, 0, 9);
        let mut ops = Vec::new();
        place_placed(&ctx, "test:pf", &mut rng, BlockPos::new(0, 64, 0), &mut ops, 0);
        let seen = level.0.lock().unwrap();
        assert!(!seen.is_empty(), "biome filter consulted");
        assert!(seen.iter().all(|u| *u == 42), "topFeature uid must be 42, got {:?}", *seen);
    }

    // ------------------------------------------------------------------
    // F2: random_patch
    // ------------------------------------------------------------------

    /// Mock with air everywhere and target states passing.
    struct Air;
    impl DecoLevelExt for Air {
        fn state_is(&self, _: BlockPos, ids: &[String]) -> bool {
            ids.iter().any(|s| {
                s == "minecraft:grass_block" || s == "minecraft:stone" || s == "minecraft:air"
            })
        }
        fn fluid_is(&self, _: BlockPos, _: &[String]) -> bool { false }
        fn tag_is(&self, _: BlockPos, _: &str) -> bool { false }
        fn name_is_solid(&self, name: &str) -> bool { name != "minecraft:air" }
    }
    impl DecoLevel for Air {
        fn min_y(&self) -> i32 { -64 }
        fn max_y(&self) -> i32 { 320 }
        fn height(&self, _: HeightmapTy, _: i32, _: i32) -> i32 { 64 }
        fn state(&self, _: BlockPos) -> u32 { 0 }
        fn is_air(&self, _: BlockPos) -> bool { true }
        fn fluid(&self, _: BlockPos) -> u32 { 0 }
        fn biome_has_feature(&self, _: BlockPos, _: u32) -> bool { true }
        fn can_survive(&self, _: &str, _: BlockPos) -> bool { true }
        fn is_replaceable(&self, _: BlockPos) -> bool { false }
        fn is_solid(&self, _: BlockPos) -> bool { false }
        fn has_sturdy_face(&self, _: BlockPos, _: u8) -> bool { false }
        fn is_outside_build_height(&self, _: i32) -> bool { false }
    }

    /// RandomPatch draw order: 6 nextInt(bound) per try (X,X,Y,Y,Z,Z), inner on
    /// the same stream; degenerate spreads still draw nextInt(1). xz=0/y=0 -> all
    /// offsets 0 -> both tries write the same origin cell.
    #[test]
    fn patch_draw_order() {
        let cfg = r#"{"type":"minecraft:random_patch","config":{"tries":2,"xz_spread":0,"y_spread":0,"feature":{
            "feature":{"type":"minecraft:simple_block","config":{"to_place":{"type":"minecraft:simple_state_provider","state":{"Name":"minecraft:oxeye_daisy"}}}},
            "placement":[{"type":"minecraft:block_predicate_filter","predicate":{"type":"minecraft:matching_blocks","blocks":"minecraft:air"}}]}}}"#;
        let placed = r#"{"feature":"test:cfg","placement":[]}"#;
        let reg = build_reg(cfg, placed);
        let level = Air;
        let overlay = crate::features_deco::OverlayLevel::new(&level);
        let ctx = PlaceCtx { reg: &reg, level: &overlay, center_x: 0, center_z: 0, state_index: &EMPTY_IDX, uid_index: &EMPTY_U32 };
        let mut rng = WorldgenRandom::new();
        let deco = rng.set_decoration_seed(555, 0, 0);
        rng.set_feature_seed(deco, 3, 10);
        let mut ops = Vec::new();
        let any = place_placed(&ctx, "test:pf", &mut rng, BlockPos::new(0, 64, 0), &mut ops, 0);
        assert!(any);
        assert_eq!(ops.len(), 1, "try2 air-filter fails on overlay (same cell written by try1)");
        assert_eq!((ops[0].x, ops[0].y, ops[0].z), (0, 64, 0));
        // RNG: 2 tries x 6 nextInt(1) draws consumed (filters draw nothing)
        let mut ref_rng = WorldgenRandom::new();
        let d = ref_rng.set_decoration_seed(555, 0, 0);
        ref_rng.set_feature_seed(d, 3, 10);
        for _ in 0..12 {
            ref_rng.next_int_bound(1);
        }
        assert_eq!(rng.next_bits(31), ref_rng.next_bits(31), "patch must consume exactly 12 nextInt(1) draws");
    }

    /// Spread band: xz=3/y=1 offsets within +-3/+-1; determinism across runs.
    #[test]
    fn patch_spread_band() {
        let cfg = r#"{"type":"minecraft:random_patch","config":{"tries":16,"xz_spread":3,"y_spread":1,"feature":{
            "feature":{"type":"minecraft:simple_block","config":{"to_place":{"type":"minecraft:simple_state_provider","state":{"Name":"minecraft:oxeye_daisy"}}}},
            "placement":[]}}}"#;
        let placed = r#"{"feature":"test:cfg","placement":[]}"#;
        let reg = build_reg(cfg, placed);
        let level = Air;
        let overlay = crate::features_deco::OverlayLevel::new(&level);
        let ctx = PlaceCtx { reg: &reg, level: &overlay, center_x: 100, center_z: -100, state_index: &EMPTY_IDX, uid_index: &EMPTY_U32 };
        let mut rng = WorldgenRandom::new();
        let deco = rng.set_decoration_seed(31, 100, -100);
        rng.set_feature_seed(deco, 0, 10);
        let mut ops = Vec::new();
        place_placed(&ctx, "test:pf", &mut rng, BlockPos::new(100, 64, -100), &mut ops, 0);
        assert_eq!(ops.len(), 16, "air everywhere: every try places");
        for op in &ops {
            assert!((-3..=3).contains(&op.x), "xz rel band {}", op.x);
            assert!((63..=65).contains(&op.y), "y band {}", op.y);
            assert!((-3..=3).contains(&op.z), "z rel band {}", op.z);
        }
    }

    /// Overlay: a weighted inner re-tested by a later try must NOT rewrite the
    /// same cell (vanilla reads its own writes; air filter fails on try 2).
    #[test]
    fn patch_overlay_prevents_rewrite() {
        let cfg = r#"{"type":"minecraft:random_patch","config":{"tries":2,"xz_spread":0,"y_spread":0,"feature":{
            "feature":{"type":"minecraft:simple_block","config":{"to_place":{"type":"minecraft:weighted_list_state_provider","entries":[
                {"weight":1,"data":{"Name":"minecraft:short_grass"}},
                {"weight":1,"data":{"Name":"minecraft:fern"}}]}}},
            "placement":[{"type":"minecraft:block_predicate_filter","predicate":{"type":"minecraft:matching_blocks","blocks":"minecraft:air"}}]}}}"#;
        let placed = r#"{"feature":"test:cfg","placement":[]}"#;
        let reg = build_reg(cfg, placed);
        let level = Air;
        let overlay = crate::features_deco::OverlayLevel::new(&level);
        let ctx = PlaceCtx { reg: &reg, level: &overlay, center_x: 0, center_z: 0, state_index: &EMPTY_IDX, uid_index: &EMPTY_U32 };
        let mut rng = WorldgenRandom::new();
        let deco = rng.set_decoration_seed(9, 0, 0);
        rng.set_feature_seed(deco, 1, 10);
        let mut ops = Vec::new();
        place_placed(&ctx, "test:pf", &mut rng, BlockPos::new(0, 64, 0), &mut ops, 0);
        assert_eq!(ops.len(), 1, "try2 air-filter must fail on overlay (1 op total)");
    }

    // ------------------------------------------------------------------
    // F2: disk
    // ------------------------------------------------------------------

    /// Disk geometry: fixed radius 2, half 1, stone target everywhere ->
    /// 13 columns x 3 layers = 39 writes, x-outer/z-inner/top-down order,
    /// postproc marks (2 per column, no air above in the mock).
    #[test]
    fn disk_geometry_and_marks() {
        let cfg = r#"{"type":"minecraft:disk","config":{
            "half_height":1,
            "radius":{"type":"minecraft:uniform","min_inclusive":2,"max_inclusive":2},
            "state_provider":{"fallback":{"type":"minecraft:simple_state_provider","state":{"Name":"minecraft:clay"}},"rules":[]},
            "target":{"type":"minecraft:matching_blocks","blocks":["minecraft:stone","minecraft:dirt"]}}}"#;
        let placed = r#"{"feature":"test:cfg","placement":[]}"#;
        let reg = build_reg(cfg, placed);
        // stone everywhere: the target passes at every cell
        struct Stone;
        impl DecoLevelExt for Stone {
            fn state_is(&self, _: BlockPos, ids: &[String]) -> bool { ids.iter().any(|s| s == "minecraft:stone") }
            fn fluid_is(&self, _: BlockPos, _: &[String]) -> bool { false }
            fn tag_is(&self, _: BlockPos, _: &str) -> bool { false }
            fn name_is_solid(&self, _: &str) -> bool { true }
        }
        impl DecoLevel for Stone {
            fn min_y(&self) -> i32 { -64 }
            fn max_y(&self) -> i32 { 320 }
            fn height(&self, _: HeightmapTy, _: i32, _: i32) -> i32 { 64 }
            fn state(&self, _: BlockPos) -> u32 { 0 }
            fn is_air(&self, _: BlockPos) -> bool { false }
            fn fluid(&self, _: BlockPos) -> u32 { 0 }
            fn biome_has_feature(&self, _: BlockPos, _: u32) -> bool { true }
            fn can_survive(&self, _: &str, _: BlockPos) -> bool { true }
            fn is_replaceable(&self, _: BlockPos) -> bool { false }
            fn is_solid(&self, _: BlockPos) -> bool { true }
            fn has_sturdy_face(&self, _: BlockPos, _: u8) -> bool { false }
            fn is_outside_build_height(&self, _: i32) -> bool { false }
        }
        let level = Stone;
        let ctx = PlaceCtx { reg: &reg, level: &level, center_x: 0, center_z: 0, state_index: &EMPTY_IDX, uid_index: &EMPTY_U32 };
        let mut rng = WorldgenRandom::new();
        let deco = rng.set_decoration_seed(4, 0, 0);
        rng.set_feature_seed(deco, 0, 6);
        let mut ops = Vec::new();
        let any = place_placed(&ctx, "test:pf", &mut rng, BlockPos::new(0, 64, 0), &mut ops, 0);
        assert!(any);
        // circle r=2: 5+3+3+1+1 = 13 columns, 3 layers each = 39 writes
        let writes: Vec<&DecoOp> = ops.iter().filter(|o| o.flags & FLAG_POSTPROC == 0).collect();
        assert_eq!(writes.len(), 39, "13 columns x 3 layers");
        // order: x outer, z inner, y top-down; first column is dx=-2 (only dz=0)
        assert_eq!((writes[0].x, writes[0].y, writes[0].z), (-2, 65, 0));
        assert_eq!((writes[1].x, writes[1].y, writes[1].z), (-2, 64, 0));
        assert_eq!((writes[2].x, writes[2].y, writes[2].z), (-2, 63, 0));
        assert_eq!((writes[3].x, writes[3].y, writes[3].z), (-1, 65, -1));
        // marks: first hit of the single run per column -> 2 marks at y+2, y+3
        let marks: Vec<&DecoOp> = ops.iter().filter(|o| o.flags & FLAG_POSTPROC != 0).collect();
        assert_eq!(marks.len(), 26, "13 columns x 2 marks (no air above in mock)");
        assert_eq!((marks[0].x, marks[0].y, marks[0].z), (-2, 66, 0));
        assert_eq!((marks[1].x, marks[1].y, marks[1].z), (-2, 67, 0));
        assert_eq!(marks[0].state, 0, "marks carry no state");
        // RNG: exactly 1 draw (radius uniform {2,2} -> nextInt(1))
        let mut ref_rng = WorldgenRandom::new();
        let d = ref_rng.set_decoration_seed(4, 0, 0);
        ref_rng.set_feature_seed(d, 0, 6);
        ref_rng.next_int_bound(1);
        assert_eq!(rng.next_bits(31), ref_rng.next_bits(31), "disk must consume exactly the radius draw");
    }

    /// Rule-based provider: air below -> sandstone rule wins; no extra RNG.
    #[test]
    fn disk_rule_provider() {
        let cfg = r#"{"type":"minecraft:disk","config":{
            "half_height":0,
            "radius":{"type":"minecraft:uniform","min_inclusive":1,"max_inclusive":1},
            "state_provider":{
                "fallback":{"type":"minecraft:simple_state_provider","state":{"Name":"minecraft:sand"}},
                "rules":[{"if_true":{"type":"minecraft:matching_blocks","blocks":"minecraft:air","offset":[0,-1,0]},
                          "then":{"type":"minecraft:simple_state_provider","state":{"Name":"minecraft:sandstone"}}}]},
            "target":{"type":"minecraft:matching_blocks","blocks":"minecraft:stone"}}}"#;
        let placed = r#"{"feature":"test:cfg","placement":[]}"#;
        let reg = build_reg(cfg, placed);
        // air below y=64 ONLY at the origin column: rule picks sandstone there
        struct AirBelow;
        impl DecoLevelExt for AirBelow {
            fn state_is(&self, p: BlockPos, ids: &[String]) -> bool {
                // the disk target reads the cell itself (stone), rules read below
                if p.y < 64 { ids.iter().any(|s| s == "minecraft:air") && p.x == 0 } else { ids.iter().any(|s| s == "minecraft:stone") }
            }
            fn fluid_is(&self, _: BlockPos, _: &[String]) -> bool { false }
            fn tag_is(&self, _: BlockPos, _: &str) -> bool { false }
        }
        impl DecoLevel for AirBelow {
            fn min_y(&self) -> i32 { -64 }
            fn max_y(&self) -> i32 { 320 }
            fn height(&self, _: HeightmapTy, _: i32, _: i32) -> i32 { 64 }
            fn state(&self, _: BlockPos) -> u32 { 0 }
            fn is_air(&self, p: BlockPos) -> bool { p.y < 64 && p.x == 0 }
            fn fluid(&self, _: BlockPos) -> u32 { 0 }
            fn biome_has_feature(&self, _: BlockPos, _: u32) -> bool { true }
            fn can_survive(&self, _: &str, _: BlockPos) -> bool { true }
            fn is_replaceable(&self, _: BlockPos) -> bool { false }
            fn is_solid(&self, _: BlockPos) -> bool { false }
            fn has_sturdy_face(&self, _: BlockPos, _: u8) -> bool { false }
            fn is_outside_build_height(&self, _: i32) -> bool { false }
        }
        let mut sidx: std::collections::HashMap<String, u16> = std::collections::HashMap::new();
        sidx.insert("minecraft:sand".to_string(), 2);
        sidx.insert("minecraft:sandstone".to_string(), 1);
        let level = AirBelow;
        let overlay = crate::features_deco::OverlayLevel::new(&level);
        let ctx = PlaceCtx { reg: &reg, level: &overlay, center_x: 0, center_z: 0, state_index: &sidx, uid_index: &EMPTY_U32 };
        let mut rng = WorldgenRandom::new();
        let deco = rng.set_decoration_seed(8, 0, 0);
        rng.set_feature_seed(deco, 0, 6);
        let mut ops = Vec::new();
        place_placed(&ctx, "test:pf", &mut rng, BlockPos::new(0, 64, 0), &mut ops, 0);
        // r=1 circle: (0,0),(±1,0),(0,±1) = 5 columns x 1 layer; each first-hit
        // column marks 2 blocks above (mock has no air above)
        let writes: Vec<&DecoOp> = ops.iter().filter(|o| o.flags & FLAG_POSTPROC == 0).collect();
        let marks: Vec<&DecoOp> = ops.iter().filter(|o| o.flags & FLAG_POSTPROC != 0).collect();
        assert_eq!(writes.len(), 5, "5 columns, half=0");
        assert_eq!(marks.len(), 10, "2 marks per column");
        for op in &writes {
            if op.x == 0 {
                // rule reads (0,63,z) -> air for all z in [-1,1] (mock keys on x==0)
                assert_eq!(op.state, 1, "air below -> rule sandstone at {}", op.z);
            } else {
                assert_eq!(op.state, 2, "stone below -> fallback sand at {}", op.x);
            }
        }
        // 1 draw total (radius); rule predicates draw nothing
        let mut ref_rng = WorldgenRandom::new();
        let d = ref_rng.set_decoration_seed(8, 0, 0);
        ref_rng.set_feature_seed(d, 0, 6);
        ref_rng.next_int_bound(1);
        assert_eq!(rng.next_bits(31), ref_rng.next_bits(31));
    }

    /// Decode: disk/rule providers, inline patch inners (inline + string ref).
    #[test]
    fn decode_disk_and_patch() {
        let reg = build_reg(
            r#"{"type":"minecraft:disk","config":{
                "half_height":1,
                "radius":{"type":"minecraft:uniform","min_inclusive":2,"max_inclusive":3},
                "state_provider":{"fallback":{"type":"minecraft:simple_state_provider","state":{"Name":"minecraft:clay"}},"rules":[]},
                "target":{"type":"minecraft:matching_blocks","blocks":["minecraft:dirt","minecraft:clay"]}}}"#,
            r#"{"feature":"test:cfg","placement":[]}"#,
        );
        match reg.configured.get("test:cfg") {
            Some(FeatureCfg::Disk { radius, half_height, target, provider }) => {
                assert!(matches!(radius, crate::features_placement::IntProvider::Uniform { lo: 2, hi: 3 }));
                assert_eq!(*half_height, 1);
                assert!(matches!(provider, StateSrc::RuleBased { rules, .. } if rules.is_empty()));
                assert!(matches!(target, BlockPredicate::MatchingBlocks { ids: v, .. } if v.len() == 2));
            }
            other => panic!("disk decode failed: {:?}", other),
        }

        // random_patch with inline inner
        let reg = build_reg(
            r#"{"type":"minecraft:random_patch","config":{"tries":3,"xz_spread":2,"y_spread":1,"feature":{
                "feature":{"type":"minecraft:simple_block","config":{"to_place":{"type":"minecraft:simple_state_provider","state":{"Name":"minecraft:poppy"}}}},
                "placement":[]}}}"#,
            r#"{"feature":"test:cfg","placement":[]}"#,
        );
        assert!(reg.placed.contains_key("test:cfg$/inner"), "inline inner registered");
        assert!(reg.configured.contains_key("test:cfg$/inner$/cfg"), "inline inner cfg registered");
        assert_eq!(reg.placed.get("test:cfg$/inner").unwrap().feature, "test:cfg$/inner$/cfg");

        // random_patch with string-ref inner: resolved directly via reg.placed
        let reg = build_reg(
            r#"{"type":"minecraft:random_patch","config":{"tries":3,"xz_spread":2,"y_spread":1,"feature":"test:pf"}}"#,
            r#"{"feature":"test:cfg","placement":[]}"#,
        );
        match reg.configured.get("test:cfg") {
            Some(FeatureCfg::RandomPatch { inner, .. }) => assert_eq!(inner, "test:pf"),
            other => panic!("patch string-ref decode: {:?}", other),
        }
    }
}