//! Trees + vegetation patches (stage F5): full vanilla 1.21.8 engine port.
//!
//! Contract: TreeFeature.doPlace/place (decompile src14) + 9 trunk placers +
//! 11 foliage placers + 2 feature sizes + mangrove roots + 10 decorators +
//! updateLeaves BFS + StructureTemplate.updateShapeAtEdge +
//! VegetationPatchFeature/WaterloggedVegetationPatchFeature.
//!
//! THE critical parity detail: decorators (and vegetation distribution)
//! iterate java.util.HashSet<BlockPos> in JAVA HASH ORDER — replicated by
//! JSetSim (Vec3i.hashCode = (y + z*31)*31 + x, HashMap spread, bin-append,
//! resize-at->0.75*cap with order-preserving lo/hi split, bin-major walk).
//! The Y-sorted decorator lists = stable sort of that order.

use crate::features_json::{
    FoliageCfg, FoliageTy, GrowSet, MinSize, RootCfg, TreeCfg, TreeDeco, TrunkCfg, TrunkTy,
    VegetationPatchCfg,
};
use crate::features_placement::IntProvider as IP;
use crate::features_place::{sample_state, PlaceCtx, AXIS_SHIFT, FLAG_ROTATED};
use crate::features_placement::BlockPos;
use crate::features_rng::WorldgenRandom;

// ---------------------------------------------------------------------------
// java.util.HashSet<BlockPos> iteration order (HashMap backing semantics)
// ---------------------------------------------------------------------------

#[inline]
fn bp_hash(p: BlockPos) -> u32 {
    // Vec3i.hashCode: (y + z*31)*31 + x (i32 wrap)
    let h = (p.y.wrapping_add(p.z.wrapping_mul(31))).wrapping_mul(31).wrapping_add(p.x);
    h as u32
}

#[inline]
fn spread(h: u32) -> u32 {
    h ^ (h >> 16)
}

pub struct JSetSim {
    items: Vec<BlockPos>,
    hashes: Vec<u32>,
    bins: Vec<Vec<u32>>,
    cap: u32,
    /// soft removal (TreeFeature BFS pops) — order()/contains() skip these
    removed: Vec<bool>,
    /// live (non-removed) count — java resize triggers on LIVE size
    live: usize,
}

impl JSetSim {
    pub fn new() -> Self {
        JSetSim {
            items: Vec::new(),
            hashes: Vec::new(),
            bins: vec![Vec::new(); 16],
            cap: 16,
            removed: Vec::new(),
            live: 0,
        }
    }

    pub fn add(&mut self, p: BlockPos) -> bool {
        let h = spread(bp_hash(p));
        let bin = (h as usize) & (self.cap as usize - 1);
        for &idx in &self.bins[bin] {
            if self.items[idx as usize] == p {
                return false;
            }
        }
        let idx = self.items.len() as u32;
        self.bins[bin].push(idx);
        self.items.push(p);
        self.hashes.push(h);
        self.removed.push(false);
        self.live += 1;
        if self.live as f32 > self.cap as f32 * 0.75 {
            self.resize();
        }
        true
    }

    fn resize(&mut self) {
        let new_cap = self.cap << 1;
        let mut bins = vec![Vec::<u32>::new(); new_cap as usize];
        // old-table bin order + chain order -> per-new-bin relative order
        // (Java resize preserves relative order within lo/hi splits)
        for chain in &self.bins {
            for &idx in chain {
                let bin = (self.hashes[idx as usize] as usize) & (new_cap as usize - 1);
                bins[bin].push(idx);
            }
        }
        self.bins = bins;
        self.cap = new_cap;
    }

    pub fn contains(&self, p: BlockPos) -> bool {
        let h = spread(bp_hash(p));
        let bin = (h as usize) & (self.cap as usize - 1);
        self.bins[bin]
            .iter()
            .any(|&idx| !self.removed[idx as usize] && self.items[idx as usize] == p)
    }

    /// pop the first element in iteration order (Java iterator.next + remove)
    pub fn pop_first(&mut self) -> Option<BlockPos> {
        for bi in 0..self.bins.len() {
            let order = {
                let chain = &self.bins[bi];
                let mut idxs: Vec<u32> = chain
                    .iter()
                    .copied()
                    .filter(|&idx| !self.removed[idx as usize])
                    .collect();
                if chain.len() >= 8 && self.cap >= 64 {
                    idxs.sort_by(|&a, &b| {
                        self.hashes[a as usize]
                            .cmp(&self.hashes[b as usize])
                            .then_with(|| {
                                block_pos_cmp(self.items[a as usize], self.items[b as usize])
                            })
                    });
                }
                idxs
            };
            if let Some(&first) = order.first() {
                self.removed[first as usize] = true;
                self.live -= 1;
                return Some(self.items[first as usize]);
            }
        }
        None
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty() || self.removed.iter().all(|&r| r)
    }

    /// java.util iteration order: bins 0..cap in order, chains in order.
    /// Treeified bins (chain >= 8, cap >= 64 — HashMap.TREEIFY_THRESHOLD)
    /// iterate in TREE order = (hash, BlockPos::compareTo) sort.
    pub fn order(&self) -> Vec<BlockPos> {
        let mut out = Vec::with_capacity(self.items.len());
        for chain in &self.bins {
            self.append_bin_order(chain, &mut out);
        }
        out
    }

    fn append_bin_order(&self, chain: &[u32], out: &mut Vec<BlockPos>) {
        if chain.len() >= 8 && self.cap >= 64 {
            let mut idxs: Vec<u32> = chain
                .iter()
                .copied()
                .filter(|&idx| !self.removed[idx as usize])
                .collect();
            idxs.sort_by(|&a, &b| {
                self.hashes[a as usize]
                    .cmp(&self.hashes[b as usize])
                    .then_with(|| block_pos_cmp(self.items[a as usize], self.items[b as usize]))
            });
            for idx in idxs {
                out.push(self.items[idx as usize]);
            }
        } else {
            for &idx in chain {
                if !self.removed[idx as usize] {
                    out.push(self.items[idx as usize]);
                }
            }
        }
    }
}

impl Default for JSetSim {
    fn default() -> Self {
        Self::new()
    }
}

/// Vec3i.compareTo: y, then z, then x
fn block_pos_cmp(a: BlockPos, b: BlockPos) -> std::cmp::Ordering {
    if a.y != b.y {
        return a.y.cmp(&b.y);
    }
    if a.z != b.z {
        return a.z.cmp(&b.z);
    }
    a.x.cmp(&b.x)
}

/// TreeDecorator.Context ctor: ObjectArrayList(set) + List.sort(byY) — STABLE.
pub fn sorted_by_y(mut v: Vec<BlockPos>) -> Vec<BlockPos> {
    v.sort_by_key(|p| p.y);
    v
}

// ---------------------------------------------------------------------------
// Level predicates (tags resolved via the JVM bridge)
// ---------------------------------------------------------------------------

#[inline]
fn valid_tree_pos(level: &dyn crate::features_deco::DecoLevelExt, p: BlockPos) -> bool {
    level.is_air(p) || level.tag_is(p, "minecraft:replaceable_by_trees")
}

#[inline]
fn is_free(level: &dyn crate::features_deco::DecoLevelExt, p: BlockPos) -> bool {
    valid_tree_pos(level, p) || level.tag_is(p, "minecraft:logs")
}

#[inline]
fn is_vine(level: &dyn crate::features_deco::DecoLevelExt, p: BlockPos) -> bool {
    level.state_is(p, &["minecraft:vine".to_string()])
}

#[inline]
fn is_dirt_like(level: &dyn crate::features_deco::DecoLevelExt, p: BlockPos) -> bool {
    // TrunkPlacer.isDirt: Feature.isDirt && !grass_block && !mycelium
    level.tag_is(p, "minecraft:dirt")
        && !level.state_is(p, &["minecraft:grass_block".to_string()])
        && !level.state_is(p, &["minecraft:mycelium".to_string()])
}

#[inline]
fn is_grass_or_dirt(level: &dyn crate::features_deco::DecoLevelExt, p: BlockPos) -> bool {
    level.tag_is(p, "minecraft:dirt")
}

#[inline]
fn is_air_or_leaves(level: &dyn crate::features_deco::DecoLevelExt, p: BlockPos) -> bool {
    level.is_air(p) || level.tag_is(p, "minecraft:leaves")
}

fn grow_set_matches(
    level: &dyn crate::features_deco::DecoLevelExt,
    p: BlockPos,
    g: &GrowSet,
) -> bool {
    match g {
        GrowSet::Tag(t) => level.tag_is(p, t),
        GrowSet::Blocks(v) => {
            let names: Vec<String> = v.iter().map(|s| s.clone()).collect();
            level.state_is(p, &names)
        }
    }
}

// ---------------------------------------------------------------------------
// state-string helpers
// ---------------------------------------------------------------------------

/// parse "ns:block[k=v,...]" -> (name, [(k, v)])
pub fn parse_state(s: &str) -> (&str, Vec<(&str, &str)>) {
    match s.split_once('[') {
        Some((n, rest)) => {
            let props = rest.trim_end_matches(']');
            let mut v = Vec::new();
            for kv in props.split(',') {
                if let Some((k, val)) = kv.split_once('=') {
                    v.push((k, val));
                }
            }
            (n, v)
        }
        None => (s, Vec::new()),
    }

}

/// rebuild "ns:block[k=v,...]" replacing/adding one property
pub fn set_prop(s: &str, key: &str, val: &str) -> String {
    let (name, props) = parse_state(s);
    let mut found = false;
    let mut kvs: Vec<String> = props
        .iter()
        .map(|(k, v)| {
            if *k == key {
                found = true;
                format!("{}={}", k, val)
            } else {
                format!("{}={}", k, v)
            }
        })
        .collect();
    if !found {
        kvs.push(format!("{}={}", key, val));
    }
    if kvs.is_empty() {
        name.to_string()
    } else {
        format!("{}[{}]", name, kvs.join(","))
    }
}

pub fn get_prop<'a>(s: &'a str, key: &str) -> Option<&'a str> {
    let (_, props) = parse_state(s);
    props.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
}

/// LeavesBlock.getOptionalDistanceAt: LOGS -> 0; has DISTANCE -> value; else none.
fn optional_distance_at(s: &str) -> Option<i32> {
    let (name, props) = parse_state(s);
    if name.ends_with("_log")
        || name.ends_with("_stem")
        || name == "minecraft:bamboo_block"
        || name.contains("log")
    {
        // tag LOGS — approximated by name; the exact tag check runs through
        // the world read path (see below)
        return Some(0);
    }
    props
        .iter()
        .find(|(k, _)| *k == "distance")
        .and_then(|(_, v)| v.parse::<i32>().ok())
}

// ---------------------------------------------------------------------------
// TreeRunner
// ---------------------------------------------------------------------------

pub struct TreeRunner<'a, 'b> {
    pub ctx: &'a PlaceCtx<'a>,
    pub ops: &'b mut Vec<crate::features_place::DecoOp>,
    pub roots: JSetSim,
    pub logs: JSetSim,
    pub leaves: JSetSim,
    pub decos: JSetSim,
}

const DIRS6: [(i32, i32, i32); 6] = [
    (0, -1, 0), // DOWN
    (0, 1, 0),  // UP
    (0, 0, -1), // NORTH
    (0, 0, 1),  // SOUTH
    (-1, 0, 0), // WEST
    (1, 0, 0),  // EAST
];

/// Plane.HORIZONTAL faces array order
const HORIZONTAL: [u8; 4] = [2, 5, 3, 4]; // N, E, S, W (dir codes)

fn dir_step(d: u8) -> (i32, i32, i32) {
    DIRS6[d as usize]
}

impl<'a, 'b> TreeRunner<'a, 'b> {
    fn emit(&mut self, pos: BlockPos, state: &str, flags: u8, aux: u16) {
        self.ctx.emit_aux(self.ops, pos, state, flags, aux);
    }

    /// root placer setter (set = roots)
    fn write_root(&mut self, pos: BlockPos, state: &str, wl: bool) {
        let s = if wl { set_prop(state, "waterlogged", "true") } else { state.to_string() };
        self.roots.add(pos);
        self.emit(pos, &s, 0, 0);
    }

