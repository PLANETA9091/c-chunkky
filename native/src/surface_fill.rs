// ---------------------------------------------------------------------------
// Native SurfaceRules engine — per-chunk column walk (config: surface.rs).
// Replicates vanilla SurfaceSystem.buildSurface (javap-verified):
//
//   per column: o = heightmap(WORLD_SURFACE_WG) + 1; updateXZ(x, z);
//   depth=0; waterHeight=MIN; stoneBottom=MAX;
//   for (y = o - 1; y >= minY; y--) {
//       b = column[y];
//       if (isAir(b)) { depth = 0; waterHeight = MIN; continue; }
//       if (!fluidEmpty(b)) { if (waterHeight == MIN) waterHeight = y + 1;
//                             continue; }               // NO updateY!
//       if (stoneBottom >= y) {                          // rescan stone run
//           stoneBottom = -32512;
//           for (y2 = y - 1; y2 >= minY - 1; y2--) {
//               if (!isStone(column[y2])) { stoneBottom = y2 + 1; break; }
//           }
//       }
//       depth++;
//       stoneDepthBelow = y - stoneBottom + 1;
//       updateY(depth, stoneDepthBelow, waterHeight, x, y, z);
//       if (b == defaultBlock) {                         // identity compare
//           st = rule.tryApply(depth, stoneDepthBelow, waterHeight);
//           if (st != null) column[y] = st;              // writes the buffer
//       }
//   }
//
// isAir = block AIR; fluidEmpty = not water/lava; isStone = !air && fluid
// empty (vanilla isStone — ANY solid counts, incl. our surface replacements).
// Rules are evaluated top-down; column writes are visible to later reads.
// ---------------------------------------------------------------------------

use super::density::{Env, InterpState, MemoCtx, Mode, Pt};
use super::surface::{noise_at, SrCond, SrRule, SurfaceConfig, TempNoises};
use super::rng::{XoroshiroPositionalRandomFactory, XoroshiroRandomSource};

pub const MIN_LEVEL: i32 = i32::MIN;

/// Per-ctx surface state (built once after ids/temps arrive from the JVM).
pub struct SurfaceCtx {
    pub cfg: SurfaceConfig,
    pub noise_handles: [u32; 3],
    pub temps: Vec<f32>,   // per native biome idx
    pub frozen: Vec<bool>, // per native biome idx
}

#[inline]
fn mth_floor(d: f64) -> i32 {
    let i = d as i32;
    if d < i as f64 { i - 1 } else { i }
}

#[inline]
fn mth_map(v: f64, lo_in: f64, hi_in: f64, lo_out: f64, hi_out: f64) -> f64 {
    (v - lo_in) / (hi_in - lo_in) * (hi_out - lo_out) + lo_out
}

/// lerp2 over f64 (Mth.lerp2): bilinear — v00, v10, v01, v11.
#[inline]
fn lerp2(dx: f64, dz: f64, v00: f64, v10: f64, v01: f64, v11: f64) -> f64 {
    let a = v00 + dx * (v10 - v00);
    let b = v01 + dx * (v11 - v01);
    a + dz * (b - a)
}

static TRACE_POS: std::sync::OnceLock<Option<(i32, i32, i32)>> = std::sync::OnceLock::new();

fn trace_pos() -> Option<(i32, i32, i32)> {
    *TRACE_POS.get_or_init(|| {
        std::env::var("SF_TRACE")
            .ok()
            .and_then(|v| {
                let p: Vec<i32> = v.split(':').filter_map(|s| s.parse().ok()).collect();
                if p.len() == 3 { Some((p[0], p[1], p[2])) } else { None }
            })
    })
}

pub struct ColWalk<'a> {
    pub cfg: &'a SurfaceConfig,
    pub noise_handles: [u32; 3],
    pub env: &'a Env,
    pub factory: &'a XoroshiroPositionalRandomFactory,
    pub temps_noises: &'a TempNoises,
    pub compiled: &'a super::density::Compiled,
    pub interp: &'a mut InterpState,
    pub memo: &'a mut MemoCtx,
    pub biome_grid: &'a [i32],
    pub temps: &'a [f32],
    pub frozen: &'a [bool],
    pub names: &'a [String],
    pub heightmap: &'a [i32; 256],
    pub prelim: std::collections::HashMap<i64, i32>,
    // per-column state
    pub block_x: i32,
    pub block_z: i32,
    pub surface_depth: i32,
    pub surface_secondary: f64,
    pub min_surface_level: i32,
    pub xz_counter: u64,
    pub y_counter: u64,
    pub last_sec_update: u64,
    pub last_min_update: u64,
    // blockY + walk vars (context)
    pub block_y: i32,
    pub stone_depth_above: i32,
    pub stone_depth_below: i32,
    pub water_height: i32,
    // per-site lazy caches
    pub xz_cache: Vec<(u64, bool)>,
    pub y_cache: Vec<(u64, bool)>,
    /// Biome cond results keyed by (xz_counter, quartY) — biome conds depend
    /// only on (column, quart-Y), but the y_cache resets per block (3D-biome
    /// semantics), so Terralith's ~hundreds-of-branches biome dispatch
    /// re-resolved quart + set scan per band block. One u64 compare now.
    biome_cache: Vec<(u64, bool)>,
    /// 26.3 ore-vein positional random factory ("minecraft:ore" from world seed)
    pub ore_factory: XoroshiroPositionalRandomFactory,
}

