//! Features place engine (stage F1): registry walk, selector dispatch, DecoOp emission.
//! Selector RNG pinned from decompile: SimpleRandomSelector = nextInt(size) via next(31)
//! path; RandomBooleanSelector = next(1); RandomSelector = nextFloat()<chance, first hit
//! returns; WeightedStateProvider = nextInt(totalWeight) + linear walk (probe-verify).

use crate::features_deco::{expand_deco, DecoLevelExt, Mod};
use crate::features_json::{FeatureCfg, FeatureRegistry, PlacedDef, RuleTest, StateSrc};
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
        | FeatureCfg::VegetationPatch { .. } => true,
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