    /// trunk setter (set1 = logs): axis flag for fancy/cherry
    fn write_log(&mut self, pos: BlockPos, state: &str, axis: Option<u8>) {
        self.logs.add(pos);
        let flags = match axis {
            Some(a) => FLAG_ROTATED | (a << AXIS_SHIFT),
            None => 0,
        };
        self.emit(pos, state, flags, 0);
    }

    /// foliage setter (set2 = leaves): aux waterlogged bit
    fn write_leaf(&mut self, pos: BlockPos, state: &str, wl: bool) {
        self.leaves.add(pos);
        let s = if wl { set_prop(state, "waterlogged", "true") } else { state.to_string() };
        self.emit(pos, &s, 0, 0);
    }

    /// decoration setter (set3)
    fn write_deco(&mut self, pos: BlockPos, state: &str) {
        self.decos.add(pos);
        self.emit(pos, state, 0, 0);
    }

    fn set_dirt_at(&mut self, pos: BlockPos, cfg: &TreeCfg, rng: &mut WorldgenRandom) {
        let level = self.ctx.level;
        if cfg.force_dirt || !is_dirt_like(level, pos) {
            let mut flags = 0u8;
            if let Some(st) = sample_state(&cfg.dirt_provider, level, pos, rng, &mut flags) {
                self.emit(pos, &st, flags, 0);
            }
        }
    }

    fn place_log(
        &mut self,
        pos: BlockPos,
        cfg: &TreeCfg,
        rng: &mut WorldgenRandom,
        axis: Option<u8>,
    ) -> bool {
        if valid_tree_pos(self.ctx.level, pos) {
            let mut flags = 0u8;
            if let Some(st) = sample_state(&cfg.trunk_provider, self.ctx.level, pos, rng, &mut flags)
            {
                // axis override (fancy/cherry): FLAG_ROTATED bits win over the
                // provider's own rotated flag
                self.write_log(pos, &st, axis);
            }
            true
        } else {
            false
        }
    }

    fn place_log_if_free(
        &mut self,
        pos: BlockPos,
        cfg: &TreeCfg,
        rng: &mut WorldgenRandom,
    ) {
        if is_free(self.ctx.level, pos) {
            self.place_log(pos, cfg, rng, None);
        }
    }

    fn try_place_leaf(
        &mut self,
        pos: BlockPos,
        cfg: &TreeCfg,
        rng: &mut WorldgenRandom,
    ) -> bool {
        let level = self.ctx.level;
        // vanilla: skip when the EXISTING state has persistent=true (foliage
        // from a previous tree placed with the persistent property)
        let existing = self.ctx.level.state_full(pos);
        if !existing.is_empty() {
            if get_prop(&existing, "persistent") == Some("true") {
                return false;
            }
            if !valid_tree_pos(level, pos) {
                return false;
            }
        } else if !valid_tree_pos(level, pos) {
            return false;
        }
        let mut flags = 0u8;
        let Some(st) = sample_state(&cfg.foliage_provider, level, pos, rng, &mut flags) else {
            return false;
        };
        let wl = has_waterlogged_prop(&st)
            && level.fluid_is(pos, &["minecraft:water".to_string()]);
        self.write_leaf(pos, &st, wl);
        true
    }
}

fn has_waterlogged_prop(s: &str) -> bool {
    let (_, props) = parse_state(s);
    props.iter().any(|(k, _)| *k == "waterlogged")
}

// ---------------------------------------------------------------------------
// doPlace + minimum size
// ---------------------------------------------------------------------------

impl<'a, 'b> TreeRunner<'a, 'b> {
    fn min_size_at(cfg: &TreeCfg, height: i32, h: i32) -> i32 {
        match cfg.minimum_size {
            MinSize::TwoLayers { limit, lower, upper } => {
                if h < limit {
                    lower
                } else {
                    upper
                }
            }
            MinSize::ThreeLayers { limit, lower, middle, upper, upper_limit } => {
                if h < limit {
                    lower
                } else if h >= height - upper_limit {
                    upper
                } else {
                    middle
                }
            }
        }
    }

    fn get_max_free_tree_height(&self, tree_height: i32, top: BlockPos, cfg: &TreeCfg) -> i32 {
        let level = self.ctx.level;
        let mut mp;
        for h in 0..=(tree_height + 1) {
            let size = Self::min_size_at(cfg, tree_height, h);
            let mut dx = -size;
            while dx <= size {
                let mut dz = -size;
                while dz <= size {
                    mp = BlockPos::new(top.x + dx, top.y + h, top.z + dz);
                    if !is_free(level, mp) || (!cfg.ignore_vines && is_vine(level, mp)) {
                        return h - 2;
                    }
                    dz += 1;
                }
                dx += 1;
            }
        }
        tree_height
    }

    fn do_place(&mut self, pos: BlockPos, cfg: &TreeCfg, rng: &mut WorldgenRandom) -> bool {
        let level = self.ctx.level;
        let tree_height =
            cfg.trunk_placer.base + rng.next_int_bound(cfg.trunk_placer.h_rand_a + 1)
                + rng.next_int_bound(cfg.trunk_placer.h_rand_b + 1);
        let foliage_height = foliage_height(cfg, rng, tree_height);
        let i1 = tree_height - foliage_height;
        let foliage_radius = foliage_radius(cfg, rng, i1);
        let trunk_origin = match &cfg.root_placer {
            Some(rp) => BlockPos {
                y: pos.y + rp.trunk_offset_y.sample(rng),
                x: pos.x,
                z: pos.z,
            },
            None => pos,
        };
        let min = pos.y.min(trunk_origin.y);
        let i3 = pos.y.max(trunk_origin.y) + tree_height + 1;
        if min < level.min_y() + 1 || i3 > level.max_y() + 1 {
            return false;
        }
        let max_free = self.get_max_free_tree_height(tree_height, trunk_origin, cfg);
        let clipped_ok = match cfg.min_clipped {
            Some(mc) => max_free >= mc,
            None => false,
        };
        if !(max_free >= tree_height || clipped_ok) {
            return false;
        }
        if let Some(rp) = &cfg.root_placer {
            if !self.place_roots(rp, pos, trunk_origin, cfg, rng) {
                return false;
            }
        }
        let attachments = self.place_trunk(&cfg.trunk_placer, max_free, trunk_origin, cfg, rng);
        for att in attachments {
            create_foliage(self, cfg, max_free, att, foliage_height, foliage_radius, rng);
        }
        true
    }
}

fn foliage_height(cfg: &TreeCfg, rng: &mut WorldgenRandom, tree_height: i32) -> i32 {
    match cfg.foliage_placer.ty {
        FoliageTy::Blob | FoliageTy::Fancy | FoliageTy::Bush | FoliageTy::MegaJungle => {
            cfg.foliage_placer.fixed_height
        }
        FoliageTy::Acacia => 0,
        FoliageTy::DarkOak => 4,
        FoliageTy::Spruce => {
            let t = cfg.foliage_placer.prov_height.sample(rng);
            4.max(tree_height - t)
        }
        FoliageTy::Pine | FoliageTy::MegaPine | FoliageTy::RandomSpread | FoliageTy::Cherry => {
            cfg.foliage_placer.prov_height.sample(rng)
        }
    }
}

fn foliage_radius(cfg: &TreeCfg, rng: &mut WorldgenRandom, radius: i32) -> i32 {
    if cfg.foliage_placer.ty == FoliageTy::Pine {
        cfg.foliage_placer.radius.sample(rng)
            + rng.next_int_bound((radius + 1).max(1))
    } else {
        cfg.foliage_placer.radius.sample(rng)
    }
}

struct FoliageAttachment {
    pos: BlockPos,
    radius_offset: i32,
    double_trunk: bool,
}

// ---------------------------------------------------------------------------
// Trunk placers
// ---------------------------------------------------------------------------

impl<'a, 'b> TreeRunner<'a, 'b> {
    fn place_trunk(
        &mut self,
        t: &TrunkCfg,
        free: i32,
        pos: BlockPos,
        cfg: &TreeCfg,
        rng: &mut WorldgenRandom,
    ) -> Vec<FoliageAttachment> {
        match t.ty {
            TrunkTy::Straight => self.trunk_straight(free, pos, cfg, rng),
            TrunkTy::Forking => self.trunk_forking(free, pos, cfg, rng),
            TrunkTy::Bending => self.trunk_bending(t, free, pos, cfg, rng),
            TrunkTy::Fancy => self.trunk_fancy(free, pos, cfg, rng),
            TrunkTy::Giant => self.trunk_giant(free, pos, cfg, rng),
            TrunkTy::MegaJungle => self.trunk_mega_jungle(free, pos, cfg, rng),
            TrunkTy::DarkOak => self.trunk_dark_oak(free, pos, cfg, rng),
            TrunkTy::UpwardsBranching => self.trunk_upwards_branching(t, free, pos, cfg, rng),
            TrunkTy::Cherry => self.trunk_cherry(t, free, pos, cfg, rng),
        }
    }

    fn trunk_straight(
        &mut self,
        free: i32,
        pos: BlockPos,
        cfg: &TreeCfg,
        rng: &mut WorldgenRandom,
    ) -> Vec<FoliageAttachment> {
        self.set_dirt_at(BlockPos::new(pos.x, pos.y - 1, pos.z), cfg, rng);
        for i in 0..free {
            self.place_log(BlockPos::new(pos.x, pos.y + i, pos.z), cfg, rng, None);
        }
        vec![FoliageAttachment { pos: BlockPos::new(pos.x, pos.y + free, pos.z), radius_offset: 0, double_trunk: false }]
    }

    fn trunk_forking(
        &mut self,
        free: i32,
        pos: BlockPos,
        cfg: &TreeCfg,
        rng: &mut WorldgenRandom,
    ) -> Vec<FoliageAttachment> {
        self.set_dirt_at(BlockPos::new(pos.x, pos.y - 1, pos.z), cfg, rng);
        let mut out = Vec::new();
        let d = random_horizontal(rng);
        let i = free - rng.next_int_bound(4) - 1;
        let mut i1 = 3 - rng.next_int_bound(3);
        let mut x = pos.x;
        let mut z = pos.z;
        let mut top_y: Option<i32> = None;
        for i2 in 0..free {
            let i3 = pos.y + i2;
            if i2 >= i && i1 > 0 {
                x += d.0;
                z += d.1;
                i1 -= 1;
            }
            if self.place_log(BlockPos::new(x, i3, z), cfg, rng, None) {
                top_y = Some(i3 + 1);
            }
        }
        if let Some(ty) = top_y {
            out.push(FoliageAttachment { pos: BlockPos::new(x, ty, z), radius_offset: 1, double_trunk: false });
        }
        x = pos.x;
        z = pos.z;
        let d1 = random_horizontal(rng);
        if d1 != d {
            let i3x = i - rng.next_int_bound(2) - 1;
            let mut i4 = 1 + rng.next_int_bound(3);
            let mut i5 = i3x;
            top_y = None;
            while i5 < free && i4 > 0 {
                if i5 >= 1 {
                    let i6 = pos.y + i5;
                    x += d1.0;
                    z += d1.1;
                    if self.place_log(BlockPos::new(x, i6, z), cfg, rng, None) {
                        top_y = Some(i6 + 1);
                    }
                }
                i4 -= 1;
                i5 += 1;
            }
            if let Some(ty) = top_y {
                out.push(FoliageAttachment { pos: BlockPos::new(x, ty, z), radius_offset: 0, double_trunk: false });
            }
        }
        out
    }

    fn trunk_bending(
        &mut self,
        t: &TrunkCfg,
        free: i32,
        pos: BlockPos,
        cfg: &TreeCfg,
        rng: &mut WorldgenRandom,
    ) -> Vec<FoliageAttachment> {
        let d = random_horizontal(rng);
        let i = free - 1;
        let mut cur = pos;
        self.set_dirt_at(BlockPos::new(cur.x, cur.y - 1, cur.z), cfg, rng);
        let mut out = Vec::new();
        for i1 in 0..=i {
            if i1 + 1 >= i + rng.next_int_bound(2) {
                cur.x += d.0;
                cur.z += d.1;
            }
            if valid_tree_pos(self.ctx.level, cur) {
                self.place_log(cur, cfg, rng, None);
            }
            if i1 >= t.bend_min_leaves {
                out.push(FoliageAttachment { pos: cur, radius_offset: 0, double_trunk: false });
            }
            cur.y += 1;
        }
        let i1 = t.bend_length.sample(rng);
        for _ in 0..=i1 {
            if valid_tree_pos(self.ctx.level, cur) {
                self.place_log(cur, cfg, rng, None);
            }
            out.push(FoliageAttachment { pos: cur, radius_offset: 0, double_trunk: false });
            cur.x += d.0;
            cur.z += d.1;
        }
        out
    }