impl<'a> ColWalk<'a> {
    pub fn new(
        cfg: &'a SurfaceConfig,
        env: &'a Env,
        factory: &'a XoroshiroPositionalRandomFactory,
        temps_noises: &'a TempNoises,
        compiled: &'a super::density::Compiled,
        interp: &'a mut InterpState,
        memo: &'a mut MemoCtx,
        biome_grid: &'a [i32],
        temps: &'a [f32],
        frozen: &'a [bool],
        names: &'a [String],
        cond_count: usize,
        heightmap: &'a [i32; 256],
        noise_handles_ref: &[u32; 3],
        ore_factory: XoroshiroPositionalRandomFactory,
    ) -> Self {
        let nh = *noise_handles_ref;
        ColWalk {
            cfg,
            noise_handles: nh,
            env,
            factory,
            temps_noises,
            compiled,
            interp,
            memo,
            biome_grid,
            temps,
            frozen,
            names,
            heightmap,
            ore_factory,
            prelim: std::collections::HashMap::new(),
            block_x: 0,
            block_z: 0,
            surface_depth: 0,
            surface_secondary: 0.0,
            min_surface_level: 0,
            xz_counter: 0,
            y_counter: 0,
            last_sec_update: 0,
            last_min_update: 0,
            block_y: 0,
            stone_depth_above: 0,
            stone_depth_below: 0,
            water_height: MIN_LEVEL,
            xz_cache: vec![(0, false); cond_count],
            y_cache: vec![(0, false); cond_count],
            biome_cache: vec![(0, false); cond_count],
        }
    }

    // -- context updates ---------------------------------------------------

    #[inline]
    fn update_xz(&mut self, bx: i32, bz: i32) {
        self.xz_counter += 1;
        self.y_counter += 1; // vanilla updateXZ bumps BOTH counters
        self.block_x = bx;
        self.block_z = bz;
        let sn = noise_at(self.env, self.noise_handles[0]);
        let d = sn.get_value(bx as f64, 0.0, bz as f64);
        let jit = self.factory.at(bx, 0, bz).next_f64();
        self.surface_depth = ((d * 2.75 + 3.0 + jit * 0.25)) as i32;
        // reset min-surface + secondary column caches (counter compare does it)
    }

    #[inline]
    fn update_y(&mut self, sda: i32, sdb: i32, wh: i32, y: i32) {
        self.y_counter += 1;
        self.stone_depth_above = sda;
        self.stone_depth_below = sdb;
        self.water_height = wh;
        self.block_y = y;
    }

    #[inline]
    fn surface_secondary(&mut self) -> f64 {
        if self.last_sec_update != self.xz_counter {
            self.last_sec_update = self.xz_counter;
            let sn = noise_at(self.env, self.noise_handles[1]);
            self.surface_secondary = sn.get_value(self.block_x as f64, 0.0, self.block_z as f64);
        }
        self.surface_secondary
    }

    // -- preliminary surface level (NoiseChunk.preliminarySurfaceLevel) ----

