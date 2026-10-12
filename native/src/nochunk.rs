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

use crate::density::{compile_cellfill, CellOp, Compiled, Env, InterpState, MemoCtx, Mode, Pt, WKind};

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
    /// Per BASE-cell summary: all 12 neighborhood statuses equal + the shared
    /// status. The 12-cell neighborhood depends only on the base cell
    /// (gx, gy, gz), never on the block itself — so the all-same verdict and
    /// the uniform status are pure functions of the base cell and can be
    /// memoized. Reset per chunk together with the other caches.
    summary: Vec<Option<(bool, FluidStatus)>>,
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
            summary: vec![None; cells],
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
        // Vanilla allocates fresh caches per NoiseChunk (per chunk); our
        // generator persists across fills and the grid origin re-bases every
        // chunk, so BOTH caches must be reset or stale cells alias into the
        // new chunk's slots (correctness bug: wrong fluid statuses).
        for v in self.loc_cache.iter_mut() {
            *v = i64::MAX;
        }
        for v in self.cache.iter_mut() {
            *v = None;
        }
        for v in self.summary.iter_mut() {
            *v = None;
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

/// Cap on per-chunk fluid-update marks (vanilla has no cap; observed counts
/// are well under 10k/chunk — overflow just skips extra marks).
const MARK_CAP: usize = 65535;

pub struct ChunkGen<'e> {
    /// vanilla shouldScheduleFluidUpdate for the last aquifer eval.
    sched: bool,
    /// packed BlockPos list of fluid-update marks for the current chunk.
    marks: Vec<i64>,
    /// per-chunk structure beardifier (vanilla Beardifier contribution); the
    /// plugin ships the real vanilla piece list per chunk via setBeard.
    beard: crate::beard::Beard,
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
    /// CellFill fast path: slot of the final_density interpolated root.
    final_density_slot: Option<u16>,
    /// Compiled CellFill program (None -> interpreter fallback).
    cellfill_prog: Option<Vec<CellOp>>,
    cellfill_stack: Vec<f64>,
    /// Per-(dx,dy) hoisted trilinear rows: per slot [row1, row2] so the
    /// per-block cost is a single z-lerp per Lerp3 slot (bit-exact: the
    /// hoisted x/y lerps are the same ops with the same inputs).
    cellfill_rows: Vec<f64>,
    /// Slot ids actually referenced by the cellfill program (hoist these only).
    cellfill_slots: Vec<u16>,
    /// preliminarySurfaceLevel cache (key: (x<<32)|z of quart-snapped column).
    prelim: std::collections::HashMap<i64, i32>,
    /// Cross-chunk corner-plane cache: the last fill's final x-plane equals
    /// the next fill's initial plane when cx increments by 1 (same cz row).
    /// Values are deterministic per corner point -> reuse is value-identical.
    plane_cache_x: i64,
    plane_cache_z: i32,
    plane_cache: Vec<f64>,
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
    /// Perf-experiment switch (CRUS_NO_ORE), NEVER set in production.
    no_ore: bool,
    /// Parallel corner evaluation threads for fill_slice (CRUS_FILL_THREADS,
    /// default 1 = serial). Corners are independent pure functions of
    /// position; per-thread memo keeps results bit-identical.
    slice_threads: usize,
}

#[derive(Default, Debug, Clone, Copy)]
pub struct Prof {
    pub enabled: bool,
    pub fill_slice_ns: u64,
    pub cellfill_ns: u64,
    pub material_ns: u64,
    pub select_ns: u64,
    pub plane_hits: u64,
    pub blocks_subst_pos: u64,
    pub blocks_subst_neg: u64,
    pub blocks_ore_reached: u64,
    pub blocks_aquifer_air: u64,
}

impl Prof {
    fn from_env() -> Prof {
        Prof {
            enabled: std::env::var("CRUS_PROFILE").ok().as_deref() == Some("1"),
            ..Default::default()
        }
    }

