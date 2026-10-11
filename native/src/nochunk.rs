// NoiseChunk-equivalent chunk fill: the exact 1.21.8 noise-stage port.
//
// Mirrors NoiseBasedChunkGenerator.doFill + NoiseChunk + Aquifer.NoiseBasedAquifer
// + OreVeinifier + MaterialRuleList. Every loop order and float expression is
// transcribed from the decompiled vanilla sources (see worklog for the map).
//
// Deliberate simplifications (value-identical, documented in worklog):
//   - cache_2d / cache_once / cache_all_in_cell / flat_cache markers are
//     pass-through (pure functions; vanilla memoization never changes values),
//     except final_density's CacheAllInCell which is materialized (cell_cache)
//     because vanilla wraps it in Java code, not JSON.
//   - postprocessing marks (shouldScheduleFluidUpdate) are skipped: they carry
//     no block data, and the Bukkit noise path cannot express them.

use crate::density::{Compiled, Env, InterpState, MemoCtx, Mode, Pt, WKind};

// ---------------------------------------------------------------------------
// frozen block ids (repo-root block_registry.txt; ore ids appended)
// ---------------------------------------------------------------------------

pub const AIR: u16 = 0;
pub const STONE: u16 = 1;
pub const WATER: u16 = 3;
pub const GRANITE: u16 = 9;
pub const TUFF: u16 = 12;
pub const LAVA: u16 = 17;
pub const COPPER_ORE: u16 = 18;
pub const RAW_COPPER_BLOCK: u16 = 19;
pub const DEEPSLATE_IRON_ORE: u16 = 20;
pub const RAW_IRON_BLOCK: u16 = 21;

/// one chunk: 16*16*384 u16 ids, layout (y-minY)*256 + z*16 + x
pub const OUT_LEN: usize = 16 * 16 * 384;

// ---------------------------------------------------------------------------
// helpers (exact vanilla bit ops)
// ---------------------------------------------------------------------------

#[inline]
fn floor_div(a: i32, b: i32) -> i32 {
    let q = a / b;
    if (a % b != 0) && ((a < 0) != (b < 0)) {
        q - 1
    } else {
        q
    }
}

#[inline]
fn block_pos_as_long(x: i32, y: i32, z: i32) -> i64 {
    ((x as i64 & 0x3FF_FFFF) << 38) | (y as i64 & 0xFFF) | ((z as i64 & 0x3FF_FFFF) << 12)
}

#[inline]
fn pos_x(p: i64) -> i32 {
    (p >> 38) as i32
}
#[inline]
fn pos_y(p: i64) -> i32 {
    ((p << 52) >> 52) as i32
}
#[inline]
fn pos_z(p: i64) -> i32 {
    ((p << 26) >> 38) as i32
}

#[inline]
fn similarity(first: i64, second: i64) -> f64 {
    1.0 - ((second - first).abs() as f64) / 25.0
}

#[inline]
fn mth_map(input: f64, in_min: f64, in_max: f64, out_min: f64, out_max: f64) -> f64 {
    let t = (input - in_min) / (in_max - in_min);
    out_min + t * (out_max - out_min)
}

#[inline]
fn mth_clamped_map(input: f64, in_min: f64, in_max: f64, out_min: f64, out_max: f64) -> f64 {
    let t = (input - in_min) / (in_max - in_min);
    if t < 0.0 {
        out_min
    } else if t > 1.0 {
        out_max
    } else {
        out_min + t * (out_max - out_min)
    }
}

const FLOWING_UPDATE_SIMILARITY: f64 = similarity_i(100, 144);
const SURFACE_SAMPLING_OFFSETS_IN_CHUNKS: [(i32, i32); 13] = [
    (0, 0),
    (-2, -1),
    (-1, -1),
    (0, -1),
    (1, -1),
    (-3, 0),
    (-2, 0),
    (-1, 0),
    (1, 0),
    (-2, 1),
    (-1, 1),
    (0, 1),
    (1, 1),
];

const fn similarity_i(a: i64, b: i64) -> f64 {
    // const-compatible approximation of similarity(a, b)
    let d = if b > a { b - a } else { a - b };
    1.0 - d as f64 / 25.0
}

// ---------------------------------------------------------------------------
// aquifer state
// ---------------------------------------------------------------------------

/// (fluid_level, fluid_type) — vanilla Aquifer.FluidStatus.
type FluidStatus = (i32, u16);

struct AquiferState {
    cache: Vec<Option<FluidStatus>>,
    loc_cache: Vec<i64>,
    min_grid_x: i32,
    min_grid_y: i32,
    min_grid_z: i32,
    grid_size_x: i32,
    grid_size_z: i32,
}