    fn prelim_level(&mut self, bx: i32, bz: i32) -> i32 {
        // quart snap: QuartPos.fromBlock -> toBlock
        let qx = (bx >> 2) << 2;
        let qz = (bz >> 2) << 2;
        let key = ((qx as i64) << 32) | (qz as i64 & 0xFFFF_FFFF);
        if let Some(v) = self.prelim.get(&key) {
            return *v;
        }
        let wg = &self.env.wg;
        let root = match self.compiled.router.get("initial_density_without_jaggedness") {
            Some(r) => *r,
            None => {
                self.prelim.insert(key, i32::MAX);
                return i32::MAX;
            }
        };
        let mut y = wg.min_y + wg.height;
        let mut out = i32::MAX;
        let mut dbg = 3;
        while y >= wg.min_y {
            let pt = Pt { x: qx, y, z: qz };
            // fresh serial per y — the shared memo must not reuse y-variant
            // nodes across scan steps (vanilla evaluates fresh per point)
            self.memo.next_serial();
            let d = self.env.eval(
                self.compiled,
                root,
                pt,
                Mode::Direct,
                self.interp,
                Some(self.memo),
            );
            if dbg > 0 && std::env::var("SF_TRACE").is_ok() {
                dbg -= 1;
                eprintln!("PRELIM ({qx},{qz}) y={y} d={d}");
            }
            if std::env::var("SF_TRACE").is_ok() && (6..64).contains(&y) && y % 4 == 0 {
                eprintln!("PRELIM2 ({qx},{qz}) y={y} d={d}");
            }
            if d > 0.390625f64 {
                out = y;
                break;
            }
            y -= wg.cell_height;
        }
        self.prelim.insert(key, out);
        out
    }

    fn min_surface_level(&mut self) -> i32 {
        if self.last_min_update != self.xz_counter {
            self.last_min_update = self.xz_counter;
            let cx0 = (self.block_x >> 4) << 4;
            let cz0 = (self.block_z >> 4) << 4;
            let s0 = self.prelim_level(cx0, cz0) as f64;
            let s1 = self.prelim_level(cx0 + 16, cz0) as f64;
            let s2 = self.prelim_level(cx0, cz0 + 16) as f64;
            let s3 = self.prelim_level(cx0 + 16, cz0 + 16) as f64;
            let dx = ((self.block_x & 15) as f32 / 16.0f32) as f64;
            let dz = ((self.block_z & 15) as f32 / 16.0f32) as f64;
            let v = lerp2(dx, dz, s0, s1, s2, s3);
            self.min_surface_level = mth_floor(v) + self.surface_depth - 8;
        }
        self.min_surface_level
    }

    /// Probe used by the deep-zone fast path: computes (and caches) the
    /// per-column min surface level exactly like the AbovePreliminary cond.
    fn min_surface_probe(&mut self) -> i32 {
        self.min_surface_level()
    }

    /// Bit-exact replica of the VerticalGradient cond band eval (only called
    /// inside a gradient band, where the plain range shortcuts don't fire).
    fn gradient_at(&mut self, name: &str, true_y: i32, false_y: i32, x: i32, y: i32, z: i32) -> bool {
        let d = mth_map(y as f64, true_y as f64, false_y as f64, 1.0, 0.0);
        let mut gf = self.factory.from_hash_of(name).fork_positional();
        let mut r: XoroshiroRandomSource = gf.at(x, y, z);
        (r.next_f32() as f64) < d
    }

    // -- biome helpers ------------------------------------------------------

    #[inline]
    fn biome_idx_at(&self, bx: i32, y: i32, bz: i32) -> usize {
        let sy = ((y - self.cfg.min_y) >> 4).max(0) as usize;
        let qy = ((y - self.cfg.min_y) >> 2) & 3;
        let qx = (bx & 15) >> 2;
        let qz = (bz & 15) >> 2;
        let i = sy * 64 + (qx as usize) * 16 + (qy as usize) * 4 + qz as usize;
        let v = self.biome_grid.get(i).copied().unwrap_or(0);
        v.max(0) as usize
    }

    fn cold_enough_to_snow(&self, bi: usize, y: i32) -> bool {
        if bi >= self.temps.len() {
            return false;
        }
        let base = self.temps[bi];
        let mut f = base;
        if self.frozen.get(bi).copied().unwrap_or(false) {
            // FROZEN modifier
            let fx = self.block_x as f64 * 0.05;
            let fz = self.block_z as f64 * 0.05;
            let d1 = self.temps_noises.frozen.value(fx, fz, false) * 7.0;
            let d2 = self.temps_noises.biome_info.value(
                self.block_x as f64 * 0.2,
                self.block_z as f64 * 0.2,
                false,
            );
            if d1 + d2 < 0.3
                && self.temps_noises.biome_info.value(
                    self.block_x as f64 * 0.09,
                    self.block_z as f64 * 0.09,
                    false,
                ) < 0.8
            {
                f = 0.2f32;
            }
        }
        let sea = self.cfg.sea_level;
        let i = sea + 17;
        if y > i {
            // all f32 arithmetic (javap: fadd/fsub/fmul/fdiv)
            let tx = (self.block_x as f32) / 8.0f32;
            let tz = (self.block_z as f32) / 8.0f32;
            let n = self.temps_noises.temperature.value(tx as f64, tz as f64, false);
            let n8 = (n * 8.0f64) as f32;
            let adj = ((n8 + y as f32) - i as f32) * 0.05f32 / 40.0f32;
            f -= adj;
        }
        !(f >= 0.15f32)
    }

