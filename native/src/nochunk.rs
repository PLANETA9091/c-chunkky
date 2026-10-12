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
use std::sync::atomic::{AtomicI32, AtomicI64, AtomicU64, Ordering};
//   - postprocessing marks (shouldScheduleFluidUpdate) are skipped: they carry
//     no block data, and the Bukkit noise path cannot express them.

use crate::density::{compile_cellfill, BatchMemo, CellOp, Compiled, Env, InterpState, MemoCtx, Mode, Pt, V4Memo, V8Memo, WKind};

#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::_mm256_setzero_pd;
#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::_mm256_setr_pd;
#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::_mm256_storeu_pd;

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

/// Shared-nothing values, shared storage: every entry is a pure function of
/// the grid cell (positions are rng constants per cell, statuses are pure
/// position functions), so parallel cell threads can share these caches via
/// Relaxed atomics — concurrent writers store identical bytes (benign race),
/// and begin_chunk resets them single-threaded before the threads spawn.
struct AquiferState {
    cache: Vec<AtomicU64>,
    loc_cache: Vec<AtomicI64>,
    /// Per BASE-cell summary: all 12 neighborhood statuses equal + the shared
    /// status. The 12-cell neighborhood depends only on the base cell
    /// (gx, gy, gz), never on the block itself — so the all-same verdict and
    /// the uniform status are pure functions of the base cell and can be
    /// memoized. Reset per chunk together with the other caches.
    summary: Vec<AtomicU64>,
    min_grid_x: AtomicI32,
    min_grid_y: i32,
    min_grid_z: AtomicI32,
    grid_size_x: i32,
    grid_size_z: i32,
}

#[inline]
fn pack_status(s: FluidStatus) -> u64 {
    ((s.0 as i64 as u64) << 20) | ((s.1 as u64) & 0xF_FFFF)
}

#[inline]
fn unpack_status(v: u64) -> FluidStatus {
    ((v >> 20) as i64 as i32, (v & 0xF_FFFF) as u16)
}

#[inline]
fn pack_summary(all_same: bool, s: FluidStatus) -> u64 {
    1 | ((all_same as u64) << 1) | (pack_status(s) << 2)
}

#[inline]
fn unpack_summary(v: u64) -> (bool, FluidStatus) {
    ((v >> 1) & 1 == 1, unpack_status(v >> 2))
}

impl AquiferState {
    fn new(min_y: i32, height: i32) -> AquiferState {
        let min_grid_y = floor_div(min_y, 12) - 1;
        let max_gy = floor_div(min_y + height, 12) + 1;
        let gy_span = max_gy - min_grid_y + 1;
        let cells = (3 * gy_span * 3) as usize;
        AquiferState {
            cache: (0..cells).map(|_| AtomicU64::new(0)).collect(),
            loc_cache: (0..cells).map(|_| AtomicI64::new(i64::MAX)).collect(),
            summary: (0..cells).map(|_| AtomicU64::new(0)).collect(),
            min_grid_x: AtomicI32::new(0),
            min_grid_y,
            min_grid_z: AtomicI32::new(0),
            grid_size_x: 3,
            grid_size_z: 3,
        }
    }

    /// Per-chunk grid origin (vanilla: chunk pos of this NoiseChunk).
    /// &self: the Arc-shared state is mutated only here (single-threaded,
    /// before the cell threads spawn) and through Relaxed atomics inside the
    /// parallel phase.
    #[inline]
    fn begin_chunk(&self, cx: i32, cz: i32) {
        self.min_grid_x.store(cx - 1, Ordering::Relaxed);
        self.min_grid_z.store(cz - 1, Ordering::Relaxed);
        // Vanilla allocates fresh caches per NoiseChunk (per chunk); our
        // generator persists across fills and the grid origin re-bases every
        // chunk, so BOTH caches must be reset or stale cells alias into the
        // new chunk's slots (correctness bug: wrong fluid statuses).
        for v in self.loc_cache.iter() {
            v.store(i64::MAX, Ordering::Relaxed);
        }
        for v in self.cache.iter() {
            v.store(0, Ordering::Relaxed);
        }
        for v in self.summary.iter() {
            v.store(0, Ordering::Relaxed);
        }
    }