impl AquiferState {
    fn new(min_y: i32, height: i32) -> AquiferState {
        let min_grid_y = floor_div(min_y, 12) - 1;
        let max_gy = floor_div(min_y + height, 12) + 1;
        let gy_span = max_gy - min_grid_y + 1;
        let cells = (3 * gy_span * 3) as usize;
        AquiferState {
            cache: vec![None; cells],
            loc_cache: vec![i64::MAX; cells],
            min_grid_x: 0,
            min_grid_y,
            min_grid_z: 0,
            grid_size_x: 3,
            grid_size_z: 3,
        }
    }

    /// Per-chunk grid origin (vanilla: chunk pos of this NoiseChunk).
    #[inline]
    fn begin_chunk(&mut self, cx: i32, cz: i32) {
        self.min_grid_x = cx - 1;
        self.min_grid_z = cz - 1;
        for v in self.loc_cache.iter_mut() {
            *v = i64::MAX;
        }
    }

    #[inline]
    fn index(&self, gx: i32, gy: i32, gz: i32) -> usize {
        let i = (gx - self.min_grid_x) as usize;
        let i1 = (gy - self.min_grid_y) as usize;
        let i2 = (gz - self.min_grid_z) as usize;
        (i1 * self.grid_size_z as usize + i2) * self.grid_size_x as usize + i
    }
}

// ---------------------------------------------------------------------------
// chunk generator
// ---------------------------------------------------------------------------

pub struct ChunkGen<'e> {
    pub env: &'e Env,
    pub compiled: Compiled,
    /// child arena index per Interpolated slot (slice-fill evaluation).
    interp_children: Vec<u32>,
    cell_width: usize,    // 4
    cell_height: usize,   // 8
    cell_count_xz: usize, // 4
    cell_count_y: usize,  // 48
    cell_noise_min_y: i32,
    min_y: i32,
    sea_level: i32,
    aquifers_enabled: bool,
    ore_veins_enabled: bool,
    // router roots
    final_density: u32,
    initial_density: u32,
    barrier: u32,
    floodedness: u32,
    spread: u32,
    lava_noise: u32,
    erosion: u32,
    depth: u32,
    vein_toggle: u32,
    vein_ridged: u32,
    vein_gap: u32,
    // reusable per-instance state
    interp: InterpState,
    cell_cache: Vec<f64>, // 4*8*4 = 128
    /// preliminarySurfaceLevel cache (key: (x<<32)|z of quart-snapped column).
    prelim: std::collections::HashMap<i64, i32>,
    aquifer: AquiferState,
    /// Per-point DAG memo (see MemoCtx). Every evaluation context (corner
    /// point / block / probe) bumps the serial; shared subtrees then evaluate
    /// once per point instead of once per reference.
    memo: MemoCtx,
    /// Env-gated phase profiler (diagnostic only; zero cost when off).
    pub prof: Prof,
    /// Diagnostic stubs (CRUS_SKIP_*), NEVER set in production.
    skip_cellfill: bool,
    skip_material: bool,
    skip_slice: bool,
}

#[derive(Default, Debug, Clone, Copy)]
pub struct Prof {
    pub enabled: bool,
    pub fill_slice_ns: u64,
    pub cellfill_ns: u64,
    pub material_ns: u64,
    pub blocks_subst_pos: u64,
    pub blocks_subst_neg: u64,
}

impl Prof {
    fn from_env() -> Prof {
        Prof {
            enabled: std::env::var("CRUS_PROFILE").ok().as_deref() == Some("1"),
            ..Default::default()
        }
    }
}