    // -- heightmap (steep) ---------------------------------------------------

    #[inline]
    fn hm(&self, lx: i32, lz: i32) -> i32 {
        self.heightmap[(lz.clamp(0, 15) * 16 + lx.clamp(0, 15)) as usize]
    }

    // -- condition evaluation ------------------------------------------------

    fn test(&mut self, ci: usize) -> bool {
        let conds = &self.cfg.conds;
        match &conds[ci] {
            SrCond::Not(inner) => {
                let i = *inner;
                !self.test(i)
            }
            _ => {
                // lazily cached per site (vanilla LazyXZ/LazyY semantics)
                let is_y_lazy = matches!(
                    &conds[ci],
                    SrCond::Biome { .. }
                        | SrCond::StoneDepth { .. }
                        | SrCond::Water { .. }
                        | SrCond::YAbove { .. }
                        | SrCond::VerticalGradient { .. }
                        | SrCond::Temperature
                        | SrCond::AbovePreliminary
                );
                let counter = if is_y_lazy { self.y_counter } else { self.xz_counter };
                let cached = {
                    let cache: &Vec<(u64, bool)> = if is_y_lazy { &self.y_cache } else { &self.xz_cache };
                    let (last, cached) = cache[ci];
                    if last == counter {
                        Some(cached)
                    } else {
                        None
                    }
                };
                if let Some(v) = cached {
                    return v;
                }
                let v = self.compute(ci);
                {
                    let cache: &mut Vec<(u64, bool)> = if is_y_lazy { &mut self.y_cache } else { &mut self.xz_cache };
                    cache[ci] = (counter, v);
                }
                if let Some((tx, ty, tz)) = trace_pos() {
                    if tx == self.block_x && tz == self.block_z {
                        let dt = match &self.cfg.conds[ci] {
                            SrCond::Biome { set } => {
                                let bi = self.biome_idx_at(self.block_x, self.block_y, self.block_z);
                                let name = self.names.get(bi as usize).map(|s| s.as_str()).unwrap_or("?");
                                format!("biome(grid={bi}={name} set={:?} hit={})", set, set.contains(&(bi as u32)))
                            }
                            SrCond::StoneDepth { offset, add_surface_depth, secondary_range, ceiling } => format!("stone_depth(off={offset} asd={add_surface_depth} sdr={secondary_range} ceil={ceiling} sda={} sdb={})", self.stone_depth_above, self.stone_depth_below),
                            SrCond::Water { offset, surface_depth_multiplier, add_stone_depth } => format!("water(off={offset} sdm={surface_depth_multiplier} asd={add_stone_depth} wh={})", self.water_height),
                            SrCond::YAbove { anchor, add_stone_depth, surface_depth_multiplier } => format!("y_above(anchor={anchor} asd={add_stone_depth} sdm={surface_depth_multiplier})"),
                            SrCond::NoiseThreshold { noise, min, max } => format!("noise_threshold(n={noise} [{min},{max}])"),
                            SrCond::Temperature => "temperature".into(),
                            SrCond::Hole => format!("hole(sd={})", self.surface_depth),
                            SrCond::Steep => "steep".into(),
                            SrCond::AbovePreliminary => format!("above_preliminary(min={})", self.min_surface_level()),
                            SrCond::VerticalGradient { name, true_y, false_y } => format!("vgrad({name} {true_y}..{false_y})"),
                            SrCond::Not(_) => "not".into(),
                        };
                        eprintln!("TRACE y={} {dt} => {v}", self.block_y);
                    }
                }
                v
            }
        }
    }

