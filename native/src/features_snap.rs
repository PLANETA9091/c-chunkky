//! 3x3 region section snapshot for the native deco path (perf pass).
//!
//! The vanilla deco queries (state_is/fluid_is/is_air/is_replaceable/...)
//! used to upcall into Java PER QUERY — 11.6K JNI calls per chunk (state_is
//! alone = 71%). The snapshot copies the 9 region chunks' section palettes +
//! bit-packed storages ONCE per section (lazy, on first touch): one JNI call
//! + one small byte[] copy, then every point query is a decode + array read.
//!
//! Exactness: the palette entries carry FLAG bits computed in Java with the
//! exact vanilla predicates (isAir/blocksMotion/fluid/leaves/replaceable/
//! solid/solidRender/faceSturdy/...), so the native answers are bit-identical
//! to the Java paths they replace. Queries the flags cannot answer
//! (wouldSurvive, exotic tags) fall back to the JNI bridge.
//!
//! Staleness: the snapshot is taken at FIRST TOUCH during THIS chunk's deco —
//! the chunk-scoped overlay (all of this chunk's feature writes) is checked
//! BEFORE the snapshot, and the region's pre-deco state never changes during
//! deco (only features write, and they go through the overlay) — the union is
//! always the live state.

use crate::features_deco::{DecoLevel, DecoLevelExt, HeightmapTy};
use crate::features_placement::BlockPos;
use std::collections::HashMap;

/// FULL pack biome -> placed-refs union (BiomeFilter native path).
/// GLOBAL (not per-ctx): the handshake runs once on pool.firstCtx() but the
/// chunk deco acquires ANY ctx from the pool — a per-ctx map would answer
/// from an empty map on the other ctxs (the 0.084% probe regression).
pub static BIOME_FULL: std::sync::LazyLock<std::sync::RwLock<HashMap<String, Vec<String>>>> =
    std::sync::LazyLock::new(|| std::sync::RwLock::new(HashMap::new()));



/// One section: block palette (nameIdx+flags), packed raw, biome palette.
pub struct SnapSection {
    pub sec_y: i32,
    /// epoch at load — sections older than the current epoch are stale
    /// (vanilla delegate features wrote through the region) and reload lazily
    pub epoch: u64,
    pub bits: u32,
    pub names: Vec<Box<str>>,
    /// (nameIdx, flags) per palette entry
    pub pal: Vec<(u16, u16)>,
    pub raw: Vec<u64>,
    pub bio_bits: u32,
    pub bio_names: Vec<Box<str>>,
    pub bio_pal: Vec<u16>,
    pub bio_raw: Vec<u64>,
}

/// Block palette flag bits — MUST match DecoDriver.stateFlags (Java).
pub mod flags {
    pub const IS_AIR: u16 = 1;
    pub const BLOCKS_MOTION: u16 = 2;
    pub const HAS_FLUID: u16 = 4;
    pub const LEAVES: u16 = 8;
    pub const REPLACEABLE: u16 = 16;
    pub const SOLID: u16 = 32;
    pub const SOLID_RENDER: u16 = 64;
    pub const STURDY_N: u16 = 128;
    pub const STURDY_S: u16 = 256;
    pub const STURDY_E: u16 = 512;
    pub const STURDY_W: u16 = 4096;
    pub const STURDY_UP: u16 = 8192;
    pub const STURDY_DOWN: u16 = 16384;
    pub const FLUID_WATER: u16 = 1024;
    pub const FLUID_LAVA: u16 = 2048;
    /// 6-bit sturdy-face mask: dir order N,S,E,W,UP,DOWN = bits 7,8,9,12,13,14
    pub const STURDY_MASK: u16 =
        STURDY_N | STURDY_S | STURDY_E | STURDY_W | STURDY_UP | STURDY_DOWN;
}

pub struct SnapRegion {
    pub origin_cx: i32,
    pub origin_cz: i32,
    pub min_sec_y: i32,
    pub n_sec: usize,
    /// bumped by every vanilla delegate feature (they write via the region
    /// directly — the snapshot cannot see those writes)
    pub epoch: std::cell::Cell<u64>,
    /// (dcx+1)*3*nsec + (dcz+1)*nsec + (sec_y - min_sec_y)
    pub secs: Vec<Option<SnapSection>>,
}