impl<'e> ChunkGen<'e> {
    pub fn new(env: &'e Env) -> ChunkGen<'e> {
        let compiled = env.compile();
        let interp_children = compiled.interp_children.clone();
        let router = |name: &str| -> u32 {
            *compiled
                .router
                .get(name)
                .unwrap_or_else(|| panic!("router field '{name}' missing"))
        };
        let cell_width = 4usize;
        let cell_height = 8usize;
        let cell_count_xz = 16 / cell_width;
        let cell_count_y = (env.wg.height as usize) / cell_height;
        let cell_noise_min_y = floor_div(env.wg.min_y, cell_height as i32);
        let sea_level = env.wg.sea_level as i32;
        let interp = InterpState::new(compiled.interp_slots, cell_count_xz, cell_count_y);

        ChunkGen {
            interp_children,
            cell_width,
            cell_height,
            cell_count_xz,
            cell_count_y,
            cell_noise_min_y,
            min_y: env.wg.min_y,
            sea_level,
            aquifers_enabled: env.wg.aquifers_enabled
                && std::env::var("CRUS_NO_AQUIFER").is_err(),
            ore_veins_enabled: env.wg.ore_veins_enabled
                && std::env::var("CRUS_NO_VEIN").is_err(),
            final_density: router("final_density"),
            initial_density: router("initial_density_without_jaggedness"),
            barrier: router("barrier"),
            floodedness: router("fluid_level_floodedness"),
            spread: router("fluid_level_spread"),
            lava_noise: router("lava"),
            erosion: router("erosion"),
            depth: router("depth"),
            vein_toggle: router("vein_toggle"),
            vein_ridged: router("vein_ridged"),
            vein_gap: router("vein_gap"),
            interp,
            cell_cache: vec![0.0; cell_width * cell_height * cell_width],
            prelim: std::collections::HashMap::new(),
            aquifer: AquiferState::new(env.wg.min_y, env.wg.height),
            memo: MemoCtx::new(compiled.arena.len()),
            env,
            compiled,
            prof: Prof::from_env(),
            skip_cellfill: std::env::var("CRUS_SKIP_CELLFILL").is_ok(),
            skip_material: std::env::var("CRUS_SKIP_MATERIAL").is_ok(),
            skip_slice: std::env::var("CRUS_SKIP_SLICE").is_ok(),
        }
    }

    #[inline]
    fn eval(&self, idx: u32, pt: Pt, mode: Mode) -> f64 {
        self.env
            .eval(&self.compiled, idx, pt, mode, &self.interp, Some(&self.memo))
    }

    /// Begin a new memo context (one point/block) and evaluate.
    #[inline]
    fn eval_fresh(&self, idx: u32, pt: Pt, mode: Mode) -> f64 {
        self.memo.next_serial();
        self.eval(idx, pt, mode)
    }

    /// Per-interpolator-child slice-cost profiler (diagnostic).
    /// Evaluates every interp child over the exact fill_slice corner points
    /// (5 x-planes x 5 z-corners x 49 y-corners = 1225) and prints timings
    /// plus a node-kind histogram of each child subtree.
    pub fn profile_slice_children(&self, cx: i32, cz: i32) {
        use std::collections::BTreeMap;
        let min_block_x = cx * 16;
        let min_block_z = cz * 16;
        let first_cell_x = floor_div(min_block_x, self.cell_width as i32);
        let first_cell_z = floor_div(min_block_z, self.cell_width as i32);
        let cw = self.cell_width as i32;
        let ch = self.cell_height as i32;
        let mut points: Vec<Pt> = Vec::with_capacity(
            (self.cell_count_xz + 1)
                * (self.cell_count_xz + 1)
                * (self.cell_count_y + 1),
        );
        for i in 0..=self.cell_count_xz {
            let corner_x = (first_cell_x + i as i32) * cw;
            for j in 0..=self.cell_count_xz {
                let corner_z = (first_cell_z + j as i32) * cw;
                for y in 0..=self.cell_count_y {
                    let corner_y = (y as i32 + self.cell_noise_min_y) * ch;
                    points.push(Pt { x: corner_x, y: corner_y, z: corner_z });
                }
            }
        }
        let mut total_ns = 0u64;
        for (slot, &child) in self.interp_children.iter().enumerate() {
            let t0 = std::time::Instant::now();
            let mut sink = 0f64;
            for &pt in &points {
                sink += self.eval_fresh(child, pt, Mode::Slice);
            }
            let dt = t0.elapsed().as_nanos() as u64;
            total_ns += dt;
            let hist = self.describe_child(child);
            let brief: Vec<String> = hist
                .iter()
                .map(|(k, n)| format!("{k}x{n}"))
                .collect();
            println!(
                "slot {:2} child {:4} total {:>8.2}ms per-pt {:>6.2}us sink {:e} [{}]",
                slot,
                child,
                dt as f64 / 1e6,
                dt as f64 / points.len() as f64 / 1e3,
                sink,
                brief.join(" ")
            );
        }
        println!("SUM over {} slots: {:.2}ms", self.interp_children.len(), total_ns as f64 / 1e6);
    }

    /// Node-kind histogram of the subtree rooted at `root` (iterative DFS).
    fn describe_child(&self, root: u32) -> std::collections::BTreeMap<&'static str, usize> {
        use std::collections::BTreeMap;
        let mut hist: BTreeMap<&'static str, usize> = BTreeMap::new();
        let mut stack = vec![root];
        while let Some(i) = stack.pop() {
            match &self.compiled.arena[i as usize] {
                WKind::Const(_) => *hist.entry("const").or_default() += 1,
                WKind::Noise { .. } => *hist.entry("noise").or_default() += 1,
                WKind::ShiftedNoise { .. } => *hist.entry("shifted").or_default() += 1,
                WKind::ShiftA { .. } => *hist.entry("shiftA").or_default() += 1,
                WKind::ShiftB { .. } => *hist.entry("shiftB").or_default() += 1,
                WKind::OldBlendedNoise { .. } => *hist.entry("BLENDED").or_default() += 1,
                WKind::Clamp { input, .. } => {
                    *hist.entry("clamp").or_default() += 1;
                    stack.push(*input);
                }
                WKind::Mapped { input, .. } => {
                    *hist.entry("map").or_default() += 1;
                    stack.push(*input);
                }
                WKind::Ap2 { a, b, .. } => {
                    *hist.entry("ap2").or_default() += 1;
                    stack.push(*a);
                    stack.push(*b);
                }
                WKind::RangeChoice { input, when_in_range, when_out_of_range, .. } => {
                    *hist.entry("range").or_default() += 1;
                    stack.push(*input);
                    stack.push(*when_in_range);
                    stack.push(*when_out_of_range);
                }
                WKind::YClampedGradient { .. } => *hist.entry("ygrad").or_default() += 1,
                WKind::Spline { coordinate, .. } => {
                    *hist.entry("spline").or_default() += 1;
                    stack.push(*coordinate);
                }
                WKind::WeirdScaledSampler { input, .. } => {
                    *hist.entry("weird").or_default() += 1;
                    stack.push(*input);
                }
                WKind::Interpolated { child, .. } => {
                    *hist.entry("INTERP").or_default() += 1;
                    stack.push(*child);
                }
                WKind::Cache2d { child }
                | WKind::CacheOnce { child }
                | WKind::FlatCache { child }
                | WKind::CacheAllInCell { child } => {
                    *hist.entry("cache").or_default() += 1;
                    stack.push(*child);
                }
                WKind::MulOrAdd { input, .. } => {
                    *hist.entry("muloradd").or_default() += 1;
                    stack.push(*input);
                }
                WKind::EndIslands => *hist.entry("endisl").or_default() += 1,
            }
        }
        hist
    }