    /// FancyTrunkPlacer — 0.618 trunk scale, branch clusters, limb walker.
    fn trunk_fancy(
        &mut self,
        free: i32,
        pos: BlockPos,
        cfg: &TreeCfg,
        rng: &mut WorldgenRandom,
    ) -> Vec<FoliageAttachment> {
        let i1 = free + 2;
        let floor = ((i1 as f64) * 0.618).floor() as i32; // Mth.floor
        self.set_dirt_at(BlockPos::new(pos.x, pos.y - 1, pos.z), cfg, rng);
        let min = 1.min((1.382 + (1.0 * (i1 as f64) / 13.0).powf(2.0)).floor() as i32);
        let i2 = pos.y + floor;
        let mut i3 = i1 - 5;
        struct FC {
            att: BlockPos,
            branch_base: i32,
        }
        let mut coords: Vec<FC> = Vec::new();
        coords.push(FC { att: BlockPos::new(pos.x, pos.y + i3, pos.z), branch_base: i2 });
        while i3 >= 0 {
            let f = tree_shape(i1, i3);
            if !(f < 0.0) {
                for _ in 0..min {
                    let d2 = 1.0 * (f as f64) * ((rng.next_f32() as f64) + 0.328);
                    let d3 = ((rng.next_f32() * 2.0f32) as f64) * std::f64::consts::PI;
                    let d4 = d2 * java_math_sin(d3) + 0.5;
                    let d5 = d2 * java_math_cos(d3) + 0.5;
                    let block_pos = BlockPos::new(
                        pos.x + (d4.floor() as i32),
                        pos.y + i3 - 1,
                        pos.z + (d5.floor() as i32),
                    );
                    let block_pos1 = BlockPos::new(block_pos.x, block_pos.y + 5, block_pos.z);
                    if self.make_limb(block_pos, block_pos1, false, cfg, rng) {
                        let i5 = pos.x - block_pos.x;
                        let i6 = pos.z - block_pos.z;
                        let d6 = (block_pos.y as f64)
                            - (((i5 * i5 + i6 * i6) as f64).sqrt()) * 0.381;
                        let i7 = if d6 > i2 as f64 { i2 } else { d6 as i32 };
                        let block_pos2 = BlockPos::new(pos.x, i7, pos.z);
                        if self.make_limb(block_pos2, block_pos, false, cfg, rng) {
                            coords.push(FC { att: block_pos, branch_base: block_pos2.y });
                        }
                    }
                }
            }
            i3 -= 1;
        }
        self.make_limb(pos, BlockPos::new(pos.x, pos.y + floor, pos.z), true, cfg, rng);
        let atts: Vec<BlockPos> = coords.iter().map(|c| c.att).collect();
        let bases: Vec<i32> = coords.iter().map(|c| c.branch_base).collect();
        self.make_branches(i1, pos, &atts, &bases, cfg, rng);
        let mut out = Vec::new();
        for c in &coords {
            if (c.branch_base - pos.y) as f64 >= (i1 as f64) * 0.2 {
                out.push(FoliageAttachment { pos: c.att, radius_offset: 0, double_trunk: false });
            }
        }
        out
    }

    fn make_limb(
        &mut self,
        base: BlockPos,
        to: BlockPos,
        modify_world: bool,
        cfg: &TreeCfg,
        rng: &mut WorldgenRandom,
    ) -> bool {
        if !modify_world && base == to {
            return true;
        }
        let dx = to.x - base.x;
        let dy = to.y - base.y;
        let dz = to.z - base.z;
        let steps = dx.abs().max(dy.abs().max(dz.abs()));
        let fx = (dx as f32) / (steps as f32);
        let fy = (dy as f32) / (steps as f32);
        let fz = (dz as f32) / (steps as f32);
        for i in 0..=steps {
            let p = BlockPos::new(
                base.x + (0.5f32 + (i as f32) * fx).floor() as i32,
                base.y + (0.5f32 + (i as f32) * fy).floor() as i32,
                base.z + (0.5f32 + (i as f32) * fz).floor() as i32,
            );
            if modify_world {
                let axis = get_log_axis(base, p);
                self.place_log(p, cfg, rng, Some(axis));
            } else if !is_free(self.ctx.level, p) {
                return false;
            }
        }
        true
    }

    fn make_branches(
        &mut self,
        max_height: i32,
        pos: BlockPos,
        coords: &[BlockPos],
        bases: &[i32],
        cfg: &TreeCfg,
        rng: &mut WorldgenRandom,
    ) {
        for (c, bb) in coords.iter().zip(bases.iter()) {
            let bp = BlockPos::new(pos.x, *bb, pos.z);
            if bp != *c && (*bb - pos.y) as f64 >= (max_height as f64) * 0.2 {
                self.make_limb(bp, *c, true, cfg, rng);
            }
        }
    }

    fn trunk_giant(
        &mut self,
        free: i32,
        pos: BlockPos,
        cfg: &TreeCfg,
        rng: &mut WorldgenRandom,
    ) -> Vec<FoliageAttachment> {
        let bx = pos.x;
        let by = pos.y - 1;
        let bz = pos.z;
        self.set_dirt_at(BlockPos::new(bx, by, bz), cfg, rng);
        self.set_dirt_at(BlockPos::new(bx + 1, by, bz), cfg, rng);
        self.set_dirt_at(BlockPos::new(bx, by, bz + 1), cfg, rng);
        self.set_dirt_at(BlockPos::new(bx + 1, by, bz + 1), cfg, rng);
        for i in 0..free {
            self.place_log_if_free(BlockPos::new(bx, pos.y + i, bz), cfg, rng);
            if i < free - 1 {
                self.place_log_if_free(BlockPos::new(bx + 1, pos.y + i, bz), cfg, rng);
                self.place_log_if_free(BlockPos::new(bx + 1, pos.y + i, bz + 1), cfg, rng);
                self.place_log_if_free(BlockPos::new(bx, pos.y + i, bz + 1), cfg, rng);
            }
        }
        vec![FoliageAttachment { pos: BlockPos::new(pos.x, pos.y + free, pos.z), radius_offset: 0, double_trunk: true }]
    }

    fn trunk_mega_jungle(
        &mut self,
        free: i32,
        pos: BlockPos,
        cfg: &TreeCfg,
        rng: &mut WorldgenRandom,
    ) -> Vec<FoliageAttachment> {
        let mut out = self.trunk_giant(free, pos, cfg, rng);
        let mut i = free - 2 - rng.next_int_bound(4);
        while i > free / 2 {
            let f = rng.next_f32() * ((std::f64::consts::PI * 2.0) as f32);
            let mut i1 = 0i32;
            let mut i2 = 0i32;
            for i3 in 0..5 {
                i1 = (1.5f32 + mth_cos(f) * (i3 as f32)) as i32;
                i2 = (1.5f32 + mth_sin(f) * (i3 as f32)) as i32;
                let bp = BlockPos::new(pos.x + i1, pos.y + i - 3 + i3 / 2, pos.z + i2);
                self.place_log(bp, cfg, rng, None);
            }
            out.push(FoliageAttachment {
                pos: BlockPos::new(pos.x + i1, pos.y + i, pos.z + i2),
                radius_offset: -2,
                double_trunk: false,
            });
            i -= 2 + rng.next_int_bound(4);
        }
        out
    }

    fn trunk_dark_oak(
        &mut self,
        free: i32,
        pos: BlockPos,
        cfg: &TreeCfg,
        rng: &mut WorldgenRandom,
    ) -> Vec<FoliageAttachment> {
        let mut out = Vec::new();
        let bx = pos.x;
        let by = pos.y - 1;
        let bz = pos.z;
        self.set_dirt_at(BlockPos::new(bx, by, bz), cfg, rng);
        self.set_dirt_at(BlockPos::new(bx + 1, by, bz), cfg, rng);
        self.set_dirt_at(BlockPos::new(bx, by, bz + 1), cfg, rng);
        self.set_dirt_at(BlockPos::new(bx + 1, by, bz + 1), cfg, rng);
        let d = random_horizontal(rng);
        let i = free - rng.next_int_bound(4);
        let mut i1 = 2 - rng.next_int_bound(3);
        let mut x2 = pos.x;
        let mut z2 = pos.z;
        let i4 = pos.y + free - 1;
        for i5 in 0..free {
            if i5 >= i && i1 > 0 {
                x2 += d.0;
                z2 += d.1;
                i1 -= 1;
            }
            let i6 = pos.y + i5;
            let bp = BlockPos::new(x2, i6, z2);
            if is_air_or_leaves(self.ctx.level, bp) {
                self.place_log(bp, cfg, rng, None);
                self.place_log(BlockPos::new(bp.x + 1, bp.y, bp.z), cfg, rng, None);
                self.place_log(BlockPos::new(bp.x, bp.y, bp.z + 1), cfg, rng, None);
                self.place_log(BlockPos::new(bp.x + 1, bp.y, bp.z + 1), cfg, rng, None);
            }
        }
        out.push(FoliageAttachment { pos: BlockPos::new(x2, i4, z2), radius_offset: 0, double_trunk: true });
        for i5 in -1..=2 {
            for i6 in -1..=2 {
                if (i5 < 0 || i5 > 1 || i6 < 0 || i6 > 1) && rng.next_int_bound(3) <= 0 {
                    let i7 = rng.next_int_bound(3) + 2;
                    for i8 in 0..i7 {
                        self.place_log(
                            BlockPos::new(pos.x + i5, i4 - i8 - 1, pos.z + i6),
                            cfg,
                            rng,
                            None,
                        );
                    }
                    out.push(FoliageAttachment {
                        pos: BlockPos::new(pos.x + i5, i4, pos.z + i6),
                        radius_offset: 0,
                        double_trunk: false,
                    });
                }
            }
        }
        out
    }

    fn trunk_upwards_branching(
        &mut self,
        t: &TrunkCfg,
        free: i32,
        pos: BlockPos,
        cfg: &TreeCfg,
        rng: &mut WorldgenRandom,
    ) -> Vec<FoliageAttachment> {
        let mut out = Vec::new();
        for i in 0..free {
            let i1 = pos.y + i;
            let placed =
                self.place_log(BlockPos::new(pos.x, i1, pos.z), cfg, rng, None);
            if placed && i < free - 1 && rng.next_f32() < t.ub_place_branch_prob {
                let d = random_horizontal(rng);
                let i2 = t.ub_extra_branch_length.sample(rng);
                let max = 0.max(i2 - t.ub_extra_branch_length.sample(rng) - 1);
                let i3 = t.ub_extra_branch_steps.sample(rng);
                self.place_branch(t, free, cfg, &mut out, pos.x, pos.z, i1, (d.0, d.1), max, i3, rng);
            }
            if i == free - 1 {
                out.push(FoliageAttachment {
                    pos: BlockPos::new(pos.x, i1 + 1, pos.z),
                    radius_offset: 0,
                    double_trunk: false,
                });
            }
        }
        out
    }

    #[allow(clippy::too_many_arguments)]
    fn place_branch(
        &mut self,
        t: &TrunkCfg,
        free: i32,
        cfg: &TreeCfg,
        out: &mut Vec<FoliageAttachment>,
        x0: i32,
        z0: i32,
        y: i32,
        d: (i32, i32),
        extra_len: i32,
        mut extra_steps: i32,
        rng: &mut WorldgenRandom,
    ) {
        let mut i = y + extra_len;
        let mut x = x0;
        let mut z = z0;
        let mut i1 = extra_len;
        while i1 < free && extra_steps > 0 {
            if i1 >= 1 {
                let i2 = y + i1;
                x += d.0;
                z += d.1;
                i = i2;
                let p = BlockPos::new(x, i2, z);
                if self.place_log(p, cfg, rng, None) {
                    i = i2 + 1;
                }
                out.push(FoliageAttachment { pos: p, radius_offset: 0, double_trunk: false });
            }
            i1 += 1;
            extra_steps -= 1;
        }
        if i - y > 1 {
            let bp = BlockPos::new(x, i, z);
            out.push(FoliageAttachment { pos: bp, radius_offset: 0, double_trunk: false });
            out.push(FoliageAttachment {
                pos: BlockPos::new(bp.x, bp.y - 2, bp.z),
                radius_offset: 0,
                double_trunk: false,
            });
        }
    }