    #[inline]
    fn index(&self, gx: i32, gy: i32, gz: i32) -> usize {
        let i = (gx - self.min_grid_x.load(Ordering::Relaxed)) as usize;
        let i1 = (gy - self.min_grid_y) as usize;
        let i2 = (gz - self.min_grid_z.load(Ordering::Relaxed)) as usize;
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
    pub compiled: std::sync::Arc<Compiled>,
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
    /// Shared across parallel cell threads: values are pure functions of the
    /// column, so concurrent resolves are identical; the lock is held only
    /// for the lookup/insert, never across the (expensive) compute.
    prelim: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<i64, i32>>>,
    /// Cross-chunk corner-plane cache: the last fill's final x-plane equals
    /// the next fill's initial plane when cx increments by 1 (same cz row).
    /// Values are deterministic per corner point -> reuse is value-identical.
    plane_cache_x: i64,
    plane_cache_z: i32,
    plane_cache: Vec<f64>,
    aquifer: std::sync::Arc<AquiferState>,
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
    /// calibrated batch width for slice corner evaluation (4 or 8)
    slice_lanes: usize,
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
        let compiled = std::sync::Arc::new(env.compile());
        let slice_lanes = Self::pick_slice_lanes(env, &compiled);
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
            prelim: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            plane_cache_x: i64::MIN,
            plane_cache_z: i32::MIN,
            plane_cache: Vec::new(),
            aquifer: std::sync::Arc::new(AquiferState::new(env.wg.min_y, env.wg.height)),
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
            slice_lanes,
        }
    }

    /// One-time micro-calibration of the slice batch width: 4-wide AVX2
    /// lanes vs 8-wide (2x AVX2 halves / AVX-512). The winner differs per
    /// CPU (512-bit downclocking on SKX/CLX makes 8 slower there); the
    /// calibration costs a few ms once per process and is bit-neutral
    /// (all widths are lane-identical to the scalar eval).
    fn pick_slice_lanes(env: &Env, compiled: &Compiled) -> usize {
        if std::env::var("CRUS_LANES").ok().and_then(|v| v.parse::<usize>().ok()) == Some(8) {
            return 8;
        }
        if std::env::var("CRUS_LANES").ok().and_then(|v| v.parse::<usize>().ok()) == Some(4) {
            return 4;
        }
        let child0 = match compiled.interp_children.first() {
            Some(&c) => c,
            None => return 4,
        };
        let interp = InterpState::new(compiled.interp_slots, 4, 48);
        let mut pts: Vec<Pt> = Vec::with_capacity(245);
        for i in 0..5i32 {
            for y in 0..49i32 {
                pts.push(Pt { x: 48, y: (y - 8) * 8, z: -32 + i * 4 });
            }
        }
        let mut sink = 0u64;
        let time = |g: usize, sink: &mut u64| -> f64 {
            let t0 = std::time::Instant::now();
            let reps = 12;
            for _ in 0..reps {
                if g == 8 {
                    let mut memo = BatchMemo::<8>::new(compiled.arena.len());
                    for c in pts.chunks(8) {
                        let mut p8 = [c[0]; 8];
                        for (k, pt) in c.iter().enumerate() {
                            p8[k] = *pt;
                        }
                        let v = env.eval_batch::<8>(compiled, child0, &p8, Mode::Slice, &interp, &memo);
                        for x in v {
                            *sink ^= x.to_bits();
                        }
                    }
                } else {
                    let mut memo = BatchMemo::<4>::new(compiled.arena.len());
                    for c in pts.chunks(4) {
                        let p4 = [c[0], c[(1).min(c.len() - 1)], c[(2).min(c.len() - 1)], c[(3).min(c.len() - 1)]];
                        let v = env.eval_batch::<4>(compiled, child0, &p4, Mode::Slice, &interp, &memo);
                        for x in v {
                            *sink ^= x.to_bits();
                        }
                    }
                }
            }
            t0.elapsed().as_nanos() as f64
        };
        // warm both, then time both
        let _ = time(4, &mut sink);
        let _ = time(8, &mut sink);
        let d4 = time(4, &mut sink);
        let d8 = time(8, &mut sink);
        let pick = if d8 < d4 * 0.97 { 8 } else { 4 };
        if std::env::var("CRUS_DEBUG_LANES").is_ok() {
            eprintln!("[crussty] slice lanes calibration: 4={d4:.0}ns 8={d8:.0}ns -> {pick} (sink={sink})");
        }
        pick
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
        {
            let map = self.prelim.lock().unwrap();
            if let Some(v) = map.get(&key) {
                return *v;
            }
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
        self.prelim.lock().unwrap().insert(key, result);
        result
    }

    /// Fresh per-thread generator clone for the parallel cell path.
    ///
    /// Shares (immutable or synchronized): env, compiled (Arc), prelim
    /// (Arc<Mutex>). Everything mutable is FRESH, not copied: memo, aquifer
    /// caches, marks, cell scratch. The beard is deep-copied (per-chunk data,
    /// already set by the plugin before fill). Prof counters start zeroed
    /// (only the enabled flag carries over) so the post-join merge is a
    /// plain sum of per-thread deltas.
    fn thread_clone(&self, cx: i32, cz: i32) -> ChunkGen<'e> {
        ChunkGen {
            sched: false,
            marks: Vec::with_capacity(1024),
            beard: self.beard.clone(),
            env: self.env,
            compiled: std::sync::Arc::clone(&self.compiled),
            interp_children: self.interp_children.clone(),
            cell_width: self.cell_width,
            cell_height: self.cell_height,
            cell_count_xz: self.cell_count_xz,
            cell_count_y: self.cell_count_y,
            cell_noise_min_y: self.cell_noise_min_y,
            min_y: self.min_y,
            sea_level: self.sea_level,
            aquifers_enabled: self.aquifers_enabled,
            ore_veins_enabled: self.ore_veins_enabled,
            final_density: self.final_density,
            initial_density: self.initial_density,
            barrier: self.barrier,
            floodedness: self.floodedness,
            spread: self.spread,
            lava_noise: self.lava_noise,
            erosion: self.erosion,
            depth: self.depth,
            vein_toggle: self.vein_toggle,
            vein_ridged: self.vein_ridged,
            vein_gap: self.vein_gap,
            interp: self.interp.clone(),
            cell_cache: vec![0.0; self.cell_cache.len()],
            final_density_slot: self.final_density_slot,
            cellfill_prog: self.cellfill_prog.clone(),
            cellfill_stack: Vec::with_capacity(16),
            cellfill_rows: vec![0.0; self.cellfill_rows.len()],
            cellfill_slots: self.cellfill_slots.clone(),
            prelim: std::sync::Arc::clone(&self.prelim),
            plane_cache_x: i64::MIN,
            plane_cache_z: i32::MIN,
            plane_cache: Vec::new(),
            aquifer: std::sync::Arc::clone(&self.aquifer),
            memo: MemoCtx::new(self.compiled.arena.len()),
            prof: Prof {
                enabled: self.prof.enabled,
                ..Default::default()
            },
            skip_cellfill: self.skip_cellfill,
            skip_material: self.skip_material,
            skip_slice: self.skip_slice,
            no_ore: self.no_ore,
            slice_threads: self.slice_threads,
            slice_lanes: self.slice_lanes,
        }
    }

    /// Load the strip's corner planes into the interp state (slice0 = plane
    /// at x-strip i2, slice1 = plane at i2+1; layout [slot][z][y]).
    fn load_planes(&mut self, p0: &[f64], p1: &[f64]) {
        self.interp.slice0.copy_from_slice(p0);
        self.interp.slice1.copy_from_slice(p1);
    }

    /// Parallel fill: all corner planes up front (workers over planes), then
    /// x-strips distributed over per-thread generator clones. Marks, prof
    /// counters and out buffers are merged after the join.
    fn fill_parallel(&mut self, cx: i32, cz: i32, out: &mut [u16; OUT_LEN], p: &mut Prof) {
        let min_block_x = cx * 16;
        let min_block_z = cz * 16;
        let first_cell_x = floor_div(min_block_x, self.cell_width as i32);
        let first_cell_z = floor_div(min_block_z, self.cell_width as i32);
        let cw = self.cell_width as i32;
        let ch = self.cell_height as i32;
        let ccx = self.cell_count_xz;
        let ccy = self.cell_count_y;
        let cnmy = self.cell_noise_min_y;
        let plane_slots = self.interp_children.len();
        let plane_len = plane_slots * (ccx + 1) * (ccy + 1);
        let nthr = self.slice_threads.min(self.cell_count_xz);
        let lanes = self.slice_lanes;

        // plane 0: cross-chunk cache or fresh evaluation
        let cache_hit = self.plane_cache_x == (first_cell_x * cw) as i64
            && self.plane_cache_z == first_cell_z
            && self.plane_cache.len() == plane_len;
        let mut planes: Vec<Vec<f64>> = Vec::with_capacity(ccx + 1);
        if cache_hit {
            p.plane_hits += 1;
            planes.push(self.plane_cache.clone());
        } else {
            self.fill_slice(first_cell_x, first_cell_z, true);
            planes.push(self.interp.slice0.clone());
        }

        // planes 1..=ccx in parallel (each plane = 5*49 corner points)
        {
            let env = self.env;
            let compiled = std::sync::Arc::clone(&self.compiled);
            let children = self.interp_children.clone();
            std::thread::scope(|s| {
                let mut handles = Vec::new();
                for k in 1..=ccx {
                    let compiled = std::sync::Arc::clone(&compiled);
                    let children = children.clone();
                    let plane_x = (first_cell_x + k as i32) * cw;
                    handles.push(s.spawn(move || {
                        let mut buf = vec![0f64; plane_len];
                        Self::eval_plane_into(
                            env,
                            &compiled,
                            &children,
                            plane_x,
                            first_cell_z,
                            ccx,
                            ccy,
                            cnmy,
                            cw,
                            ch,
                            lanes,
                            &mut buf,
                        );
                        buf
                    }));
                }
                for h in handles {
                    planes.push(h.join().expect("plane worker panicked"));
                }
            });
        }

        // distribute x-strips over per-thread clones
        let strips_per = (ccx + nthr - 1) / nthr;
        let mut gens: Vec<ChunkGen<'e>> = (1..nthr).map(|_| self.thread_clone(cx, cz)).collect();
        let mut outs: Vec<Box<[u16; OUT_LEN]>> =
            (1..nthr).map(|_| Box::new([0u16; OUT_LEN])).collect();
        let planes_ref = &planes;
        // main thread's own strip buffer (kept out of the threaded split)
        let mut out0 = Box::new([0u16; OUT_LEN]);
        std::thread::scope(|s| {
            let outs_split: Vec<&mut Box<[u16; OUT_LEN]>> = outs.iter_mut().collect();
            for (t, (g, out_t)) in gens
                .iter_mut()
                .zip(outs_split.into_iter())
                .enumerate()
            {
                let t = t + 1;
                let strips: Vec<i32> = (0..ccx)
                    .filter(|&k| k / strips_per == t)
                    .map(|k| k as i32)
                    .collect();
                s.spawn(move || {
                    let prof_en = g.prof.enabled;
                    // local Prof copy: process_strip takes a separate &mut Prof
                    // (cannot alias &mut g with &mut g.prof in one call)
                    let mut tp = g.prof;
                    for k in strips {
                        g.load_planes(&planes_ref[k as usize], &planes_ref[k as usize + 1]);
                        g.process_strip(
                            k,
                            first_cell_x,
                            first_cell_z,
                            min_block_x,
                            min_block_z,
                            prof_en,
                            false,
                            out_t,
                            &mut tp,
                        );
                    }
                    g.prof = tp;
                });
            }
            // main thread: strip group 0
            for k in 0..ccx {
                if k / strips_per != 0 {
                    continue;
                }
                self.load_planes(&planes_ref[k], &planes_ref[k + 1]);
                let prof_en = self.prof.enabled;
                self.process_strip(
                    k as i32,
                    first_cell_x,
                    first_cell_z,
                    min_block_x,
                    min_block_z,
                    prof_en,
                    false,
                    &mut out0,
                    p,
                );
            }
        });

        // merge outs (disjoint positions; AIR=0 so OR is exact)
        for (o, b) in out.iter_mut().zip(out0.iter()) {
            *o |= *b;
        }
        for t in 0..outs.len() {
            for (o, b) in out.iter_mut().zip(outs[t].iter()) {
                *o |= *b;
            }
        }
        // merge marks + prof deltas from clones
        for t in 1..nthr {
            let g = &mut gens[t - 1];
            if self.marks.len() + g.marks.len() <= MARK_CAP {
                self.marks.extend(g.marks.drain(..));
            }
            p.fill_slice_ns += g.prof.fill_slice_ns;
            p.cellfill_ns += g.prof.cellfill_ns;
            p.material_ns += g.prof.material_ns;
            p.select_ns += g.prof.select_ns;
            p.blocks_subst_pos += g.prof.blocks_subst_pos;
            p.blocks_subst_neg += g.prof.blocks_subst_neg;
            p.blocks_ore_reached += g.prof.blocks_ore_reached;
            p.blocks_aquifer_air += g.prof.blocks_aquifer_air;
        }
        // save the final plane for the next fill's plane-0 cache
        self.plane_cache.clear();
        self.plane_cache.extend_from_slice(&planes[ccx]);
        self.plane_cache_x = ((first_cell_x + ccx as i32) * cw) as i64;
        self.plane_cache_z = first_cell_z;
    }

    /// Vanilla NoiseBasedChunkGenerator.doFill.
    pub fn fill(&mut self, cx: i32, cz: i32, out: &mut [u16; OUT_LEN]) {
        // Prof counters ACCUMULATE across fills (bench resets after warmup).
        let mut p = self.prof;
        let prof = p.enabled;
        let dbg = std::env::var("CRUS_DBG_SLICE").is_ok();
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

        // Parallel cell path (CRUS_FILL_THREADS > 1): evaluate every corner
        // plane up front (workers over planes), then process x-strips on
        // per-thread generator clones. Bit-identical: cells are pure
        // functions of the corner planes; per-thread aquifer/memo/marks state
        // is fresh or shared-immutable; out writes are disjoint per strip.
        if self.slice_threads > 1 && self.cell_count_xz > 0 && !self.skip_slice && !self.skip_cellfill
        {
            self.fill_parallel(cx, cz, out, &mut p);
            self.prof = p;
            return;
        }

        // initializeForFirstCellX: fill slice0 at x-plane first_cell_x
        let t_fs0 = if prof { Some(std::time::Instant::now()) } else { None };
        let dbg = std::env::var("CRUS_DBG_SLICE").is_ok();
        let t0 = std::time::Instant::now();
        let plane_slots = self.interp_children.len() as usize;
        let plane_len = plane_slots * (self.cell_count_xz + 1) * (self.cell_count_y + 1);
        let lanes = self.slice_lanes;
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
            let compiled_addr = std::sync::Arc::as_ptr(&self.compiled) as usize;
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
                        lanes,
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
                let compiled_addr = std::sync::Arc::as_ptr(&self.compiled) as usize;
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
                            lanes,
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
            self.process_strip(
                i2 as i32,
                first_cell_x,
                first_cell_z,
                min_block_x,
                min_block_z,
                prof,
                dbg,
                out,
                &mut p,
            );

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

    /// One x-strip of cells (vanilla: the i2 loop body from cellStartBlockX
    /// through the material walk). The caller guarantees interp.slice0 ==
    /// plane(i2) and interp.slice1 == plane(i2+1). Shared verbatim by the
    /// serial path and the parallel cell threads (bit-identical by
    /// construction).
    fn process_strip(
        &mut self,
        i2: i32,
        first_cell_x: i32,
        first_cell_z: i32,
        min_block_x: i32,
        min_block_z: i32,
        prof: bool,
        dbg: bool,
        out: &mut [u16; OUT_LEN],
        p: &mut Prof,
    ) {
        let cw = self.cell_width as i32;
        let ch = self.cell_height as i32;
        let cell_start_block_x = (first_cell_x + i2) * cw;

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
                                if cw_i == 4 && cellfill_simd::avx2_ok() {
                                    // AVX2: one 4-lane run per row (lane = z-block).
                                    // dz per lane replicates the scalar += dx_step
                                    // accumulation exactly.
                                    let (dzv, mut vstack, mut vsp) =
                                        cellfill_simd::make_row_state(dx_step);
                                    let res = unsafe {
                                        cellfill_simd::run_cellfill_row4(
                                            prog,
                                            &self.cellfill_rows,
                                            dzv,
                                            block_y,
                                            &mut vstack,
                                            &mut vsp,
                                        )
                                    };
                                    unsafe {
                                        _mm256_storeu_pd(
                                            self.cell_cache.as_mut_ptr().add(idx),
                                            res,
                                        );
                                    }
                                    idx += cw_i as usize;
                                } else {
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
                        eprintln!("cell[{}, {}, {}] cf={:?}", i2, i3, i5, t_cf.map(|t| t.elapsed()));
                    }

                    let t_mt = if prof { Some(std::time::Instant::now()) } else { None };
                    for i6 in (0..self.cell_height as i32).rev() {
                        let block_y = (self.cell_noise_min_y + i5 as i32) * ch + i6;

                        for i9 in 0..self.cell_width as i32 {
                            let block_x = cell_start_block_x + i9;

                            // row fast path: 4 z-blocks, constant aquifer result
                            let base_out = ((block_y - self.min_y) * 256
                                + (cell_start_block_z - min_block_z) * 16
                                + (block_x - min_block_x))
                                as usize;
                            if self.material_row4(
                                block_x,
                                block_y,
                                cell_start_block_z,
                                i6,
                                i9,
                                base_out,
                                out,
                            ) {
                                continue;
                            }

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
        // merge per-block counters (material_rule mutates self.prof directly;
        // p carries running totals — replace per strip keeps them cumulative)
        p.blocks_subst_pos = self.prof.blocks_subst_pos;
        p.blocks_subst_neg = self.prof.blocks_subst_neg;
        p.blocks_ore_reached = self.prof.blocks_ore_reached;
        p.blocks_aquifer_air = self.prof.blocks_aquifer_air;
        self.prof = *p;
    }

    /// Slot-0 corner evaluation in groups of G (tail-padded duplicates).
    /// Generic over the batch width so the machine-preferred lane count
    /// (calibrated at startup) applies without code duplication.
    fn slice_slot0<const G: usize>(
        env: &Env,
        compiled: &Compiled,
        child0: u32,
        interp: &InterpState,
        corner_x: i32,
        first_cell_z: i32,
        cw: i32,
        ch: i32,
        ny: usize,
        cell_noise_min_y: i32,
        lo: usize,
        hi: usize,
        out: &mut [f64],
    ) {
        let mut memo = BatchMemo::<G>::new(compiled.arena.len());
        let mut pts = [Pt { x: 0, y: 0, z: 0 }; G];
        let mut p = lo;
        while p < hi {
            let n = (hi - p).min(G);
            for k in 0..G {
                let pp = p + (n - 1).min(k);
                let i = pp / ny;
                let y = pp % ny;
                pts[k] = Pt {
                    x: corner_x,
                    y: (y as i32 + cell_noise_min_y) * ch,
                    z: (first_cell_z + i as i32) * cw,
                };
            }
            let v = env.eval_batch::<G>(compiled, child0, &pts, Mode::Slice, interp, &memo);
            out[p - lo..p - lo + n].copy_from_slice(&v[..n]);
            p += G;
        }
    }

    /// Vanilla NoiseChunk.fillSlice: each interpolator's wrapped function is
    /// evaluated at the (x, y, z) corner (Mode::Slice — child direct).
    pub fn fill_slice(&mut self, start_cell_x: i32, first_cell_z: i32, is_first: bool) {
        let corner_x = start_cell_x * self.cell_width as i32;
        let cw = self.cell_width as i32;
        let ch = self.cell_height as i32;
        let nz = self.cell_count_xz as usize + 1;
        let ny = self.cell_count_y as usize + 1;
        let nslots = self.interp_children.len();
        let total = nz * ny;
        if self.skip_slice {
            for i in 0..=self.cell_count_xz {
                for y in 0..=self.cell_count_y {
                    for slot in 0..nslots {
                        let slot = slot as u16;
                        if is_first {
                            self.interp.slice0_set(slot, i as usize, y, 0.0);
                        } else {
                            self.interp.slice1_set(slot, i as usize, y, 0.0);
                        }
                    }
                }
            }
            return;
        }
        // Slot 0 (final_density) is ~88% of the slice cost: evaluate it in
        // 4-corner batches through eval4 (lane-identical arithmetic + SIMD
        // noise kernels — bit-identical to the scalar eval, verified by
        // eval4_matches_scalar_eval and the fill out_hash checks). Slots 1-7
        // stay on the scalar memoized path: they are single-noise children
        // that reuse slot 0's per-point CSE context.
        let nthr = self.slice_threads.min(total);
        if nthr > 1 {
            // Parallel: corners are pure functions of (x, y, z); slot 0
            // batches go through eval4 (no mutable state), slots 1-7 use
            // per-thread MemoCtx instances (bit-identical, disjoint ranges).
            let env = self.env;
            let compiled = &self.compiled;
            let children = &self.interp_children;
            let interp = &self.interp;
            let cell_noise_min_y = self.cell_noise_min_y;
            let child0 = self.interp_children[0];
            let lanes = self.slice_lanes;
            let mut slot0 = vec![0f64; total];
            let mut results = vec![0.0f64; nslots * total];
            let per = total.div_ceil(nthr);
            let chunk_len = per * nslots;
            let result_chunks: Vec<&mut [f64]> = results.chunks_mut(chunk_len).collect();
            let slot0_chunks: Vec<&mut [f64]> = slot0.chunks_mut(per).collect();
            std::thread::scope(|s| {
                for (t, (out, slot0_out)) in result_chunks.into_iter().zip(slot0_chunks).enumerate() {
                    let lo = t * per;
                    if lo >= total {
                        break;
                    }
                    let hi = (lo + per).min(total);
                    s.spawn(move || {
                        let mut memo = MemoCtx::new(compiled.arena.len());
                        // slot 0 in groups of G corners (tail-padded duplicates)
                        match lanes {
                            8 => Self::slice_slot0::<8>(
                                env, compiled, child0, interp, corner_x, first_cell_z,
                                cw, ch, ny, cell_noise_min_y, lo, hi, slot0_out,
                            ),
                            _ => Self::slice_slot0::<4>(
                                env, compiled, child0, interp, corner_x, first_cell_z,
                                cw, ch, ny, cell_noise_min_y, lo, hi, slot0_out,
                            ),
                        }
                        for p in lo..hi {
                            let i = p / ny;
                            let y = p % ny;
                            let corner_z = (first_cell_z + i as i32) * cw;
                            let corner_y = (y as i32 + cell_noise_min_y) * ch;
                            let pt = Pt { x: corner_x, y: corner_y, z: corner_z };
                            memo.next_serial();
                            for slot in 1..nslots {
                                let child = children[slot];
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
                if is_first {
                    self.interp.slice0_set(0, i, y, slot0[p]);
                } else {
                    self.interp.slice1_set(0, i, y, slot0[p]);
                }
                for slot in 1..nslots {
                    let v = results[p * nslots + slot];
                    let slot = slot as u16;
                    if is_first {
                        self.interp.slice0_set(slot, i, y, v);
                    } else {
                        self.interp.slice1_set(slot, i, y, v);
                    }
                }
            }
            return;
        }
        // Serial: slot 0 via batch groups, slots 1-7 scalar with memo.
        let mut slot0 = vec![0f64; total];
        {
            let env = self.env;
            let compiled = &self.compiled;
            let interp = &self.interp;
            let child0 = self.interp_children[0];
            let cell_noise_min_y = self.cell_noise_min_y;
            let lanes = self.slice_lanes;
            match lanes {
                8 => Self::slice_slot0::<8>(
                    env, compiled, child0, interp, corner_x, first_cell_z,
                    cw, ch, ny, cell_noise_min_y, 0, total, &mut slot0,
                ),
                _ => Self::slice_slot0::<4>(
                    env, compiled, child0, interp, corner_x, first_cell_z,
                    cw, ch, ny, cell_noise_min_y, 0, total, &mut slot0,
                ),
            }
        }
        for i in 0..=self.cell_count_xz {
            let corner_z = (first_cell_z + i as i32) * cw;
            for y in 0..=self.cell_count_y {
                let corner_y = (y as i32 + self.cell_noise_min_y) * ch;
                let pt = Pt { x: corner_x, y: corner_y, z: corner_z };
                let p = i * ny + y;
                {
                    let slot = 0u16;
                    if is_first {
                        self.interp.slice0_set(slot, i as usize, y, slot0[p]);
                    } else {
                        self.interp.slice1_set(slot, i as usize, y, slot0[p]);
                    }
                }
                for slot in 1..nslots {
                    let child = self.interp_children[slot];
                    let v = self.eval(child, pt, Mode::Slice);
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

    /// Row-level material fast path: 4 z-blocks sharing (x, y) — same
    /// global_fluid(y), and when every block has substance <= 0 and every
    /// touched aquifer base cell has an all-same cached summary with one
    /// uniform status, the vanilla result is one constant block for the
    /// whole row (sched provably false: it requires a status DIFFERENCE or
    /// the water-above-lava scan at y == -54). Returns false when the row
    /// needs the per-block path (any subst > 0 -> ore per block, uncached
    /// summary, mixed statuses, beard, or the y == -54 water case).
    fn material_row4(
        &mut self,
        block_x: i32,
        block_y: i32,
        block_z0: i32,
        iy: i32,
        ix: i32,
        base_out: usize,
        out: &mut [u16; OUT_LEN],
    ) -> bool {
        if !self.beard.is_empty() || self.skip_material || !self.aquifers_enabled {
            return false;
        }
        // substances: all four must be <= 0 (ore path is per-block)
        if self.cell_cache_value(iy, ix, 0) > 0.0
            || self.cell_cache_value(iy, ix, 1) > 0.0
            || self.cell_cache_value(iy, ix, 2) > 0.0
            || self.cell_cache_value(iy, ix, 3) > 0.0
        {
            return false;
        }
        let global = self.global_fluid(block_y);
        let g_at = aquifer_at(global, block_y);
        if g_at == LAVA {
            // vanilla early return: LAVA, sched = false
            self.sched = false;
            for iz in 0..4i32 {
                out[base_out + (iz * 16) as usize] = LAVA;
            }
            return true;
        }
        // summaries of the (up to 2) distinct base cells in the row
        let mut s0: Option<FluidStatus> = None;
        let mut gz_prev = i32::MIN;
        for iz in 0..4i32 {
            let z = block_z0 + iz;
            let gx = floor_div(block_x - 5, 16);
            let gz = floor_div(z - 5, 16);
            if gz != gz_prev {
                gz_prev = gz;
                let gy = floor_div(block_y + 1, 12);
                let base = self.aquifer.index(gx, gy, gz);
                let sv = self.aquifer.summary[base].load(Ordering::Relaxed);
                if sv == 0 {
                    return false; // uncached — per-block path will populate
                }
                let (same, s) = unpack_summary(sv);
                if !same {
                    return false;
                }
                match s0 {
                    None => s0 = Some(s),
                    Some(prev) if prev == s => {}
                    Some(_) => return false, // mixed statuses across the row
                }
            }
        }
        let s = match s0 {
            Some(s) => s,
            None => return false,
        };
        let block = aquifer_at(s, block_y);
        if block == WATER && aquifer_at(self.global_fluid(block_y - 1), block_y - 1) == LAVA {
            // y == -54 water-above-lava: per-block scan_two_smallest decides sched
            return false;
        }
        self.sched = false;
        for iz in 0..4i32 {
            out[base_out + (iz * 16) as usize] = block;
        }
        true
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
        let sv = self.aquifer.summary[base].load(Ordering::Relaxed);
        if sv != 0 {
            let (same, s) = unpack_summary(sv);
            if same {
            let block = aquifer_at(s, y);
            self.sched = block == WATER
                && aquifer_at(self.global_fluid(y - 1), y - 1) == LAVA
                && {
                    let (i6, i7) = self.scan_two_smallest(x, y, z, gx, gy, gz);
                    similarity(i6, i7) > 0.0
                };
            return Some(block);
            }
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
                    let packed = {
                        let l = self.aquifer.loc_cache[index].load(Ordering::Relaxed);
                        if l != i64::MAX {
                            l
                        } else {
                        let mut rng = self.env.aquifer_random.clone().at(gxx, gyy, gzz);
                        let p = block_pos_as_long(
                            gxx * 16 + rng.next_int_bound(10),
                            gyy * 12 + rng.next_int_bound(9),
                            gzz * 16 + rng.next_int_bound(10),
                        );
                            self.aquifer.loc_cache[index].store(p, Ordering::Relaxed);
                            p
                        }
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
                        let st = {
                            let packed_v = self.aquifer.cache[index].load(Ordering::Relaxed);
                            if packed_v != 0 {
                                unpack_status(packed_v)
                            } else {
                                let v = self.compute_aquifer_fluid(
                                    pos_x(packed),
                                    pos_y(packed),
                                    pos_z(packed),
                                );
                                self.aquifer.cache[index].store(pack_status(v), Ordering::Relaxed);
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
        self.aquifer.summary[base].store(pack_summary(all_same, first), Ordering::Relaxed);
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
                    let packed = {
                        let l = self.aquifer.loc_cache[index].load(Ordering::Relaxed);
                        if l != i64::MAX {
                            l
                        } else {
                        let mut rng = self.env.aquifer_random.clone().at(gxx, gyy, gzz);
                        let p = block_pos_as_long(
                            gxx * 16 + rng.next_int_bound(10),
                            gyy * 12 + rng.next_int_bound(9),
                            gzz * 16 + rng.next_int_bound(10),
                        );
                            self.aquifer.loc_cache[index].store(p, Ordering::Relaxed);
                            p
                        }
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
        let packed_v = self.aquifer.cache[index].load(Ordering::Relaxed);
        if packed_v != 0 {
            return unpack_status(packed_v);
        }
        let v = self.compute_aquifer_fluid(x, y, z);
        self.aquifer.cache[index].store(pack_status(v), Ordering::Relaxed);
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
    pub fn cell_width_pub(&self) -> usize { self.cell_width }
    pub fn cell_height_pub(&self) -> usize { self.cell_height }
    pub fn cell_count_xz_pub(&self) -> usize { self.cell_count_xz }
    pub fn cell_count_y_pub(&self) -> usize { self.cell_count_y }
    pub fn cell_noise_min_y_pub(&self) -> i32 { self.cell_noise_min_y }
    pub fn slice1_pub(&self) -> &[f64] { &self.interp.slice1 }
    pub fn compiled_pub(&self) -> &std::sync::Arc<Compiled> { &self.compiled }
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
    pub fn eval_plane_into(
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
        lanes: usize,
        out: &mut [f64],
    ) {
        let interp = InterpState::new(compiled.interp_slots, cell_count_xz, cell_count_y);
        let cols = cell_count_y + 1;
        let y_total = cell_count_y + 1;
        match lanes {
            8 => Self::eval_plane_batched::<8>(
                env, compiled, children, corner_x, first_cell_z, cell_count_xz,
                cell_noise_min_y, cw, ch, cols, y_total, &interp, out,
            ),
            _ => Self::eval_plane_batched::<4>(
                env, compiled, children, corner_x, first_cell_z, cell_count_xz,
                cell_noise_min_y, cw, ch, cols, y_total, &interp, out,
            ),
        }
    }

    /// Per-plane corner evaluation in groups of G y-corners (all slots).
    fn eval_plane_batched<const G: usize>(
        env: &Env,
        compiled: &Compiled,
        children: &[u32],
        corner_x: i32,
        first_cell_z: i32,
        cell_count_xz: usize,
        cell_noise_min_y: i32,
        cw: i32,
        ch: i32,
        cols: usize,
        y_total: usize,
        interp: &InterpState,
        out: &mut [f64],
    ) {
        let memo = BatchMemo::<G>::new(compiled.arena.len());
        let y_batch = y_total & !(G - 1); // groups of G
        for i in 0..=cell_count_xz {
            let corner_z = (first_cell_z + i as i32) * cw;
            let mut y = 0usize;
            while y < y_batch {
                let mut pts = [Pt { x: 0, y: 0, z: 0 }; G];
                for (k, pt) in pts.iter_mut().enumerate() {
                    *pt = Pt {
                        x: corner_x,
                        y: (y + k) as i32 * ch + cell_noise_min_y * ch,
                        z: corner_z,
                    };
                }
                for (slot, &child) in children.iter().enumerate() {
                    // G-wide batch: lane-identical arithmetic (bit-exact)
                    let v = env.eval_batch::<G>(compiled, child, &pts, Mode::Slice, interp, &memo);
                    let base = slot * (cell_count_xz + 1) * cols + i * cols + y;
                    out[base..base + G].copy_from_slice(&v);
                }
                y += G;
            }
            for y in y_batch..y_total {
                let corner_y = (y as i32 + cell_noise_min_y) * ch;
                let pt = Pt { x: corner_x, y: corner_y, z: corner_z };
                for (slot, &child) in children.iter().enumerate() {
                    let v = env.eval(compiled, child, pt, Mode::Slice, interp, None);
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

// ---------------- AVX2 cellfill row runner ----------------
#[cfg(target_arch = "x86_64")]
mod cellfill_simd {
    use super::CellOp;
    use std::arch::x86_64::*;
    use std::sync::OnceLock;

    pub fn avx2_ok() -> bool {
        static OK: OnceLock<bool> = OnceLock::new();
        *OK.get_or_init(|| std::arch::is_x86_feature_detected!("avx2"))
    }

    /// dz vector [0, s, s+s, s+s+s] (replicates scalar `dz += dx_step`)
    /// plus a zeroed stack.
    pub fn make_row_state(dx_step: f64) -> (__m256d, [__m256d; 16], usize) {
        unsafe {
            (
                _mm256_setr_pd(0.0, dx_step, dx_step + dx_step, dx_step + dx_step + dx_step),
                [_mm256_setzero_pd(); 16],
                0,
            )
        }
    }

    /// 4-wide stack-machine run: one row of 4 z-blocks sharing dx/dy (rows[]
    /// precomputed) and block_y; only dz varies per lane. Lane l = block l.
    /// Every op performs the exact per-lane IEEE operation sequence of the
    /// scalar runner (no FMA, no reassociation) — bit-identical results.
    #[target_feature(enable = "avx2")]
    pub unsafe fn run_cellfill_row4(
        prog: &[CellOp],
        rows: &[f64],
        dzv: __m256d,
        block_y: i32,
        stack: &mut [__m256d; 16],
        sp: &mut usize,
    ) -> __m256d {
        let zero = _mm256_setzero_pd();
        let sign_abs = _mm256_set1_pd(-0.0);
        *sp = 0;
        for op in prog {
            match op {
                CellOp::PushConst(v) => {
                    stack[*sp] = _mm256_set1_pd(*v);
                    *sp += 1;
                }
                CellOp::Lerp3 { slot } => {
                    let r1 = _mm256_set1_pd(rows[*slot as usize * 2]);
                    let r2 = _mm256_set1_pd(rows[*slot as usize * 2 + 1]);
                    // a + t*(b-a)
                    stack[*sp] = _mm256_add_pd(r1, _mm256_mul_pd(dzv, _mm256_sub_pd(r2, r1)));
                    *sp += 1;
                }
                CellOp::YGrad { from_y, to_y, from_value, to_value } => {
                    // block_y is constant across the row: fully scalar decision
                    let t = (block_y as f64 - *from_y as f64) / (*to_y as f64 - *from_y as f64);
                    let r = if t < 0.0 {
                        *from_value
                    } else if t > 1.0 {
                        *to_value
                    } else {
                        *from_value + t * (*to_value - *from_value)
                    };
                    stack[*sp] = _mm256_set1_pd(r);
                    *sp += 1;
                }
                CellOp::Add => {
                    *sp -= 1;
                    let b = stack[*sp];
                    *sp -= 1;
                    let a = stack[*sp];
                    stack[*sp] = _mm256_add_pd(a, b);
                    *sp += 1;
                }
                CellOp::Mul => {
                    *sp -= 1;
                    let b = stack[*sp];
                    *sp -= 1;
                    let a = stack[*sp];
                    // vanilla short-circuit: a == 0.0 ? 0.0 : a * b
                    let mask = _mm256_cmp_pd(a, zero, _CMP_EQ_OQ);
                    let prod = _mm256_mul_pd(a, b);
                    stack[*sp] = _mm256_blendv_pd(prod, zero, mask);
                    *sp += 1;
                }
                CellOp::Min => {
                    *sp -= 1;
                    let b = stack[*sp];
                    *sp -= 1;
                    let a = stack[*sp];
                    let mask = _mm256_cmp_pd(a, b, _CMP_LT_OQ);
                    stack[*sp] = _mm256_blendv_pd(b, a, mask);
                    *sp += 1;
                }
                CellOp::Max => {
                    *sp -= 1;
                    let b = stack[*sp];
                    *sp -= 1;
                    let a = stack[*sp];
                    let mask = _mm256_cmp_pd(a, b, _CMP_GT_OQ);
                    stack[*sp] = _mm256_blendv_pd(b, a, mask);
                    *sp += 1;
                }
                CellOp::Abs => {
                    *sp -= 1;
                    let a = stack[*sp];
                    stack[*sp] = _mm256_andnot_pd(sign_abs, a);
                    *sp += 1;
                }
                CellOp::Square => {
                    *sp -= 1;
                    let a = stack[*sp];
                    stack[*sp] = _mm256_mul_pd(a, a);
                    *sp += 1;
                }
                CellOp::Cube => {
                    *sp -= 1;
                    let a = stack[*sp];
                    let a2 = _mm256_mul_pd(a, a);
                    stack[*sp] = _mm256_mul_pd(a2, a);
                    *sp += 1;
                }
                CellOp::HalfNeg => {
                    *sp -= 1;
                    let a = stack[*sp];
                    let mask = _mm256_cmp_pd(a, zero, _CMP_GT_OQ);
                    stack[*sp] = _mm256_blendv_pd(_mm256_mul_pd(a, _mm256_set1_pd(0.5)), a, mask);
                    *sp += 1;
                }
                CellOp::QuarterNeg => {
                    *sp -= 1;
                    let a = stack[*sp];
                    let mask = _mm256_cmp_pd(a, zero, _CMP_GT_OQ);
                    stack[*sp] = _mm256_blendv_pd(_mm256_mul_pd(a, _mm256_set1_pd(0.25)), a, mask);
                    *sp += 1;
                }
                CellOp::Squeeze => {
                    *sp -= 1;
                    let a = stack[*sp];
                    // d = clamp(a, -1, 1); d/2.0 - d*d*d/24.0
                    let d = _mm256_min_pd(
                        _mm256_max_pd(a, _mm256_set1_pd(-1.0)),
                        _mm256_set1_pd(1.0),
                    );
                    let d3 = _mm256_mul_pd(_mm256_mul_pd(d, d), d);
                    let res = _mm256_sub_pd(
                        _mm256_div_pd(d, _mm256_set1_pd(2.0)),
                        _mm256_div_pd(d3, _mm256_set1_pd(24.0)),
                    );
                    stack[*sp] = res;
                    *sp += 1;
                }
                CellOp::Clamp { min, max } => {
                    *sp -= 1;
                    let a = stack[*sp];
                    stack[*sp] = _mm256_min_pd(
                        _mm256_max_pd(a, _mm256_set1_pd(*min)),
                        _mm256_set1_pd(*max),
                    );
                    *sp += 1;
                }
                CellOp::RangeChoice { min_inclusive, max_exclusive } => {
                    *sp -= 1;
                    let out = stack[*sp];
                    *sp -= 1;
                    let inr = stack[*sp];
                    *sp -= 1;
                    let v = stack[*sp];
                    let m1 = _mm256_cmp_pd(v, _mm256_set1_pd(*min_inclusive), _CMP_GE_OQ);
                    let m2 = _mm256_cmp_pd(v, _mm256_set1_pd(*max_exclusive), _CMP_LT_OQ);
                    let mask = _mm256_and_pd(m1, m2);
                    stack[*sp] = _mm256_blendv_pd(out, inr, mask);
                    *sp += 1;
                }
                CellOp::MulOrAdd { is_add, argument } => {
                    *sp -= 1;
                    let a = stack[*sp];
                    stack[*sp] = if *is_add {
                        _mm256_add_pd(a, _mm256_set1_pd(*argument))
                    } else if *argument == 0.0 {
                        zero
                    } else {
                        _mm256_mul_pd(a, _mm256_set1_pd(*argument))
                    };
                    *sp += 1;
                }
            }
        }
        stack[0]
    }
}