impl SnapRegion {
    pub fn new(origin_cx: i32, origin_cz: i32, min_sec_y: i32, n_sec: usize) -> Self {
        SnapRegion {
            origin_cx,
            origin_cz,
            min_sec_y,
            n_sec,
            epoch: std::cell::Cell::new(0),
            secs: (0..9 * n_sec).map(|_| None).collect(),
        }
    }

    pub fn reset(&mut self, origin_cx: i32, origin_cz: i32, min_sec_y: i32) {
        self.origin_cx = origin_cx;
        self.origin_cz = origin_cz;
        self.min_sec_y = min_sec_y;
        self.epoch.set(0);
        for s in self.secs.iter_mut() {
            *s = None;
        }
    }

    /// vanilla delegate wrote through the region — invalidate everything
    pub fn bump_epoch(&self) {
        self.epoch.set(self.epoch.get() + 1);
    }

    #[inline]
    fn slot(&self, cx: i32, cz: i32, sec_y: i32) -> Option<usize> {
        let dcx = cx - self.origin_cx;
        let dcz = cz - self.origin_cz;
        if !(-1..=1).contains(&dcx) || !(-1..=1).contains(&dcz) {
            return None;
        }
        let sy = sec_y - self.min_sec_y;
        if !(0..self.n_sec as i32).contains(&sy) {
            return None;
        }
        Some(((dcx + 1) as usize * 3 + (dcz + 1) as usize) * self.n_sec + sy as usize)
    }

    /// fresh (loaded at the current epoch) section, if snapshotted
    #[inline]
    fn fresh(&self, cx: i32, cz: i32, sec_y: i32) -> Option<&SnapSection> {
        let slot = self.slot(cx, cz, sec_y)?;
        let sec = self.secs.get(slot)?.as_ref()?;
        if sec.epoch != self.epoch.get() {
            return None; // stale (a vanilla delegate wrote after this load)
        }
        Some(sec)
    }

    /// Decode the block at (x,y,z): Some((block_name, flags)); None = the
    /// section is not snapshotted (caller falls back to the bridge).
    pub fn block(&self, x: i32, y: i32, z: i32) -> Option<(&str, u16)> {
        let sec_y = y >> 4;
        let sec = self.fresh(x >> 4, z >> 4, sec_y)?;
        let idx = ((y & 15) as usize) << 8 | ((z & 15) as usize) << 4 | (x & 15) as usize;
        let v = unpack(&sec.bits, &sec.raw, idx)? as usize;
        let (ni, fl) = *sec.pal.get(v)?;
        let name = sec.names.get(ni as usize)?;
        Some((name, fl))
    }

    /// Biome name at the raw quart cell (container read; NO zoom — use
    /// biome_zoom for the vanilla BiomeManager.getBiome semantics).
    pub fn biome(&self, x: i32, y: i32, z: i32) -> Option<&str> {
        let sec_y = y >> 4;
        let sec = self.fresh(x >> 4, z >> 4, sec_y)?;
        let qi = ((y & 15) as usize >> 2) << 4 | ((z & 15) as usize >> 2) << 2 | (x & 15) as usize >> 2;
        let v = unpack(&sec.bio_bits, &sec.bio_raw, qi)? as usize;
        let ni = *sec.bio_pal.get(v)?;
        Some(&sec.bio_names[ni as usize])
    }

    /// Biome name at a QUART coordinate (container read).
    pub fn biome_at_quart(&self, qx: i32, qy: i32, qz: i32) -> Option<&str> {
        let sec_y = qy >> 2;
        let sec = self.fresh(qx >> 2, qz >> 2, sec_y)?;
        let idx = ((qy & 3) as usize) << 4 | ((qz & 3) as usize) << 2 | (qx & 3) as usize;
        let v = unpack(&sec.bio_bits, &sec.bio_raw, idx)? as usize;
        let ni = *sec.bio_pal.get(v)?;
        Some(&sec.bio_names[ni as usize])
    }

    /// Full query: does the snapshotted block at pos match ANY of ids?
    /// None = the section is not snapshotted (caller falls back).
    pub fn state_is(&self, x: i32, y: i32, z: i32, ids: &[String]) -> Option<bool> {
        let (name, _) = self.block(x, y, z)?;
        Some(ids.iter().any(|i| i == name))
    }

    /// flag-mask query at pos; None = not snapshotted
    pub fn flag(&self, x: i32, y: i32, z: i32, mask: u16) -> Option<bool> {
        let (_, f) = self.block(x, y, z)?;
        Some(f & mask != 0)
    }