    /// Vanilla NoiseChunk.preliminarySurfaceLevel (quart-snapped, cached).
    fn preliminary_surface_level(&mut self, x: i32, z: i32) -> i32 {
        let bx = (x >> 2) << 2;
        let bz = (z >> 2) << 2;
        let key = ((bx as i64) << 32) | (bz as i64 & 0xFFFF_FFFF);
        if let Some(v) = self.prelim.get(&key) {
            return *v;
        }
        let mut result = i32::MAX;
        let mut y = self.min_y + self.env.wg.height;
        while y >= self.min_y {
            let d = self.eval_fresh(
                self.initial_density,
                Pt { x: bx, y, z: bz },
                Mode::Direct,
            );
            if d > 0.390625 {
                result = y;
                break;
            }
            y -= self.cell_height as i32;
        }
        self.prelim.insert(key, result);
        result
    }

    /// Vanilla NoiseBasedChunkGenerator.doFill.
    pub fn fill(&mut self, cx: i32, cz: i32, out: &mut [u16; OUT_LEN]) {
        let mut p = self.prof;
        p.fill_slice_ns = 0;
        p.cellfill_ns = 0;
        p.material_ns = 0;
        p.blocks_subst_pos = 0;
        p.blocks_subst_neg = 0;
        let prof = p.enabled;
        out.fill(AIR);
        let min_block_x = cx * 16;
        let min_block_z = cz * 16;
        let first_cell_x = floor_div(min_block_x, self.cell_width as i32);
        let first_cell_z = floor_div(min_block_z, self.cell_width as i32);
        let cw = self.cell_width as i32;
        let ch = self.cell_height as i32;
        self.aquifer.begin_chunk(cx, cz);

        // initializeForFirstCellX: fill slice0 at x-plane first_cell_x
        let t_fs0 = if prof { Some(std::time::Instant::now()) } else { None };
        self.fill_slice(first_cell_x, first_cell_z, true);
        if let Some(t) = t_fs0 {
            p.fill_slice_ns += t.elapsed().as_nanos() as u64;
        }

        for i2 in 0..self.cell_count_xz {
            // advanceCellX(i2): fill slice1 at x-plane first_cell_x+i2+1
            let t_fs = if prof { Some(std::time::Instant::now()) } else { None };
            self.fill_slice(first_cell_x + i2 as i32 + 1, first_cell_z, false);
            if let Some(t) = t_fs {
                p.fill_slice_ns += t.elapsed().as_nanos() as u64;
            }
            let cell_start_block_x = (first_cell_x + i2 as i32) * cw;

            for i3 in 0..self.cell_count_xz {
                let cell_start_block_z = (first_cell_z + i3 as i32) * cw;

                for i5 in (0..self.cell_count_y).rev() {
                    // selectCellYZ(i5, i3)
                    self.interp.select_cell_yz(i5, i3);
                    let cell_start_block_y = (i5 as i32 + self.cell_noise_min_y) * ch;

                    // CacheAllInCell (vanilla Java-level wrap of final_density):
                    // fill 128 in-cell values via lerp3 path (fillingCell=true)
                    let t_cf = if prof { Some(std::time::Instant::now()) } else { None };
                    let mut idx = 0usize;
                    let mut iy = self.cell_height as i32 - 1;
                    while iy >= 0 {
                        let mut ix = 0i32;
                        while ix < cw {
                            let mut iz = 0i32;
                            while iz < cw {
                                self.interp.cell_frac = (
                                    ix as f64 / self.cell_width as f64,
                                    iy as f64 / self.cell_height as f64,
                                    iz as f64 / self.cell_width as f64,
                                );
                                let pt = Pt {
                                    x: cell_start_block_x + ix,
                                    y: cell_start_block_y + iy,
                                    z: cell_start_block_z + iz,
                                };
                                let cellfill_skip = self.skip_cellfill;
                                self.cell_cache[idx] = if cellfill_skip {
                                    0.0
                                } else {
                                    self.eval_fresh(self.final_density, pt, Mode::CellFill)
                                };
                                idx += 1;
                                iz += 1;
                            }
                            ix += 1;
                        }
                        iy -= 1;
                    }
                    if let Some(t) = t_cf {
                        p.cellfill_ns += t.elapsed().as_nanos() as u64;
                    }

                    let t_mt = if prof { Some(std::time::Instant::now()) } else { None };
                    for i6 in (0..self.cell_height as i32).rev() {
                        let block_y = (self.cell_noise_min_y + i5 as i32) * ch + i6;
                        self.interp.update_for_y(i6 as f64 / self.cell_height as f64);

                        for i9 in 0..self.cell_width as i32 {
                            self.interp.update_for_x(i9 as f64 / self.cell_width as f64);
                            let block_x = cell_start_block_x + i9;

                            for i12 in 0..self.cell_width as i32 {
                                self.interp.update_for_z(i12 as f64 / self.cell_width as f64);
                                let block_z = cell_start_block_z + i12;

                                let skip_material = self.skip_material;
                                let block = if skip_material {
                                    1u16
                                } else {
                                    self.material_rule(
                                        block_x,
                                        block_y,
                                        block_z,
                                        i9,
                                        i6,
                                        i12,
                                    )
                                };
                                if block != AIR {
                                    let out_idx = ((block_y - self.min_y) * 256
                                        + (block_z - min_block_z) * 16
                                        + (block_x - min_block_x))
                                        as usize;
                                    out[out_idx] = block;
                                }
                            }
                        }
                    }
                    if let Some(t) = t_mt {
                        p.material_ns += t.elapsed().as_nanos() as u64;
                    }
                }
            }

            self.interp.swap_slices();
        }
        self.prof = p;
    }