    fn compute(&mut self, ci: usize) -> bool {
        match &self.cfg.conds[ci] {
            SrCond::Not(_) => unreachable!(),
            SrCond::Biome { set } => {
                // cache per (column, quartY): the y_cache layer above resets
                // per block, but the biome only changes with the quart.
                let qy = ((self.block_y - self.cfg.min_y) >> 2).max(0) as u64;
                let key = ((self.xz_counter as u64) << 12) | (qy & 0xFFF);
                let cached = self.biome_cache[ci];
                if cached.0 == key {
                    return cached.1;
                }
                let bi = self.biome_idx_at(self.block_x, self.block_y, self.block_z);
                let v = set.contains(&(bi as u32));
                self.biome_cache[ci] = (key, v);
                v
            }
            SrCond::Temperature => {
                let bi = self.biome_idx_at(self.block_x, self.block_y, self.block_z);
                self.cold_enough_to_snow(bi, self.block_y)
            }
            SrCond::Hole => self.surface_depth <= 0,
            SrCond::Steep => {
                // z pair then x pair (javap SteepMaterialCondition)
                let x15 = self.block_x & 15;
                let z15 = self.block_z & 15;
                let z1 = (z15 - 1).max(0);
                let z2 = (z15 + 1).min(15);
                let h1 = self.hm(x15, z1);
                let h2 = self.hm(x15, z2);
                if h2 >= h1 + 4 {
                    return true;
                }
                let x1 = (x15 - 1).max(0);
                let x2 = (x15 + 1).min(15);
                let h3 = self.hm(x1, z15);
                let h4 = self.hm(x2, z15);
                h3 >= h4 + 4
            }
            SrCond::NoiseThreshold { noise, min, max } => {
                let n = noise_at(self.env, *noise);
                let v = n.get_value(self.block_x as f64, 0.0, self.block_z as f64);
                v >= *min && v <= *max
            }
            SrCond::StoneDepth { offset, add_surface_depth, secondary_range, ceiling } => {
                let i = if *ceiling { self.stone_depth_below } else { self.stone_depth_above };
                let j = if *add_surface_depth { self.surface_depth } else { 0 };
                let k = if *secondary_range != 0 {
                    mth_map(
                        self.surface_secondary(),
                        -1.0,
                        1.0,
                        0.0,
                        *secondary_range as f64,
                    ) as i32
                } else {
                    0
                };
                // javap: true iff i <= 1 + offset + j + k (if_icmpgt)
                i <= 1 + *offset + j + k
            }
            SrCond::Water { offset, surface_depth_multiplier, add_stone_depth } => {
                if self.water_height == MIN_LEVEL {
                    return true;
                }
                let y2 = self.block_y
                    + if *add_stone_depth { self.stone_depth_above } else { 0 };
                // javap WaterCondition: if_icmplt jumps to FALSE — the test is
                // y2 >= waterHeight + offset + surfaceDepth*multiplier, i.e.
                // TRUE when the block sits at/above the water line (+offset),
                // FALSE when deeply submerged. (Vanilla default branch: grass
                // near the waterline, dirt on submerged land.)
                y2 >= self.water_height + *offset + self.surface_depth * *surface_depth_multiplier
            }
            SrCond::YAbove { anchor, add_stone_depth, surface_depth_multiplier } => {
                let y2 = self.block_y
                    + if *add_stone_depth { self.stone_depth_above } else { 0 };
                y2 >= *anchor + self.surface_depth * *surface_depth_multiplier
            }
            SrCond::VerticalGradient { name, true_y, false_y } => {
                let y = self.block_y;
                if y <= *true_y {
                    return true;
                }
                if y >= *false_y {
                    return false;
                }
                let d = mth_map(y as f64, *true_y as f64, *false_y as f64, 1.0, 0.0);
                // vanilla: randomState.getOrCreateRandomFactory(name) =
                //   random.fromHashOf(name).forkPositional(); the gradient
                // samples THAT factory at the block position.
                let mut gf = self.factory.from_hash_of(name).fork_positional();
                let mut r: XoroshiroRandomSource = gf.at(self.block_x, self.block_y, self.block_z);
                return (r.next_f32() as f64) < d;
            }
            SrCond::AbovePreliminary => self.block_y >= self.min_surface_level(),
        }
    }

    // -- rule application ------------------------------------------------------