    /// has-sturdy-face by dir (DecoLevel order: 0=down,1=up,2=n,3=s,4=w,5=e)
    pub fn sturdy(&self, x: i32, y: i32, z: i32, dir: u8) -> Option<bool> {
        let bit = match dir {
            0 => flags::STURDY_DOWN,
            1 => flags::STURDY_UP,
            2 => flags::STURDY_N,
            3 => flags::STURDY_S,
            4 => flags::STURDY_W,
            _ => flags::STURDY_E,
        };
        self.flag(x, y, z, bit)
    }

    /// the block NAME at pos (owned copy — no borrow lifetime leakage)
    pub fn name_at(&self, x: i32, y: i32, z: i32) -> Option<String> {
        let (n, _) = self.block(x, y, z)?;
        Some(n.to_string())
    }

    /// fluid name check: Some(match) when decidable from the flags
    pub fn fluid_is(&self, x: i32, y: i32, z: i32, ids: &[String]) -> Option<bool> {
        let (_, f) = self.block(x, y, z)?;
        if f & flags::FLUID_WATER != 0 {
            return Some(ids.iter().any(|i| i == "minecraft:water"));
        }
        if f & flags::FLUID_LAVA != 0 {
            return Some(ids.iter().any(|i| i == "minecraft:lava"));
        }
        if f & flags::HAS_FLUID != 0 {
            return Some(false); // exotic fluid — not in the vanilla id sets
        }
        Some(false)
    }
}

/// Vanilla SimpleBitStorage decode: values_per_long = 64/bits, value at
/// (idx % vpl) * bits inside long[idx / vpl]. bits == 0 => single-value
/// palette (all cells = palette entry 0, raw is empty).
#[inline]
fn unpack(bits: &u32, raw: &[u64], idx: usize) -> Option<u64> {
    if *bits == 0 {
        return Some(0);
    }
    let vpl = 64 / *bits as usize;
    let li = idx / vpl;
    let sh = (idx % vpl) * *bits as usize;
    let l = *raw.get(li)?;
    Some((l >> sh) & ((1u64 << *bits) - 1))
}

/// Parse the DecoDriver.decoSnapSection little-endian payload.
pub fn parse_section(data: &[u8]) -> Option<SnapSection> {
    let mut p = 0usize;
    macro_rules! ri32 {
        () => {{
            if p + 4 > data.len() {
                return None;
            }
            let v = i32::from_le_bytes(data[p..p + 4].try_into().ok()?);
            p += 4;
            v
        }};
    }
    macro_rules! ru16 {
        () => {{
            if p + 2 > data.len() {
                return None;
            }
            let v = u16::from_le_bytes(data[p..p + 2].try_into().ok()?);
            p += 2;
            v
        }};
    }
    macro_rules! ru64 {
        () => {{
            if p + 8 > data.len() {
                return None;
            }
            let v = u64::from_le_bytes(data[p..p + 8].try_into().ok()?);
            p += 8;
            v
        }};
    }
    let sec_y = ri32!();
    let bits = ri32!();
    if bits < 0 {
        // out-of-range section marker (si out of bounds)
        return None;
    }
    let pal_len = ri32!() as usize;
    let mut names: Vec<Box<str>> = Vec::new();
    let mut pal: Vec<(u16, u16)> = Vec::with_capacity(pal_len);
    for _ in 0..pal_len {
        let ni = ru16!();
        let fl = ru16!();
        pal.push((ni, fl));
    }
    let raw_len = ri32!() as usize;
    let mut raw = Vec::with_capacity(raw_len);
    for _ in 0..raw_len {
        raw.push(ru64!());
    }
    let bio_bits = ri32!();
    let bio_len = ri32!() as usize;
    let mut bio_names: Vec<Box<str>> = Vec::new();
    let mut bio_pal: Vec<u16> = Vec::with_capacity(bio_len);
    for _ in 0..bio_len {
        bio_pal.push(ru16!());
    }
    let bio_raw_len = ri32!() as usize;
    let mut bio_raw = Vec::with_capacity(bio_raw_len);
    for _ in 0..bio_raw_len {
        bio_raw.push(ru64!());
    }
    let n_names = ri32!() as usize;
    for _ in 0..n_names {
        let l = ru16!() as usize;
        if p + l > data.len() {
            return None;
        }
        names.push(std::str::from_utf8(&data[p..p + l]).ok()?.into());
        p += l;
    }
    let n_bio = ri32!() as usize;
    for _ in 0..n_bio {
        let l = ru16!() as usize;
        if p + l > data.len() {
            return None;
        }
        bio_names.push(std::str::from_utf8(&data[p..p + l]).ok()?.into());
        p += l;
    }
    Some(SnapSection {
        sec_y,
        epoch: 0, // stamped by ChunkLevel::ensure at load
        bits: bits as u32,
        names,
        pal,
        raw,
        bio_bits: bio_bits as u32,
        bio_names,
        bio_pal,
        bio_raw,
    })
}