    /// Vanilla NoiseChunk.fillSlice: each interpolator's wrapped function is
    /// evaluated at the (x, y, z) corner (Mode::Slice — child direct).
    fn fill_slice(&mut self, start_cell_x: i32, first_cell_z: i32, is_first: bool) {
        let corner_x = start_cell_x * self.cell_width as i32;
        let cw = self.cell_width as i32;
        let ch = self.cell_height as i32;
        for i in 0..=self.cell_count_xz {
            let corner_z = (first_cell_z + i as i32) * cw;
            for y in 0..=self.cell_count_y {
                let corner_y = (y as i32 + self.cell_noise_min_y) * ch;
                let pt = Pt { x: corner_x, y: corner_y, z: corner_z };
                for (slot, &child) in self.interp_children.iter().enumerate() {
                    let v = if self.skip_slice {
                        0.0
                    } else if slot == 0 {
                        // bump memo context once per corner point; remaining
                        // slots share it (cross-slot subexpression reuse)
                        self.eval_fresh(child, pt, Mode::Slice)
                    } else {
                        self.eval(child, pt, Mode::Slice)
                    };
                    let slot = slot as u16;
                    if is_first {
                        self.interp.slice0_set(slot, i as usize, y, v);
                    } else {
                        self.interp.slice1_set(slot, i as usize, y, v);
                    }
                }
            }
        }
    }

    #[inline]
    fn cell_cache_value(&self, iy: i32, ix: i32, iz: i32) -> f64 {
        let cw = self.cell_width;
        self.cell_cache
            [(((self.cell_height as i32 - 1 - iy) as usize) * cw + ix as usize) * cw + iz as usize]
    }