    fn try_apply(&mut self, ri: usize) -> Option<u16> {
        match &self.cfg.rules[ri] {
            SrRule::Sequence(kids) => {
                for k in kids {
                    if let Some(s) = self.try_apply(*k) {
                        return Some(s);
                    }
                }
                None
            }
            SrRule::IfTrue { cond, then } => {
                let c = *cond;
                if self.test(c) {
                    self.try_apply(*then)
                } else {
                    None
                }
            }
            SrRule::Block { state } => Some(self.cfg.ids[*state as usize]),
            SrRule::Bandlands => {
                // getBand(x, y, z): bands[(y + round(offset*4) + 192) % 192]
                let off_n = noise_at(self.env, self.noise_handles[2]);
                let v = off_n.get_value(self.block_x as f64, 0.0, self.block_z as f64) * 4.0;
                // Math.round(f64): floor(v + 0.5) semantics
                let r = (v + 0.5).floor() as i64;
                let y = self.block_y as i64;
                let idx = ((y + r + 192) % 192) as usize;
                let st = self.cfg.bandlands.get(idx).copied().unwrap_or(self.cfg.terracotta[0]);
                Some(self.cfg.ids[st as usize])
            }
            SrRule::OreVein { ore, raw, filler, raw_chance, density_df, richness_df, gap_df } => {
                // 26.3 OreVeinRule.apply: density<=0 -> null; rng.at(x,y,z);
                // nextFloat>density -> null; richness/gap decide ore vs filler
                let pt = crate::density::Pt { x: self.block_x, y: self.block_y, z: self.block_z };
                self.memo.next_serial();
                let density = self
                    .env
                    .eval(self.compiled, *density_df, pt, Mode::Direct, self.interp, Some(self.memo))
                    as f32;
                if density <= 0.0 {
                    return None;
                }
                let mut rnd = self.ore_factory.at(self.block_x, self.block_y, self.block_z);
                if rnd.next_f32() > density {
                    return None;
                }
                self.memo.next_serial();
                let richness = self
                    .env
                    .eval(self.compiled, *richness_df, pt, Mode::Direct, self.interp, Some(self.memo))
                    as f32;
                self.memo.next_serial();
                let gap = self
                    .env
                    .eval(self.compiled, *gap_df, pt, Mode::Direct, self.interp, Some(self.memo))
                    as f32;
                if (rnd.next_f32() as f64) < richness as f64 && gap < 0.0 {
                    if rnd.next_f32() < *raw_chance {
                        Some(self.cfg.ids[*raw as usize])
                    } else {
                        Some(self.cfg.ids[*ore as usize])
                    }
                } else {
                    Some(self.cfg.ids[*filler as usize])
                }
            }
        }
    }
}

/// Apply the surface rules to a filled chunk buffer. `biome_grid` is the
/// fillBiomes output for this chunk (sections x 64 quart entries) or None
/// (native computes it as a fallback — slow path).

/// Deep-zone fast path for the Terralith/vanilla-shaped root:
///   seq[ vgrad(bedrock_floor) -> block,
///        above_preliminary_surface -> (huge surface tree),
///        not(above_preliminary_surface) -> (small deep rules),
///        vgrad(deepslate) -> block ]
/// Below the per-column preliminary surface kid[1] is statically false and
/// kid[0]/kid[3] are constant outside their y bands, so the ~60k deep solid
/// blocks per chunk skip the root recursion: one small generic walk for
/// kid[2]'s branch + band math for the gradients. Bit-exact by construction;
/// any structural mismatch falls back to the full walk.
struct DeepFast {
    bed_name: String,
    bed_true: i32,
    bed_false: i32,
    bed_state: u16,
    ds_name: String,
    ds_true: i32,
    ds_false: i32,
    ds_state: u16,
    deep_then: usize,
    /// kid[2]'s first cond — a Biome cond (guarded): pure function of
    /// (column, quartY), so the gate is resolved once per quart instead of
    /// per block. u32::MAX when the structure differs (no gate).
    gate_cond: usize,
}

fn block_state_of(cfg: &super::surface::SurfaceConfig, ri: usize) -> Option<u16> {
    match cfg.rules.get(ri)? {
        super::surface::SrRule::Block { state } => cfg.ids.get(*state as usize).copied(),
        super::surface::SrRule::Sequence(k) if k.len() == 1 => block_state_of(cfg, k[0]),
        _ => None,
    }
}

fn deep_fast(cfg: &super::surface::SurfaceConfig) -> Option<DeepFast> {
    let kids = match cfg.rules.get(cfg.root)? {
        super::surface::SrRule::Sequence(k) => k,
        _ => return None,
    };
    if kids.len() != 4 {
        return None;
    }
    let (r0, r1, r2, r3) = (
        cfg.rules.get(kids[0])?,
        cfg.rules.get(kids[1])?,
        cfg.rules.get(kids[2])?,
        cfg.rules.get(kids[3])?,
    );
    let super::surface::SrRule::IfTrue { cond: c0, then: t0 } = r0 else { return None };
    let super::surface::SrRule::IfTrue { cond: c1, .. } = r1 else { return None };
    let super::surface::SrRule::IfTrue { cond: c2, then: t2 } = r2 else { return None };
    let super::surface::SrRule::IfTrue { cond: c3, then: t3 } = r3 else { return None };
    if !matches!(cfg.conds.get(*c1)?, super::surface::SrCond::AbovePreliminary) {
        return None;
    }
    let super::surface::SrCond::Not(inner) = cfg.conds.get(*c2)? else { return None };
    if !matches!(cfg.conds.get(*inner)?, super::surface::SrCond::AbovePreliminary) {
        return None;
    }
    let super::surface::SrCond::VerticalGradient { name: bn, true_y: bt, false_y: bf } =
        cfg.conds.get(*c0)? else { return None };
    let super::surface::SrCond::VerticalGradient { name: dn, true_y: dt, false_y: df } =
        cfg.conds.get(*c3)? else { return None };
    let bed_state = block_state_of(cfg, *t0)?;
    let ds_state = block_state_of(cfg, *t3)?;
    // kid[2]'s then often starts with `if biome(...) -> ...` (e.g. Terralith
    // jungle-cave mud): hoist that gate to a per-quart cache.
    let gate_cond = match cfg.rules.get(*t2) {
        Some(super::surface::SrRule::IfTrue { cond, .. }) => {
            match cfg.conds.get(*cond) {
                Some(super::surface::SrCond::Biome { .. }) => *cond,
                _ => usize::MAX,
            }
        }
        _ => usize::MAX,
    };
    Some(DeepFast {
        bed_name: bn.clone(),
        bed_true: *bt,
        bed_false: *bf,
        bed_state,
        ds_name: dn.clone(),
        ds_true: *dt,
        ds_false: *df,
        ds_state,
        deep_then: *t2,
        gate_cond,
    })
}