// ---------------------------------------------------------------------------
// ChunkLevel: overlay (chunk-scoped) -> snapshot -> JNI bridge
// ---------------------------------------------------------------------------

/// Chunk-scoped feature-write overlay map (all of this chunk's feature
/// writes; vanilla setBlock semantics — later features read earlier writes).
pub type OvMap = std::collections::HashMap<(i32, i32, i32), String>;

/// Wrapper: the per-chunk overlay over the region snapshot over the JNI
/// bridge. Replaces the per-feature OverlayLevel in the deco path.
pub struct ChunkLevel<'a> {
    inner: &'a dyn DecoLevelExt,
    ov: &'a std::cell::RefCell<OvMap>,
    snap: &'a std::cell::RefCell<SnapRegion>,
    /// bridge fallback loader for missing sections
    loader: &'a dyn Fn(i32, i32, i32) -> Option<SnapSection>,
    /// WG heightmap handshake loader: (cx, cz) -> 512 i32 (WS_WG 256 +
    /// OF_WG 256, +1 semantics) — one JNI per chunk, cached by the caller
    /// (DecoCtx.chunk_hms survives the whole chunk deco)
    hm_loader: &'a dyn Fn(i32, i32) -> Option<std::sync::Arc<Vec<i32>>>,
    /// uid -> placed name (BiomeFilter) — native answer path
    uid_names: &'a [String],
    /// BiomeManager.obfuscateSeed(world seed) — the zoom fiddle seed
    zoom_seed: i64,
    /// CRUS_DECO_NOSNAP=1: bypass the snapshot (all queries -> the JNI
    /// bridge — the pre-snapshot semantics; bisect tool)
    no_snap: bool,
}

impl<'a> ChunkLevel<'a> {
    pub fn new(
        inner: &'a dyn DecoLevelExt,
        ov: &'a std::cell::RefCell<OvMap>,
        snap: &'a std::cell::RefCell<SnapRegion>,
        loader: &'a dyn Fn(i32, i32, i32) -> Option<SnapSection>,
        hm_loader: &'a dyn Fn(i32, i32) -> Option<std::sync::Arc<Vec<i32>>>,
        uid_names: &'a [String],
        zoom_seed: i64,
    ) -> Self {
        let no_snap = std::env::var("CRUS_DECO_NOSNAP").ok().as_deref() == Some("1");
        ChunkLevel { inner, ov, snap, loader, hm_loader, uid_names, zoom_seed, no_snap }
    }

    /// WG heightmap snapshot for a region chunk (cached by the hm_loader's
    /// owner — DecoCtx.chunk_hms). None = load failed → JNI fallback.
    fn ensure_hm(&self, cx: i32, cz: i32) -> Option<std::sync::Arc<Vec<i32>>> {
        (self.hm_loader)(cx, cz)
    }

    /// The world-side block NAME at pos (NO overlay — can_survive semantics
    /// match the live region the JNI path reads, which cannot see the
    /// pending apply either). None = not snapshotted → the caller falls back.
    pub fn world_name_at(&self, pos: BlockPos) -> Option<String> {
        self.ensure(pos.x, pos.y, pos.z);
        self.snap.borrow().name_at(pos.x, pos.y, pos.z)
    }

    /// The world-side flag bits at pos (NO overlay). None = not snapshotted.
    pub fn world_flag_at(&self, pos: BlockPos, mask: u16) -> Option<bool> {
        self.ensure(pos.x, pos.y, pos.z);
        self.snap.borrow().flag(pos.x, pos.y, pos.z, mask)
    }