    /// Zero the phase counters (bench does this after warmup).
    pub fn reset_phases(&mut self) {
        self.fill_slice_ns = 0;
        self.cellfill_ns = 0;
        self.material_ns = 0;
        self.blocks_ore_reached = 0;
        self.blocks_aquifer_air = 0;
        self.select_ns = 0;
        self.plane_hits = 0;
        self.blocks_subst_pos = 0;
        self.blocks_subst_neg = 0;
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
            sched: false,
            marks: Vec::with_capacity(1024),
            beard: crate::beard::Beard::default(),
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
            // fast path: vanilla wraps final_density in an interpolated marker;
            // CellFill then reduces to a pure lerp3 of corner values.
            final_density_slot: match &compiled.arena[compiled.router["final_density"] as usize] {
                WKind::Interpolated { slot, .. } => Some(*slot),
                _ => None,
            },
            cellfill_prog: compile_cellfill(&compiled.arena, compiled.router["final_density"]),
            cellfill_stack: Vec::with_capacity(16),
            cellfill_rows: vec![0.0; compiled.interp_slots as usize * 2],
            cellfill_slots: {
                let mut seen = std::collections::BTreeSet::new();
                if let Some(prog) = &compile_cellfill(&compiled.arena, compiled.router["final_density"]) {
                    for op in prog {
                        if let CellOp::Lerp3 { slot } = op {
                            seen.insert(*slot);
                        }
                    }
                }
                seen.into_iter().collect()
            },
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
            plane_cache_x: i64::MIN,
            plane_cache_z: i32::MIN,
            plane_cache: Vec::new(),
            aquifer: AquiferState::new(env.wg.min_y, env.wg.height),
            memo: MemoCtx::new(compiled.arena.len()),
            env,
            compiled,
            prof: Prof::from_env(),
            skip_cellfill: std::env::var("CRUS_SKIP_CELLFILL").is_ok(),
            skip_material: std::env::var("CRUS_SKIP_MATERIAL").is_ok(),
            skip_slice: std::env::var("CRUS_SKIP_SLICE").is_ok(),
            no_ore: std::env::var("CRUS_NO_ORE").is_ok(),
            slice_threads: std::env::var("CRUS_FILL_THREADS")
                .ok()
                .and_then(|v| v.parse::<usize>().ok())
                .filter(|&n| n >= 1)
                .unwrap_or(1)
                .min(16),
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
        // Prof counters ACCUMULATE across fills (bench resets after warmup).
        let mut p = self.prof;
        let prof = p.enabled;
        let dbg = std::env::var("CRUS_DBG_SLICE").is_ok();
        let mut dbg_last_mt: Option<std::time::Duration> = None;
        out.fill(AIR);
        let min_block_x = cx * 16;
        let min_block_z = cz * 16;
        let first_cell_x = floor_div(min_block_x, self.cell_width as i32);
        let first_cell_z = floor_div(min_block_z, self.cell_width as i32);
        let cw = self.cell_width as i32;
        let ch = self.cell_height as i32;
        let t_bc = if prof { Some(std::time::Instant::now()) } else { None };
        self.aquifer.begin_chunk(cx, cz);
        self.marks.clear();
        // NOTE: the beard is NOT cleared here — the plugin ships a fresh list
        // per chunk via the atomic fillBeard JNI call (set_beard + fill), so
        // there is no stale-structure leak through the ctx pool.
        if let Some(t) = t_bc {
            p.select_ns += t.elapsed().as_nanos() as u64; // reuse select bucket as "misc"
        }

        // initializeForFirstCellX: fill slice0 at x-plane first_cell_x
        let t_fs0 = if prof { Some(std::time::Instant::now()) } else { None };
        let dbg = std::env::var("CRUS_DBG_SLICE").is_ok();
        let t0 = std::time::Instant::now();
        let plane_slots = self.interp_children.len() as usize;
        let plane_len = plane_slots * (self.cell_count_xz + 1) * (self.cell_count_y + 1);
        // plane_cache_x stores the plane's BLOCK-space corner x
        let cache_hit = self.plane_cache_x == (first_cell_x * cw) as i64
            && self.plane_cache_z == first_cell_z
            && self.plane_cache.len() == plane_len;
        if cache_hit {
            p.plane_hits += 1;
        }
        if cache_hit {
            // previous fill's final plane == this fill's initial plane
            self.interp.copy_slice0_from(&self.plane_cache);
        } else {
            self.fill_slice(first_cell_x, first_cell_z, true);
        }
        if dbg {
            eprintln!("slice0 plane_x={} hit={}: {:?}", first_cell_x * cw, cache_hit, t0.elapsed());
        }
        if let Some(t) = t_fs0 {
            p.fill_slice_ns += t.elapsed().as_nanos() as u64;
        }

        // Pipelined plane precompute: a worker thread evaluates the NEXT
        // x-plane while the main thread processes the current cells. Values
        // are pure functions of the corner points -> bit-exact. SAFETY: the
        // worker only reads Env (lifetime 'e, outlives this call), Compiled
        // and interp_children (never mutated during fill); the handle is
        // joined inside the loop, before any of them can dangle.
        let mut pending: Option<std::thread::JoinHandle<Vec<f64>>> = None;
        let plane_len = self.interp_children.len() * (self.cell_count_xz + 1) * (self.cell_count_y + 1);
        // Pipeline: overlaps the next x-plane's corner evaluation (worker)
        // with this plane's cell work. On 2 cores it LOSES (worker steals
        // the only cell core, cache pressure); on 4+ cores it wins. Default:
        // on when >= 4 logical cores; CRUS_NO_PIPELINE=1 forces off,
        // CRUS_PIPELINE=1 forces on.
        let pipeline = match (
            std::env::var("CRUS_NO_PIPELINE").is_ok(),
            std::env::var("CRUS_PIPELINE").ok().as_deref() == Some("1"),
        ) {
            (true, _) => false,
            (false, true) => true,
            (false, false) => std::thread::available_parallelism()
                .map(|n| n.get() >= 4)
                .unwrap_or(false),
        };
        if pipeline && self.cell_count_xz > 0 && !self.skip_slice {
            let env_addr = self.env as *const Env as usize;
            let compiled_addr = &self.compiled as *const Compiled as usize;
            let children_addr = self.interp_children.as_ptr() as usize;
            let n_children = self.interp_children.len();
            let plane_x = (first_cell_x + 1) * cw;
            let cell_count_xz = self.cell_count_xz;
            let cell_count_y = self.cell_count_y;
            let cell_noise_min_y = self.cell_noise_min_y;
            pending = Some(std::thread::Builder::new()
                .stack_size(1 << 21)
                .spawn(move || {
                    let mut buf = vec![0f64; plane_len];
                    // SAFETY: see comment above; join happens before fill returns
                    let env: &Env = unsafe { &*(env_addr as *const Env) };
                    let compiled: &Compiled = unsafe { &*(compiled_addr as *const Compiled) };
                    let children: &[u32] = unsafe { std::slice::from_raw_parts(children_addr as *const u32, n_children) };
                    Self::eval_plane_into(
                        env,
                        compiled,
                        children,
                        plane_x,
                        first_cell_z,
                        cell_count_xz,
                        cell_count_y,
                        cell_noise_min_y,
                        cw,
                        ch,
                        &mut buf,
                    );
                    buf
                })
                .expect("spawn plane worker"));
        }
        for i2 in 0..self.cell_count_xz {
            // advanceCellX(i2): fill slice1 at x-plane first_cell_x+i2+1
            let t_fs = if prof { Some(std::time::Instant::now()) } else { None };
            let t0 = std::time::Instant::now();
            if let Some(handle) = pending.take() {
                let buf = handle.join().expect("plane worker panicked");
                // scatter buf into slice1 ([slot][z][y])
                let cols = self.cell_count_y + 1;
                for slot in 0..self.interp_children.len() {
                    let slot_id = slot as u16;
                    for i in 0..=self.cell_count_xz {
                        for y in 0..=self.cell_count_y {
                            let v = buf[slot * (self.cell_count_xz + 1) * cols + i * cols + y];
                            self.interp.slice1_set(slot_id, i, y, v);
                        }
                    }
                }
            } else if !self.skip_slice {
                self.fill_slice(first_cell_x + i2 as i32 + 1, first_cell_z, false);
            }
            // spawn the plane after next for overlap with these cells
            if pipeline && i2 + 2 <= self.cell_count_xz && !self.skip_slice {
                let env_addr = self.env as *const Env as usize;
                let compiled_addr = &self.compiled as *const Compiled as usize;
                let children_addr = self.interp_children.as_ptr() as usize;
                let n_children = self.interp_children.len();
                let plane_x = (first_cell_x + i2 as i32 + 2) * cw;
                let cell_count_xz = self.cell_count_xz;
                let cell_count_y = self.cell_count_y;
                let cell_noise_min_y = self.cell_noise_min_y;
                pending = Some(std::thread::Builder::new()
                    .stack_size(1 << 21)
                    .spawn(move || {
                        let mut buf = vec![0f64; plane_len];
                        // SAFETY: see comment above; joined before fill returns
                        let env: &Env = unsafe { &*(env_addr as *const Env) };
                        let compiled: &Compiled = unsafe { &*(compiled_addr as *const Compiled) };
                        let children: &[u32] = unsafe { std::slice::from_raw_parts(children_addr as *const u32, n_children) };
                        Self::eval_plane_into(
                            env,
                            compiled,
                            children,
                            plane_x,
                            first_cell_z,
                            cell_count_xz,
                            cell_count_y,
                            cell_noise_min_y,
                            cw,
                            ch,
                            &mut buf,
                        );
                        buf
                    })
                    .expect("spawn plane worker"));
            }
            let dbg2 = dbg && i2 == 0;
            if dbg2 { eprintln!("slice1 plane_x={}: {:?}", (first_cell_x + 1) * cw, t0.elapsed()); }
            if let Some(t) = t_fs {
                p.fill_slice_ns += t.elapsed().as_nanos() as u64;
            }
            let cell_start_block_x = (first_cell_x + i2 as i32) * cw;

            for i3 in 0..self.cell_count_xz {
                let cell_start_block_z = (first_cell_z + i3 as i32) * cw;

                for i5 in (0..self.cell_count_y).rev() {
                    // selectCellYZ(i5, i3)
                    let t_sel = if prof { Some(std::time::Instant::now()) } else { None };
                    self.interp.select_cell_yz(i5, i3);
                    if let Some(t) = t_sel {
                        p.select_ns += t.elapsed().as_nanos() as u64;
                    }
                    let cell_start_block_y = (i5 as i32 + self.cell_noise_min_y) * ch;

                    // CacheAllInCell (vanilla Java-level wrap of final_density):
                    // fill 128 in-cell values via lerp3 path (fillingCell=true).
                    // Branchless per-block: loop variants hoisted out of the
                    // inner z-loop; fractions via multiply (bit-identical:
                    // /4 == *0.25 and /8 == *0.125 exactly in IEEE754).
                    let t_cf = if prof || dbg { Some(std::time::Instant::now()) } else { None };
                    let cw_i = cw;
                    let dx_step = 1.0f64 / self.cell_width as f64; // exact: 0.25
                    let dy_step = 1.0f64 / self.cell_height as f64; // exact: 0.125
                    let slot_fast = self.final_density_slot;
                    let skipping = self.skip_cellfill;
                    let mut idx = 0usize;
                    let mut iy = self.cell_height as i32 - 1;
                    while iy >= 0 {
                        let dy = iy as f64 * dy_step;
                        let mut ix = 0i32;
                        while ix < cw_i {
                            let dx = ix as f64 * dx_step;
                            if skipping {
                                for _ in 0..cw_i {
                                    self.cell_cache[idx] = 0.0;
                                    idx += 1;
                                }
                            } else if self.cellfill_prog.is_some() {
                                // compiled straight-line program: bit-exact
                                // replay of the interpreter in CellFill mode.
                                // Row hoisting: dx/dy are constant across the
                                // 4 z-blocks, so each slot's first two trilinear
                                // stages (lerp_dx + lerp_dy) are computed once
                                // per row; per block only the z-lerp remains.
                                let prog = self.cellfill_prog.as_ref().unwrap();
                                let block_y = cell_start_block_y + iy;
                                let rows = &mut self.cellfill_rows;
                                for &s in &self.cellfill_slots {
                                    let n = self.interp.corners_pub(s);
                                    rows[s as usize * 2] = lerp_local(
                                        dy,
                                        lerp_local(dx, n[0], n[1]),
                                        lerp_local(dx, n[2], n[3]),
                                    );
                                    rows[s as usize * 2 + 1] = lerp_local(
                                        dy,
                                        lerp_local(dx, n[4], n[5]),
                                        lerp_local(dx, n[6], n[7]),
                                    );
                                }
                                let mut dz = 0.0f64;
                                let mut stack = [0.0f64; 16];
                                let mut sp = 0usize;
                                for _ in 0..cw_i {
                                    self.cell_cache[idx] = run_cellfill_rows(
                                        prog,
                                        &self.cellfill_rows,
                                        dz,
                                        block_y,
                                        &mut stack,
                                        &mut sp,
                                    );
                                    idx += 1;
                                    dz += dx_step;
                                }
                            } else {
                                let mut dz = 0.0f64;
                                let mut iz = 0i32;
                                for _ in 0..cw_i {
                                    let pt = Pt {
                                        x: cell_start_block_x + ix,
                                        y: cell_start_block_y + iy,
                                        z: cell_start_block_z + iz,
                                    };
                                    // nested interpolated markers (custom
                                    // datapacks) read cell_frac in CellFill
                                    self.interp.cell_frac = (dx, dy, dz);
                                    // mem=None: value-identical (all nodes pure;
                                    // vanilla recomputes diamonds as well) and
                                    // skips memo stamp overhead in this hot path
                                    self.cell_cache[idx] = self.env.eval(
                                        &self.compiled,
                                        self.final_density,
                                        pt,
                                        Mode::CellFill,
                                        &self.interp,
                                        None,
                                    );
                                    idx += 1;
                                    iz += 1;
                                    dz += dx_step;
                                }
                            }
                            ix += 1;
                        }
                        iy -= 1;
                    }
                    if let Some(t) = t_cf {
                        p.cellfill_ns += t.elapsed().as_nanos() as u64;
                    }
                    if dbg {
                        eprintln!("cell[{}, {}, {}] cf={:?} mt_prev={:?}", i2, i3, i5, t_cf.map(|t| t.elapsed()), dbg_last_mt);
                    }

                    let t_mt = if prof { Some(std::time::Instant::now()) } else { None };
                    for i6 in (0..self.cell_height as i32).rev() {
                        let block_y = (self.cell_noise_min_y + i5 as i32) * ch + i6;

                        for i9 in 0..self.cell_width as i32 {
                            let block_x = cell_start_block_x + i9;

                            for i12 in 0..self.cell_width as i32 {
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
        // save the final x-plane (slice0 after the last swap) for the next fill
        {
            let plane_x = ((first_cell_x + self.cell_count_xz as i32) * cw) as i64;
            self.interp.copy_slice0_into(&mut self.plane_cache);
            self.plane_cache_x = plane_x;
            self.plane_cache_z = first_cell_z;
        }
        // merge per-block counters (material_rule mutates self.prof directly;
        // self.prof is the live accumulator — replace, do not add, or the
        // totals compound across fills)
        p.blocks_subst_pos = self.prof.blocks_subst_pos;
        p.blocks_subst_neg = self.prof.blocks_subst_neg;
        p.blocks_ore_reached = self.prof.blocks_ore_reached;
        p.blocks_aquifer_air = self.prof.blocks_aquifer_air;
        self.prof = p;
    }

    /// Vanilla NoiseChunk.fillSlice: each interpolator's wrapped function is
    /// evaluated at the (x, y, z) corner (Mode::Slice — child direct).
    fn fill_slice(&mut self, start_cell_x: i32, first_cell_z: i32, is_first: bool) {
        let corner_x = start_cell_x * self.cell_width as i32;
        let cw = self.cell_width as i32;
        let ch = self.cell_height as i32;
        let nz = self.cell_count_xz as usize + 1;
        let ny = self.cell_count_y as usize + 1;
        let nslots = self.interp_children.len();
        let total = nz * ny;
        let nthr = self.slice_threads.min(total);
        if nthr > 1 {
            // Parallel corner evaluation: corners are pure functions of
            // (x, y, z); Mode::Slice never touches mutable interp state and
            // memo is a per-point stamp cache — per-thread MemoCtx instances
            // keep results bit-identical while dropping the shared-serial
            // serialization. Threads write disjoint result ranges; the main
            // thread replays them into the slice buffers.
            let env = self.env;
            let compiled = &self.compiled;
            let children = &self.interp_children;
            let interp = &self.interp;
            let cell_noise_min_y = self.cell_noise_min_y;
            let mut results = vec![0.0f64; nslots * total];
            let per = total.div_ceil(nthr);
            let chunk_len = per * nslots;
            let result_chunks: Vec<&mut [f64]> = results.chunks_mut(chunk_len).collect();
            std::thread::scope(|s| {
                for (t, out) in result_chunks.into_iter().enumerate() {
                    let lo = t * per;
                    if lo >= total {
                        break;
                    }
                    s.spawn(move || {
                        let mut memo = MemoCtx::new(compiled.arena.len());
                        for p in lo..lo + out.len() / nslots {
                            let i = p / ny;
                            let y = p % ny;
                            let corner_z = (first_cell_z + i as i32) * cw;
                            let corner_y = (y as i32 + cell_noise_min_y) * ch;
                            let pt = Pt { x: corner_x, y: corner_y, z: corner_z };
                            memo.next_serial();
                            for (slot, &child) in children.iter().enumerate() {
                                let v = env.eval(compiled, child, pt, Mode::Slice, interp, Some(&memo));
                                out[(p - lo) * nslots + slot] = v;
                            }
                        }
                    });
                }
            });
            for p in 0..total {
                let i = p / ny;
                let y = p % ny;
                for slot in 0..nslots {
                    let v = results[p * nslots + slot];
                    if is_first {
                        self.interp.slice0_set(slot as u16, i, y, v);
                    } else {
                        self.interp.slice1_set(slot as u16, i, y, v);
                    }
                }
            }
            return;
        }
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
        let mut substance = self.cell_cache_value(iy, ix, iz);
        if !self.beard.is_empty() {
            // vanilla: cacheAllInCell(add(finalDensity, beardifier)) — the
            // cached value already includes the beard contribution.
            substance += self.beard.contribution(x, y, z);
        }
        if self.prof.enabled {
            if substance > 0.0 {
                self.prof.blocks_subst_pos += 1;
            } else {
                self.prof.blocks_subst_neg += 1;
            }
        }
        // rule 1: aquifer (or disabled aquifer)
        let mut state: Option<u16> = if self.aquifers_enabled {
            let r = self.aquifer_compute_substance(pt, substance);
            if self.prof.enabled && matches!(r, Some(b) if b == AIR) {
                self.prof.blocks_aquifer_air += 1;
            }
            if r.is_some() && self.sched {
                let b = r.unwrap();
                // vanilla doFill: mark when flag && written block has fluid state
                if (b == WATER || b == LAVA) && self.marks.len() < MARK_CAP {
                    self.marks.push(block_pos_as_long(x, y, z));
                }
            }
            r
        } else if substance > 0.0 {
            None
        } else {
            let g = self.global_fluid(y);
            Some(aquifer_at(g, y))
        };
        // rule 2: ore veins (CRUS_NO_ORE is a perf-experiment switch only)
        if state.is_none() && self.ore_veins_enabled && !self.no_ore {
            if self.prof.enabled {
                self.prof.blocks_ore_reached += 1;
            }
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
    /// Also sets self.sched = vanilla shouldScheduleFluidUpdate (per block).
    fn aquifer_compute_substance(&mut self, pt: Pt, substance: f64) -> Option<u16> {
        let x = pt.x;
        let y = pt.y;
        let z = pt.z;
        if substance > 0.0 {
            self.sched = false;
            return None;
        }
        let global = self.global_fluid(y);
        let g_at = aquifer_at(global, y);
        if g_at == LAVA {
            self.sched = false;
            return Some(LAVA);
        }

        let gx = floor_div(x - 5, 16);
        let gy = floor_div(y + 1, 12);
        let gz = floor_div(z - 5, 16);

        // Base-cell summary fast path: the 12-cell neighborhood (hence the
        // all-same verdict and the uniform status) is a pure function of the
        // base cell (gx, gy, gz). On a hit we skip the per-block scan
        // entirely: result = status.at(y); sched is provably false unless
        // the block is deep water above the lava layer (y <= -54 territory),
        // in which case a targeted two-smallest-distance scan decides the
        // vanilla d67 > 0.0 gate.
        let base = self.aquifer.index(gx, gy, gz);
        if let Some((true, s)) = self.aquifer.summary[base] {
            let block = aquifer_at(s, y);
            self.sched = block == WATER
                && aquifer_at(self.global_fluid(y - 1), y - 1) == LAVA
                && {
                    let (i6, i7) = self.scan_two_smallest(x, y, z, gx, gy, gz);
                    similarity(i6, i7) > 0.0
                };
            return Some(block);
        }

        // Single-pass scan in the exact vanilla order (i10 outer, i11 middle,
        // i12 inner) that serves BOTH the uniform fast path and the full
        // border logic below. It tracks the four smallest squared distances
        // (i6..i9, l0..l3) with vanilla `>=` tie semantics AND the
        // all-statuses-equal flag. When all 12 candidate cells share one
        // FluidStatus, the pressure machinery provably collapses
        // (calculate_pressure == 0.0, every flag needs a status difference):
        //   result  = status.at(y)
        //   sched   = d67 > 0.0 && water-above-lava   (vanilla d > 0 branch)
        // and in the vanilla early-return branch (d67 <= 0.0) the uniform
        // region makes !status.equals(status1) false, so sched = false there.
        // Statuses are resolved per SOURCE POSITION (packed, rng-offset inside
        // the cell) exactly like getAquiferStatus — never from bare cell
        // coords; once all_same turns false the remaining statuses stay
        // unresolved (vanilla lazily resolves only the l0..l3 winners).
        let stride_z = self.aquifer.grid_size_x as usize;
        let stride_y = stride_z * self.aquifer.grid_size_z as usize;
        let mut first: FluidStatus = (0, 0);
        let mut all_same = true;
        let mut i6 = i64::MAX;
        let mut i7 = i64::MAX;
        let mut i8 = i64::MAX;
        let mut i9 = i64::MAX;
        let mut l0 = 0i64;
        let mut l1 = 0i64;
        let mut l2 = 0i64;
        let mut l3 = 0i64;
        let mut idx10 = base;
        for i10 in 0..=1i32 {
            let gxx = gx + i10;
            let mut idx11 = idx10.wrapping_sub(stride_y); // i11 = -1
            for i11 in -1..=1i32 {
                let gyy = gy + i11;
                let mut index = idx11;
                for i12 in 0..=1i32 {
                    let gzz = gz + i12;
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
                    if all_same {
                        // The source position always lands inside its own
                        // grid cell (cell*dim + rng(dim-1)), so cache[index]
                        // IS the status this cell would resolve to.
                        let st = match self.aquifer.cache[index] {
                            Some(st) => st,
                            None => {
                                let v = self.compute_aquifer_fluid(
                                    pos_x(packed),
                                    pos_y(packed),
                                    pos_z(packed),
                                );
                                self.aquifer.cache[index] = Some(v);
                                v
                            }
                        };
                        if first.1 == 0 {
                            first = st;
                        } else if st != first {
                            all_same = false;
                        }
                    }
                    index += stride_z;
                }
                idx11 += stride_y;
            }
            idx10 += 1;
        }
        // Memoize the base-cell verdict (pure function of (gx, gy, gz)).
        self.aquifer.summary[base] = Some((all_same, first));
        if all_same {
            let block = aquifer_at(first, y);
            // Vanilla: d <= 0.0 early-returns with sched = false (the
            // !status.equals(status1) test is false for a uniform region);
            // only the d > 0.0 path can reach the water-above-lava mark.
            self.sched = similarity(i6, i7) > 0.0
                && block == WATER
                && aquifer_at(self.global_fluid(y - 1), y - 1) == LAVA;
            return Some(block);
        }

        let aquifer_status = self.aquifer_status(l0);
        let d = similarity(i6, i7);
        let block = aquifer_at(aquifer_status, y);
        if d <= 0.0 {
            // vanilla: flag = (d >= FLOWING) && nearest != 2nd-nearest status
            self.sched = if d >= FLOWING_UPDATE_SIMILARITY {
                self.aquifer_status(l1) != aquifer_status
            } else {
                false
            };
            return Some(block);
        }
        if block == WATER {
            let below = aquifer_at(self.global_fluid(y - 1), y - 1);
            if below == LAVA {
                self.sched = true;
                return Some(block);
            }
        }
        self.sched = false;
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
        // vanilla postprocessing (fluid update) flag — exact branch structure
        let flag = aquifer_status != status1;
        let flag1 = d3 >= FLOWING_UPDATE_SIMILARITY && status1 != status2;
        let flag2 = d2 >= FLOWING_UPDATE_SIMILARITY && aquifer_status != status2;
        self.sched = if !flag && !flag1 && !flag2 {
            d2 >= FLOWING_UPDATE_SIMILARITY
                && similarity(i6, i9) >= FLOWING_UPDATE_SIMILARITY
                && self.aquifer_status(l3) != aquifer_status
        } else {
            true
        };
        Some(block)
    }

    /// Targeted scan: two smallest squared distances (i6, i7) to the 12
    /// neighborhood source positions, identical order and tie semantics as
    /// the full scan. Used only by the summary fast path when a deep
    /// water-above-lava mark is possible — the vanilla d67 > 0.0 gate.
    fn scan_two_smallest(
        &mut self,
        x: i32,
        y: i32,
        z: i32,
        gx: i32,
        gy: i32,
        gz: i32,
    ) -> (i64, i64) {
        let stride_z = self.aquifer.grid_size_x as usize;
        let stride_y = stride_z * self.aquifer.grid_size_z as usize;
        let mut idx10 = self.aquifer.index(gx, gy, gz);
        let mut i6 = i64::MAX;
        let mut i7 = i64::MAX;
        let mut i8 = i64::MAX;
        let mut i9 = i64::MAX;
        for i10 in 0..=1i32 {
            let gxx = gx + i10;
            let mut idx11 = idx10.wrapping_sub(stride_y); // i11 = -1
            for i11 in -1..=1i32 {
                let gyy = gy + i11;
                let mut index = idx11;
                for i12 in 0..=1i32 {
                    let gzz = gz + i12;
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
                        i9 = i8;
                        i8 = i7;
                        i7 = i6;
                        i6 = dist;
                    } else if i7 >= dist {
                        i9 = i8;
                        i8 = i7;
                        i7 = dist;
                    } else if i8 >= dist {
                        i9 = i8;
                        i8 = dist;
                    } else if i9 >= dist {
                        i9 = dist;
                    }
                    index += stride_z;
                }
                idx11 += stride_y;
            }
            idx10 += 1;
        }
        (i6, i7)
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
        // Both vein tuples' y-ranges (copper [0,50], iron [-60,-8]) fail
        // outside [-60, 50] REGARDLESS of toggle sign -> result is None
        // either way; the toggle noise need not be sampled.
        let y0 = pt.y;
        if y0 > 50 || y0 < -60 {
            return None;
        }
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

impl<'e> ChunkGen<'e> {
    /// Diagnostic accessors for bench probes.
    pub fn interp_children_pub(&self) -> &[u32] {
        &self.interp_children
    }

    /// Fluid-update marks (packed BlockPos) collected during the last fill.
    pub fn marks(&self) -> &[i64] {
        &self.marks
    }

    /// Ship the per-chunk structure beardifier (vanilla Beardifier piece
    /// list, collected by the plugin via the real vanilla collection loop).
    /// Call BEFORE fill; fill() clears any previous list first.
    pub fn set_beard(&mut self, data: &[i32]) {
        self.beard = crate::beard::Beard::decode(data);
    }

    pub fn eval_child_pub(&self, child: u32, pt: Pt) -> f64 {
        self.eval_fresh(child, pt, Mode::Slice)
    }
}

/// Pass B: per-block value. Lerp3 leaves consume the precomputed rows and
/// apply only lerp(dz, row1, row2) — identical arithmetic to the interpreter.
#[inline]


/// CellFill op replay with per-(dx,dy) hoisted trilinear rows: Lerp3 reduces
/// to a single z-lerp per slot. Bit-exact vs the interpreter replay (same op
/// order, same lerp formula, same inputs). Fixed-array stack: the program's
/// max depth is <= 16 (validated by construction: 17-op straight-line tree).
fn run_cellfill_rows(
    prog: &[CellOp],
    rows: &[f64],
    dz: f64,
    block_y: i32,
    stack: &mut [f64; 16],
    sp: &mut usize,
) -> f64 {
    *sp = 0;
    for op in prog {
        match op {
            CellOp::PushConst(v) => {
                stack[*sp] = *v;
                *sp += 1;
            }
            CellOp::Lerp3 { slot } => {
                let r1 = rows[*slot as usize * 2];
                let r2 = rows[*slot as usize * 2 + 1];
                stack[*sp] = lerp_local(dz, r1, r2);
                *sp += 1;
            }
            CellOp::YGrad { from_y, to_y, from_value, to_value } => {
                let t = (block_y as f64 - *from_y as f64) / (*to_y as f64 - *from_y as f64);
                stack[*sp] = if t < 0.0 {
                    *from_value
                } else if t > 1.0 {
                    *to_value
                } else {
                    lerp_local(t, *from_value, *to_value)
                };
                *sp += 1;
            }
            CellOp::Add => {
                *sp -= 1;
                let b = stack[*sp];
                *sp -= 1;
                let a = stack[*sp];
                stack[*sp] = a + b;
                *sp += 1;
            }
            CellOp::Mul => {
                *sp -= 1;
                let b = stack[*sp];
                *sp -= 1;
                let a = stack[*sp];
                // vanilla Ap2.Mul short-circuit on zero argument1
                stack[*sp] = if a == 0.0 { 0.0 } else { a * b };
                *sp += 1;
            }
            CellOp::Min => {
                *sp -= 1;
                let b = stack[*sp];
                *sp -= 1;
                let a = stack[*sp];
                stack[*sp] = if a < b { a } else { b };
                *sp += 1;
            }
            CellOp::Max => {
                *sp -= 1;
                let b = stack[*sp];
                *sp -= 1;
                let a = stack[*sp];
                stack[*sp] = if a > b { a } else { b };
                *sp += 1;
            }
            CellOp::Abs => {
                *sp -= 1;
                let a = stack[*sp];
                stack[*sp] = a.abs();
                *sp += 1;
            }
            CellOp::Square => {
                *sp -= 1;
                let a = stack[*sp];
                stack[*sp] = a * a;
                *sp += 1;
            }
            CellOp::Cube => {
                *sp -= 1;
                let a = stack[*sp];
                stack[*sp] = a * a * a;
                *sp += 1;
            }
            CellOp::HalfNeg => {
                *sp -= 1;
                let a = stack[*sp];
                stack[*sp] = if a > 0.0 { a } else { a * 0.5 };
                *sp += 1;
            }
            CellOp::QuarterNeg => {
                *sp -= 1;
                let a = stack[*sp];
                stack[*sp] = if a > 0.0 { a } else { a * 0.25 };
                *sp += 1;
            }
            CellOp::Squeeze => {
                *sp -= 1;
                let a = stack[*sp];
                let d = clamp_local(a, -1.0, 1.0);
                stack[*sp] = d / 2.0 - d * d * d / 24.0;
                *sp += 1;
            }
            CellOp::Clamp { min, max } => {
                *sp -= 1;
                let a = stack[*sp];
                stack[*sp] = clamp_local(a, *min, *max);
                *sp += 1;
            }
            CellOp::RangeChoice { min_inclusive, max_exclusive } => {
                *sp -= 1;
                let out = stack[*sp];
                *sp -= 1;
                let inr = stack[*sp];
                *sp -= 1;
                let v = stack[*sp];
                stack[*sp] = if v >= *min_inclusive && v < *max_exclusive { inr } else { out };
                *sp += 1;
            }
            CellOp::MulOrAdd { is_add, argument } => {
                *sp -= 1;
                let a = stack[*sp];
                stack[*sp] = if *is_add {
                    a + *argument
                } else if *argument == 0.0 {
                    0.0
                } else {
                    a * *argument
                };
                *sp += 1;
            }
        }
    }
    stack[0]
}


#[inline]
fn lerp_local(t: f64, a: f64, b: f64) -> f64 {
    a + t * (b - a)
}

#[inline]
fn clamp_local(v: f64, min: f64, max: f64) -> f64 {
    if v < min {
        min
    } else if v > max {
        max
    } else {
        v
    }
}

impl<'e> ChunkGen<'e> {
    /// Evaluate one corner plane into `out` laid out [slot][z][y].
    /// Pure function of (env, compiled, plane corner x, z origin) — used by
    /// the pipelined worker thread and by the main thread.
    fn eval_plane_into(
        env: &Env,
        compiled: &Compiled,
        children: &[u32],
        corner_x: i32,
        first_cell_z: i32,
        cell_count_xz: usize,
        cell_count_y: usize,
        cell_noise_min_y: i32,
        cw: i32,
        ch: i32,
        out: &mut [f64],
    ) {
        let interp = InterpState::new(compiled.interp_slots, cell_count_xz, cell_count_y);
        let cols = cell_count_y + 1;
        let y_total = cell_count_y + 1;
        let y_batch = y_total & !3; // groups of 4
        for i in 0..=cell_count_xz {
            let corner_z = (first_cell_z + i as i32) * cw;
            let mut y = 0usize;
            while y < y_batch {
                let pts = [
                    Pt { x: corner_x, y: (y as i32 + cell_noise_min_y) * ch, z: corner_z },
                    Pt { x: corner_x, y: (y as i32 + 1 + cell_noise_min_y) * ch, z: corner_z },
                    Pt { x: corner_x, y: (y as i32 + 2 + cell_noise_min_y) * ch, z: corner_z },
                    Pt { x: corner_x, y: (y as i32 + 3 + cell_noise_min_y) * ch, z: corner_z },
                ];
                for (slot, &child) in children.iter().enumerate() {
                    // 4-wide batch: lane-identical arithmetic (bit-exact)
                    let v = env.eval4(compiled, child, &pts, Mode::Slice, &interp);
                    let base = slot * (cell_count_xz + 1) * cols + i * cols + y;
                    out[base] = v[0];
                    out[base + 1] = v[1];
                    out[base + 2] = v[2];
                    out[base + 3] = v[3];
                }
                y += 4;
            }
            for y in y_batch..y_total {
                let corner_y = (y as i32 + cell_noise_min_y) * ch;
                let pt = Pt { x: corner_x, y: corner_y, z: corner_z };
                for (slot, &child) in children.iter().enumerate() {
                    let v = env.eval(compiled, child, pt, Mode::Slice, &interp, None);
                    out[slot * (cell_count_xz + 1) * cols + i * cols + y] = v;
                }
            }
        }
    }
}

#[cfg(test)]
mod marks_tests {
    use super::*;

    /// Fill the judged region and check the fluid-update marks: the region
    /// carries marks (deep lava/water interfaces), every mark decodes into
    /// its chunk bounds, packs round-trip (vanilla BlockPos.asLong layout)
    /// and sits on a fluid block in the output.
    #[test]
    fn fill_collects_valid_marks() {
        let seed: i64 = 3257840388504953787;
        let env: &'static Env = Box::leak(Box::new(Env::new(seed)));
        let mut gen = ChunkGen::new(env);
        let mut out = [0u16; OUT_LEN];
        let mut total = 0usize;
        for cz in -1..=1 {
            for cx in -1..=1 {
                gen.fill(cx, cz, &mut out);
                let marks = gen.marks().to_vec();
                total += marks.len();
                for &p in marks.iter() {
                    let x = pos_x(p);
                    let y = pos_y(p);
                    let z = pos_z(p);
                    assert!(x >= cx * 16 && x < cx * 16 + 16, "mark x {x} out of chunk");
                    assert!(z >= cz * 16 && z < cz * 16 + 16, "mark z {z} out of chunk");
                    assert!(y >= -64 && y < 320, "mark y {y} out of range");
                    assert_eq!(block_pos_as_long(x, y, z), p, "packing round-trip");
                    let lx = (x - cx * 16) as usize;
                    let lz = (z - cz * 16) as usize;
                    let b = out[((y + 64) as usize * 256 + lz * 16 + lx) as usize];
                    assert!(b == WATER || b == LAVA, "mark on non-fluid block {b}");
                }
            }
        }
        assert!(total > 0, "region should carry fluid-update marks");
    }

    /// Fill must reset marks between chunks (no cross-chunk bleed).
    #[test]
    fn marks_reset_per_chunk() {
        let seed: i64 = 3257840388504953787;
        let env: &'static Env = Box::leak(Box::new(Env::new(seed)));
        let mut gen = ChunkGen::new(env);
        let mut out = [0u16; OUT_LEN];
        gen.fill(0, 0, &mut out);
        let n0 = gen.marks().len();
        gen.fill(1, 0, &mut out);
        let n1 = gen.marks().len();
        for &p in gen.marks().iter() {
            let x = pos_x(p);
            assert!(x >= 16 && x < 32, "stale mark from previous chunk: x={x}");
        }
        let _ = (n0, n1);
    }
}