    fn trunk_cherry(
        &mut self,
        t: &TrunkCfg,
        free: i32,
        pos: BlockPos,
        cfg: &TreeCfg,
        rng: &mut WorldgenRandom,
    ) -> Vec<FoliageAttachment> {
        self.set_dirt_at(BlockPos::new(pos.x, pos.y - 1, pos.z), cfg, rng);
        let (so_lo, so_hi) = match t.ch_branch_start_off {
            IP::Uniform { lo, hi } => (lo, hi),
            _ => (0, 0),
        };
        let mut max = 0.max(free - 1 + sample_uniform_off(rng, so_lo, so_hi));
        let mut max1 = 0.max(free - 1 + sample_uniform_off(rng, so_lo, so_hi - 1));
        if max1 >= max {
            max1 += 1;
        }
        let i = t.ch_branch_count.sample(rng);
        let flag = i == 3;
        let flag1 = i >= 2;
        let i1 = if flag {
            free
        } else if flag1 {
            max.max(max1) + 1
        } else {
            max + 1
        };
        for i2 in 0..i1 {
            self.place_log(BlockPos::new(pos.x, pos.y + i2, pos.z), cfg, rng, None);
        }
        let mut out = Vec::new();
        if flag {
            out.push(FoliageAttachment { pos: BlockPos::new(pos.x, pos.y + i1, pos.z), radius_offset: 0, double_trunk: false });
        }
        let d = random_horizontal(rng);
        let (dx, dz) = (d.0, d.1);
        let axis = dir_code_axis(d.2);
        let br = self.generate_branch(
            free, pos, cfg, rng, dx, dz, axis, max, max < i1 - 1,
            t.ch_branch_hor_len, t.ch_branch_end_off,
        );
        out.push(br);
        if flag1 {
            let (dx1, dz1, code1) = opposite_dir(d.2);
            let axis1 = dir_code_axis(code1);
            let br1 = self.generate_branch(
                free, pos, cfg, rng, dx1, dz1, axis1, max1, max1 < i1 - 1,
                t.ch_branch_hor_len, t.ch_branch_end_off,
            );
            out.push(br1);
        }
        out
    }

    #[allow(clippy::too_many_arguments)]
    fn generate_branch(
        &mut self,
        free: i32,
        pos: BlockPos,
        cfg: &TreeCfg,
        rng: &mut WorldgenRandom,
        dx: i32,
        dz: i32,
        axis: u8,
        start_off: i32,
        double_branch: bool,
        hor_len: IP,
        end_off: IP,
    ) -> FoliageAttachment {
        let mut cur = BlockPos::new(pos.x, pos.y + start_off, pos.z);
        let i = free - 1 + end_off.sample(rng);
        let flag = double_branch || i < start_off;
        let i1 = hor_len.sample(rng) + if flag { 1 } else { 0 };
        let target = BlockPos::new(pos.x + dx * i1, pos.y + i, pos.z + dz * i1);
        let i2 = if flag { 2 } else { 1 };
        for _ in 0..i2 {
            cur.x += dx;
            cur.z += dz;
            self.place_log(cur, cfg, rng, Some(axis));
        }
        let up_first = target.y > cur.y;
        loop {
            let dist = manhattan(cur, target);
            if dist == 0 {
                return FoliageAttachment {
                    pos: BlockPos::new(target.x, target.y + 1, target.z),
                    radius_offset: 0,
                    double_trunk: false,
                };
            }
            let f = ((target.y - cur.y).abs() as f32) / (dist as f32);
            let flag1 = rng.next_f32() < f;
            if flag1 {
                if up_first {
                    cur.y += 1;
                } else {
                    cur.y -= 1;
                }
            } else {
                cur.x += dx;
                cur.z += dz;
            }
            self.place_log(cur, cfg, rng, if flag1 { None } else { Some(axis) });
        }
    }
}

fn sample_uniform_off(rng: &mut WorldgenRandom, lo: i32, hi: i32) -> i32 {
    rng.next_int_bound((hi - lo + 1).max(1)) + lo
}

fn manhattan(a: BlockPos, b: BlockPos) -> i32 {
    (a.x - b.x).abs() + (a.y - b.y).abs() + (a.z - b.z).abs()
}

/// Plane.HORIZONTAL.getRandomDirection = faces[nextInt(4)]; faces [N,E,S,W].
/// Returns (stepX, stepZ, dirCode).
fn random_horizontal(rng: &mut WorldgenRandom) -> (i32, i32, u8) {
    match HORIZONTAL[rng.next_int_bound(4) as usize] {
        2 => (0, -1, 2),  // NORTH
        5 => (1, 0, 5),   // EAST
        3 => (0, 1, 3),   // SOUTH
        _ => (-1, 0, 4),  // WEST
    }
}

fn opposite_dir(code: u8) -> (i32, i32, u8) {
    match code {
        2 => (0, 1, 3),
        3 => (0, -1, 2),
        4 => (1, 0, 5),
        _ => (-1, 0, 4),
    }
}

/// axis code for FLAG_ROTATED bits: 0=X 1=Y 2=Z
fn dir_code_axis(code: u8) -> u8 {
    match code {
        4 | 5 => 0, // X
        2 | 3 => 2, // Z
        _ => 1,     // Y
    }
}

fn get_log_axis(base: BlockPos, other: BlockPos) -> u8 {
    let ax = (other.x - base.x).abs();
    let az = (other.z - base.z).abs();
    let max = ax.max(az);
    if max > 0 {
        if ax == max {
            0 // X
        } else {
            2 // Z
        }
    } else {
        1 // Y
    }
}

/// FancyTrunkPlacer.treeShape — f32 exact
fn tree_shape(height: i32, current_y: i32) -> f32 {
    if (current_y as f32) < (height as f32) * 0.3 {
        return -1.0;
    }
    let f = (height as f32) / 2.0;
    let f1 = f - (current_y as f32);
    let mut sq = ((f * f - f1 * f1) as f64).sqrt() as f32;
    if f1 == 0.0 {
        sq = f;
    } else if f1.abs() >= f {
        return 0.0;
    }
    sq * 0.5
}

// Java Math.sin/cos on non-f32-promoted f64 inputs: HotSpot intrinsic vs
// glibc may differ in the last ulp; the fancy-trunk floor() boundaries sit
// at transcendental offsets (2^-46 hit chance) — plain libm is used.
#[inline]
fn java_math_sin(d: f64) -> f64 {
    d.sin()
}

#[inline]
fn java_math_cos(d: f64) -> f64 {
    d.cos()
}

/// Mth.cos(float) = (float)Math.cos((double)f) — f32-promoted domain (pinned
/// by the sincos correction table)
#[inline]
fn mth_cos(f: f32) -> f32 {
    crate::features_math::java_cos(f as f64) as f32
}

#[inline]
fn mth_sin(f: f32) -> f32 {
    crate::features_math::java_sin(f as f64) as f32
}

// ---------------------------------------------------------------------------
// Foliage placers
// ---------------------------------------------------------------------------

impl<'a, 'b> TreeRunner<'a, 'b> {
    fn place_leaves_row(
        &mut self,
        pos: BlockPos,
        range: i32,
        local_y: i32,
        large: bool,
        cfg: &TreeCfg,
        rng: &mut WorldgenRandom,
    ) {
        let ext = if large { 1 } else { 0 };
        let mut i1 = -range;
        while i1 <= range + ext {
            let mut i2 = -range;
            while i2 <= range + ext {
                if !self.should_skip_signed(cfg, rng, i1, local_y, i2, range, large) {
                    let p = BlockPos::new(pos.x + i1, pos.y + local_y, pos.z + i2);
                    self.try_place_leaf(p, cfg, rng);
                }
                i2 += 1;
            }
            i1 += 1;
        }
    }

    fn should_skip_signed(
        &mut self,
        cfg: &TreeCfg,
        rng: &mut WorldgenRandom,
        local_x: i32,
        local_y: i32,
        local_z: i32,
        range: i32,
        large: bool,
    ) -> bool {
        // DarkOak overrides shouldSkipLocationSigned with RAW signed coords
        if cfg.foliage_placer.ty == FoliageTy::DarkOak {
            if local_y == 0
                && large
                && (local_x == -range || local_x >= range)
                && (local_z == -range || local_z >= range)
            {
                return true;
            }
        }
        let (mx, mz) = if large {
            (local_x.abs().min((local_x - 1).abs()), local_z.abs().min((local_z - 1).abs()))
        } else {
            (local_x.abs(), local_z.abs())
        };
        should_skip_location(cfg, rng, mx, local_y, mz, range, large)
    }
}

fn should_skip_location(
    cfg: &TreeCfg,
    rng: &mut WorldgenRandom,
    lx: i32,
    ly: i32,
    lz: i32,
    range: i32,
    large: bool,
) -> bool {
    match cfg.foliage_placer.ty {
        FoliageTy::Blob => lx == range && lz == range && (rng.next_int_bound(2) == 0 || ly == 0),
        FoliageTy::Fancy => {
            ((lx as f32) + 0.5).powi(2) + ((lz as f32) + 0.5).powi(2) > (range * range) as f32
        }
        FoliageTy::Bush => lx == range && lz == range && rng.next_int_bound(2) == 0,
        FoliageTy::Spruce | FoliageTy::Pine => {
            lx == range && lz == range && range > 0
        }
        FoliageTy::MegaPine => lx + lz >= 7 || lx * lx + lz * lz > range * range,
        FoliageTy::Acacia => {
            if ly == 0 {
                (lx > 1 || lz > 1) && lx != 0 && lz != 0
            } else {
                lx == range && lz == range && range > 0
            }
        }
        FoliageTy::MegaJungle => lx + lz >= 7 || lx * lx + lz * lz > range * range,
        FoliageTy::DarkOak => {
            if ly == -1 && !large {
                lx == range && lz == range
            } else {
                ly == 1 && lx + lz > range * 2 - 2
            }
        }
        FoliageTy::RandomSpread => false,
        FoliageTy::Cherry => {
            if ly == -1 && (lx == range || lz == range) && rng.next_f32() < cfg.foliage_placer.wide_bottom_hole {
                return true;
            }
            let flag = lx == range && lz == range;
            let flag1 = range > 2;
            if flag1 {
                flag || (lx + lz > range * 2 - 2 && rng.next_f32() < cfg.foliage_placer.corner_hole)
            } else {
                flag && rng.next_f32() < cfg.foliage_placer.corner_hole
            }
        }
    }
}

impl<'a, 'b> TreeRunner<'a, 'b> {
    /// CherryFoliagePlacer hanging-leaves helper (placeLeavesRowWithHangingLeavesBelow)
    #[allow(clippy::too_many_arguments)]
    fn leaves_row_hanging(
        &mut self,
        pos: BlockPos,
        range: i32,
        local_y: i32,
        large: bool,
        chance: f32,
        ext_chance: f32,
        cfg: &TreeCfg,
        rng: &mut WorldgenRandom,
    ) {
        self.place_leaves_row(pos, range, local_y, large, cfg, rng);
        let ext = if large { 1 } else { 0 };
        let log_pos = BlockPos::new(pos.x, pos.y - 1, pos.z);
        for &dir in HORIZONTAL.iter() {
            let cw = clock_wise(dir);
            let i1 = if cw_is_positive(cw) { range + ext } else { range };
            // start: pos + (0, localY-1, 0).move(cw, i1).move(dir, -range)
            let s3 = dir_step(dir);
            let c3 = dir_step(cw);
            let (sdx, sdz) = (s3.0, s3.2);
            let (cdx, cdz) = (c3.0, c3.2);
            let mut mp = BlockPos::new(
                pos.x + cdx * i1 + sdx * -range,
                pos.y + local_y - 1,
                pos.z + cdz * i1 + sdz * -range,
            );
            let mut i2 = -range;
            while i2 < range + ext {
                mp.y += 1;
                let is_set = self.leaves.contains(mp);
                mp.y -= 1;
                if is_set
                    && manhattan(mp, log_pos) < 7
                    && !(rng.next_f32() > chance)
                    && self.try_place_leaf(mp, cfg, rng)
                {
                    mp.y -= 1;
                    let _ = manhattan(mp, log_pos) < 7
                        && !(rng.next_f32() > ext_chance)
                        && self.try_place_leaf(mp, cfg, rng);
                    mp.y += 1;
                }
                i2 += 1;
                mp.x += sdx;
                mp.z += sdz;
            }
        }
    }
}