    /// Snapshot column scan for the FINAL heightmap types; None = a needed
    /// section is not snapshotted (caller falls back to the JNI bridge).
    fn scan_final_height(&self, ty: HeightmapTy, x: i32, z: i32) -> Option<i32> {
        debug_assert!(!matches!(
            ty,
            HeightmapTy::WorldSurfaceWg | HeightmapTy::OceanFloorWg
        ));
        let ws_wg = self.height(HeightmapTy::WorldSurfaceWg, x, z);
        let top = (ws_wg + 48).min(self.max_y() - 1);
        if top < self.min_y() {
            return None;
        }
        for y in (self.min_y()..=top).rev() {
            self.ensure(x, y, z);
            let (_, f) = self.snap.borrow().block(x, y, z)?;
            let hit = match ty {
                HeightmapTy::WorldSurface => f & flags::IS_AIR == 0,
                // vanilla switch: OCEAN_FLOOR and MOTION_BLOCKING share the
                // predicate blocksMotion() || !fluid.isEmpty()
                HeightmapTy::OceanFloor | HeightmapTy::MotionBlocking => {
                    f & (flags::BLOCKS_MOTION | flags::HAS_FLUID) != 0
                }
                HeightmapTy::MotionBlockingNoLeaves => {
                    (f & flags::BLOCKS_MOTION != 0 && f & flags::LEAVES == 0)
                        || f & flags::HAS_FLUID != 0
                }
                HeightmapTy::WorldSurfaceWg | HeightmapTy::OceanFloorWg => unreachable!(),
            };
            if hit {
                return Some(y + 1);
            }
        }
        Some(self.min_y())
    }

    /// vanilla BiomeManager.getBiome: the 8-neighbor zoom over the snapshot
    /// containers. None = a neighbor quart fell outside the snap grid
    /// (region-edge positions) — the caller falls back to the bridge.
    fn biome_zoomed(&self, pos: BlockPos) -> Option<String> {
        if self.no_snap {
            return None;
        }
        let i = pos.x as i64 - 2;
        let i1 = pos.y as i64 - 2;
        let i2 = pos.z as i64 - 2;
        let (i3, i4, i5) = (i >> 2, i1 >> 2, i2 >> 2);
        let d = (i & 3) as f64 / 4.0;
        let d1 = (i1 & 3) as f64 / 4.0;
        let d2 = (i2 & 3) as f64 / 4.0;
        let seed = self.zoom_seed;
        let mut best = f64::INFINITY;
        let mut bi = 0usize;
        for j in 0..8usize {
            let (dx, ddx) = if j & 4 == 0 { (0i64, d) } else { (1, d - 1.0) };
            let (dy, ddy) = if j & 2 == 0 { (0, d1) } else { (1, d1 - 1.0) };
            let (dz, ddz) = if j & 1 == 0 { (0, d2) } else { (1, d2 - 1.0) };
            let dist = fiddled_distance(seed, i3 + dx, i4 + dy, i5 + dz, ddx, ddy, ddz);
            if best > dist {
                best = dist;
                bi = j;
            }
        }
        let qx = i3 + if bi & 4 == 0 { 0 } else { 1 };
        let qy = i4 + if bi & 2 == 0 { 0 } else { 1 };
        let qz = i5 + if bi & 1 == 0 { 0 } else { 1 };
        let snap = self.snap.borrow();
        snap.biome_at_quart(qx as i32, qy as i32, qz as i32).map(|s| s.to_string())
    }