    /// Material rule per block (vanilla MaterialRuleList).
    #[inline]
    fn material_rule(&mut self, x: i32, y: i32, z: i32, ix: i32, iy: i32, iz: i32) -> u16 {
        let pt = Pt { x, y, z };
        let substance = self.cell_cache_value(iy, ix, iz);
        if self.prof.enabled {
            if substance > 0.0 {
                self.prof.blocks_subst_pos += 1;
            } else {
                self.prof.blocks_subst_neg += 1;
            }
        }
        // rule 1: aquifer (or disabled aquifer)
        let mut state: Option<u16> = if self.aquifers_enabled {
            self.aquifer_compute_substance(pt, substance)
        } else if substance > 0.0 {
            None
        } else {
            let g = self.global_fluid(y);
            Some(aquifer_at(g, y))
        };
        // rule 2: ore veins
        if state.is_none() && self.ore_veins_enabled {
            state = self.ore_vein(pt);
        }
        state.unwrap_or(STONE)
    }

    /// Global fluid picker (NoiseBasedChunkGenerator.createFluidPicker):
    /// y < min(-54, seaLevel) ? FluidStatus(-54, LAVA) : FluidStatus(seaLevel, WATER).
    #[inline]
    fn global_fluid(&self, y: i32) -> FluidStatus {
        if y < self.sea_level.min(-54) {
            (-54, LAVA)
        } else {
            (self.sea_level, WATER)
        }
    }

    /// Vanilla NoiseBasedAquifer.computeSubstance.
    fn aquifer_compute_substance(&mut self, pt: Pt, substance: f64) -> Option<u16> {
        let x = pt.x;
        let y = pt.y;
        let z = pt.z;
        if substance > 0.0 {
            return None;
        }
        let global = self.global_fluid(y);
        let g_at = aquifer_at(global, y);
        if g_at == LAVA {
            return Some(LAVA);
        }

        let gx = floor_div(x - 5, 16);
        let gy = floor_div(y + 1, 12);
        let gz = floor_div(z - 5, 16);
        let mut i6 = i64::MAX;
        let mut i7 = i64::MAX;
        let mut i8 = i64::MAX;
        let mut i9 = i64::MAX;
        let mut l0 = 0i64;
        let mut l1 = 0i64;
        let mut l2 = 0i64;
        let mut l3 = 0i64;

        for i10 in 0..=1i32 {
            for i11 in -1..=1i32 {
                for i12 in 0..=1i32 {
                    let gxx = gx + i10;
                    let gyy = gy + i11;
                    let gzz = gz + i12;
                    let index = self.aquifer.index(gxx, gyy, gzz);
                    let packed = if self.aquifer.loc_cache[index] != i64::MAX {
                        self.aquifer.loc_cache[index]
                    } else {
                        let mut rng = self.env.aquifer_random.clone().at(gxx, gyy, gzz);
                        let p = block_pos_as_long(
                            gxx * 16 + rng.next_int_bound(10),
                            gyy * 12 + rng.next_int_bound(9),
                            gzz * 16 + rng.next_int_bound(10),
                        );
                        self.aquifer.loc_cache[index] = p;
                        p
                    };
                    let dx = pos_x(packed) as i64 - x as i64;
                    let dy = pos_y(packed) as i64 - y as i64;
                    let dz = pos_z(packed) as i64 - z as i64;
                    let dist = dx * dx + dy * dy + dz * dz;
                    if i6 >= dist {
                        l3 = l2;
                        l2 = l1;
                        l1 = l0;
                        l0 = packed;
                        i9 = i8;
                        i8 = i7;
                        i7 = i6;
                        i6 = dist;
                    } else if i7 >= dist {
                        l3 = l2;
                        l2 = l1;
                        l1 = packed;
                        i9 = i8;
                        i8 = i7;
                        i7 = dist;
                    } else if i8 >= dist {
                        l3 = l2;
                        l2 = packed;
                        i9 = i8;
                        i8 = dist;
                    } else if i9 >= dist {
                        l3 = packed;
                        i9 = dist;
                    }
                }
            }
        }

        let aquifer_status = self.aquifer_status(l0);
        let d = similarity(i6, i7);
        let block = aquifer_at(aquifer_status, y);
        if d <= 0.0 {
            return Some(block);
        }
        if block == WATER {
            let below = aquifer_at(self.global_fluid(y - 1), y - 1);
            if below == LAVA {
                return Some(block);
            }
        }
        let status1 = self.aquifer_status(l1);
        let mut barrier_cache = f64::NAN;
        let d1 = d * self.calculate_pressure(pt, &mut barrier_cache, aquifer_status, status1);
        if substance + d1 > 0.0 {
            return None;
        }
        let status2 = self.aquifer_status(l2);
        let d2 = similarity(i6, i8);
        if d2 > 0.0 {
            let d3 = d * d2 * self.calculate_pressure(pt, &mut barrier_cache, aquifer_status, status2);
            if substance + d3 > 0.0 {
                return None;
            }
        }
        let d3 = similarity(i7, i8);
        if d3 > 0.0 {
            let d4 = d * d3 * self.calculate_pressure(pt, &mut barrier_cache, status1, status2);
            if substance + d4 > 0.0 {
                return None;
            }
        }
        let _ = FLOWING_UPDATE_SIMILARITY; // postprocessing only
        let _ = l3;
        Some(block)
    }