fn clock_wise(d: u8) -> u8 {
    match d {
        2 => 5, // N -> E
        5 => 3, // E -> S
        3 => 4, // S -> W
        _ => 2, // W -> N
    }
}

fn cw_is_positive(d: u8) -> bool {
    d == 5 || d == 3 // EAST, SOUTH
}

fn create_foliage(
    t: &mut TreeRunner,
    cfg: &TreeCfg,
    _max_free: i32,
    att: FoliageAttachment,
    fh: i32,
    fr: i32,
    rng: &mut WorldgenRandom,
) {
    let offset = cfg.foliage_placer.offset.sample(rng);
    let ro = att.radius_offset;
    let dt = att.double_trunk;
    match cfg.foliage_placer.ty {
        FoliageTy::Blob => {
            let mut i = offset;
            while i >= offset - fh {
                let r = (fr + ro - 1 - i / 2).max(0);
                t.place_leaves_row(att.pos, r, i, dt, cfg, rng);
                i -= 1;
            }
        }
        FoliageTy::Fancy => {
            let mut i = offset;
            while i >= offset - fh {
                let r = fr + if i != offset && i != offset - fh { 1 } else { 0 };
                t.place_leaves_row(att.pos, r, i, dt, cfg, rng);
                i -= 1;
            }
        }
        FoliageTy::Bush => {
            let mut i = offset;
            while i >= offset - fh {
                let r = fr + ro - 1 - i;
                t.place_leaves_row(att.pos, r, i, dt, cfg, rng);
                i -= 1;
            }
        }
        FoliageTy::Spruce => {
            let mut random_int = rng.next_int_bound(2);
            let mut i = 1;
            let mut i1 = 0;
            let mut i2 = offset;
            while i2 >= -fh {
                t.place_leaves_row(att.pos, random_int, i2, dt, cfg, rng);
                if random_int >= i {
                    random_int = i1;
                    i1 = 1;
                    i = (i + 1).min(fr + ro);
                } else {
                    random_int += 1;
                }
                i2 -= 1;
            }
        }
        FoliageTy::Pine => {
            let mut i = 0;
            let mut i1 = offset;
            while i1 >= offset - fh {
                t.place_leaves_row(att.pos, i, i1, dt, cfg, rng);
                if i >= 1 && i1 == offset - fh + 1 {
                    i -= 1;
                } else if i < fr + ro {
                    i += 1;
                }
                i1 -= 1;
            }
        }
        FoliageTy::MegaPine => {
            let mut i = 0;
            let mut y = att.pos.y - fh + offset;
            while y <= att.pos.y + offset {
                let i2 = att.pos.y - y;
                let i3 = fr + ro + (((i2 as f32) / (fh as f32)) * 3.5f32).floor() as i32;
                let i4 = if i2 > 0 && i3 == i && (y & 1) == 0 { i3 + 1 } else { i3 };
                t.place_leaves_row(
                    BlockPos::new(att.pos.x, y, att.pos.z),
                    i4,
                    0,
                    dt,
                    cfg,
                    rng,
                );
                i = i3;
                y += 1;
            }
        }
        FoliageTy::Acacia => {
            let bp = BlockPos::new(att.pos.x, att.pos.y + offset, att.pos.z);
            t.place_leaves_row(bp, fr + ro, -1 - fh, dt, cfg, rng);
            t.place_leaves_row(bp, fr - 1, -fh, dt, cfg, rng);
            t.place_leaves_row(bp, fr + ro - 1, 0, dt, cfg, rng);
        }
        FoliageTy::MegaJungle => {
            let i = if dt { fh } else { 1 + rng.next_int_bound(2) };
            let mut i1 = offset;
            while i1 >= offset - i {
                let r = fr + ro + 1 - i1;
                t.place_leaves_row(att.pos, r, i1, dt, cfg, rng);
                i1 -= 1;
            }
        }
        FoliageTy::DarkOak => {
            let bp = BlockPos::new(att.pos.x, att.pos.y + offset, att.pos.z);
            if dt {
                t.place_leaves_row(bp, fr + 2, -1, dt, cfg, rng);
                t.place_leaves_row(bp, fr + 3, 0, dt, cfg, rng);
                t.place_leaves_row(bp, fr + 2, 1, dt, cfg, rng);
                if rng.next_bool() {
                    t.place_leaves_row(bp, fr, 2, dt, cfg, rng);
                }
            } else {
                t.place_leaves_row(bp, fr + 2, -1, dt, cfg, rng);
                t.place_leaves_row(bp, fr + 1, 0, dt, cfg, rng);
            }
        }
        FoliageTy::RandomSpread => {
            let attempts = cfg.foliage_placer.leaf_attempts;
            let fr2 = fr.max(1);
            let fh2 = fh.max(1);
            for _ in 0..attempts {
                let dx = rng.next_int_bound(fr2) - rng.next_int_bound(fr2);
                let dy = rng.next_int_bound(fh2) - rng.next_int_bound(fh2);
                let dz = rng.next_int_bound(fr2) - rng.next_int_bound(fr2);
                let p = BlockPos::new(att.pos.x + dx, att.pos.y + dy, att.pos.z + dz);
                t.try_place_leaf(p, cfg, rng);
            }
        }
        FoliageTy::Cherry => {
            let bp = BlockPos::new(att.pos.x, att.pos.y + offset, att.pos.z);
            let i = fr + ro - 1;
            t.place_leaves_row(bp, i - 2, fh - 3, dt, cfg, rng);
            t.place_leaves_row(bp, i - 1, fh - 4, dt, cfg, rng);
            let mut i1 = fh - 5;
            while i1 >= 0 {
                t.place_leaves_row(bp, i, i1, dt, cfg, rng);
                i1 -= 1;
            }
            let hc = cfg.foliage_placer.hanging_leaves;
            let he = cfg.foliage_placer.hanging_ext;
            t.leaves_row_hanging(bp, i, -1, dt, hc, he, cfg, rng);
            t.leaves_row_hanging(bp, i - 1, -2, dt, hc, he, cfg, rng);
        }
    }
}

// ---------------------------------------------------------------------------
// Root placer (mangrove)
// ---------------------------------------------------------------------------

impl<'a, 'b> TreeRunner<'a, 'b> {
    fn can_place_root(&self, rp: &RootCfg, p: BlockPos) -> bool {
        valid_tree_pos(self.ctx.level, p) || grow_set_matches(self.ctx.level, p, &rp.can_grow_through)
    }

    fn place_roots(
        &mut self,
        rp: &RootCfg,
        pos: BlockPos,
        trunk_origin: BlockPos,
        cfg: &TreeCfg,
        rng: &mut WorldgenRandom,
    ) -> bool {
        let mut cur = pos;
        while cur.y < trunk_origin.y {
            if !self.can_place_root(rp, cur) {
                return false;
            }
            cur.y += 1;
        }
        let mut list: Vec<BlockPos> = vec![BlockPos::new(trunk_origin.x, trunk_origin.y - 1, trunk_origin.z)];
        for &dir in HORIZONTAL.iter() {
            let (dx, dz) = match dir {
                2 => (0, -1),
                5 => (1, 0),
                3 => (0, 1),
                _ => (-1, 0),
            };
            let bp = BlockPos::new(trunk_origin.x + dx, trunk_origin.y, trunk_origin.z + dz);
            let mut roots = Vec::new();
            if !self.simulate_roots(rp, bp, dir, trunk_origin, &mut roots, 0, rng) {
                return false;
            }
            list.extend(roots);
            list.push(bp);
        }
        for p in list {
            self.place_root(rp, p, cfg, rng);
        }
        true
    }

    fn simulate_roots(
        &mut self,
        rp: &RootCfg,
        pos: BlockPos,
        dir: u8,
        trunk_origin: BlockPos,
        roots: &mut Vec<BlockPos>,
        length: i32,
        rng: &mut WorldgenRandom,
    ) -> bool {
        if length == rp.max_root_length || roots.len() as i32 > rp.max_root_length {
            return false;
        }
        for p in self.potential_root_positions(rp, pos, dir, trunk_origin, rng) {
            if self.can_place_root(rp, p) {
                roots.push(p);
                if !self.simulate_roots(rp, p, dir, trunk_origin, roots, length + 1, rng) {
                    return false;
                }
            }
        }
        true
    }

    fn potential_root_positions(
        &self,
        rp: &RootCfg,
        pos: BlockPos,
        dir: u8,
        trunk_origin: BlockPos,
        rng: &mut WorldgenRandom,
    ) -> Vec<BlockPos> {
        let below = BlockPos::new(pos.x, pos.y - 1, pos.z);
        let (dx, dz) = match dir {
            2 => (0, -1),
            5 => (1, 0),
            3 => (0, 1),
            _ => (-1, 0),
        };
        let side = BlockPos::new(pos.x + dx, pos.y, pos.z + dz);
        let dist = manhattan(pos, trunk_origin);
        let i = dist;
        let maxw = rp.max_root_width;
        let skew = rp.random_skew_chance;
        if i > maxw - 3 && i <= maxw {
            if rng.next_f32() < skew {
                vec![below, BlockPos::new(side.x, side.y - 1, side.z)]
            } else {
                vec![below]
            }
        } else if i > maxw {
            vec![below]
        } else if rng.next_f32() < skew {
            vec![below]
        } else if rng.next_bool() {
            vec![side]
        } else {
            vec![below]
        }
    }