#[allow(clippy::too_many_arguments)]
pub fn apply_surface(
    env: &Env,
    compiled: &super::density::Compiled,
    cfg: &SurfaceConfig,
    noise_handles: [u32; 3],
    temps: &[f32],
    frozen: &[bool],
    temps_noises: &TempNoises,
    biome_state: Option<(&crate::biomes::BiomeSourceData, &crate::biomes::ClimateDFs)>,
    names: &[String],
    cx: i32,
    cz: i32,
    out: &mut [u16; super::nochunk::OUT_LEN],
    biome_grid: Option<&[i32]>,
) {
    let height = env.wg.height as usize;
    let min_y = env.wg.min_y;
    let sections = height / 16;
    // fallback biome grid (should not happen when BiomeSwap is active)
    let mut fb_grid: Vec<i32> = Vec::new();
    let grid: &[i32] = match biome_grid {
        Some(g) => g,
        None => {
            fb_grid = vec![0; sections * 64];
            if let Some((src, climate)) = biome_state {
                super::biomes::fill_biomes(
                    src,
                    climate,
                    env,
                    compiled,
                    cx,
                    cz,
                    &mut fb_grid,
                );
            }
            &fb_grid
        }
    };
    // heightmap WORLD_SURFACE_WG: topmost non-air per column
    // (CRUS_SF_NOHM=1: diagnostic skip — timing only, results invalid)
    let sf_nohm = std::env::var("CRUS_SF_NOHM").ok().as_deref() == Some("1");
    let mut hm = [0i32; 256];
    if !sf_nohm {
    for lz in 0..16i32 {
        for lx in 0..16i32 {
            let mut top = min_y - 1;
            for sy in (0..sections).rev() {
                let base = sy * 256 + (lz * 16 + lx) as usize;
                let y0 = min_y + (sy as i32) * 16;
                let mut found = false;
                for yy in (0..16).rev() {
                    if out[base + yy * 256] != 0 {
                        top = y0 + yy as i32;
                        found = true;
                        break;
                    }
                }
                if found {
                    break;
                }
            }
            hm[(lz * 16 + lx) as usize] = top;
        }
    }
    } else {
        hm = [min_y; 256];
    }
    let cond_count = cfg.conds.len();
    let mut interp = InterpState::new(compiled.interp_slots, 4, height / 8);
    let mut memo = MemoCtx::new(compiled.arena.len()).with_cols(compiled.col_count as usize);
    let root_factory = super::surface::root_factory(env.seed);
    // vanilla RandomState.getOrCreateRandomFactory("minecraft:ore") — the ore
    // vein material rule draws its per-block randomness from this factory
    let ore_factory = super::surface::root_factory(env.seed);
    let mut walk = ColWalk::new(
        cfg, env, &root_factory, temps_noises, compiled, &mut interp, &mut memo, grid,
        temps, frozen, names, cond_count, &hm, &noise_handles, ore_factory,
    );
    let default_id = cfg.default_id;
    // (CRUS_SF_NOWALK / CRUS_SF_NOSCAN: diagnostic skips — timing only)
    let sf_nowalk = std::env::var("CRUS_SF_NOWALK").ok().as_deref() == Some("1");
    let sf_noscan = std::env::var("CRUS_SF_NOSCAN").ok().as_deref() == Some("1");
    let nofast = std::env::var("CRUS_SF_NOFAST").ok().as_deref() == Some("1");
    let fast = if nofast { None } else { deep_fast(cfg) };
    // per (column, quartY) gate results for the deep-zone biome cond:
    // 255 = unknown, else 0/1. Allocated once per chunk.
    let quarts = (cfg.height / 4).max(1) as usize;
    let mut gate_cache: Vec<u8> = vec![255u8; 256 * quarts];
    for lz in 0..16i32 {
        for lx in 0..16i32 {
            let bx = cx * 16 + lx;
            let bz = cz * 16 + lz;
            let col_base = lz * 16 + lx;
            let top = hm[col_base as usize]; // topmost non-air y
            walk.update_xz(bx, bz);
            let mut depth = 0i32;
            let mut water_height = MIN_LEVEL;
            let mut stone_bottom = i32::MAX;
            let mut y = top;
            while y >= min_y {
                let bi = ((y - min_y) * 256 + col_base) as usize;
                let b = out[bi];
                if b == 0 {
                    // air
                    depth = 0;
                    water_height = MIN_LEVEL;
                } else if b == 3 || b == 17 {
                    // water / lava (fluid non-empty)
                    if water_height == MIN_LEVEL {
                        water_height = y + 1;
                    }
                } else {
                    if sf_noscan {
                        stone_bottom = -32512;
                    } else if stone_bottom >= y {
                        stone_bottom = -32512;
                        let mut y2 = y - 1;
                        while y2 >= min_y - 1 {
                            if y2 < min_y {
                                break;
                            }
                            let i2 = ((y2 - min_y) * 256 + col_base) as usize;
                            let b2 = out[i2];
                            let is_stone = b2 != 0 && b2 != 3 && b2 != 17;
                            if !is_stone {
                                stone_bottom = y2 + 1;
                                break;
                            }
                            y2 -= 1;
                        }
                    }
                    depth += 1;
                    let sdb = y - stone_bottom + 1;
                    walk.update_y(depth, sdb, water_height, y);
                    if b == default_id && !sf_nowalk {
                        let st = if let Some(fp) = &fast {
                            if y >= walk.min_surface_probe() {
                                // surface band — full rule tree
                                walk.try_apply(cfg.root)
                            } else {
                                // deep zone: kid[0] bedrock gradient, kid[2]
                                // small deep rules (always-true branch),
                                // kid[3] deepslate gradient — kid[1] is
                                // statically false here.
                                let mut st: Option<u16> = None;
                                if y <= fp.bed_true {
                                    st = Some(fp.bed_state);
                                } else if y < fp.bed_false
                                    && walk.gradient_at(&fp.bed_name, fp.bed_true, fp.bed_false, bx, y, bz)
                                {
                                    st = Some(fp.bed_state);
                                }
                                if st.is_none() {
                                    // kid[2] gate: resolve its biome cond once
                                    // per (column, quartY) — the branch is
                                    // biome-gated and false for the vast
                                    // majority of quarts.
                                    let mut st2: Option<u16> = None;
                                    if fp.gate_cond != usize::MAX {
                                        let qy = (((y - cfg.min_y) >> 2).max(0) as usize).min(quarts - 1);
                                        let gslot = (col_base as usize) * quarts + qy;
                                        let g = gate_cache[gslot];
                                        let gate = if g != 255 {
                                            g != 0
                                        } else {
                                            let v = walk.test(fp.gate_cond);
                                            gate_cache[gslot] = v as u8;
                                            v
                                        };
                                        if gate {
                                            st2 = walk.try_apply(fp.deep_then);
                                        }
                                    } else {
                                        st2 = walk.try_apply(fp.deep_then);
                                    }
                                    st = st2;
                                    if st.is_none() {
                                        if y <= fp.ds_true {
                                            st = Some(fp.ds_state);
                                        } else if y < fp.ds_false
                                            && walk.gradient_at(&fp.ds_name, fp.ds_true, fp.ds_false, bx, y, bz)
                                        {
                                            st = Some(fp.ds_state);
                                        }
                                    }
                                }
                                st
                            }
                        } else {
                            walk.try_apply(cfg.root)
                        };
                        if let Some(st) = st {
                            if let Some((tx, ty, tz)) = trace_pos() {
                                if tx == bx && tz == bz {
                                    let name = if (st as usize) < super::surface::MAT_NAMES.len() {
                                        super::surface::MAT_NAMES[st as usize].to_string()
                                    } else {
                                        format!("state#{}", st)
                                    };
                                    eprintln!("APPLY y={y} rule=root -> {name}");
                                }
                            }
                            out[bi] = st;
                        } else if let Some((tx, ty, tz)) = trace_pos() {
                            if tx == bx && tz == bz {
                                eprintln!("APPLY y={y} rule=root -> NO MATCH");
                            }
                        }
                    }
                }
                y -= 1;
            }
        }
    }
}