    /// Vanilla NoiseBasedAquifer.calculatePressure.
    fn calculate_pressure(
        &mut self,
        pt: Pt,
        barrier: &mut f64,
        first: FluidStatus,
        second: FluidStatus,
    ) -> f64 {
        self.memo.next_serial(); // fresh pt context (block coords, barrier)
        let y = pt.y;
        let first_at = aquifer_at(first, y);
        let second_at = aquifer_at(second, y);
        if (first_at == LAVA && second_at == WATER) || (first_at == WATER && second_at == LAVA) {
            return 2.0;
        }
        let abs = (first.0 - second.0).abs();
        if abs == 0 {
            return 0.0;
        }
        let d = 0.5 * (first.0 + second.0) as f64;
        let d1 = y as f64 + 0.5 - d;
        let d2 = abs as f64 / 2.0;
        let d9 = d2 - d1.abs();
        let d11 = if d1 > 0.0 {
            let d10 = d9;
            if d10 > 0.0 {
                d10 / 1.5
            } else {
                d10 / 2.5
            }
        } else {
            let d10 = 3.0 + d9;
            if d10 > 0.0 {
                d10 / 3.0
            } else {
                d10 / 10.0
            }
        };
        let d12 = if !(d11 < -2.0) && !(d11 > 2.0) {
            if barrier.is_nan() {
                let v = self.eval(self.barrier, pt, Mode::Direct);
                *barrier = v;
                v
            } else {
                *barrier
            }
        } else {
            0.0
        };
        2.0 * (d12 + d11)
    }

    /// Vanilla NoiseBasedAquifer.getAquiferStatus (grid-memoized).
    fn aquifer_status(&mut self, packed: i64) -> FluidStatus {
        let x = pos_x(packed);
        let y = pos_y(packed);
        let z = pos_z(packed);
        let index = self
            .aquifer
            .index(floor_div(x, 16), floor_div(y, 12), floor_div(z, 16));
        if let Some(v) = self.aquifer.cache[index] {
            return v;
        }
        let v = self.compute_aquifer_fluid(x, y, z);
        self.aquifer.cache[index] = Some(v);
        v
    }

    /// Vanilla NoiseBasedAquifer.computeFluid.
    fn compute_aquifer_fluid(&mut self, x: i32, y: i32, z: i32) -> FluidStatus {
        let fluid_status = self.global_fluid(y);
        let mut min_surface = i32::MAX;
        let i1 = y + 12;
        let i2 = y - 12;
        let mut fluid_present = false;
        for &(ox, oz) in SURFACE_SAMPLING_OFFSETS_IN_CHUNKS.iter() {
            let sx = x + ox * 16;
            let sz = z + oz * 16;
            let level = self.preliminary_surface_level(sx, sz);
            let i6 = level + 8;
            let is_center = ox == 0 && oz == 0;
            if is_center && i2 > i6 {
                return fluid_status;
            }
            let flag2 = i1 > i6;
            if flag2 || is_center {
                let status = self.global_fluid(i6);
                let at = aquifer_at(status, i6);
                if at != AIR {
                    if is_center {
                        fluid_present = true;
                    }
                    if flag2 {
                        return status;
                    }
                }
            }
            min_surface = min_surface.min(level);
        }
        let surface = self.compute_surface_level(x, y, z, fluid_status, min_surface, fluid_present);
        let kind = self.compute_fluid_type(x, y, z, fluid_status.1, surface);
        (surface, kind)
    }