    fn place_root(&mut self, rp: &RootCfg, pos: BlockPos, cfg: &TreeCfg, rng: &mut WorldgenRandom) {
        let level = self.ctx.level;
        // muddy override first
        if grow_set_matches(level, pos, &rp.muddy_roots_in) {
            let mut flags = 0u8;
            if let Some(st) = sample_state(&rp.muddy_roots_provider, level, pos, rng, &mut flags) {
                let wl = has_waterlogged_prop(&st)
                    && level.fluid_is(pos, &["minecraft:water".to_string(), "minecraft:flowing_water".to_string()]);
                self.write_root(pos, &st, wl);
            }
            return;
        }
        if !valid_tree_pos(level, pos) {
            return;
        }
        let mut flags = 0u8;
        if let Some(st) = sample_state(&rp.root_provider, level, pos, rng, &mut flags) {
            let wl = has_waterlogged_prop(&st)
                && level.fluid_is(pos, &["minecraft:water".to_string(), "minecraft:flowing_water".to_string()]);
            self.write_root(pos, &st, wl);
            if let Some((above_prov, chance)) = &rp.above_root {
                let ap = BlockPos::new(pos.x, pos.y + 1, pos.z);
                if rng.next_f32() < *chance && level.is_air(ap) {
                    let mut f2 = 0u8;
                    if let Some(st2) = sample_state(above_prov, level, ap, rng, &mut f2) {
                        let wl2 = has_waterlogged_prop(&st2)
                            && level.fluid_is(ap, &["minecraft:water".to_string(), "minecraft:flowing_water".to_string()]);
                        self.write_root(ap, &st2, wl2);
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tree decorators
// ---------------------------------------------------------------------------

const VINE_FACE: [&str; 6] = ["down", "up", "north", "south", "west", "east"];

fn vine_state(dir: u8) -> String {
    // the face property names the side the vine ATTACHES THROUGH (points at
    // the support): placing at west with the leaf to the east -> east=true.
    // Full canonical 5-prop form (vine has north/east/south/west/up).
    let (n, e, so, w, u) = match dir {
        2 => ("true", "false", "false", "false", "false"), // north
        5 => ("false", "true", "false", "false", "false"), // east
        3 => ("false", "false", "true", "false", "false"), // south
        4 => ("false", "false", "false", "true", "false"), // west
        _ => ("false", "false", "false", "false", "true"), // up
    };
    format!("minecraft:vine[north={n},east={e},south={so},west={w},up={u}]")
}

impl<'a, 'b> TreeRunner<'a, 'b> {
    fn logs_sorted(&self) -> Vec<BlockPos> {
        sorted_by_y(self.logs.order())
    }
    fn leaves_sorted(&self) -> Vec<BlockPos> {
        sorted_by_y(self.leaves.order())
    }
    fn roots_sorted(&self) -> Vec<BlockPos> {
        sorted_by_y(self.roots.order())
    }

    /// TreeFeature.getLowestTrunkOrRootOfTree
    fn lowest_trunk_or_root(&self) -> Vec<BlockPos> {
        let r = self.roots_sorted();
        let l = self.logs_sorted();
        if r.is_empty() {
            l
        } else if !l.is_empty() && r[0].y == l[0].y {
            let mut v = l;
            v.extend(r);
            v
        } else {
            r
        }
    }

    pub fn run_decorators(&mut self, cfg: &TreeCfg, rng: &mut WorldgenRandom) {
        for d in &cfg.decorators {
            match d {
                TreeDeco::LeaveVine(p) => self.deco_leave_vine(*p, rng),
                TreeDeco::TrunkVine => self.deco_trunk_vine(rng),
                TreeDeco::Beehive(p) => self.deco_beehive(*p, rng),
                TreeDeco::Cocoa(p) => self.deco_cocoa(*p, rng),
                TreeDeco::AlterGround(prov) => self.deco_alter_ground(prov, rng),
                TreeDeco::AttachedToLogs { probability, provider, dirs } => {
                    self.deco_attached_logs(*probability, provider, dirs, rng)
                }
                TreeDeco::AttachedToLeaves { probability, excl_xz, excl_y, provider, req_empty, dirs } => {
                    self.deco_attached_leaves(*probability, *excl_xz, *excl_y, provider, *req_empty, dirs, rng)
                }
                TreeDeco::PlaceOnGround { tries, radius, height, provider } => {
                    self.deco_place_on_ground(*tries, *radius, *height, provider, rng)
                }
                TreeDeco::PaleMoss { leaves, trunk, ground } => {
                    self.deco_pale_moss(*leaves, *trunk, *ground, rng)
                }
                TreeDeco::CreakingHeart(p) => self.deco_creaking_heart(*p, rng),
            }
        }
    }

    fn place_vine_at(&mut self, pos: BlockPos, face: u8) {
        // face = the direction property set true (points AT the support)
        self.write_deco(pos, &vine_state(face));
    }

    fn deco_leave_vine(&mut self, probability: f32, rng: &mut WorldgenRandom) {
        let leaves = self.leaves_sorted();
        for pos in leaves {
            // west: leaf support is to the EAST
            if rng.next_f32() < probability {
                let p = BlockPos::new(pos.x - 1, pos.y, pos.z);
                if self.ctx.level.is_air(p) {
                    self.hanging_vine(p, 5, rng);
                }
            }
            if rng.next_f32() < probability {
                let p = BlockPos::new(pos.x + 1, pos.y, pos.z);
                if self.ctx.level.is_air(p) {
                    self.hanging_vine(p, 4, rng);
                }
            }
            if rng.next_f32() < probability {
                let p = BlockPos::new(pos.x, pos.y, pos.z - 1);
                if self.ctx.level.is_air(p) {
                    self.hanging_vine(p, 3, rng);
                }
            }
            if rng.next_f32() < probability {
                let p = BlockPos::new(pos.x, pos.y, pos.z + 1);
                if self.ctx.level.is_air(p) {
                    self.hanging_vine(p, 2, rng);
                }
            }
        }
    }

    fn hanging_vine(&mut self, pos: BlockPos, face: u8, _rng: &mut WorldgenRandom) {
        self.place_vine_at(pos, face);
        let mut i = 4;
        let mut p = BlockPos::new(pos.x, pos.y - 1, pos.z);
        while self.ctx.level.is_air(p) && i > 0 {
            self.place_vine_at(p, face);
            p.y -= 1;
            i -= 1;
        }
    }

    fn deco_trunk_vine(&mut self, rng: &mut WorldgenRandom) {
        let logs = self.logs_sorted();
        for pos in logs {
            if rng.next_int_bound(3) > 0 {
                let p = BlockPos::new(pos.x - 1, pos.y, pos.z);
                if self.ctx.level.is_air(p) {
                    self.place_vine_at(p, 5);
                }
            }
            if rng.next_int_bound(3) > 0 {
                let p = BlockPos::new(pos.x + 1, pos.y, pos.z);
                if self.ctx.level.is_air(p) {
                    self.place_vine_at(p, 4);
                }
            }
            if rng.next_int_bound(3) > 0 {
                let p = BlockPos::new(pos.x, pos.y, pos.z - 1);
                if self.ctx.level.is_air(p) {
                    self.place_vine_at(p, 3);
                }
            }
            if rng.next_int_bound(3) > 0 {
                let p = BlockPos::new(pos.x, pos.y, pos.z + 1);
                if self.ctx.level.is_air(p) {
                    self.place_vine_at(p, 2);
                }
            }
        }
    }

    fn deco_beehive(&mut self, probability: f32, rng: &mut WorldgenRandom) {
        let leaves = self.leaves_sorted();
        let logs = self.logs_sorted();
        if logs.is_empty() {
            return;
        }
        if rng.next_f32() >= probability {
            return;
        }
        let i = if !leaves.is_empty() {
            (leaves[0].y - 1).max(logs[0].y + 1)
        } else {
            (logs[0].y + 1 + rng.next_int_bound(3)).min(logs[logs.len() - 1].y)
        };
        // SPAWN_DIRECTIONS = [EAST, SOUTH, WEST] (horizontal minus NORTH)
        let mut list2: Vec<BlockPos> = Vec::new();
        for l in &logs {
            if l.y == i {
                list2.push(BlockPos::new(l.x + 1, l.y, l.z)); // EAST
                list2.push(BlockPos::new(l.x, l.y, l.z + 1)); // SOUTH
                list2.push(BlockPos::new(l.x - 1, l.y, l.z)); // WEST
            }
        }
        if list2.is_empty() {
            return;
        }
        util_shuffle(&mut list2, rng);
        for p in list2 {
            let south = BlockPos::new(p.x, p.y, p.z + 1);
            if self.ctx.level.is_air(p) && self.ctx.level.is_air(south) {
                self.write_deco(p, "minecraft:bee_nest[facing=south,honey_level=0]");
                return;
            }
        }
    }

    fn deco_cocoa(&mut self, probability: f32, rng: &mut WorldgenRandom) {
        let logs = self.logs_sorted();
        if logs.is_empty() || rng.next_f32() >= probability {
            return;
        }
        let y = logs[0].y;
        for pos in logs.iter().filter(|p| p.y - y <= 2) {
            for &dir in HORIZONTAL.iter() {
                if rng.next_f32() <= 0.25 {
                    let (ox, oz) = opposite_step(dir);
                    let p = BlockPos::new(pos.x + ox, pos.y, pos.z + oz);
                    if self.ctx.level.is_air(p) {
                        let age = rng.next_int_bound(3);
                        let facing = match dir {
                            2 => "north",
                            5 => "east",
                            3 => "south",
                            _ => "west",
                        };
                        self.write_deco(
                            p,
                            &format!("minecraft:cocoa[age={},facing={}]", age, facing),
                        );
                    }
                }
            }
        }
    }

    fn deco_alter_ground(&mut self, prov: &crate::features_json::StateSrc, rng: &mut WorldgenRandom) {
        let lowest = self.lowest_trunk_or_root();
        if lowest.is_empty() {
            return;
        }
        let y = lowest[0].y;
        for pos in lowest.iter().filter(|p| p.y == y) {
            self.alter_circle(BlockPos::new(pos.x - 1, pos.y, pos.z - 1), prov, rng);
            self.alter_circle(BlockPos::new(pos.x + 2, pos.y, pos.z - 1), prov, rng);
            self.alter_circle(BlockPos::new(pos.x - 1, pos.y, pos.z + 2), prov, rng);
            self.alter_circle(BlockPos::new(pos.x + 2, pos.y, pos.z + 2), prov, rng);
            for _ in 0..5 {
                let r = rng.next_int_bound(64);
                let i1 = r % 8;
                let i2 = r / 8;
                if i1 == 0 || i1 == 7 || i2 == 0 || i2 == 7 {
                    self.alter_circle(BlockPos::new(pos.x - 3 + i1, pos.y, pos.z - 3 + i2), prov, rng);
                }
            }
        }
    }

    fn alter_circle(&mut self, pos: BlockPos, prov: &crate::features_json::StateSrc, rng: &mut WorldgenRandom) {
        for i in -2i32..=2 {
            for i1 in -2i32..=2 {
                if i.abs() != 2 || i1.abs() != 2 {
                    self.alter_block_at(BlockPos::new(pos.x + i, pos.y, pos.z + i1), prov, rng);
                }
            }
        }
    }

    fn alter_block_at(&mut self, pos: BlockPos, prov: &crate::features_json::StateSrc, rng: &mut WorldgenRandom) {
        let mut i = 2;
        while i >= -3 {
            let bp = BlockPos::new(pos.x, pos.y + i, pos.z);
            if is_grass_or_dirt(self.ctx.level, bp) {
                let mut flags = 0u8;
                if let Some(st) = sample_state(prov, self.ctx.level, pos, rng, &mut flags) {
                    self.write_deco(bp, &st);
                }
                return;
            }
            if !self.ctx.level.is_air(bp) && i < 0 {
                return;
            }
            i -= 1;
        }
    }

    fn deco_attached_logs(
        &mut self,
        probability: f32,
        provider: &crate::features_json::StateSrc,
        dirs: &[u8],
        rng: &mut WorldgenRandom,
    ) {
        let logs = self.logs_sorted();
        let shuffled = shuffled_copy(logs, rng);
        for pos in shuffled {
            let dir = dirs[rng.next_int_bound(dirs.len() as i32) as usize];
            let (dx, dy, dz) = DIRS6[dir as usize];
            let p = BlockPos::new(pos.x + dx, pos.y + dy, pos.z + dz);
            if rng.next_f32() <= probability && self.ctx.level.is_air(p) {
                let mut flags = 0u8;
                if let Some(st) = sample_state(provider, self.ctx.level, p, rng, &mut flags) {
                    self.write_deco(p, &st);
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn deco_attached_leaves(
        &mut self,
        probability: f32,
        excl_xz: i32,
        excl_y: i32,
        provider: &crate::features_json::StateSrc,
        req_empty: i32,
        dirs: &[u8],
        rng: &mut WorldgenRandom,
    ) {
        let leaves = self.leaves_sorted();
        let shuffled = shuffled_copy(leaves, rng);
        let mut excluded: std::collections::HashSet<(i32, i32, i32)> = std::collections::HashSet::new();
        for pos in shuffled {
            let dir = dirs[rng.next_int_bound(dirs.len() as i32) as usize];
            let (dx, dy, dz) = DIRS6[dir as usize];
            let p = BlockPos::new(pos.x + dx, pos.y + dy, pos.z + dz);
            if !excluded.contains(&(p.x, p.y, p.z))
                && rng.next_f32() < probability
                && self.has_required_empty(pos, dir, req_empty)
            {
                for ex in -excl_xz..=excl_xz {
                    for ey in -excl_y..=excl_y {
                        for ez in -excl_xz..=excl_xz {
                            excluded.insert((p.x + ex, p.y + ey, p.z + ez));
                        }
                    }
                }
                let mut flags = 0u8;
                if let Some(st) = sample_state(provider, self.ctx.level, p, rng, &mut flags) {
                    self.write_deco(p, &st);
                }
            }
        }
    }

    fn has_required_empty(&self, pos: BlockPos, dir: u8, req: i32) -> bool {
        let (dx, dy, dz) = DIRS6[dir as usize];
        for i in 1..=req {
            let p = BlockPos::new(pos.x + dx * i, pos.y + dy * i, pos.z + dz * i);
            if !self.ctx.level.is_air(p) {
                return false;
            }
        }
        true
    }

    fn deco_place_on_ground(
        &mut self,
        tries: i32,
        radius: i32,
        height: i32,
        provider: &crate::features_json::StateSrc,
        rng: &mut WorldgenRandom,
    ) {
        let lowest = self.lowest_trunk_or_root();
        if lowest.is_empty() {
            return;
        }
        let first = lowest[0];
        let mut x = first.x;
        let mut x1 = first.x;
        let mut z = first.z;
        let mut z1 = first.z;
        for p in &lowest {
            if p.y == first.y {
                x = x.min(p.x);
                x1 = x1.max(p.x);
                z = z.min(p.z);
                z1 = z1.max(p.z);
            }
        }
        let min_x = x - radius;
        let max_x = x1 + radius;
        let min_y = first.y - height;
        let max_y = first.y + height;
        let min_z = z - radius;
        let max_z = z1 + radius;
        for _ in 0..tries {
            let px = rng.next_int_bound(max_x - min_x + 1) + min_x;
            let py = rng.next_int_bound(max_y - min_y + 1) + min_y;
            let pz = rng.next_int_bound(max_z - min_z + 1) + min_z;
            let pos = BlockPos::new(px, py, pz);
            let above = BlockPos::new(px, py + 1, pz);
            let air_or_vine = self.ctx.level.is_air(above)
                || self.ctx.level.state_is(above, &["minecraft:vine".to_string()]);
            if air_or_vine
                && self.ctx.level.is_solid_render(pos)
                && self.ctx.level.height(crate::features_deco::HeightmapTy::MotionBlockingNoLeaves, px, pz)
                    <= above.y
            {
                let mut flags = 0u8;
                if let Some(st) = sample_state(provider, self.ctx.level, above, rng, &mut flags) {
                    self.write_deco(above, &st);
                }
            }
        }
    }

    fn deco_pale_moss(&mut self, leaves: f32, trunk: f32, ground: f32, rng: &mut WorldgenRandom) {
        let logs = self.logs_sorted();
        let shuffled = shuffled_copy(logs, rng);
        if shuffled.is_empty() {
            return;
        }
        let mut lowest = shuffled[0];
        for p in &shuffled {
            if p.y < lowest.y {
                lowest = *p;
            }
        }
        if rng.next_f32() < ground {
            // vanilla places the built-in pale_moss_patch configured feature
            let _ = crate::features_place::place_configured(
                self.ctx,
                "minecraft:pale_moss_patch",
                rng,
                BlockPos::new(lowest.x, lowest.y + 1, lowest.z),
                self.ops,
                1,
            );
        }
        let logs = self.logs_sorted();
        for pos in logs {
            if rng.next_f32() < trunk {
                let p = BlockPos::new(pos.x, pos.y - 1, pos.z);
                if self.ctx.level.is_air(p) {
                    self.moss_hanger(p, rng);
                }
            }
        }
        let leaves_list = self.leaves_sorted();
        for pos in leaves_list {
            if rng.next_f32() < leaves {
                let p = BlockPos::new(pos.x, pos.y - 1, pos.z);
                if self.ctx.level.is_air(p) {
                    self.moss_hanger(p, rng);
                }
            }
        }
    }

    fn moss_hanger(&mut self, pos: BlockPos, rng: &mut WorldgenRandom) {
        let mut p = pos;
        while self.ctx.level.is_air(BlockPos::new(p.x, p.y - 1, p.z))
            && !((rng.next_f32() as f64) < 0.5)
        {
            self.write_deco(p, "minecraft:pale_hanging_moss[tip=false]");
            p.y -= 1;
        }
        self.write_deco(p, "minecraft:pale_hanging_moss[tip=true]");
    }

    fn deco_creaking_heart(&mut self, probability: f32, rng: &mut WorldgenRandom) {
        let logs = self.logs_sorted();
        if logs.is_empty() || rng.next_f32() >= probability {
            return;
        }
        let mut shuffled = logs;
        util_shuffle(&mut shuffled, rng);
        for pos in shuffled {
            let all_logs = DIRS6.iter().all(|(dx, dy, dz)| {
                self.ctx
                    .level
                    .tag_is(BlockPos::new(pos.x + dx, pos.y + dy, pos.z + dz), "minecraft:logs")
            });
            if all_logs {
                self.write_deco(
                    pos,
                    "minecraft:creaking_heart[creaking_heart_state=dormant,natural=true]",
                );
                return;
            }
        }
    }
}

fn opposite_step(dir: u8) -> (i32, i32) {
    match dir {
        2 => (0, 1),
        3 => (0, -1),
        4 => (1, 0),
        _ => (-1, 0),
    }
}

/// Util.shuffle: Fisher-Yates descending (list.set(i-1, list.set(j, list.get(i-1))))
fn util_shuffle<T>(v: &mut [T], rng: &mut WorldgenRandom) {
    let size = v.len();
    let mut i = size;
    while i > 1 {
        let j = rng.next_int_bound(i as i32) as usize;
        v.swap(i - 1, j);
        i -= 1;
    }
}

fn shuffled_copy(v: Vec<BlockPos>, rng: &mut WorldgenRandom) -> Vec<BlockPos> {
    let mut c = v;
    util_shuffle(&mut c, rng);
    c
}

// ---------------------------------------------------------------------------
// updateLeaves (BFS distance 1..7) + updateShapeAtEdge (vine recompute)
// ---------------------------------------------------------------------------

fn is_vine_name(s: &str) -> bool {
    crate::features_deco::state_block_name(s) == "minecraft:vine"
}

/// full-cube approximation for isFaceFull support checks (getBlockSupportShape
/// / collision full face). Trees' supports are all full cubes; the excluded
/// set covers the non-full states decoration can produce.
fn is_full_cube(name: &str) -> bool {
    !matches!(
        name,
        "minecraft:air"
            | "minecraft:cave_air"
            | "minecraft:void_air"
            | "minecraft:water"
            | "minecraft:lava"
            | "minecraft:vine"
            | "minecraft:pale_hanging_moss"
            | "minecraft:moss_carpet"
            | "minecraft:cocoa"
            | "minecraft:glow_lichen"
            | "minecraft:mangrove_propagule"
            | "minecraft:pink_petals"
            | "minecraft:wildflowers"
            | "minecraft:snow"
            | "minecraft:short_grass"
            | "minecraft:fern"
            | "minecraft:sea_pickle"
            | "minecraft:pointed_dripstone"
            | "minecraft:chain"
            | "minecraft:lantern"
    )
}

impl<'a, 'b> TreeRunner<'a, 'b> {
    /// current FULL state string at pos (overlay first, then the bridge)
    fn state_full(&self, pos: BlockPos) -> Option<String> {
        if let Some(s) = self.ctx.level.read_overlay(pos) {
            return Some(s);
        }
        let s = self.ctx.level.state_full(pos);
        if s.is_empty() {
            None
        } else {
            Some(s)
        }
    }

    /// updateLeaves: BitSetDiscreteVoxelShape BFS over logs -> leaves.
    /// box = bounding box of roots+logs+leaves+decos; seeds = logs (level 0);
    /// pre-filled = decos ∪ roots.
    pub fn update_leaves(&mut self, rng_unused: ()) {
        let _ = rng_unused;
        // bounding box over all four sets (union iteration order is irrelevant)
        let all: Vec<BlockPos> = self
            .logs
            .order()
            .into_iter()
            .chain(self.leaves.order())
            .chain(self.decos.order())
            .chain(self.roots.order())
            .collect();
        if all.is_empty() {
            return;
        }
        let (mut min_x, mut min_y, mut min_z) = (i32::MAX, i32::MAX, i32::MAX);
        let (mut max_x, mut max_y, mut max_z) = (i32::MIN, i32::MIN, i32::MIN);
        for p in &all {
            min_x = min_x.min(p.x);
            min_y = min_y.min(p.y);
            min_z = min_z.min(p.z);
            max_x = max_x.max(p.x);
            max_y = max_y.max(p.y);
            max_z = max_z.max(p.z);
        }
        let sx = (max_x - min_x + 1) as usize;
        let sy = (max_y - min_y + 1) as usize;
        let sz = (max_z - min_z + 1) as usize;
        let idx = |p: BlockPos| -> Option<usize> {
            if p.x < min_x || p.x > max_x || p.y < min_y || p.y > max_y || p.z < min_z || p.z > max_z {
                return None;
            }
            Some(
                ((p.z - min_z) as usize) * sy * sx
                    + ((p.y - min_y) as usize) * sx
                    + ((p.x - min_x) as usize),
            )
        };
        let mut shape = vec![false; sx * sy * sz];
        // pre-fill: trunkPositions=decos, foliagePositions=roots (call-arg order)
        for p in self.decos.order().into_iter().chain(self.roots.order()) {
            if let Some(i) = idx(p) {
                shape[i] = true;
            }
        }
        // BFS queues — vanilla uses List<HashSet<BlockPos>> per level: the
        // pop order within a level is the JAVA HASH order, not insertion
        let mut list: [JSetSim; 7] = Default::default();
        for p in self.logs.order() {
            list[0].add(p);
        }
        let mut i2: usize = 0;
        'outer: loop {
            while i2 < 7 && !list[i2].is_empty() {
                let bp = list[i2].pop_first().unwrap();
                if idx(bp).is_some() {
                    if i2 != 0 {
                        // setBlockKnownShape(state.setValue(DISTANCE, i2)) —
                        // vanilla writes UNCONDITIONALLY (even same-value)
                        if let Some(st) = self.state_full(bp) {
                            let new_st = set_prop(&st, "distance", &i2.to_string());
                            self.emit(bp, &new_st, 0, 0);
                        }
                    }
                    if let Some(i) = idx(bp) {
                        shape[i] = true;
                    }
                    for (dx, dy, dz) in DIRS6 {
                        let np = BlockPos::new(bp.x + dx, bp.y + dy, bp.z + dz);
                        if let Some(j) = idx(np) {
                            if !shape[j] {
                                if let Some(st) = self.state_full(np) {
                                    if let Some(d) = optional_distance_world(&st) {
                                        let min = d.min(i2 as i32 + 1);
                                        if min < 7 {
                                            list[min as usize].add(np);
                                            i2 = (i2 as i32).min(min) as usize;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            if i2 >= 7 {
                break 'outer;
            }
            i2 += 1;
        }
    }

    /// StructureTemplate.updateShapeAtEdge(3): for every shape boundary face,
    /// recompute both sides' states. In our state set only vines change
    /// (leaves schedule a tick and return the same state).
    /// forAllFaces order (pinned from decompile):
    ///   pass1 NONE: x-major, y-minor, walk z -> NORTH/SOUTH faces
    ///   pass2 FORWARD.inverse: z-major, x-minor, walk y -> DOWN/UP faces
    ///   pass3 BACKWARD.inverse: y-major, z-minor, walk x -> WEST/EAST faces
    pub fn update_shape_at_edge(&mut self) {
        let cells: Vec<BlockPos> = self
            .logs
            .order()
            .into_iter()
            .chain(self.leaves.order())
            .chain(self.decos.order())
            .chain(self.roots.order())
            .collect();
        let mut shape: std::collections::HashSet<(i32, i32, i32)> = std::collections::HashSet::new();
        for p in cells {
            shape.insert((p.x, p.y, p.z));
        }
        let (mut min_x, mut min_y, mut min_z) = (i32::MAX, i32::MAX, i32::MAX);
        let (mut max_x, mut max_y, mut max_z) = (i32::MIN, i32::MIN, i32::MIN);
        for &(x, y, z) in &shape {
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            min_z = min_z.min(z);
            max_x = max_x.max(x);
            max_y = max_y.max(y);
            max_z = max_z.max(z);
        }
        if min_x == i32::MAX {
            return;
        }
        // per-pass face walks in vanilla forAllFaces order; each event is
        // applied immediately (later recomputes see earlier writes)
        macro_rules! walk {
            ($a0:expr, $a1:expr, $c_range:expr, $neg:expr, $cell_of:expr) => {{
                for a0 in $a0 {
                    for a1 in $a1 {
                        let mut prev = false;
                        for c in $c_range {
                            let cur = shape.contains(&$cell_of(a0, a1, c));
                            if !prev && cur {
                                self.apply_face($neg, $cell_of(a0, a1, c));
                            }
                            if prev && !cur {
                                self.apply_face($neg + 1, $cell_of(a0, a1, c - 1));
                            }
                            prev = cur;
                        }
                    }
                }
            }};
        }
        // pass1: x, y, walk z; neg = NORTH(2)
        walk!(
            min_x..=max_x,
            min_y..=max_y,
            min_z - 1..=max_z + 1,
            2u8,
            |x: i32, y: i32, z: i32| (x, y, z)
        );
        // pass2: z, x, walk y; neg = DOWN(0)
        walk!(
            min_z..=max_z,
            min_x..=max_x,
            min_y - 1..=max_y + 1,
            0u8,
            |x: i32, y: i32, z: i32| (x, y, z)
        );
        // pass3: y, z, walk x; neg = WEST(4)
        walk!(
            min_y..=max_y,
            min_z..=max_z,
            min_x - 1..=max_x + 1,
            4u8,
            |x: i32, y: i32, z: i32| (x, y, z)
        );
    }

    /// one forAllFaces event: updateShape on the filled cell + the neighbor
    fn apply_face(&mut self, dir: u8, cell: (i32, i32, i32)) {
        let (dx, dy, dz) = DIRS6[dir as usize];
        let c = BlockPos::new(cell.0, cell.1, cell.2);
        let n = BlockPos::new(c.x + dx, c.y + dy, c.z + dz);
        self.vine_recompute(c);
        self.vine_recompute(n);
    }

    /// VineBlock.updateShape: recompute all faces from the current supports;
    /// vine with no faces -> air (direction == DOWN path is a no-op and is
    /// folded in: recomputes only run on non-DOWN faces here).
    fn vine_recompute(&mut self, pos: BlockPos) {
        let Some(st) = self.state_full(pos) else { return };
        if !is_vine_name(&st) {
            return;
        }
        let level = self.ctx.level;
        let mut new_st = st.clone();
        // UP face support: block above's DOWN face full (isAcceptableNeighbour)
        if get_prop(&new_st, "up") == Some("true") {
            let above = BlockPos::new(pos.x, pos.y + 1, pos.z);
            let above_full = self
                .state_full(above)
                .map(|s| is_full_cube(crate::features_deco::state_block_name(&s)))
                .unwrap_or(false);
            new_st = set_prop(&new_st, "up", if above_full { "true" } else { "false" });
        }
        for &d in HORIZONTAL.iter() {
            let key = match d {
                2 => "north",
                5 => "east",
                3 => "south",
                _ => "west",
            };
            if get_prop(&new_st, key) != Some("true") {
                continue;
            }
            let (dx, dy, dz) = DIRS6[d as usize];
            let np = BlockPos::new(pos.x + dx, pos.y + dy, pos.z + dz);
            let mut can = self
                .state_full(np)
                .map(|s| is_full_cube(crate::features_deco::state_block_name(&s)))
                .unwrap_or(false);
            if !can {
                // horizontal fallback: the vine ABOVE with the same face set
                let above = BlockPos::new(pos.x, pos.y + 1, pos.z);
                if let Some(s) = self.state_full(above) {
                    can = is_vine_name(&s) && get_prop(&s, key) == Some("true");
                }
            }
            new_st = set_prop(&new_st, key, if can { "true" } else { "false" });
        }
        // no faces left -> air
        let (_, props) = parse_state(&new_st);
        let any = props.iter().any(|(k, v)| {
            matches!(*k, "north" | "east" | "south" | "west" | "up") && *v == "true"
        });
        if new_st != st {
            if any {
                self.emit(pos, &new_st, 0, 0);
            } else {
                self.emit(pos, "minecraft:air", 0, 0);
            }
        }
    }
}


/// getOptionalDistanceAt over a full state string (logs -> 0, leaves -> value)
fn optional_distance_world(s: &str) -> Option<i32> {
    let (name, props) = parse_state(s);
    if name.ends_with("log") || name.ends_with("_log") {
        return Some(0);
    }
    props
        .iter()
        .find(|(k, _)| *k == "distance")
        .and_then(|(_, v)| v.parse::<i32>().ok())
}

// ---------------------------------------------------------------------------
// Public entry: TreeFeature.place
// ---------------------------------------------------------------------------

/// vanilla TreeFeature.place — returns "any block written" (BoundingBox present)
pub fn place_tree(
    ctx: &PlaceCtx,
    cfg: &TreeCfg,
    rng: &mut WorldgenRandom,
    pos: BlockPos,
    ops: &mut Vec<crate::features_place::DecoOp>,
) -> bool {
    let mut t = TreeRunner {
        ctx,
        ops,
        roots: JSetSim::new(),
        logs: JSetSim::new(),
        leaves: JSetSim::new(),
        decos: JSetSim::new(),
    };
    let flag = t.do_place(pos, cfg, rng);
    if flag && (!t.logs.is_empty() || !t.leaves.is_empty()) {
        t.run_decorators(cfg, rng);
        t.update_leaves(());
        t.update_shape_at_edge();
        true
    } else {
        false
    }
}

// ---------------------------------------------------------------------------
// VegetationPatchFeature / WaterloggedVegetationPatchFeature
// ---------------------------------------------------------------------------

/// ground/vegetation patch runner. Returns "positions non-empty".
pub fn place_vegetation_patch(
    ctx: &PlaceCtx,
    cfg: &VegetationPatchCfg,
    rng: &mut WorldgenRandom,
    pos: BlockPos,
    ops: &mut Vec<crate::features_place::DecoOp>,
) -> bool {
    let i = cfg.xz_radius.sample(rng) + 1;
    let i1 = cfg.xz_radius.sample(rng) + 1;
    let mut runner = VegRunner { ctx, ops, overlay: std::cell::RefCell::new(std::collections::HashMap::new()) };
    let set = runner.place_ground_patch(cfg, rng, pos, i, i1);
    // distribution iterates the JAVA hash order
    let order = set.order();
    for bp in order {
        if cfg.vegetation_chance > 0.0 && rng.next_f32() < cfg.vegetation_chance {
            runner.place_vegetation(cfg, rng, bp);
        }
    }
    !set.is_empty()
}

/// level view that serves the runner's own writes (vanilla setBlock is live)
struct VegRunner<'a, 'b> {
    ctx: &'a PlaceCtx<'a>,
    ops: &'b mut Vec<crate::features_place::DecoOp>,
    overlay: std::cell::RefCell<std::collections::HashMap<(i32, i32, i32), String>>,
}

impl<'a, 'b> VegRunner<'a, 'b> {
    fn full(&self, p: BlockPos) -> Option<String> {
        if let Some(s) = self.overlay.borrow().get(&(p.x, p.y, p.z)) {
            return Some(s.clone());
        }
        // the PlaceCtx level is an OverlayLevel for the FEATURE's own writes;
        // the patch runner needs ITS writes too — mirror through ctx reads
        if let Some(s) = self.ctx.level.read_overlay(p) {
            return Some(s);
        }
        let s = self.ctx.level.state_full(p);
        if s.is_empty() { None } else { Some(s) }
    }

    fn emit(&mut self, pos: BlockPos, state: &str) {
        self.ctx.emit_aux(self.ops, pos, state, 0, 0);
        self.overlay.borrow_mut().insert((pos.x, pos.y, pos.z), state.to_string());
    }

    fn is_air(&self, p: BlockPos) -> bool {
        if let Some(s) = self.full(p) {
            let n = crate::features_deco::state_block_name(&s);
            return matches!(n, "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air");
        }
        self.ctx.level.is_air(p)
    }

    fn place_ground_patch(
        &mut self,
        cfg: &VegetationPatchCfg,
        rng: &mut WorldgenRandom,
        pos: BlockPos,
        x_radius: i32,
        z_radius: i32,
    ) -> JSetSim {
        let dir_y: i32 = if cfg.ceiling { 1 } else { -1 };
        let opp_y: i32 = -dir_y;
        let mut set = JSetSim::new();
        let mut i = -x_radius;
        while i <= x_radius {
            let edge_x = i == -x_radius || i == x_radius;
            let mut i1 = -z_radius;
            while i1 <= z_radius {
                let edge_z = i1 == -z_radius || i1 == z_radius;
                let flag2 = edge_x || edge_z;
                let flag3 = edge_x && edge_z;
                let flag4 = flag2 && !flag3;
                if !flag3
                    && (!flag4
                        || (cfg.extra_edge_chance != 0.0 && !(rng.next_f32() > cfg.extra_edge_chance)))
                {
                    let mut mp = BlockPos::new(pos.x + i, pos.y, pos.z + i1);
                    // walk while air, up to vertical_range
                    let mut steps = 0;
                    while self.is_air(mp) && steps < cfg.vertical_range {
                        mp.y += dir_y;
                        steps += 1;
                    }
                    // walk while NOT air
                    let mut steps = 0;
                    while !self.is_air(mp) && steps < cfg.vertical_range {
                        mp.y += opp_y;
                        steps += 1;
                    }
                    let mp1 = BlockPos::new(mp.x, mp.y + dir_y, mp.z);
                    // isEmptyBlock(mp) && blockState.isFaceSturdy(mp1, opposite)
                    let sturdy = {
                        let dir_code: u8 = if cfg.ceiling { 0 } else { 1 };
                        // isFaceSturdy(mp1, direction.getOpposite()):
                        // floor: DOWN dir -> mp1 below mp, face UP sturdy
                        // ceiling: UP dir -> mp1 above mp, face DOWN sturdy
                        self.ctx.level.has_sturdy_face(mp1, dir_code)
                    };
                    if self.is_air(mp) && sturdy {
                        let depth = cfg.depth.sample(rng)
                            + if cfg.extra_bottom_chance > 0.0 && rng.next_f32() < cfg.extra_bottom_chance {
                                1
                            } else {
                                0
                            };
                        let bp = mp1;
                        if self.place_ground(cfg, rng, mp1, depth, dir_y) {
                            set.add(bp);
                        }
                    }
                }
                i1 += 1;
            }
            i += 1;
        }
        if cfg.waterlogged {
            // WaterloggedVegetationPatchFeature: replace unexposed with water
            let mut set1 = JSetSim::new();
            for bp in set.order() {
                if !self.is_exposed(cfg, &set, bp) {
                    set1.add(bp);
                }
            }
            for bp in set1.order() {
                self.emit(bp, "minecraft:water");
            }
            return set1;
        }
        set
    }

    fn is_exposed(&self, cfg: &VegetationPatchCfg, set: &JSetSim, pos: BlockPos) -> bool {
        // exposed when any of N/E/S/W/DOWN neighbors lacks a sturdy face
        // toward the patch; NOTE: the neighbor SET test is NOT part of vanilla
        // isExposed (it scans the world level) — set membership unused here
        let _ = set;
        let down_face: u8 = if cfg.ceiling { 0 } else { 1 };
        let dirs = [2u8, 5, 3, 4, 0]; // N, E, S, W, DOWN
        for d in dirs {
            let (dx, dy, dz) = DIRS6[d as usize];
            let np = BlockPos::new(pos.x + dx, pos.y + dy, pos.z + dz);
            if !self.ctx.level.has_sturdy_face(np, down_face) {
                return true;
            }
        }
        false
    }

    fn place_ground(
        &mut self,
        cfg: &VegetationPatchCfg,
        rng: &mut WorldgenRandom,
        mut mp: BlockPos,
        max_distance: i32,
        dir_y: i32,
    ) -> bool {
        for i in 0..max_distance {
            let mut flags = 0u8;
            let Some(state) = sample_state(&cfg.ground_state, self.ctx.level, mp, rng, &mut flags)
            else {
                return i != 0;
            };
            let cur = self.full(mp);
            let same_block = match &cur {
                Some(s) => crate::features_deco::state_block_name(s)
                    == crate::features_deco::state_block_name(&state),
                None => false,
            };
            if !same_block {
                let replaceable = match &cur {
                    Some(s) => self
                        .ctx
                        .level
                        .tag_is(mp, cfg.replaceable.as_str())
                        .then_some(())
                        .map(|_| crate::features_deco::state_block_name(s)),
                    None => None,
                };
                let _ = replaceable;
                // the replaceable predicate tests the WORLD state
                if !self.ctx.level.tag_is(mp, cfg.replaceable.as_str()) {
                    return i != 0;
                }
                self.emit(mp, &state);
                mp.y += dir_y;
            }
        }
        true
    }

    fn place_vegetation(&mut self, cfg: &VegetationPatchCfg, rng: &mut WorldgenRandom, bp: BlockPos) -> bool {
        if cfg.waterlogged {
            // waterlogged variant: super.placeVegetation(.., pos.below()) —
            // the inner feature lands AT the patch position for both surfaces
            let ok = crate::features_place::place_placed(
                self.ctx,
                &cfg.vegetation_feature,
                rng,
                bp,
                self.ops,
                1,
            );
            if ok {
                if let Some(st) = self.full(bp) {
                    if get_prop(&st, "waterlogged") == Some("false") {
                        self.emit(bp, &set_prop(&st, "waterlogged", "true"));
                    }
                }
            }
            ok
        } else {
            let dir_y: i32 = if cfg.ceiling { 1 } else { -1 };
            crate::features_place::place_placed(
                self.ctx,
                &cfg.vegetation_feature,
                rng,
                BlockPos::new(bp.x, bp.y - dir_y, bp.z),
                self.ops,
                1,
            )
        }
    }
}

#[cfg(test)]
#[path = "features_tree_goldens.rs"]
mod tree_goldens;