    /// ensure the section of pos is loaded (lazy snapshot; epoch-aware:
    /// stale sections reload)
    fn ensure(&self, x: i32, y: i32, z: i32) {
        if self.no_snap {
            return;
        }
        let sec_y = y >> 4;
        let cx = x >> 4;
        let cz = z >> 4;
        {
            let s = self.snap.borrow();
            let Some(slot) = s.slot(cx, cz, sec_y) else {
                return; // outside the 3x3 grid or section range
            };
            match s.secs[slot].as_ref() {
                Some(sec) if sec.epoch == s.epoch.get() => return, // fresh
                _ => {}
            }
        }
        if let Some(mut loaded) = (self.loader)(cx, cz, sec_y) {
            let mut s = self.snap.borrow_mut();
            if let Some(slot) = s.slot(cx, cz, sec_y) {
                loaded.epoch = s.epoch.get();
                s.secs[slot] = Some(loaded);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// BiomeManager.getBiome zoom (vanilla 1.18+ biome resolution): the deco biome
// queries go through reg.getBiomeManager().getBiome(pos) — a ZOOMED lookup
// over the 8 surrounding quart cells with an LCG "fertility" fiddle, NOT the
// raw container value. Reading the container directly diverges wherever the
// zoom picks a neighbor (3D biomes, quart borders — invisible on the 2D-biome
// probe, broke dptt ops by 46%).
// ---------------------------------------------------------------------------

#[inline]
fn lcg_next(left: i64, right: i64) -> i64 {
    left.wrapping_mul(left.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407))
        .wrapping_add(right)
}

#[inline]
fn fiddle(l: i64) -> f64 {
    (((l >> 24) & 1023) - 512) as f64 * 8.7890625E-4
}

#[inline]
fn fiddled_distance(seed: i64, x: i64, y: i64, z: i64, xn: f64, yn: f64, zn: f64) -> f64 {
    let mut l = lcg_next(seed, x);
    l = lcg_next(l, y);
    l = lcg_next(l, z);
    l = lcg_next(l, x);
    l = lcg_next(l, y);
    l = lcg_next(l, z);
    let f0 = fiddle(l);
    l = lcg_next(l, seed);
    let f1 = fiddle(l);
    l = lcg_next(l, seed);
    let f2 = fiddle(l);
    let sq = |v: f64| v * v;
    sq(zn + f2) + sq(yn + f1) + sq(xn + f0)
}

fn tag_matches_name(tag: &str, name: &str) -> Option<bool> {
    // the block name in the snapshot list is the FULL registry key
    // ("minecraft:stone"); the ore-replaceable sets compare short names.
    let short = name.strip_prefix("minecraft:").unwrap_or(name);
    match tag {
        "minecraft:stone_ore_replaceables" => Some(matches!(
            short,
            "stone" | "granite" | "diorite" | "andesite"
        )),
        "minecraft:deepslate_ore_replaceables" => Some(matches!(short, "deepslate" | "tuff")),
        _ => None,
    }
}

impl DecoLevel for ChunkLevel<'_> {
    fn min_y(&self) -> i32 {
        self.inner.min_y()
    }
    fn max_y(&self) -> i32 {
        self.inner.max_y()
    }
    fn height(&self, ty: HeightmapTy, x: i32, z: i32) -> i32 {
        // WG types (WORLD_SURFACE_WG/OCEAN_FLOOR_WG): the frozen pre-deco
        // heightmaps are chunk-scoped data that the block snapshot CANNOT
        // reconstruct (margin spills changed blocks but not the WG maps).
        // One JNI handshake pulls 2x256 columns per chunk (lazily, cached).
        if matches!(ty, HeightmapTy::WorldSurfaceWg | HeightmapTy::OceanFloorWg)
            && !self.no_snap
        {
            if let Some(hm) = self.ensure_hm(x >> 4, z >> 4) {
                let off = if ty == HeightmapTy::WorldSurfaceWg { 0 } else { 256 };
                let idx = off + ((z & 15) as usize) * 16 + (x & 15) as usize;
                if let Some(v) = hm.get(idx) {
                    return *v;
                }
            }
            return self.inner.height(ty, x, z);
        }
        // FINAL types: the vanilla lane (WorldGenRegion.getHeight) recomputes
        // them from the blocks — a snapshot column scan with the exact flag
        // predicates is bit-identical and kills the per-column Java scan
        // (~200 getBlockState each). The scan starts at ws_wg + 48: the
        // FINAL top can only exceed the frozen WS_WG by margin feature
        // spills (vanilla tree height <= 33), and it can never sit BELOW
        // ws_wg. Above the start there is provably nothing; if a section in
        // the scan range is missing (outside the snap grid), fall back.
        if !self.no_snap {
            if let Some(h) = self.scan_final_height(ty, x, z) {
                return h;
            }
        }
        self.inner.height(ty, x, z)
    }
    fn state(&self, pos: BlockPos) -> u32 {
        self.inner.state(pos)
    }
    fn fluid(&self, pos: BlockPos) -> u32 {
        self.inner.fluid(pos)
    }
    fn biome_has_feature(&self, pos: BlockPos, feature: u32) -> bool {
        // vanilla BiomeManager.getBiome ZOOM over the snapshot containers (the
        // bridge path cost ~1.5us/call; 284 calls/chunk on dptt)
        self.ensure(pos.x, pos.y, pos.z);
        if let Some(biome) = self.biome_zoomed(pos) {
            let Some(placed) = self.uid_names.get(feature as usize) else {
                return false;
            };
            let bf = BIOME_FULL.read().unwrap();
            let native = bf.get(biome.as_str()).map(|refs| refs.iter().any(|p| p == placed)).unwrap_or(false);
            if std::env::var("CRUS_BIOMECHK").ok().as_deref() == Some("1") {
                static LOGGED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
                let java = self.inner.biome_has_feature(pos, feature);
                if java != native {
                    let n = LOGGED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if n < 12 {
                        crate::ffi::log_warn(&format!(
                            "[biomechk] ({},{},{}) snap_biome={} placed={} native={} java={} bf_has_entry={}"
                            , pos.x, pos.y, pos.z, biome, placed, native, java, bf.contains_key(biome.as_str())
                        ));
                    }
                }
                return java;
            }
            return native;
        }
        self.inner.biome_has_feature(pos, feature)
    }

    fn can_survive(&self, state: &str, pos: BlockPos) -> bool {
        // Native wouldSurvive table (bytecode-verified vs 1.21.8
        // paper-1.21.8.jar). Unknown states fall back to the JNI bridge, so
        // an untabled override can never answer wrongly.
        if !self.no_snap {
            if let Some(b) = crate::features_survive::survive_native(state, pos, self) {
                return b;
            }
        }
        self.inner.can_survive(state, pos)
    }
    fn is_air(&self, pos: BlockPos) -> bool {
        if let Some(st) = self.ov.borrow().get(&(pos.x, pos.y, pos.z)) {
            let name = crate::features_deco::state_block_name(st);
            return name == "minecraft:air" || name == "minecraft:cave_air"
                || name == "minecraft:void_air";
        }
        self.ensure(pos.x, pos.y, pos.z);
        if let Some(b) = self.snap.borrow().flag(pos.x, pos.y, pos.z, flags::IS_AIR) {
            return b;
        }
        self.inner.is_air(pos)
    }
    fn is_replaceable(&self, pos: BlockPos) -> bool {
        if let Some(st) = self.ov.borrow().get(&(pos.x, pos.y, pos.z)) {
            // VERBATIM OverlayLevel semantics (F2 goldens pinned them — the
            // bare-name list is intentionally kept as-is, incl. dead branches:
            // overlay strings carry the "minecraft:" prefix).
            let name = crate::features_deco::state_block_name(st);
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
        self.ensure(pos.x, pos.y, pos.z);
        if let Some(b) = self.snap.borrow().flag(pos.x, pos.y, pos.z, flags::REPLACEABLE) {
            return b;
        }
        self.inner.is_replaceable(pos)
    }
    fn is_solid(&self, pos: BlockPos) -> bool {
        if let Some(st) = self.ov.borrow().get(&(pos.x, pos.y, pos.z)) {
            let name = crate::features_deco::state_block_name(st);
            return self.inner.name_is_solid(name);
        }
        self.ensure(pos.x, pos.y, pos.z);
        if let Some(b) = self.snap.borrow().flag(pos.x, pos.y, pos.z, flags::SOLID) {
            return b;
        }
        self.inner.is_solid(pos)
    }
    fn has_sturdy_face(&self, pos: BlockPos, dir: u8) -> bool {
        self.ensure(pos.x, pos.y, pos.z);
        if let Some(b) = self.snap.borrow().sturdy(pos.x, pos.y, pos.z, dir) {
            return b;
        }
        self.inner.has_sturdy_face(pos, dir)
    }
    fn is_outside_build_height(&self, y: i32) -> bool {
        self.inner.is_outside_build_height(y)
    }
}

impl DecoLevelExt for ChunkLevel<'_> {
    fn state_is(&self, pos: BlockPos, ids: &[String]) -> bool {
        if let Some(st) = self.ov.borrow().get(&(pos.x, pos.y, pos.z)) {
            let name = crate::features_deco::state_block_name(st);
            return ids.iter().any(|i| i == name);
        }
        self.ensure(pos.x, pos.y, pos.z);
        if let Some(b) = self.snap.borrow().state_is(pos.x, pos.y, pos.z, ids) {
            return b;
        }
        self.inner.state_is(pos, ids)
    }
    fn fluid_is(&self, pos: BlockPos, ids: &[String]) -> bool {
        if self.ov.borrow().contains_key(&(pos.x, pos.y, pos.z)) {
            return false; // feature writes are never fluids
        }
        self.ensure(pos.x, pos.y, pos.z);
        if let Some(b) = self.snap.borrow().fluid_is(pos.x, pos.y, pos.z, ids) {
            return b;
        }
        self.inner.fluid_is(pos, ids)
    }
    fn tag_is(&self, pos: BlockPos, tag: &str) -> bool {
        if let Some(st) = self.ov.borrow().get(&(pos.x, pos.y, pos.z)) {
            // The chunk-scoped overlay holds PRIOR features' writes too —
            // those must answer with the real region semantics (vanilla reads
            // the live BlockState: andesite written by an earlier ore DOES
            // match stone_ore_replaceables for a later vein). The old
            // per-feature bare-name lists only ever saw the current feature's
            // re-visits; for cross-feature reads they'd answer false where
            // vanilla answers true (the dptt 2.128% missing-ore divergence).
            let name = crate::features_deco::state_block_name(st);
            return match tag_matches_name(tag, name) {
                Some(b) => b,
                None => self.inner.tag_is(pos, tag),
            };
        }
        self.ensure(pos.x, pos.y, pos.z);
        // world blocks can match exotic tags — only the ore sets are native;
        // exotic tags take the exact bridge path
        if let Some(name) = self.snap.borrow().name_at(pos.x, pos.y, pos.z) {
            return match tag_matches_name(tag, &name) {
                Some(b) => b,
                None => self.inner.tag_is(pos, tag),
            };
        }
        self.inner.tag_is(pos, tag)
    }
    fn name_is_solid(&self, name: &str) -> bool {
        self.inner.name_is_solid(name)
    }
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

/// Snapshot-level tests: parse + decode round-trip.
#[cfg(test)]
mod tests {
    use super::*;

    fn build_payload() -> Vec<u8> {
        // one section: secY=5, bits=4, 2 palette entries, 4096 blocks packed
        let mut buf = Vec::new();
        buf.extend_from_slice(&5i32.to_le_bytes());
        buf.extend_from_slice(&4i32.to_le_bytes());
        buf.extend_from_slice(&2i32.to_le_bytes());
        buf.extend_from_slice(&0u16.to_le_bytes());
        buf.extend_from_slice(&(flags::SOLID | flags::STURDY_UP).to_le_bytes());
        buf.extend_from_slice(&1u16.to_le_bytes());
        buf.extend_from_slice(&flags::IS_AIR.to_le_bytes());
        let vpl = 64 / 4;
        let raw_len = 4096 / vpl;
        buf.extend_from_slice(&(raw_len as i32).to_le_bytes());
        for i in 0..raw_len {
            let mut l = 0u64;
            for k in 0..vpl {
                let v = if (i * vpl + k) % 7 == 0 { 1 } else { 0 };
                l |= (v as u64) << (k * 4);
            }
            buf.extend_from_slice(&l.to_le_bytes());
        }
        // biomes: bits=0 (single value), 1 palette entry
        buf.extend_from_slice(&0i32.to_le_bytes());
        buf.extend_from_slice(&1i32.to_le_bytes());
        buf.extend_from_slice(&0u16.to_le_bytes());
        buf.extend_from_slice(&0i32.to_le_bytes()); // raw len 0
        // block names
        buf.extend_from_slice(&2i32.to_le_bytes());
        for s in ["minecraft:stone", "minecraft:air"] {
            buf.extend_from_slice(&(s.len() as u16).to_le_bytes());
            buf.extend_from_slice(s.as_bytes());
        }
        // biome names
        buf.extend_from_slice(&1i32.to_le_bytes());
        let s = "minecraft:plains";
        buf.extend_from_slice(&(s.len() as u16).to_le_bytes());
        buf.extend_from_slice(s.as_bytes());
        buf
    }

    #[test]
    fn parse_and_decode() {
        let payload = build_payload();
        let sec = parse_section(&payload).expect("parse");
        assert_eq!(sec.sec_y, 5);
        assert_eq!(sec.pal.len(), 2);
        let mut reg = SnapRegion::new(0, 0, 0, 24);
        reg.secs[(1 * 3 + 1) * 24 + 5] = Some(sec);
        // air at multiples of 7 (v=1 -> palette entry 1 = air)
        let (n, f) = reg.block(1, 5 * 16 + 0, 0).unwrap();
        assert_eq!(n, "minecraft:stone");
        assert_eq!(f & flags::SOLID, flags::SOLID);
        let (n, f) = reg.block(0, 5 * 16 + 0, 0).unwrap();
        assert_eq!(n, "minecraft:air");
        assert_eq!(f & flags::IS_AIR, flags::IS_AIR);
        let (n, f) = reg.block(15, 5 * 16 + 15, 15).unwrap();
        assert_eq!(n, "minecraft:air");
        assert!(f & flags::IS_AIR != 0);
        // biome from a zero-bit container
        assert_eq!(reg.biome(3, 90, 4), Some("minecraft:plains"));
        // out of region
        assert!(reg.block(16, 80, 0).is_none());
    }
}