    /// Vanilla NoiseBasedAquifer.computeSurfaceLevel.
    fn compute_surface_level(
        &mut self,
        x: i32,
        y: i32,
        z: i32,
        fluid_status: FluidStatus,
        max_surface_level: i32,
        fluid_present: bool,
    ) -> i32 {
        let pt = Pt { x, y, z };
        let d: f64;
        let d1: f64;
        self.memo.next_serial(); // fresh pt context (aquifer cell coords)
        let erosion = self.eval(self.erosion, pt, Mode::Direct);
        let depth = self.eval(self.depth, pt, Mode::Direct);
        // isDeepDarkRegion: erosion < (double)(float)-0.225 && depth > (double)(float)0.9
        if erosion < (-0.225f32 as f64) && depth > (0.9f32 as f64) {
            d = -1.0;
            d1 = -1.0;
        } else {
            let i = max_surface_level + 8 - y;
            let d2 = if fluid_present {
                mth_clamped_map(i as f64, 0.0, 64.0, 1.0, 0.0)
            } else {
                0.0
            };
            let d3 = {
                let v = self.eval(self.floodedness, pt, Mode::Direct);
                if v < -1.0 {
                    -1.0
                } else if v > 1.0 {
                    1.0
                } else {
                    v
                }
            };
            let d4 = mth_map(d2, 1.0, 0.0, -0.3, 0.8);
            let d5 = mth_map(d2, 1.0, 0.0, -0.8, 0.4);
            d = d3 - d5;
            d1 = d3 - d4;
        }
        if d1 > 0.0 {
            fluid_status.0
        } else if d > 0.0 {
            self.compute_randomized_fluid_surface_level(x, y, z, max_surface_level)
        } else {
            -32512
        }
    }

    /// Vanilla NoiseBasedAquifer.computeRandomizedFluidSurfaceLevel.
    fn compute_randomized_fluid_surface_level(
        &mut self,
        x: i32,
        y: i32,
        z: i32,
        max_surface_level: i32,
    ) -> i32 {
        let i3 = floor_div(y, 40);
        let i5 = i3 * 40 + 20;
        let pt = Pt {
            x: floor_div(x, 16),
            y: i3,
            z: floor_div(z, 16),
        };
        self.memo.next_serial(); // fresh pt context (spread grid coords)
        let d = self.eval(self.spread, pt, Mode::Direct) * 10.0;
        // Mth.quantize(d, 3) = floor(d / 3) * 3
        let i7 = (d / 3.0).floor() as i32 * 3;
        (i5 + i7).min(max_surface_level)
    }

    /// Vanilla NoiseBasedAquifer.computeFluidType.
    fn compute_fluid_type(&mut self, x: i32, y: i32, z: i32, base: u16, surface_level: i32) -> u16 {
        let mut kind = base;
        if surface_level <= -10 && surface_level != -32512 && base != LAVA {
            let pt = Pt {
                x: floor_div(x, 64),
                y: floor_div(y, 40),
                z: floor_div(z, 64),
            };
            self.memo.next_serial(); // fresh pt context (lava grid coords)
            let d = self.eval(self.lava_noise, pt, Mode::Direct);
            if d.abs() > 0.3 {
                kind = LAVA;
            }
        }
        kind
    }

    /// Vanilla OreVeinifier.create closure.
    fn ore_vein(&self, pt: Pt) -> Option<u16> {
        self.memo.next_serial(); // fresh pt context (block coords)
        let d = self.eval(self.vein_toggle, pt, Mode::Block);
        let y = pt.y;
        let (ore, raw, filler, ty_min_y, ty_max_y) = if d > 0.0 {
            (COPPER_ORE, RAW_COPPER_BLOCK, GRANITE, 0, 50)
        } else {
            (DEEPSLATE_IRON_ORE, RAW_IRON_BLOCK, TUFF, -60, -8)
        };
        let abs = d.abs();
        let i1 = ty_max_y - y;
        let i2 = y - ty_min_y;
        if i2 < 0 || i1 < 0 {
            return None;
        }
        let min_edge = i1.min(i2);
        let d1 = mth_clamped_map(min_edge as f64, 0.0, 20.0, -0.2, 0.0);
        if abs + d1 < (0.4f32 as f64) {
            return None;
        }
        let mut rng = self.env.ore_random.clone().at(pt.x, y, pt.z);
        if rng.next_f32() > 0.7f32 {
            return None;
        }
        let ridged = self.eval(self.vein_ridged, pt, Mode::Block);
        if ridged >= 0.0 {
            return None;
        }
        let d2 = mth_clamped_map(
            abs,
            0.4f32 as f64,
            0.6f32 as f64,
            0.1f32 as f64,
            0.3f32 as f64,
        );
        if (rng.next_f32() as f64) < d2 {
            let gap = self.eval(self.vein_gap, pt, Mode::Block);
            if gap > (-0.3f32 as f64) {
                return if rng.next_f32() < 0.02f32 {
                    Some(raw)
                } else {
                    Some(ore)
                };
            }
        }
        Some(filler)
    }
}

/// Aquifer.FluidStatus.at(y): y < fluidLevel ? fluidType : AIR.
#[inline]
fn aquifer_at(status: FluidStatus, y: i32) -> u16 {
    if y < status.0 {
        status.1
    } else {
        AIR
    }
}
