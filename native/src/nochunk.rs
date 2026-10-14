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
use std::sync::Mutex;
use std::sync::atomic::{AtomicI32, AtomicI64, AtomicU32, AtomicUsize, AtomicU64, Ordering};
//   - postprocessing marks (shouldScheduleFluidUpdate) are skipped: they carry
//     no block data, and the Bukkit noise path cannot express them.

use crate::density::{compile_cellfill, flatten_cellfill, BatchMemo, CellOp, Compiled, Env, FlatOp, FlatProg, InterpState, MemoCtx, Mode, Pt, V4Memo, V8Memo, WKind};

#[cfg(target_arch = "x86_64")]
#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::_mm512_castpd512_pd256;
#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::_mm512_extractf64x4_pd;
#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::_mm512_setr_pd;
#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::_mm256_loadu_pd;
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
/// Aquifer caches, SHARED across fill threads (Arc): all four strips of a
/// chunk touch the SAME base cells (gx spans only 0..1 for a 16-block chunk),
/// so per-thread caches duplicated the whole status/loc/summary work
/// (fluid_calls 261 -> 429, worker strips 1.3x slower). The entries are
/// first-touch-then-read-only per chunk -> no ping-pong after warmup, and
/// each entry is cache-line PADDED so adjacent grid-x indices never share
/// a line (cont-20).
#[repr(align(64))]
struct PaddedU64(std::sync::atomic::AtomicU64);
#[repr(align(64))]
struct PaddedI64(std::sync::atomic::AtomicI64);

struct AquiferState {
    cache: Vec<PaddedU64>,
    loc_cache: Vec<PaddedI64>,
    /// Per BASE-cell summary: all 12 neighborhood statuses equal + the shared
    /// status. The 12-cell neighborhood depends only on the base cell
    /// (gx, gy, gz), never on the block itself — so the all-same verdict and
    /// the uniform status are pure functions of the base cell and can be
    /// memoized. Reset per chunk together with the other caches.
    summary: Vec<PaddedU64>,
    min_grid_x: std::sync::atomic::AtomicI32,
    min_grid_y: i32,
    min_grid_z: std::sync::atomic::AtomicI32,
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
            cache: (0..cells).map(|_| PaddedU64(std::sync::atomic::AtomicU64::new(0))).collect(),
            loc_cache: (0..cells).map(|_| PaddedI64(std::sync::atomic::AtomicI64::new(i64::MAX))).collect(),
            summary: (0..cells).map(|_| PaddedU64(std::sync::atomic::AtomicU64::new(0))).collect(),
            min_grid_x: std::sync::atomic::AtomicI32::new(0),
            min_grid_y,
            min_grid_z: std::sync::atomic::AtomicI32::new(0),
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
            v.0.store(i64::MAX, Ordering::Relaxed);
        }
        for v in self.cache.iter() {
            v.0.store(0, Ordering::Relaxed);
        }
        for v in self.summary.iter() {
            v.0.store(0, Ordering::Relaxed);
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

// ---- Global native-thread budget ----
// Paper calls fill() from SEVERAL chunk-worker threads concurrently. Without
// coordination, each fill spawns its own plane/strip/pipeline workers ->
// (Paper workers) x (internal threads) oversubscription: on a 4-vCPU CI runner
// that measured 16.4ms p50 per chunk vs ~7ms local. A process-wide permit pool
// (budget = cores) hands internal parallelism only to fills that can actually
// use it; concurrent fills run serial or partially parallel instead of
// thrashing. Bit-exactness is unaffected: all parallel paths were already
// deterministic partitions of pure functions.
static THREAD_PERMITS: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

fn thread_budget() -> usize {
    static BUDGET: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *BUDGET.get_or_init(|| {
        std::env::var("CRUS_MAX_THREADS")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|&n| n >= 1)
            .unwrap_or_else(|| std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1))
    })
}

/// Take up to `want` permits from the global pool; returns granted (0..=want).
#[inline]
fn take_permits(want: usize) -> usize {
    if want == 0 {
        return 0;
    }
    loop {
        let cur = THREAD_PERMITS.load(std::sync::atomic::Ordering::Relaxed);
        let avail = (thread_budget() as isize - cur).max(0) as usize;
        let grant = avail.min(want);
        if grant > 0 {
            if THREAD_PERMITS
                .compare_exchange_weak(
                    cur,
                    cur + grant as isize,
                    std::sync::atomic::Ordering::Relaxed,
                    std::sync::atomic::Ordering::Relaxed,
                )
                .is_ok()
            {
                return grant;
            }
        } else {
            return 0;
        }
    }
}

#[inline]
fn give_permits(n: usize) {
    if n > 0 {
        THREAD_PERMITS.fetch_sub(n as isize, std::sync::atomic::Ordering::Relaxed);
    }
}

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
    /// Fused 8-lane AVX-512 2-row cellfill runner (calibrated at startup).
    cellfill_wide8: bool,
    /// Static-stack-slot form of cellfill_prog (preferred by the AVX2 runners).
    cellfill_flat: Option<FlatProg>,
    /// Reusable strip-worker clones (fill_parallel): avoids re-allocating
    /// and re-zeroing ~1MB of worker buffers (cell_cache, interp, rows) on
    /// every parallel fill. Taken out at fill start (no aliasing), refreshed
    /// with the current fill's per-fill state, returned after the join.
    strip_pool: Option<Vec<ChunkGen<'e>>>,
    /// Reused per-fill strip out buffers (192KB each): fresh Box allocations
    /// fault ~96 pages per buffer per fill on the parallel path.
    out_pool: Vec<Box<[u16; OUT_LEN]>>,
    /// Persistent plane worker for the pipeline path (main gen only).
    plane_worker: Option<PlaneWorker>,
    cellfill_stack: Vec<f64>,
    /// Per-(dx,dy) hoisted trilinear rows: per slot [row1, row2] so the
    /// per-block cost is a single z-lerp per Lerp3 slot (bit-exact: the
    /// hoisted x/y lerps are the same ops with the same inputs).
    cellfill_rows: Vec<f64>,
    cellfill_rows8: Vec<f64>,
    /// Slot ids actually referenced by the cellfill program (hoist these only).
    cellfill_slots: Vec<u16>,
    /// 4-lane batch memo for the AVX2 preliminary-surface scan (per-thread).
    /// 8 lanes measured SLOWER on the 2-core VM (material 1.18 vs 1.04):
    /// the [f64;8] tree walk doubles the memo footprint and the wider
    /// [f64;8] stack ops spill (same lesson as the slice lanes=8 A/B).
    prelim_memo: crate::density::BatchMemo<4>,
    /// Prelim scan mode calibration: the 4-lane batch wins on land worlds
    /// (early stops; real-seed p50 2.45 vs 3.62 scalar) and loses on ocean
    /// worlds (full-height scans; seed-0 13.8 batched vs 9.3 scalar). The
    /// first 12 scans alternate modes with timing; the winner is then fixed
    /// for the process (the world type is a per-seed constant).
    prelim_use_batch: bool,
    prelim_cal_scans: i32,
    prelim_batch_ns: u64,
    prelim_scalar_ns: u64,
    /// preliminarySurfaceLevel cache: lock-free direct-mapped atomic slots
    /// keyed by the quart column (bx, bz). The level is a pure function of
    /// the column + seed, so racing writers store IDENTICAL values (benign
    /// race, bit-exact). Shared across fill threads via Arc: the parallel
    /// 12-cell scans read ~3.9K entries per fill — a Mutex made them 1.9x
    /// slower, per-thread maps duplicated the expensive surface scans
    /// (46 -> 73 scans/fill); this cache has neither problem (cont-20).
    prelim: std::sync::Arc<PrelimCache>,
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
    /// Fast-path resolve of the vein_toggle node: when the router root is a
    /// bare Noise node, the 4 z-neighborhood blocks of an all-solid row can
    /// sample it through one AVX2 batch (lane-identical arithmetic,
    /// bit-exact vs the per-block scalar eval).
    ore_toggle_fast: Option<OreToggleFast>,
    /// Env-gated diagnostics, resolved ONCE at construction (getenv in the
    /// hot path costs several us per fill at 5+ call sites).
    dbg_slice: bool,
    dbg_plane: bool,
    dump_planes_want: Option<i32>,
    pipeline_forced: Option<bool>,
}

#[derive(Clone, Copy)]
struct OreToggleFast {
    noise_idx: usize,
    xz_scale: f64,
    y_scale: f64,
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
    // fill_parallel wall breakdown (main-thread view)
    pub par_planes_ns: u64,
    pub par_strips_ns: u64,
    pub par_merge_ns: u64,
}

// Aquifer slow-path counters (CRUS_DIAG=1): localize the parallel material
// expansion by counting sub-path events globally across all threads.
pub static AQ_SUBSTANCE_CALLS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);
pub static AQ_SUBSTANCE_NS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);
pub static AQ_SUMMARY_HITS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);
pub static AQ_SUMMARY_MISS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);
pub static AQ_SCAN_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static PRELIM_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static AQ_HIT_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static PRELIM_SCANS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static PRELIM_SCAN_STEPS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);
pub static AQ_FLUID_CALLS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

// Global per-fill phase accumulators (JNI diagnostics under Paper): the
// plugin reads them via profStatsGlobal() to see the fill's phase split
// inside the measured window.
pub static JP_FILLS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static JP_WALL_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static JP_SLICE_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static JP_CELLFILL_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static JP_MATERIAL_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static JP_SELECT_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static JP_PAR_PLANES_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static JP_PAR_STRIPS_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Accumulate one fill's phase deltas into the global JNI prof.
fn jni_prof_accum(before: &Prof, after: &Prof, wall_ns: u64) {
    use std::sync::atomic::Ordering::Relaxed;
    JP_FILLS.fetch_add(1, Relaxed);
    JP_WALL_NS.fetch_add(wall_ns, Relaxed);
    JP_SLICE_NS.fetch_add(after.fill_slice_ns - before.fill_slice_ns, Relaxed);
    JP_CELLFILL_NS.fetch_add(after.cellfill_ns - before.cellfill_ns, Relaxed);
    JP_MATERIAL_NS.fetch_add(after.material_ns - before.material_ns, Relaxed);
    JP_SELECT_NS.fetch_add(after.select_ns - before.select_ns, Relaxed);
    JP_PAR_PLANES_NS.fetch_add(after.par_planes_ns - before.par_planes_ns, Relaxed);
    JP_PAR_STRIPS_NS.fetch_add(after.par_strips_ns - before.par_strips_ns, Relaxed);
}

/// Compact one-line snapshot for the plugin stats command.
pub fn jni_prof_snapshot() -> String {
    use std::sync::atomic::Ordering::Relaxed;
    let n = JP_FILLS.load(Relaxed).max(1) as f64;
    let f = |a: &std::sync::atomic::AtomicU64| a.load(Relaxed) as f64 / n / 1e6;
    format!(
        "jni_prof[fills={} wall={:.2}ms slice={:.2} cellfill={:.2} material={:.2} select={:.2} par_planes={:.2} par_strips={:.2}]",
        JP_FILLS.load(Relaxed),
        f(&JP_WALL_NS),
        f(&JP_SLICE_NS),
        f(&JP_CELLFILL_NS),
        f(&JP_MATERIAL_NS),
        f(&JP_SELECT_NS),
        f(&JP_PAR_PLANES_NS),
        f(&JP_PAR_STRIPS_NS),
    )
}

struct AqTimer {
    t0: std::time::Instant,
}
impl Drop for AqTimer {
    fn drop(&mut self) {
        AQ_SUBSTANCE_NS.fetch_add(
            self.t0.elapsed().as_nanos() as u64,
            std::sync::atomic::Ordering::Relaxed,
        );
    }
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

/// Dependency-aware fill graph shared state. Tasks: corner planes
/// (P1..=Pccx) and x-strips (S0..Sccx-1); strip S_k unlocks when planes
/// k and k+1 are ready. Threads (main + workers) pull from atomic
/// counters: ready strips first, else the next unclaimed plane. On one
/// active thread this reproduces the exact old pipeline order
/// (P1 -> S0 -> P2 -> S1 -> ...). Planes are evaluated exactly once;
/// each strip runs exactly once on its own clone + out buffer, so the
/// merged output is bit-identical to the serial fill.
struct FillGraph<'a, 'e> {
    store: &'a Mutex<Vec<Option<std::sync::Arc<Vec<f64>>>>>,
    plane_ready: &'a AtomicU32,
    next_plane: &'a AtomicUsize,
    next_strip: &'a AtomicUsize,
    strips_done: &'a AtomicUsize,
    plane_ns: &'a AtomicU64,
    strip_ns: &'a AtomicU64,
    env: &'e Env,
    compiled: &'a std::sync::Arc<Compiled>,
    children: &'a [u32],
    first_cell_x: i32,
    first_cell_z: i32,
    min_block_x: i32,
    min_block_z: i32,
    ccx: usize,
    ccy: usize,
    cnmy: i32,
    cw: i32,
    ch: i32,
    lanes: usize,
    plane_len: usize,
    diag: bool,
}

impl<'a, 'e> FillGraph<'a, 'e> {
    fn run(&self, g: &mut ChunkGen<'e>, out_t: &mut [u16; OUT_LEN], tp: &mut Prof) {
        loop {
            // 1) claim a ready strip (k-order scan)
            let mut claimed = false;
            loop {
                let k = self.next_strip.load(Ordering::Relaxed);
                if k >= self.ccx {
                    break;
                }
                let need = (1u32 << k) | (1u32 << (k + 1));
                if self.plane_ready.load(Ordering::Acquire) & need == need {
                    if self
                        .next_strip
                        .compare_exchange_weak(k, k + 1, Ordering::AcqRel, Ordering::Relaxed)
                        .is_ok()
                    {
                        let (p0, p1) = {
                            let st = self.store.lock().unwrap();
                            (
                                st[k].clone().expect("plane k missing"),
                                st[k + 1].clone().expect("plane k+1 missing"),
                            )
                        };
                        let t0 = std::time::Instant::now();
                        g.load_planes(&p0, &p1);
                        let prof_en = tp.enabled;
                        g.process_strip(
                            k as i32,
                            self.first_cell_x,
                            self.first_cell_z,
                            self.min_block_x,
                            self.min_block_z,
                            prof_en,
                            false,
                            out_t,
                            tp,
                        );
                        self.strip_ns.fetch_add(
                            t0.elapsed().as_nanos() as u64,
                            Ordering::Relaxed,
                        );
                        self.strips_done.fetch_add(1, Ordering::AcqRel);
                        claimed = true;
                        break;
                    }
                } else {
                    break;
                }
            }
            if claimed {
                continue;
            }
            // 2) claim the next plane
            let k = self.next_plane.fetch_add(1, Ordering::Relaxed);
            if k <= self.ccx {
                let t0 = std::time::Instant::now();
                let mut buf = vec![0f64; self.plane_len];
                ChunkGen::eval_plane_into(
                    self.env,
                    self.compiled,
                    self.children,
                    (self.first_cell_x + k as i32) * self.cw,
                    self.first_cell_z,
                    self.ccx,
                    self.ccy,
                    self.cnmy,
                    self.cw,
                    self.ch,
                    self.lanes,
                    &mut buf,
                );
                self.plane_ns
                    .fetch_add(t0.elapsed().as_nanos() as u64, Ordering::Relaxed);
                self.store.lock().unwrap()[k] = Some(std::sync::Arc::new(buf));
                self.plane_ready.fetch_or(1 << k, Ordering::AcqRel);
                continue;
            }
            // 3) planes exhausted: exit when every strip has run
            if self.strips_done.load(Ordering::Acquire) == self.ccx {
                break;
            }
            std::hint::spin_loop();
        }
        if self.diag {
            eprintln!(
                "[par-diag] worker cf={:.3}mt mt={:.3}mt",
                tp.cellfill_ns as f64 / 1e6,
                tp.material_ns as f64 / 1e6
            );
        }
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
        let cellfill_prog_opt = compile_cellfill(&compiled.arena, compiled.router["final_density"]);
        let ore_toggle_fast = {
            let vt = compiled.router["vein_toggle"] as usize;
            match &compiled.arena[vt] {
                WKind::Noise { noise_id, xz_scale, y_scale } => Some(OreToggleFast {
                    noise_idx: compiled.noise_resolved[*noise_id as usize] as usize,
                    xz_scale: *xz_scale,
                    y_scale: *y_scale,
                }),
                _ => None,
            }
        };
        let pipeline_forced = if std::env::var("CRUS_NO_PIPELINE").is_ok() {
            Some(false)
        } else if std::env::var("CRUS_PIPELINE").ok().as_deref() == Some("1") {
            Some(true)
        } else {
            None
        };
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
            cellfill_prog: cellfill_prog_opt.clone(),
            cellfill_wide8: cellfill_prog_opt
                .as_ref()
                .and_then(|p| flatten_cellfill(p))
                .map(|flat| cellfill_simd::pick_cellfill_wide(&flat))
                .unwrap_or(false),
            cellfill_flat: cellfill_prog_opt
                .as_ref()
                .and_then(|p| flatten_cellfill(p)),
            strip_pool: Some(Vec::new()),
            out_pool: Vec::new(),
            plane_worker: None,
            cellfill_stack: Vec::with_capacity(16),
            cellfill_rows: vec![0.0; compiled.interp_slots as usize * 4],
            cellfill_rows8: vec![0.0; compiled.interp_slots as usize * 16],
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
            prelim_memo: crate::density::BatchMemo::new(compiled.arena.len())
                .with_cols(compiled.col_count as usize),
            prelim_use_batch: true,
            prelim_cal_scans: 0,
            prelim_batch_ns: 0,
            prelim_scalar_ns: 0,
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
            ore_toggle_fast,
            dbg_slice: std::env::var("CRUS_DBG_SLICE").is_ok(),
            dbg_plane: std::env::var("CRUS_DBG_PLANE").is_ok(),
            dump_planes_want: std::env::var("CRUS_DUMP_PLANES")
                .ok()
                .and_then(|v| v.parse::<i32>().ok()),
            pipeline_forced,
            interp,
            cell_cache: vec![0.0; cell_width * cell_height * cell_width],
            prelim: std::sync::Arc::new(PrelimCache::new()),
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
                    let mut memo = BatchMemo::<8>::new(compiled.arena.len()).with_cols(compiled.col_count as usize);
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
                    let mut memo = BatchMemo::<4>::new(compiled.arena.len()).with_cols(compiled.col_count as usize);
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
    pub fn interp_children_len(&self) -> usize {
        self.interp_children.len()
    }
    pub fn slice0_len(&self) -> usize {
        self.interp.slice0_len()
    }
    pub fn plane_cache_len(&self) -> usize {
        self.plane_cache.len()
    }

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
        let key = ((bx as u64 & 0x3FF_FFFF) << 26) | (bz as u64 & 0x3FF_FFFF);
        if let Some(v) = self.prelim.get(key) {
            return v;
        }
        let mut result = i32::MAX;
        let mut y = self.min_y + self.env.wg.height;
        let diag = crate::density::PROF_ON.load(std::sync::atomic::Ordering::Relaxed);
        let prelim_t0 = if diag { Some(std::time::Instant::now()) } else { None };
        if diag {
            PRELIM_SCANS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        let ch = self.cell_height as i32;
        if std::env::var("CRUS_PRELIM_SCALAR").is_ok() {
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
                y -= ch;
            }
            if let Some(t) = prelim_t0 {
                PRELIM_NS.fetch_add(t.elapsed().as_nanos() as u64, std::sync::atomic::Ordering::Relaxed);
            }
            self.prelim.put(key, result);
            return result;
        }
        // AVX2 4-lane scan: four descending grid points per batch
        // (lane-identical arithmetic -> bit-exact vs scalar; the first lane
        // above the threshold wins, matching the top-down scalar loop).
        // The mode (batch vs scalar) is calibrated per world — see the
        // prelim_use_batch field doc.
        let calibrating = self.prelim_cal_scans < 12;
        let use_batch = if calibrating {
            self.prelim_cal_scans % 2 == 0
        } else {
            self.prelim_use_batch
        };
        let mode_t0 = if calibrating { Some(std::time::Instant::now()) } else { None };
        if use_batch {
        let mut pts = [Pt { x: bx, y: 0, z: bz }; 4];
        while y - 3 * ch >= self.min_y {
            for (k, pt) in pts.iter_mut().enumerate() {
                pt.y = y - (k as i32) * ch;
                if diag {
                    PRELIM_SCAN_STEPS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
            }
            let v = self.env.eval_batch::<4>(
                &self.compiled,
                self.initial_density,
                &pts,
                Mode::Direct,
                &self.interp,
                &self.prelim_memo,
            );
            let mut hit = None;
            for (k, &val) in v.iter().enumerate() {
                if val > 0.390625 {
                    hit = Some(k);
                    break;
                }
            }
            if let Some(k) = hit {
                result = y - (k as i32) * ch;
                break;
            }
            y -= 4 * ch;
        }
        } else {
        while y >= self.min_y {
            if diag {
                PRELIM_SCAN_STEPS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            let d = self.eval_fresh(
                self.initial_density,
                Pt { x: bx, y, z: bz },
                Mode::Direct,
            );
            if d > 0.390625 {
                result = y;
                break;
            }
            y -= ch;
        }
        }
        if let (true, Some(t)) = (calibrating, mode_t0) {
            let dt = t.elapsed().as_nanos() as u64;
            if use_batch {
                self.prelim_batch_ns += dt;
            } else {
                self.prelim_scalar_ns += dt;
            }
            self.prelim_cal_scans += 1;
            if self.prelim_cal_scans == 12 {
                self.prelim_use_batch = self.prelim_batch_ns <= self.prelim_scalar_ns;
            }
        }
        if let Some(t) = prelim_t0 {
            PRELIM_NS.fetch_add(t.elapsed().as_nanos() as u64, std::sync::atomic::Ordering::Relaxed);
        }
        self.prelim.put(key, result);
        result
    }

}

const PRELIM_SLOT_BITS: u64 = 14;
const PRELIM_SLOTS: usize = 1 << PRELIM_SLOT_BITS;

/// Lock-free direct-mapped cache: entry = (key << 9) | (level + 65);
/// 0 = empty. level in [-64, 384] + 65 fits 9 bits; key = 26-bit bx/bz.
struct PrelimCache {
    slots: Vec<std::sync::atomic::AtomicU64>,
    mask: u64,
}

impl PrelimCache {
    fn new() -> PrelimCache {
        PrelimCache {
            slots: (0..PRELIM_SLOTS)
                .map(|_| std::sync::atomic::AtomicU64::new(0))
                .collect(),
            mask: (PRELIM_SLOTS - 1) as u64,
        }
    }

    #[inline]
    fn get(&self, key: u64) -> Option<i32> {
        use std::sync::atomic::Ordering::Relaxed;
        let idx = (key.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 24) & self.mask;
        let e = self.slots[idx as usize].load(Relaxed);
        if e != 0 && (e >> 9) == key {
            Some(((e & 0x1FF) as i32) - 65)
        } else {
            None
        }
    }

    #[inline]
    fn put(&self, key: u64, level: i32) {
        use std::sync::atomic::Ordering::Relaxed;
        let idx = (key.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 24) & self.mask;
        let e = (key << 9) | (level + 65) as u64;
        self.slots[idx as usize].store(e, Relaxed);
    }
}

/// Persistent plane-evaluation worker: ONE long-lived thread per main
/// generator, parked on a channel between planes. The spawn-per-plane
/// pipeline paid a schedule round-trip per plane; under Paper's saturated
/// 2-core scheduler that latency dominates (judge nat_p50 5-7.3ms vs 1.8ms
/// isolated). The worker owns the shared-immutable world state (env as a
/// raw 'e-borrow: ChunkGen::drop sends Stop and joins BEFORE self can
/// unwind — the env cannot dangle), the Compiled Arc and the children list.
/// Plane buffers are double-buffered through the channel: the worker fills
/// one Vec while the main thread scatters the previous one.
enum PlaneMsg {
    Eval {
        plane_x: i32,
        first_cell_z: i32,
        buf: Vec<f64>,
    },
    Stop,
}

struct PlaneWorker {
    tx: std::sync::mpsc::Sender<PlaneMsg>,
    rx: std::sync::mpsc::Receiver<Vec<f64>>,
    handle: Option<std::thread::JoinHandle<()>>,
    /// the buffer to send with the next request (None until the first fill)
    spare: Option<Vec<f64>>,
}

impl PlaneWorker {
    fn spawn(
        env_addr: usize,
        compiled: std::sync::Arc<Compiled>,
        children: Vec<u32>,
        cell_count_xz: usize,
        cell_count_y: usize,
        cell_noise_min_y: i32,
        cw: i32,
        ch: i32,
        lanes: usize,
        plane_len: usize,
    ) -> PlaneWorker {
        let (tx, rx) = std::sync::mpsc::channel::<PlaneMsg>();
        let (dtx, drx) = std::sync::mpsc::channel::<Vec<f64>>();
        let handle = std::thread::Builder::new()
            .stack_size(1 << 21)
            .spawn(move || {
                // SAFETY: ChunkGen::drop sends Stop and joins this thread
                // before the env borrow can end.
                let env: &Env = unsafe { &*(env_addr as *const Env) };
                let children: &[u32] = &children;
                loop {
                    match rx.recv() {
                        Ok(PlaneMsg::Eval { plane_x, first_cell_z, mut buf }) => {
                            debug_assert_eq!(buf.len(), plane_len);
                            <ChunkGen<'static>>::eval_plane_into(
                                env,
                                &compiled,
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
                            if dtx.send(buf).is_err() {
                                return;
                            }
                        }
                        Ok(PlaneMsg::Stop) | Err(_) => return,
                    }
                }
            })
            .expect("spawn persistent plane worker");
        PlaneWorker {
            tx,
            rx: drx,
            handle: Some(handle),
            spare: None,
        }
    }

    fn request(&mut self, plane_x: i32, first_cell_z: i32, plane_len: usize) {
        let buf = self.spare.take().unwrap_or_else(|| vec![0f64; plane_len]);
        if self.tx.send(PlaneMsg::Eval { plane_x, first_cell_z, buf }).is_err() {
            // worker died (panic): fall back to inline evaluation by leaving
            // the channel closed; fill() checks recv errors.
        }
    }

    /// Receive the last requested plane buffer (blocks until ready).
    fn take_done(&mut self) -> Option<Vec<f64>> {
        self.rx.recv().ok()
    }

    fn stop_and_join(mut self) {
        let _ = self.tx.send(PlaneMsg::Stop);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl<'e> Drop for ChunkGen<'e> {
    fn drop(&mut self) {
        if let Some(w) = self.plane_worker.take() {
            w.stop_and_join();
        }
    }
}

impl<'e> ChunkGen<'e> {
    /// Fresh per-thread generator clone for the parallel cell path.
    ///
    /// Shares (immutable or synchronized): env, compiled (Arc), prelim
    /// (lock-free atomic cache). Everything mutable is FRESH, not copied: memo, aquifer
    /// caches, marks, cell scratch. The beard is deep-copied (per-chunk data,
    /// already set by the plugin before fill). Prof counters start zeroed
    /// (only the enabled flag carries over) so the post-join merge is a
    /// plain sum of per-thread deltas.
    fn thread_clone(&self, cx: i32, cz: i32) -> ChunkGen<'e> {
        ChunkGen {
            sched: false,
            marks: Vec::with_capacity(1024),
            beard: self.beard.clone(),
            cellfill_wide8: self.cellfill_wide8,
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
            ore_toggle_fast: self.ore_toggle_fast,
            dbg_slice: self.dbg_slice,
            dbg_plane: self.dbg_plane,
            dump_planes_want: self.dump_planes_want,
            pipeline_forced: self.pipeline_forced,
            interp: self.interp.clone(),
            cell_cache: vec![0.0; self.cell_cache.len()],
            final_density_slot: self.final_density_slot,
            cellfill_prog: self.cellfill_prog.clone(),
            cellfill_flat: self.cellfill_flat.clone(),
            strip_pool: None,
            out_pool: Vec::new(),
            plane_worker: None,
            cellfill_stack: Vec::with_capacity(16),
            cellfill_rows: vec![0.0; self.cellfill_rows.len()],
            cellfill_rows8: vec![0.0; self.cellfill_rows8.len()],
            cellfill_slots: self.cellfill_slots.clone(),
            prelim_memo: crate::density::BatchMemo::new(self.compiled.arena.len())
                .with_cols(self.compiled.col_count as usize),
            prelim_use_batch: self.prelim_use_batch,
            prelim_cal_scans: self.prelim_cal_scans,
            prelim_batch_ns: self.prelim_batch_ns,
            prelim_scalar_ns: self.prelim_scalar_ns,
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

    /// Refresh a pooled strip-worker clone with the current fill's per-fill
    /// state (everything else — cell_cache, interp planes, rows — is fully
    /// overwritten by process_strip before it is read).
    fn strip_reset_for_fill(&mut self, beard: &crate::beard::Beard) {
        self.sched = false;
        self.marks.clear();
        self.beard = beard.clone();
        self.prof = Prof {
            enabled: self.prof.enabled,
            ..Default::default()
        };
        self.memo = MemoCtx::new(self.compiled.arena.len());
    }

    /// Parallel fill: all corner planes up front (workers over planes), then
    /// x-strips distributed over per-thread generator clones. Marks, prof
    /// counters and out buffers are merged after the join.
    fn fill_parallel(&mut self, cx: i32, cz: i32, out: &mut [u16; OUT_LEN], p: &mut Prof) {
        let prof = p.enabled;
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
        let slice_threads_cap = self.slice_threads.min(ccx);
        let nthr = slice_threads_cap;
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

        // Dependency-aware fill graph: planes and strips are one task pool
        // pulled by main + workers. Replaces the phase-sequential scheme
        // (all planes -> barrier -> all strips) whose two full sync points
        // cost ~0.4-0.5ms per fill on 2 cores. Worker count is bounded by
        // the GLOBAL thread budget (permits); with 0 permits main runs the
        // loop alone, reproducing the old pipeline order exactly.
        let par_t0 = if prof { Some(std::time::Instant::now()) } else { None };
        let workers = take_permits(slice_threads_cap.saturating_sub(1));
        let plane_ns_total = AtomicU64::new(0);
        let strip_ns_total = AtomicU64::new(0);
        let env = self.env;
            let compiled = std::sync::Arc::clone(&self.compiled);
            let children = self.interp_children.clone();
            let plane0 = planes.pop().expect("plane0 present");
            let store: Mutex<Vec<Option<std::sync::Arc<Vec<f64>>>>> =
                Mutex::new((0..=ccx).map(|_| None).collect());
            store.lock().unwrap()[0] = Some(std::sync::Arc::new(plane0));
            let plane_ready = AtomicU32::new(1);
            let next_plane = AtomicUsize::new(1);
            let next_strip = AtomicUsize::new(0);
            let strips_done = AtomicUsize::new(0);
            let graph = FillGraph {
                store: &store,
                plane_ready: &plane_ready,
                next_plane: &next_plane,
                next_strip: &next_strip,
                strips_done: &strips_done,
                plane_ns: &plane_ns_total,
                strip_ns: &strip_ns_total,
                env,
                compiled: &compiled,
                children: &children,
                first_cell_x,
                first_cell_z,
                min_block_x,
                min_block_z,
                ccx,
                ccy,
                cnmy,
                cw,
                ch,
                lanes,
                plane_len,
                diag: std::env::var("CRUS_PAR_DIAG").ok().as_deref() == Some("1"),
            };

            // worker clones + out buffers (reuse pools; buffers are fully
            // overwritten per strip; per-fill state refreshed below)
            let mut pool = self.strip_pool.take().unwrap_or_default();
            let mut gens: Vec<ChunkGen<'e>> = Vec::with_capacity(workers);
            for _ in 0..workers {
                match pool.pop() {
                    Some(mut g) => {
                        g.strip_reset_for_fill(&self.beard);
                        // pooled clones carry the PREVIOUS fill's grid origin
                        // and stale aquifer entries -> re-base per fill.
                        g.aquifer.begin_chunk(cx, cz);
                        gens.push(g);
                    }
                    None => gens.push(self.thread_clone(cx, cz)),
                }
            }
            let mut out_pool = std::mem::take(&mut self.out_pool);
            let mut outs: Vec<Box<[u16; OUT_LEN]>> = Vec::with_capacity(workers);
            for _ in 0..workers {
                match out_pool.pop() {
                    Some(mut b) => {
                        b.fill(AIR);
                        outs.push(b);
                    }
                    None => outs.push(Box::new([0u16; OUT_LEN])),
                }
            }
            let mut out0 = out_pool.pop().unwrap_or_else(|| Box::new([0u16; OUT_LEN]));
            out0.fill(AIR);

            std::thread::scope(|s| {
                for (g, out_t) in gens.iter_mut().zip(outs.iter_mut()) {
                    s.spawn(|| {
                        let mut tp = g.prof;
                        graph.run(g, out_t, &mut tp);
                        g.prof = tp;
                    });
                }
                // main thread participates in the same task pool
                graph.run(self, &mut out0, p);
            });

            p.par_planes_ns += plane_ns_total.load(Ordering::Relaxed);
            p.par_strips_ns += strip_ns_total.load(Ordering::Relaxed);

            // save the final plane for the next fill's plane-0 cache
            let last = store.lock().unwrap()[ccx].clone().expect("final plane");
            self.plane_cache.clear();
            self.plane_cache.extend_from_slice(&last);
            self.plane_cache_x = ((first_cell_x + ccx as i32) * cw) as i64;
        self.plane_cache_z = first_cell_z;

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
        for t in 0..gens.len() {
            let g = &mut gens[t];
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
        if let Some(t) = par_t0 {
            p.par_merge_ns += t.elapsed().as_nanos() as u64;
        }
        // hand the worker clones back to the pool for the next fill
        // (after the marks/prof merge above — gens must stay intact)
        pool.extend(gens.drain(..));
        self.strip_pool = Some(pool);
        // recycle the strip out buffers
        for mut b in outs.into_iter() {
            b.fill(AIR);
            out_pool.push(b);
        }
        out0.fill(AIR);
        out_pool.push(out0);
        self.out_pool = out_pool;
        give_permits(workers);
    }

    /// Vanilla NoiseBasedChunkGenerator.doFill.
    pub fn fill(&mut self, cx: i32, cz: i32, out: &mut [u16; OUT_LEN]) {
        let jni_before = self.prof;
        let jni_t0 = std::time::Instant::now();
        self.fill_inner(cx, cz, out);
        jni_prof_accum(&jni_before, &self.prof, jni_t0.elapsed().as_nanos() as u64);
    }

    fn fill_inner(&mut self, cx: i32, cz: i32, out: &mut [u16; OUT_LEN]) {
        // Prof counters ACCUMULATE across fills (bench resets after warmup).
        let mut p = self.prof;
        let prof = p.enabled;
        let dbg = self.dbg_slice;
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
        let dbg = self.dbg_slice;
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
        } else if self.dbg_plane {
            eprintln!(
                "[plane-miss] want_x={} have_x={} want_z={} have_z={} len {}/{}",
                first_cell_x * cw, self.plane_cache_x, first_cell_z, self.plane_cache_z,
                self.plane_cache.len(), plane_len
            );
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
        let plane_len = self.interp_children.len() * (self.cell_count_xz + 1) * (self.cell_count_y + 1);
        // Pipeline: overlaps the next x-plane's corner evaluation (worker)
        // with this plane's cell work. After the cellfill/material fast
        // paths (flat cellfill, ore row4, aquifer summary) the strip work
        // (2.5ms) comfortably hides plane evaluation (0.41ms/plane) even
        // on 2 cores: measured wall 3.2 vs 4.6 (CRUS_PIPELINE=1 A/B on the
        // 2-core dev VM, cont-17). Default: on when >= 2 logical cores.
        let pipeline = match (self.pipeline_forced, std::env::var("CRUS_PIPELINE").ok().as_deref() == Some("1")) {
            (Some(false), _) => false,
            (Some(true), _) => true,
            (None, _) => std::thread::available_parallelism()
                .map(|n| n.get() >= 2)
                .unwrap_or(false),
        };
        // the pipeline worker is an extra internal thread: gate it on the
        // global budget too (with concurrent fills it stays off)
        let pipeline = pipeline && take_permits(1) == 1;
        // PERSISTENT plane worker (one thread per gen, parked between
        // planes): the spawn-per-plane scheme paid a scheduler round-trip
        // per plane, which under Paper's saturated 2-core runner dominated
        // the measured window (judge nat_p50 5-7.3ms vs 1.8ms isolated).
        if pipeline && self.cell_count_xz > 0 && !self.skip_slice && self.plane_worker.is_none() {
            let env_addr = self.env as *const Env as usize;
            self.plane_worker = Some(PlaneWorker::spawn(
                env_addr,
                std::sync::Arc::clone(&self.compiled),
                self.interp_children.clone(),
                self.cell_count_xz,
                self.cell_count_y,
                self.cell_noise_min_y,
                cw,
                ch,
                lanes,
                plane_len,
            ));
        }
        let pw = self.plane_worker.as_mut();
        if pipeline && self.cell_count_xz > 0 && !self.skip_slice {
            if let Some(w) = pw {
                w.request((first_cell_x + 1) * cw, first_cell_z, plane_len);
            }
        }
        // a pending spawn (kept for the no-worker fallback path)
        let mut pending: Option<std::thread::JoinHandle<Vec<f64>>> = None;
        for i2 in 0..self.cell_count_xz {
            // advanceCellX(i2): fill slice1 at x-plane first_cell_x+i2+1
            let t_fs = if prof { Some(std::time::Instant::now()) } else { None };
            let t0 = std::time::Instant::now();
            // persistent-worker path: receive the plane requested before the
            // loop (or at the previous iteration), scatter it, then reuse the
            // buffer for the next request (double buffering).
            let mut scattered: Option<Vec<f64>> = None;
            if pipeline && self.cell_count_xz > 0 && !self.skip_slice {
                if let Some(mut w) = self.plane_worker.take() {
                    self.plane_worker = None; // avoid Drop; reinstalled below
                    match w.take_done() {
                        Some(buf) => {
                            let cols = self.cell_count_y + 1;
                            for slot in 0..self.interp_children.len() {
                                let slot_id = slot as u16;
                                for i in 0..=self.cell_count_xz {
                                    for y in 0..=self.cell_count_y {
                                        let v = buf[slot
                                            * (self.cell_count_xz + 1)
                                            * cols
                                            + i * cols
                                            + y];
                                        self.interp.slice1_set(slot_id, i, y, v);
                                    }
                                }
                            }
                            scattered = Some(buf);
                        }
                        None => {
                            // worker died: fall back to inline evaluation
                            self.fill_slice(
                                first_cell_x + i2 as i32 + 1,
                                first_cell_z,
                                false,
                            );
                        }
                    }
                    if i2 + 2 <= self.cell_count_xz {
                        if let Some(buf) = scattered.take() {
                            w.spare = Some(buf);
                            w.request(
                                (first_cell_x + i2 as i32 + 2) * cw,
                                first_cell_z,
                                plane_len,
                            );
                        }
                    } else {
                        // keep the last buffer as the next fill's spare
                        w.spare = scattered.take();
                    }
                    self.plane_worker = Some(w);
                }
            } else if let Some(handle) = pending.take() {
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
        if pipeline {
            give_permits(1);
        }
        // save the final x-plane (slice0 after the last swap) for the next fill
        {
            let plane_x = ((first_cell_x + self.cell_count_xz as i32) * cw) as i64;
            self.interp.copy_slice0_into(&mut self.plane_cache);
            self.plane_cache_x = plane_x;
            self.plane_cache_z = first_cell_z;
        }
        if self.dbg_plane {
            eprintln!("[fill-end] ({},{}) plane_hits_now={} cache_x={} want_next={}", cx, cz, p.plane_hits, self.plane_cache_x, ((first_cell_x + self.cell_count_xz as i32) * cw) as i64);
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

                    // Cell verdict gate: interval-evaluate the compiled cellfill
                    // program over the cell's corner ranges. AllNeg / AllPos
                    // prove every in-cell substance has that sign (every op's
                    // interval is a sound superset; Lerp3 of corners stays in
                    // [min, max] since lerp weights are in [0,1)).
                    //
                    // What is safe to skip is dictated by where the exact
                    // substance VALUE is read:
                    //  - substance > 0.0 gates (aquifer early return, ore
                    //    eligibility): sign only.
                    //  - aquifer PRESSURE BARRIER (substance + d1 > 0.0 ->
                    //    None -> stone): exact value, but only reachable when
                    //    the 12-cell neighborhood statuses are MIXED and the
                    //    nearest-pair similarity is positive.
                    // Therefore:
                    //  - AllPos: poison +1.0 is bit-exact everywhere (the
                    //    pressure path requires substance <= 0), and when the
                    //    whole cell is outside the ore y-range the material
                    //    collapses to STONE outright.
                    //  - AllNeg: exact values are required unless the block
                    //    result provably never consults them:
                    //      * whole cell below the lava line: global-fluid
                    //        early return (LAVA, sched=false) fires before any
                    //        scan -> direct LAVA;
                    //      * whole cell at y >= 64 AND every base aquifer
                    //        cell summary in the covered 2x2x2 neighborhood is
                    //        cached-uniform: every block early-returns with
                    //        status.at(y) = AIR (no fluid level reaches 64),
                    //        sched=false -> direct AIR;
                    //      * otherwise: full cellfill (exact values) + normal
                    //        material walk.
                    // Gated on an empty beardifier (a beard contribution could
                    // flip the sign per block) and on the material/cellfill
                    // probe switches (test-only).
                    let verdict = if self.beard.is_empty()
                        && !self.skip_material
                        && !self.skip_cellfill
                    {
                        match self.cellfill_prog.as_deref() {
                            Some(prog) => {
                                match crate::density::cell_interval(
                                    prog,
                                    &self.interp,
                                    cell_start_block_y,
                                ) {
                                    Some((lo, hi)) if hi < 0.0 => 1i8, // AllNeg
                                    Some((lo, hi)) if lo > 0.0 => 2,  // AllPos
                                    _ => 0,
                                }
                            }
                            None => 0,
                        }
                    } else {
                        0
                    };
                    let lava_line = self.sea_level.min(-54);
                    let ore_free = !self.ore_veins_enabled
                        || self.no_ore
                        || cell_start_block_y > 50
                        || cell_start_block_y + (self.cell_height as i32 - 1) < -60;
                    let y_top = cell_start_block_y + (self.cell_height as i32 - 1);
                    // All base aquifer cells covered by this 4x8x4 material
                    // cell (gx/gz span at most 2, gy at most 2) must have a
                    // cached all-same summary for the AIR direct write.
                    let air_zone_safe = cell_start_block_y >= 64 && {
                        let mut ok = true;
                        'scan: for gx in [
                            floor_div(cell_start_block_x - 5, 16),
                            floor_div(cell_start_block_x + self.cell_width as i32 - 1 - 5, 16),
                        ] {
                            for gy in [
                                floor_div(cell_start_block_y + 1, 12),
                                floor_div(y_top + 1, 12),
                            ] {
                                for gz in [
                                    floor_div(cell_start_block_z - 5, 16),
                                    floor_div(cell_start_block_z + self.cell_width as i32 - 1 - 5, 16),
                                ] {
                                    let base = self.aquifer.index(gx, gy, gz);
                                    let sv = self.aquifer.summary[base].0.load(Ordering::Relaxed);
                                    if sv == 0 {
                                        ok = false;
                                        break 'scan;
                                    }
                                    let (same, _) = unpack_summary(sv);
                                    if !same {
                                        ok = false;
                                        break 'scan;
                                    }
                                }
                            }
                        }
                        ok
                    };
                    let direct: Option<u16> = match verdict {
                        1 if y_top < lava_line => Some(LAVA),
                        1 if air_zone_safe => Some(AIR),
                        2 if ore_free => Some(STONE),
                        _ => None,
                    };
                    // AllNeg cells that fall back keep their exact values:
                    // only AllPos cells take the poison fill (the +1.0 value
                    // is never read beyond the subst>0 gates).
                    let poison = verdict == 2;
                    if let Some(b) = direct {
                        // constant cell: write it out, no cellfill, no walk
                        if b != AIR {
                            for iy2 in 0..self.cell_height as i32 {
                                let row_base = (((cell_start_block_y + iy2 - self.min_y) * 256
                                    + (cell_start_block_z - min_block_z) * 16
                                    + (cell_start_block_x - min_block_x)))
                                    as usize;
                                for iz2 in 0..self.cell_width as usize {
                                    let rr = row_base + iz2 * 16;
                                    for ix2 in 0..self.cell_width as usize {
                                        out[rr + ix2] = b;
                                    }
                                }
                            }
                        }
                        if self.prof.enabled {
                            let cell_blocks =
                                (self.cell_width * self.cell_width * self.cell_height) as u64;
                            if verdict == 1 {
                                self.prof.blocks_subst_neg += cell_blocks;
                            } else {
                                self.prof.blocks_subst_pos += cell_blocks;
                            }
                        }
                        continue;
                    }

                    // All-solid cell gate: if every final_density corner is
                    // > 0, every trilinearly interpolated in-cell substance
                    // is > 0 too (convex combination, IEEE lerp of positives
                    // stays positive for world-range magnitudes). Vanilla then
                    // returns ore/stone for all 128 blocks without ever using
                    // the interpolated values (aquifer early-returns on
                    // subst > 0; ore reads vein noises only), so the entire
                    // cellfill pass for the cell can be skipped. Gated on an
                    // empty beardifier (a beard contribution could push
                    // substance <= 0).
                    let fds = match self.final_density_slot { Some(s) => s, None => 0 };

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
                        // try the 2-row path: iy (row A) + iy-1 (row B)
                        if !skipping
                            && !poison
                            && self.cellfill_prog.is_some()
                            && cw_i == 4
                            && iy >= 1
                            && cellfill_simd::avx2_ok()
                                                    {
                            // cell_cache layout is [iy][ix][iz]: row A (iy) must
                            // be fully written before row B (iy-1) — buffer each
                            // row separately, then place both blocks in order.
                            let prog = self.cellfill_prog.as_ref().unwrap();
                            let prog_flat = self.cellfill_flat.as_ref();
                            let dy_a = iy as f64 * dy_step;
                            let iy_b = iy - 1;
                            let dy_b = iy_b as f64 * dy_step;
                            let block_y_a = cell_start_block_y + iy;
                            let block_y_b = cell_start_block_y + iy_b;
                            let idx_base = idx;


                            // Vectorized rows hoisting: cell corners are fixed
                            // per (iy-pair, iz-cell) while dx takes the four
                            // fractions [0, .25, .5, .75] — exactly the dz lane
                            // layout. Per slot: 8 vector ops produce the four
                            // rows8 planes over all four ix lanes at once
                            // (rows8 layout [slot][ix][k], k = r1a/r2a/r1b/r2b).
                            unsafe {
                                cellfill_simd::hoist_rows_2row(
                                    &self.interp,
                                    &self.cellfill_slots,
                                    &mut self.cellfill_rows8,
                                    dy_a,
                                    dy_b,
                                    dx_step,
                                );
                            }
                            // Stacks created ONCE per iy-pair (not per run):
                            // the flat program is topologically ordered, so
                            // every register read is preceded by a write in
                            // the same run — the per-run re-zeroing of
                            // 32x__m256d was pure waste.
                            let dzv = cellfill_simd::dz_vec(dx_step);
                            let (mut vstack_a, mut vstack_b, mut vsp) =
                                cellfill_simd::fresh_stacks();
                            // fused 8-lane path (AVX-512, calibrated): the
                            // program runs ONCE per ix with row A in lanes
                            // 0..3 and row B in lanes 4..7.
                            if self.cellfill_wide8 && cellfill_simd::avx512_ok() {
                                if let Some(flat) = prog_flat {
                                    let dzv8 = unsafe {
                                        _mm512_setr_pd(0.0, dx_step, dx_step + dx_step, dx_step + dx_step + dx_step,
                                                       0.0, dx_step, dx_step + dx_step, dx_step + dx_step + dx_step)
                                    };
                                    let mut reg8 = cellfill_simd::fresh_stack512();
                                    let mut ix = 0i32;
                                    while ix < cw_i {
                                        let res = unsafe {
                                            cellfill_simd::run_cellfill_2rows_wide512(
                                                flat,
                                                &self.cellfill_rows8,
                                                block_y_a,
                                                block_y_b,
                                                dzv8,
                                                &ix,
                                                &mut reg8,
                                            )
                                        };
                                        unsafe {
                                            let lo = _mm512_castpd512_pd256(res);
                                            let hi = _mm512_extractf64x4_pd(res, 1);
                                            _mm256_storeu_pd(
                                                self.cell_cache.as_mut_ptr().add(idx_base + (ix * 4) as usize),
                                                lo,
                                            );
                                            _mm256_storeu_pd(
                                                self.cell_cache.as_mut_ptr().add(idx_base + 16 + (ix * 4) as usize),
                                                hi,
                                            );
                                        }
                                        ix += 1;
                                    }
                                    idx = idx_base + 32;
                                    iy -= 2;
                                    continue;
                                }
                            }
                            let mut ix = 0i32;
                            while ix < cw_i {
                                let (res_a, res_b) = if let Some(flat) = prog_flat {
                                    unsafe {
                                        cellfill_simd::run_cellfill_2rows_flat(
                                            flat,
                                            &self.cellfill_rows8,
                                            block_y_a,
                                            block_y_b,
                                            dzv,
                                            &ix,
                                            &mut vstack_a,
                                            &mut vstack_b,
                                        )
                                    }
                                } else {
                                    unsafe {
                                        cellfill_simd::run_cellfill_2rows(
                                            prog,
                                            &self.cellfill_rows8,
                                            block_y_a,
                                            block_y_b,
                                            dzv,
                                            &ix,
                                            &mut vstack_a,
                                            &mut vstack_b,
                                            &mut vsp,
                                        )
                                    }
                                };
                                // store straight into cell_cache (row A then
                                // row B); the [0f64;16] stack buffers + memcpy
                                // were redundant — LLVM cannot elide the
                                // copy through the static-slice indices.
                                unsafe {
                                    _mm256_storeu_pd(
                                        self.cell_cache.as_mut_ptr().add(idx_base + (ix * 4) as usize),
                                        res_a,
                                    );
                                    _mm256_storeu_pd(
                                        self.cell_cache.as_mut_ptr().add(idx_base + 16 + (ix * 4) as usize),
                                        res_b,
                                    );
                                }
                                ix += 1;
                            }
                            idx += 32;
                            iy -= 2;
                            continue;
                        }
                        let dy = iy as f64 * dy_step;
                        let mut ix = 0i32;
                        while ix < cw_i {
                            let dx = ix as f64 * dx_step;
                            // verdict != 0 (AllPos only): poison the cell_cache
                            // with a sign-representative value. The exact value
                            // is never read: the aquifer early-returns on
                            // substance > 0.0 before the pressure barrier (the
                            // only value-sensitive read), and the ore path is
                            // position-driven. AllNeg cells never poison —
                            // they fall back to the exact cellfill.
                            if skipping || poison {
                                let fill_v = if skipping {
                                    0.0f64
                                } else {
                                    1.0f64
                                };
                                for _ in 0..cw_i {
                                    self.cell_cache[idx] = fill_v;
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
                                let prog_flat = self.cellfill_flat.as_ref();
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
                                    let res = if let Some(flat) = prog_flat {
                                        unsafe {
                                            cellfill_simd::run_cellfill_row4_flat(
                                                flat,
                                                &self.cellfill_rows,
                                                dzv,
                                                block_y,
                                                &mut vstack,
                                            )
                                        }
                                    } else {
                                        unsafe {
                                            cellfill_simd::run_cellfill_row4(
                                                prog,
                                                &self.cellfill_rows,
                                                dzv,
                                                block_y,
                                                &mut vstack,
                                                &mut vsp,
                                            )
                                        }
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
                    // TEMP DEBUG: verify interval soundness against the real
                    // cellfill values (verdict==0 cells only).
                    if std::env::var("CRUS_GATE_DEBUG").is_ok() && verdict == 0 {
                        if let Some(prog) = self.cellfill_prog.as_deref() {
                            if let Some((ilo, ihi)) = crate::density::cell_interval(
                                prog,
                                &self.interp,
                                cell_start_block_y,
                            ) {
                                let n = (self.cell_width
                                    * self.cell_width
                                    * self.cell_height) as usize;
                                let mut rmin = f64::INFINITY;
                                let mut rmax = f64::NEG_INFINITY;
                                for &v in &self.cell_cache[..n] {
                                    rmin = rmin.min(v);
                                    rmax = rmax.max(v);
                                }
                                if rmin < ilo || rmax > ihi {
                                    eprintln!(
                                        "INTERVAL VIOLATION cell(i2={i2},i3={i3},i5={i5}): real [{rmin:.6}, {rmax:.6}] interval [{ilo:.6}, {ihi:.6}]"
                                    );
                                }
                                if std::env::var("CRUS_GATE_DEBUG").as_deref() == Ok("2")
                                    && (40..48).contains(&cell_start_block_y)
                                {
                                    eprintln!(
                                        "cell(i2={i2},i3={i3},i5={i5}) by={cell_start_block_y}: real [{rmin:.6}, {rmax:.6}] interval [{ilo:.6}, {ihi:.6}]"
                                    );
                                }
                            }
                        }
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
        let mut memo = BatchMemo::<G>::new(compiled.arena.len()).with_cols(compiled.col_count as usize);
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
                // The batch slot0 eval does not touch self.memo; slots 1-7
                // read it — bump the context once per corner point exactly
                // like the pre-batch eval_fresh did, otherwise slots 1-7
                // resolve stale stamps from an older point (silently wrong
                // plane values -> wrong substances).
                self.memo.next_serial();
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
        if let Some(want) = self.dump_planes_want {
            if start_cell_x == want {
                let nz = self.cell_count_xz + 1;
                let ny = self.cell_count_y + 1;
                for y in 0..ny {
                    for z in 0..nz {
                        let v0 = self.interp.slice0_get_pub(0, z, y);
                        let v1 = self.interp.slice1_get_pub(0, z, y);
                        println!("PLANE x={} z={} y={} s0={:016x} s1={:016x}", start_cell_x, z, y, v0.to_bits(), v1.to_bits());
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
        // substances: all four must be <= 0 (ore path is per-block);
        // all four > 0 -> whole row takes the vanilla subst>0 early return
        // (aquifer None, sched=false) and ore runs per block below.
        // The four z-values are contiguous in cell_cache: one vector load.
        let cw = self.cell_width;
        let base_i = (((self.cell_height as i32 - 1 - iy) as usize) * cw + ix as usize) * cw;
        let (s0, s1, s2, s3) = if cw == 4 && cellfill_simd::avx2_ok() {
            unsafe {
                let v = _mm256_loadu_pd(self.cell_cache.as_ptr().add(base_i));
                let mut a = [0f64; 4];
                _mm256_storeu_pd(a.as_mut_ptr(), v);
                (a[0], a[1], a[2], a[3])
            }
        } else {
            (
                self.cell_cache[base_i],
                self.cell_cache[base_i + 1],
                self.cell_cache[base_i + 2],
                self.cell_cache[base_i + 3],
            )
        };
        if s0 > 0.0 && s1 > 0.0 && s2 > 0.0 && s3 > 0.0 {
            self.sched = false;
            // Both vein tuples' y-ranges (copper [0,50], iron [-60,-8]) fail
            // outside [-60, 50] regardless of toggle sign: vanilla's ore
            // rule resolves to None for EVERY block -> STONE fallback. The
            // whole row can skip toggle/rng/ridged sampling entirely.
            if !self.ore_veins_enabled
                || self.no_ore
                || block_y > 50
                || block_y < -60
            {
                for iz in 0..4i32 {
                    out[base_out + (iz * 16) as usize] = STONE;
                }
                return true;
            }
            // 4-wide vein_toggle batch: one AVX2 NormalNoise sample for the
            // whole row (bit-exact lane-identical arithmetic).
            let ds = self.ore_toggle_row4(block_x, block_y, block_z0);
            for iz in 0..4i32 {
                let block_z = block_z0 + iz;
                let block = match ds[iz as usize] {
                    Some(d) => self
                        .ore_finish(Pt { x: block_x, y: block_y, z: block_z }, d)
                        .unwrap_or(STONE),
                    None => self.ore_row_block(block_x, block_y, block_z),
                };
                if block != AIR {
                    let out_idx = base_out + (iz * 16) as usize;
                    out[out_idx] = block;
                }
            }
            return true;
        }
        if s0 > 0.0 || s1 > 0.0 || s2 > 0.0 || s3 > 0.0 {
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
        // gx/gy are constant across the z-row — hoisted out of the loop
        let gx = floor_div(block_x - 5, 16);
        let gy = floor_div(block_y + 1, 12);
        let mut s0: Option<FluidStatus> = None;
        let mut gz_prev = i32::MIN;
        for iz in 0..4i32 {
            let z = block_z0 + iz;
            let gz = floor_div(z - 5, 16);
            if gz != gz_prev {
                gz_prev = gz;
                let base = self.aquifer.index(gx, gy, gz);
                let sv = self.aquifer.summary[base].0.load(Ordering::Relaxed);
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

    /// Rule-2 ore path for an all-solid row (aquifer returned None for every
    /// block via the subst>0 early return).
    #[inline]
    fn ore_row_block(&mut self, x: i32, y: i32, z: i32) -> u16 {
        if self.ore_veins_enabled && !self.no_ore {
            if self.prof.enabled {
                self.prof.blocks_ore_reached += 1;
            }
            if let Some(b) = self.ore_vein(Pt { x, y, z }) {
                return b;
            }
        }
        STONE
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
        let _aq_guard = if crate::density::PROF_ON.load(std::sync::atomic::Ordering::Relaxed) {
            AQ_SUBSTANCE_CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Some(AqTimer { t0: std::time::Instant::now() })
        } else {
            None
        };
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
        let sv = self.aquifer.summary[base].0.load(Ordering::Relaxed);
        let hit_t0 = if crate::density::PROF_ON.load(std::sync::atomic::Ordering::Relaxed) {
            Some(std::time::Instant::now())
        } else { None };
        if sv != 0 {
            if hit_t0.is_some() {
                AQ_SUMMARY_HITS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
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
        if let Some(t) = hit_t0 {
            AQ_HIT_NS.fetch_add(t.elapsed().as_nanos() as u64, std::sync::atomic::Ordering::Relaxed);
            AQ_SUMMARY_MISS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
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
                        let l = self.aquifer.loc_cache[index].0.load(Ordering::Relaxed);
                        if l != i64::MAX {
                            l
                        } else {
                        let mut rng = self.env.aquifer_random.clone().at(gxx, gyy, gzz);
                        let p = block_pos_as_long(
                            gxx * 16 + rng.next_int_bound(10),
                            gyy * 12 + rng.next_int_bound(9),
                            gzz * 16 + rng.next_int_bound(10),
                        );
                            self.aquifer.loc_cache[index].0.store(p, Ordering::Relaxed);
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
                            let packed_v = self.aquifer.cache[index].0.load(Ordering::Relaxed);
                            if packed_v != 0 {
                                unpack_status(packed_v)
                            } else {
                                let v = self.compute_aquifer_fluid(
                                    pos_x(packed),
                                    pos_y(packed),
                                    pos_z(packed),
                                );
                                self.aquifer.cache[index]
                                    .0
                                    .store(pack_status(v), Ordering::Relaxed);
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
        if let Some(t) = hit_t0 {
            AQ_SCAN_NS.fetch_add(t.elapsed().as_nanos() as u64, std::sync::atomic::Ordering::Relaxed);
        }
        // Memoize the base-cell verdict (pure function of (gx, gy, gz)).
        self.aquifer.summary[base]
            .0
            .store(pack_summary(all_same, first), Ordering::Relaxed);
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
                        let l = self.aquifer.loc_cache[index].0.load(Ordering::Relaxed);
                        if l != i64::MAX {
                            l
                        } else {
                        let mut rng = self.env.aquifer_random.clone().at(gxx, gyy, gzz);
                        let p = block_pos_as_long(
                            gxx * 16 + rng.next_int_bound(10),
                            gyy * 12 + rng.next_int_bound(9),
                            gzz * 16 + rng.next_int_bound(10),
                        );
                            self.aquifer.loc_cache[index].0.store(p, Ordering::Relaxed);
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
        let packed_v = self.aquifer.cache[index].0.load(Ordering::Relaxed);
        if packed_v != 0 {
            return unpack_status(packed_v);
        }
        let v = self.compute_aquifer_fluid(x, y, z);
        self.aquifer.cache[index]
            .0
            .store(pack_status(v), Ordering::Relaxed);
        v
    }

    /// Vanilla NoiseBasedAquifer.computeFluid.
    fn compute_aquifer_fluid(&mut self, x: i32, y: i32, z: i32) -> FluidStatus {
        if crate::density::PROF_ON.load(std::sync::atomic::Ordering::Relaxed) {
            AQ_FLUID_CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
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
        self.ore_finish(pt, d)
    }

    /// Continuation of ore_vein after the toggle sample (shared by the
    /// scalar path and the 4-wide batch toggle path in material_row4).
    fn ore_finish(&self, pt: Pt, d: f64) -> Option<u16> {
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
        // Fresh serial: the batch toggle path bypasses memo entirely, so
        // this evaluation context must not inherit the previous block's
        // stamps (scalar path re-bumps too — harmless serial bump).
        self.memo.next_serial();
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

    /// 4-wide vein_toggle batch for an all-solid row (x, y fixed, z0..z0+4).
    /// Bit-exact: mirrors the WKind::Noise node exactly (coordinate scales,
    /// then NormalNoise::get_value_batch — lane-identical arithmetic to the
    /// scalar eval). None entries (fast path unavailable) fall back to the
    /// per-block scalar eval in the caller.
    #[inline]
    fn ore_toggle_row4(&self, x: i32, y: i32, z0: i32) -> [Option<f64>; 4] {
        let mut out = [None; 4];
        if let Some(f) = &self.ore_toggle_fast {
            let mut xi = [0f64; 4];
            let mut yi = [0f64; 4];
            let mut zi = [0f64; 4];
            for l in 0..4 {
                xi[l] = x as f64 * f.xz_scale;
                yi[l] = y as f64 * f.y_scale;
                zi[l] = (z0 + l as i32) as f64 * f.xz_scale;
            }
            let n = &self.env.noise.list[f.noise_idx];
            let v = n.get_value_batch(&xi, &yi, &zi);
            for l in 0..4 {
                out[l] = Some(v[l]);
            }
        }
        out
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
    pub fn eval_plane_into_pub(
        env: &Env,
        compiled: &Compiled,
        children: &[u32],
        corner_x: i32,
        first_cell_z: i32,
        ccx: usize,
        ccy: usize,
        cnmy: i32,
        cw: i32,
        ch: i32,
        lanes: usize,
        out: &mut [f64],
    ) {
        Self::eval_plane_into(env, compiled, children, corner_x, first_cell_z, ccx, ccy, cnmy, cw, ch, lanes, out)
    }

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
        let memo = BatchMemo::<G>::new(compiled.arena.len()).with_cols(compiled.col_count as usize);
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
    use crate::density::{FlatOp, FlatProg};
    use std::arch::x86_64::*;
    use std::sync::OnceLock;

    pub fn avx2_ok() -> bool {
        static OK: OnceLock<bool> = OnceLock::new();
        *OK.get_or_init(|| std::arch::is_x86_feature_detected!("avx2"))
    }

    pub fn avx512_ok() -> bool {
        static OK: OnceLock<bool> = OnceLock::new();
        *OK.get_or_init(|| std::arch::is_x86_feature_detected!("avx512f"))
    }

    /// One-time micro-calibration: run the actual CellFill flat program in
    /// the fused 8-lane AVX-512 2-row runner vs the 4-lane AVX2 one and pick
    /// the winner for this CPU. Bit-neutral (both are lane-identical to the
    /// scalar eval); costs a few ms once per process.
    pub fn pick_cellfill_wide(prog: &FlatProg) -> bool {
        if !avx512_ok() || !avx2_ok() {
            return false;
        }
        match std::env::var("CRUS_CELLFILL_WIDE").ok().as_deref() {
            Some("0") => return false,
            Some("1") => return true,
            _ => {}
        }
        // synthetic rows: any values work — the timing is what matters.
        let rows8 = vec![0.375f64; 8 * 16];
        let reps = 2000usize;
        let time4 = |sink: &mut u64| -> f64 {
            let (mut reg_a, mut reg_b, _) = fresh_stacks();
            let dzv = unsafe { _mm256_setr_pd(0.0, 0.25, 0.5, 0.75) };
            let t0 = std::time::Instant::now();
            for i in 0..reps {
                let ix = (i & 3) as i32;
                let (ra, rb) = unsafe {
                    run_cellfill_2rows_flat(prog, &rows8, i as i32, i as i32 + 1, dzv, &ix, &mut reg_a, &mut reg_b)
                };
                let mut t4 = [0f64; 4];
                unsafe { _mm256_storeu_pd(t4.as_mut_ptr(), ra) };
                *sink ^= t4[0].to_bits();
                unsafe { _mm256_storeu_pd(t4.as_mut_ptr(), rb) };
                *sink ^= t4[0].to_bits();
            }
            t0.elapsed().as_nanos() as f64
        };
        let time8 = |sink: &mut u64| -> f64 {
            let mut reg = unsafe { [_mm512_setzero_pd(); 16] };
            let dzv8 = unsafe { _mm512_setr_pd(0.0, 0.25, 0.5, 0.75, 0.0, 0.25, 0.5, 0.75) };
            let t0 = std::time::Instant::now();
            for i in 0..reps {
                let ix = (i & 3) as i32;
                let r = unsafe {
                    run_cellfill_2rows_wide512(prog, &rows8, i as i32, i as i32 + 1, dzv8, &ix, &mut reg)
                };
                let mut tmp8 = [0f64; 8];
                unsafe { _mm512_storeu_pd(tmp8.as_mut_ptr(), r) };
                *sink ^= tmp8[0].to_bits();
            }
            t0.elapsed().as_nanos() as f64
        };
        let mut s1 = 0u64;
        let mut s2 = 0u64;
        let _ = time4(&mut s1);
        let _ = time8(&mut s2);
        let d4 = time4(&mut s1);
        let d8 = time8(&mut s2);
        let pick = d8 < d4 * 0.97;
        if std::env::var("CRUS_DEBUG_LANES").is_ok() {
            eprintln!("[crussty] cellfill width calibration: 4={d4:.0}ns 8={d8:.0}ns -> wide8={pick}");
        }
        pick
    }

    /// Fused 8-lane 2-row runner: lanes 0..3 = row A z-blocks, lanes 4..7 =
    /// row B z-blocks. The flat program executes ONCE (the 4-lane runner
    /// executes it twice — once per row). Lane-identical arithmetic keeps
    /// every lane bit-identical to the scalar path.
    #[target_feature(enable = "avx512f")]
    pub unsafe fn run_cellfill_2rows_wide512(
        prog: &FlatProg,
        rows8: &[f64],
        block_y_a: i32,
        block_y_b: i32,
        dzv8: __m512d,
        ixp: &i32,
        reg: &mut [__m512d; 16],
    ) -> __m512d {
        let zero = _mm512_setzero_pd();
        let sign_abs = _mm512_set1_pd(-0.0);
        for op in &prog.ops {
            match *op {
                FlatOp::Const { dst, v } => {
                    *reg.get_unchecked_mut(dst as usize) = _mm512_set1_pd(v);
                }
                FlatOp::Lerp3 { dst, slot } => {
                    // rows8[slot]: [r1a(4 ix lanes) | r2a(4) | r1b(4) | r2b(4)].
                    // For this ix the row values are SCALARS broadcast to the
                    // 4 z-lanes of each row half: lanes 0..3 = row A (r1a/r2a
                    // at index slot*16+ix), lanes 4..7 = row B (+8).
                    let base = slot as usize * 16 + *ixp as usize;
                    let r1 = _mm512_insertf64x4(
                        _mm512_castpd256_pd512(_mm256_set1_pd(*rows8.get_unchecked(base))),
                        _mm256_set1_pd(*rows8.get_unchecked(base + 8)),
                        1,
                    );
                    let r2 = _mm512_insertf64x4(
                        _mm512_castpd256_pd512(_mm256_set1_pd(*rows8.get_unchecked(base + 4))),
                        _mm256_set1_pd(*rows8.get_unchecked(base + 12)),
                        1,
                    );
                    *reg.get_unchecked_mut(dst as usize) =
                        _mm512_add_pd(r1, _mm512_mul_pd(dzv8, _mm512_sub_pd(r2, r1)));
                }
                FlatOp::YGrad { dst, from_y, to_y, from_value, to_value } => {
                    let grad = |by: i32| -> f64 {
                        let t = (by as f64 - from_y as f64) / (to_y as f64 - from_y as f64);
                        if t < 0.0 {
                            from_value
                        } else if t > 1.0 {
                            to_value
                        } else {
                            from_value + t * (to_value - from_value)
                        }
                    };
                    *reg.get_unchecked_mut(dst as usize) = _mm512_setr_pd(
                        grad(block_y_a), grad(block_y_a), grad(block_y_a), grad(block_y_a),
                        grad(block_y_b), grad(block_y_b), grad(block_y_b), grad(block_y_b),
                    );
                }
                FlatOp::Add { dst, a, b } => {
                    let av = *reg.get_unchecked(a as usize);
                    let bv = *reg.get_unchecked(b as usize);
                    *reg.get_unchecked_mut(dst as usize) = _mm512_add_pd(av, bv);
                }
                FlatOp::Mul { dst, a, b } => {
                    let av = *reg.get_unchecked(a as usize);
                    let bv = *reg.get_unchecked(b as usize);
                    *reg.get_unchecked_mut(dst as usize) =
                        _mm512_maskz_mov_pd(_mm512_cmpneq_pd_mask(av, zero), _mm512_mul_pd(av, bv));
                }
                FlatOp::Min { dst, a, b } => {
                    let av = *reg.get_unchecked(a as usize);
                    let bv = *reg.get_unchecked(b as usize);
                    *reg.get_unchecked_mut(dst as usize) =
                        _mm512_mask_mov_pd(bv, _mm512_cmplt_pd_mask(av, bv), av);
                }
                FlatOp::Max { dst, a, b } => {
                    let av = *reg.get_unchecked(a as usize);
                    let bv = *reg.get_unchecked(b as usize);
                    *reg.get_unchecked_mut(dst as usize) =
                        _mm512_mask_mov_pd(bv, _mm512_cmplt_pd_mask(bv, av), av);
                }
                FlatOp::Abs { dst, a } => {
                    let av = *reg.get_unchecked(a as usize);
                    *reg.get_unchecked_mut(dst as usize) = _mm512_andnot_pd(sign_abs, av);
                }
                FlatOp::Square { dst, a } => {
                    let av = *reg.get_unchecked(a as usize);
                    *reg.get_unchecked_mut(dst as usize) = _mm512_mul_pd(av, av);
                }
                FlatOp::Cube { dst, a } => {
                    let av = *reg.get_unchecked(a as usize);
                    let a2 = _mm512_mul_pd(av, av);
                    *reg.get_unchecked_mut(dst as usize) = _mm512_mul_pd(a2, av);
                }
                FlatOp::HalfNeg { dst, a } => {
                    let av = *reg.get_unchecked(a as usize);
                    *reg.get_unchecked_mut(dst as usize) = _mm512_mask_mov_pd(
                        _mm512_mul_pd(av, _mm512_set1_pd(0.5)),
                        _mm512_cmplt_pd_mask(zero, av),
                        av,
                    );
                }
                FlatOp::QuarterNeg { dst, a } => {
                    let av = *reg.get_unchecked(a as usize);
                    *reg.get_unchecked_mut(dst as usize) = _mm512_mask_mov_pd(
                        _mm512_mul_pd(av, _mm512_set1_pd(0.25)),
                        _mm512_cmplt_pd_mask(zero, av),
                        av,
                    );
                }
                FlatOp::Squeeze { dst, a } => {
                    let av = *reg.get_unchecked(a as usize);
                    let d = _mm512_min_pd(_mm512_max_pd(av, _mm512_set1_pd(-1.0)), _mm512_set1_pd(1.0));
                    let d3 = _mm512_mul_pd(_mm512_mul_pd(d, d), d);
                    *reg.get_unchecked_mut(dst as usize) = _mm512_sub_pd(
                        _mm512_div_pd(d, _mm512_set1_pd(2.0)),
                        _mm512_div_pd(d3, _mm512_set1_pd(24.0)),
                    );
                }
                FlatOp::Clamp { dst, a, min, max } => {
                    let av = *reg.get_unchecked(a as usize);
                    *reg.get_unchecked_mut(dst as usize) = _mm512_min_pd(
                        _mm512_max_pd(av, _mm512_set1_pd(min)),
                        _mm512_set1_pd(max),
                    );
                }
                FlatOp::RangeChoice { dst, a, min_inclusive, max_exclusive } => {
                    let av = *reg.get_unchecked(a as usize);
                    let inr_v = *reg.get_unchecked(a as usize + 1);
                    let out_v = *reg.get_unchecked(a as usize + 2);
                    let inr = _mm512_cmple_pd_mask(_mm512_set1_pd(min_inclusive), av)
                        & _mm512_cmplt_pd_mask(av, _mm512_set1_pd(max_exclusive));
                    *reg.get_unchecked_mut(dst as usize) =
                        _mm512_mask_mov_pd(out_v, inr, inr_v);
                }
                FlatOp::MulOrAdd { dst, a, is_add, argument } => {
                    let av = *reg.get_unchecked(a as usize);
                    *reg.get_unchecked_mut(dst as usize) = if is_add {
                        _mm512_add_pd(av, _mm512_set1_pd(argument))
                    } else if argument == 0.0 {
                        zero
                    } else {
                        _mm512_mul_pd(av, _mm512_set1_pd(argument))
                    };
                }
            }
        }
        *reg.get_unchecked(prog.result as usize)
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

    /// Flat-form single-row runner: static stack slots (FlatOp), no
    /// stack-pointer dependency chain. Every op performs the exact same
    /// per-lane IEEE sequence as the stack-machine runner — bit-identical.
    /// Slot indices are < 16 by construction (flatten_cellfill rejects
    /// deeper programs), so unchecked indexing is sound.
    #[target_feature(enable = "avx2")]
    pub unsafe fn run_cellfill_row4_flat(
        prog: &FlatProg,
        rows: &[f64],
        dzv: __m256d,
        block_y: i32,
        reg: &mut [__m256d; 16],
    ) -> __m256d {
        let zero = _mm256_setzero_pd();
        let sign_abs = _mm256_set1_pd(-0.0);
        for op in &prog.ops {
            match *op {
                FlatOp::Const { dst, v } => {
                    *reg.get_unchecked_mut(dst as usize) = _mm256_set1_pd(v);
                }
                FlatOp::Lerp3 { dst, slot } => {
                    let r1 = _mm256_set1_pd(*rows.get_unchecked(slot as usize * 2));
                    let r2 = _mm256_set1_pd(*rows.get_unchecked(slot as usize * 2 + 1));
                    *reg.get_unchecked_mut(dst as usize) =
                        _mm256_add_pd(r1, _mm256_mul_pd(dzv, _mm256_sub_pd(r2, r1)));
                }
                FlatOp::YGrad { dst, from_y, to_y, from_value, to_value } => {
                    let t = (block_y as f64 - from_y as f64) / (to_y as f64 - from_y as f64);
                    let r = if t < 0.0 {
                        from_value
                    } else if t > 1.0 {
                        to_value
                    } else {
                        from_value + t * (to_value - from_value)
                    };
                    *reg.get_unchecked_mut(dst as usize) = _mm256_set1_pd(r);
                }
                FlatOp::Add { dst, a, b } => {
                    let av = *reg.get_unchecked(a as usize);
                    let bv = *reg.get_unchecked(b as usize);
                    *reg.get_unchecked_mut(dst as usize) = _mm256_add_pd(av, bv);
                }
                FlatOp::Mul { dst, a, b } => {
                    let av = *reg.get_unchecked(a as usize);
                    let bv = *reg.get_unchecked(b as usize);
                    // vanilla short-circuit: a == 0.0 ? 0.0 : a * b
                    let mask = _mm256_cmp_pd(av, zero, _CMP_EQ_OQ);
                    let prod = _mm256_mul_pd(av, bv);
                    *reg.get_unchecked_mut(dst as usize) = _mm256_blendv_pd(prod, zero, mask);
                }
                FlatOp::Min { dst, a, b } => {
                    let av = *reg.get_unchecked(a as usize);
                    let bv = *reg.get_unchecked(b as usize);
                    let mask = _mm256_cmp_pd(av, bv, _CMP_LT_OQ);
                    *reg.get_unchecked_mut(dst as usize) = _mm256_blendv_pd(bv, av, mask);
                }
                FlatOp::Max { dst, a, b } => {
                    let av = *reg.get_unchecked(a as usize);
                    let bv = *reg.get_unchecked(b as usize);
                    let mask = _mm256_cmp_pd(av, bv, _CMP_GT_OQ);
                    *reg.get_unchecked_mut(dst as usize) = _mm256_blendv_pd(bv, av, mask);
                }
                FlatOp::Abs { dst, a } => {
                    let av = *reg.get_unchecked(a as usize);
                    *reg.get_unchecked_mut(dst as usize) = _mm256_andnot_pd(sign_abs, av);
                }
                FlatOp::Square { dst, a } => {
                    let av = *reg.get_unchecked(a as usize);
                    *reg.get_unchecked_mut(dst as usize) = _mm256_mul_pd(av, av);
                }
                FlatOp::Cube { dst, a } => {
                    let av = *reg.get_unchecked(a as usize);
                    let a2 = _mm256_mul_pd(av, av);
                    *reg.get_unchecked_mut(dst as usize) = _mm256_mul_pd(a2, av);
                }
                FlatOp::HalfNeg { dst, a } => {
                    let av = *reg.get_unchecked(a as usize);
                    let mask = _mm256_cmp_pd(av, zero, _CMP_GT_OQ);
                    *reg.get_unchecked_mut(dst as usize) =
                        _mm256_blendv_pd(_mm256_mul_pd(av, _mm256_set1_pd(0.5)), av, mask);
                }
                FlatOp::QuarterNeg { dst, a } => {
                    let av = *reg.get_unchecked(a as usize);
                    let mask = _mm256_cmp_pd(av, zero, _CMP_GT_OQ);
                    *reg.get_unchecked_mut(dst as usize) =
                        _mm256_blendv_pd(_mm256_mul_pd(av, _mm256_set1_pd(0.25)), av, mask);
                }
                FlatOp::Squeeze { dst, a } => {
                    let av = *reg.get_unchecked(a as usize);
                    let d = _mm256_min_pd(
                        _mm256_max_pd(av, _mm256_set1_pd(-1.0)),
                        _mm256_set1_pd(1.0),
                    );
                    let d3 = _mm256_mul_pd(_mm256_mul_pd(d, d), d);
                    *reg.get_unchecked_mut(dst as usize) = _mm256_sub_pd(
                        _mm256_div_pd(d, _mm256_set1_pd(2.0)),
                        _mm256_div_pd(d3, _mm256_set1_pd(24.0)),
                    );
                }
                FlatOp::Clamp { dst, a, min, max } => {
                    let av = *reg.get_unchecked(a as usize);
                    *reg.get_unchecked_mut(dst as usize) = _mm256_min_pd(
                        _mm256_max_pd(av, _mm256_set1_pd(min)),
                        _mm256_set1_pd(max),
                    );
                }
                FlatOp::RangeChoice { dst, a, min_inclusive, max_exclusive } => {
                    let v = *reg.get_unchecked(a as usize);
                    let m1 = _mm256_cmp_pd(v, _mm256_set1_pd(min_inclusive), _CMP_GE_OQ);
                    let m2 = _mm256_cmp_pd(v, _mm256_set1_pd(max_exclusive), _CMP_LT_OQ);
                    // when_in / when_out slots: input slot + 1 / + 2
                    let inr = *reg.get_unchecked(a as usize + 1);
                    let out = *reg.get_unchecked(a as usize + 2);
                    *reg.get_unchecked_mut(dst as usize) =
                        _mm256_blendv_pd(out, inr, _mm256_and_pd(m1, m2));
                }
                FlatOp::MulOrAdd { dst, a, is_add, argument } => {
                    let av = *reg.get_unchecked(a as usize);
                    *reg.get_unchecked_mut(dst as usize) = if is_add {
                        _mm256_add_pd(av, _mm256_set1_pd(argument))
                    } else if argument == 0.0 {
                        zero
                    } else {
                        _mm256_mul_pd(av, _mm256_set1_pd(argument))
                    };
                }
            }
        }
        *reg.get_unchecked(prog.result as usize)
    }

    /// Flat-form two-row runner: two register files driven by one dispatch
    /// loop (A = block_y_a row, B = block_y_b row). Bit-identical to the
    /// stack-machine runner; static slots remove the sp dependency chain.
    #[target_feature(enable = "avx2")]
    pub unsafe fn run_cellfill_2rows_flat(
        prog: &FlatProg,
        rows8: &[f64],
        block_y_a: i32,
        block_y_b: i32,
        dzv: __m256d,
        ixp: &i32,
        reg_a: &mut [__m256d; 16],
        reg_b: &mut [__m256d; 16],
    ) -> (__m256d, __m256d) {
        let zero = _mm256_setzero_pd();
        let sign_abs = _mm256_set1_pd(-0.0);
        for op in &prog.ops {
            match *op {
                FlatOp::Const { dst, v } => {
                    let c = _mm256_set1_pd(v);
                    *reg_a.get_unchecked_mut(dst as usize) = c;
                    *reg_b.get_unchecked_mut(dst as usize) = c;
                }
                FlatOp::Lerp3 { dst, slot } => {
                    let base = slot as usize * 16 + *ixp as usize;
                    let r1a = _mm256_set1_pd(*rows8.get_unchecked(base));
                    let r2a = _mm256_set1_pd(*rows8.get_unchecked(base + 4));
                    let r1b = _mm256_set1_pd(*rows8.get_unchecked(base + 8));
                    let r2b = _mm256_set1_pd(*rows8.get_unchecked(base + 12));
                    *reg_a.get_unchecked_mut(dst as usize) =
                        _mm256_add_pd(r1a, _mm256_mul_pd(dzv, _mm256_sub_pd(r2a, r1a)));
                    *reg_b.get_unchecked_mut(dst as usize) =
                        _mm256_add_pd(r1b, _mm256_mul_pd(dzv, _mm256_sub_pd(r2b, r1b)));
                }
                FlatOp::YGrad { dst, from_y, to_y, from_value, to_value } => {
                    let grad = |by: i32| -> f64 {
                        let t = (by as f64 - from_y as f64) / (to_y as f64 - from_y as f64);
                        if t < 0.0 {
                            from_value
                        } else if t > 1.0 {
                            to_value
                        } else {
                            from_value + t * (to_value - from_value)
                        }
                    };
                    *reg_a.get_unchecked_mut(dst as usize) = _mm256_set1_pd(grad(block_y_a));
                    *reg_b.get_unchecked_mut(dst as usize) = _mm256_set1_pd(grad(block_y_b));
                }
                FlatOp::Add { dst, a, b } => {
                    let (ia, ib) = (a as usize, b as usize);
                    let aa = *reg_a.get_unchecked(ia);
                    let ba = *reg_a.get_unchecked(ib);
                    let ab = *reg_b.get_unchecked(ia);
                    let bb = *reg_b.get_unchecked(ib);
                    *reg_a.get_unchecked_mut(dst as usize) = _mm256_add_pd(aa, ba);
                    *reg_b.get_unchecked_mut(dst as usize) = _mm256_add_pd(ab, bb);
                }
                FlatOp::Mul { dst, a, b } => {
                    let (ia, ib) = (a as usize, b as usize);
                    let aa = *reg_a.get_unchecked(ia);
                    let ba = *reg_a.get_unchecked(ib);
                    let ab = *reg_b.get_unchecked(ia);
                    let bb = *reg_b.get_unchecked(ib);
                    let ma = _mm256_cmp_pd(aa, zero, _CMP_EQ_OQ);
                    let mb = _mm256_cmp_pd(ab, zero, _CMP_EQ_OQ);
                    *reg_a.get_unchecked_mut(dst as usize) =
                        _mm256_blendv_pd(_mm256_mul_pd(aa, ba), zero, ma);
                    *reg_b.get_unchecked_mut(dst as usize) =
                        _mm256_blendv_pd(_mm256_mul_pd(ab, bb), zero, mb);
                }
                FlatOp::Min { dst, a, b } => {
                    let (ia, ib) = (a as usize, b as usize);
                    let aa = *reg_a.get_unchecked(ia);
                    let ba = *reg_a.get_unchecked(ib);
                    let ab = *reg_b.get_unchecked(ia);
                    let bb = *reg_b.get_unchecked(ib);
                    *reg_a.get_unchecked_mut(dst as usize) =
                        _mm256_blendv_pd(ba, aa, _mm256_cmp_pd(aa, ba, _CMP_LT_OQ));
                    *reg_b.get_unchecked_mut(dst as usize) =
                        _mm256_blendv_pd(bb, ab, _mm256_cmp_pd(ab, bb, _CMP_LT_OQ));
                }
                FlatOp::Max { dst, a, b } => {
                    let (ia, ib) = (a as usize, b as usize);
                    let aa = *reg_a.get_unchecked(ia);
                    let ba = *reg_a.get_unchecked(ib);
                    let ab = *reg_b.get_unchecked(ia);
                    let bb = *reg_b.get_unchecked(ib);
                    *reg_a.get_unchecked_mut(dst as usize) =
                        _mm256_blendv_pd(ba, aa, _mm256_cmp_pd(aa, ba, _CMP_GT_OQ));
                    *reg_b.get_unchecked_mut(dst as usize) =
                        _mm256_blendv_pd(bb, ab, _mm256_cmp_pd(ab, bb, _CMP_GT_OQ));
                }
                FlatOp::Abs { dst, a } => {
                    let av = *reg_a.get_unchecked(a as usize);
                    let bv = *reg_b.get_unchecked(a as usize);
                    *reg_a.get_unchecked_mut(dst as usize) = _mm256_andnot_pd(sign_abs, av);
                    *reg_b.get_unchecked_mut(dst as usize) = _mm256_andnot_pd(sign_abs, bv);
                }
                FlatOp::Square { dst, a } => {
                    let av = *reg_a.get_unchecked(a as usize);
                    let bv = *reg_b.get_unchecked(a as usize);
                    *reg_a.get_unchecked_mut(dst as usize) = _mm256_mul_pd(av, av);
                    *reg_b.get_unchecked_mut(dst as usize) = _mm256_mul_pd(bv, bv);
                }
                FlatOp::Cube { dst, a } => {
                    let av = *reg_a.get_unchecked(a as usize);
                    let bv = *reg_b.get_unchecked(a as usize);
                    let a2 = _mm256_mul_pd(av, av);
                    let b2 = _mm256_mul_pd(bv, bv);
                    *reg_a.get_unchecked_mut(dst as usize) = _mm256_mul_pd(a2, av);
                    *reg_b.get_unchecked_mut(dst as usize) = _mm256_mul_pd(b2, bv);
                }
                FlatOp::HalfNeg { dst, a } => {
                    let av = *reg_a.get_unchecked(a as usize);
                    let bv = *reg_b.get_unchecked(a as usize);
                    let ma = _mm256_cmp_pd(av, zero, _CMP_GT_OQ);
                    let mb = _mm256_cmp_pd(bv, zero, _CMP_GT_OQ);
                    *reg_a.get_unchecked_mut(dst as usize) =
                        _mm256_blendv_pd(_mm256_mul_pd(av, _mm256_set1_pd(0.5)), av, ma);
                    *reg_b.get_unchecked_mut(dst as usize) =
                        _mm256_blendv_pd(_mm256_mul_pd(bv, _mm256_set1_pd(0.5)), bv, mb);
                }
                FlatOp::QuarterNeg { dst, a } => {
                    let av = *reg_a.get_unchecked(a as usize);
                    let bv = *reg_b.get_unchecked(a as usize);
                    let ma = _mm256_cmp_pd(av, zero, _CMP_GT_OQ);
                    let mb = _mm256_cmp_pd(bv, zero, _CMP_GT_OQ);
                    *reg_a.get_unchecked_mut(dst as usize) =
                        _mm256_blendv_pd(_mm256_mul_pd(av, _mm256_set1_pd(0.25)), av, ma);
                    *reg_b.get_unchecked_mut(dst as usize) =
                        _mm256_blendv_pd(_mm256_mul_pd(bv, _mm256_set1_pd(0.25)), bv, mb);
                }
                FlatOp::Squeeze { dst, a } => {
                    let av = *reg_a.get_unchecked(a as usize);
                    let bv = *reg_b.get_unchecked(a as usize);
                    let da = _mm256_min_pd(
                        _mm256_max_pd(av, _mm256_set1_pd(-1.0)),
                        _mm256_set1_pd(1.0),
                    );
                    let db = _mm256_min_pd(
                        _mm256_max_pd(bv, _mm256_set1_pd(-1.0)),
                        _mm256_set1_pd(1.0),
                    );
                    let d3a = _mm256_mul_pd(_mm256_mul_pd(da, da), da);
                    let d3b = _mm256_mul_pd(_mm256_mul_pd(db, db), db);
                    *reg_a.get_unchecked_mut(dst as usize) = _mm256_sub_pd(
                        _mm256_div_pd(da, _mm256_set1_pd(2.0)),
                        _mm256_div_pd(d3a, _mm256_set1_pd(24.0)),
                    );
                    *reg_b.get_unchecked_mut(dst as usize) = _mm256_sub_pd(
                        _mm256_div_pd(db, _mm256_set1_pd(2.0)),
                        _mm256_div_pd(d3b, _mm256_set1_pd(24.0)),
                    );
                }
                FlatOp::Clamp { dst, a, min, max } => {
                    let av = *reg_a.get_unchecked(a as usize);
                    let bv = *reg_b.get_unchecked(a as usize);
                    *reg_a.get_unchecked_mut(dst as usize) = _mm256_min_pd(
                        _mm256_max_pd(av, _mm256_set1_pd(min)),
                        _mm256_set1_pd(max),
                    );
                    *reg_b.get_unchecked_mut(dst as usize) = _mm256_min_pd(
                        _mm256_max_pd(bv, _mm256_set1_pd(min)),
                        _mm256_set1_pd(max),
                    );
                }
                FlatOp::RangeChoice { dst, a, min_inclusive, max_exclusive } => {
                    let va = *reg_a.get_unchecked(a as usize);
                    let vb = *reg_b.get_unchecked(a as usize);
                    let inra = *reg_a.get_unchecked(a as usize + 1);
                    let inrb = *reg_b.get_unchecked(a as usize + 1);
                    let outa = *reg_a.get_unchecked(a as usize + 2);
                    let outb = *reg_b.get_unchecked(a as usize + 2);
                    let m1a = _mm256_cmp_pd(va, _mm256_set1_pd(min_inclusive), _CMP_GE_OQ);
                    let m2a = _mm256_cmp_pd(va, _mm256_set1_pd(max_exclusive), _CMP_LT_OQ);
                    let m1b = _mm256_cmp_pd(vb, _mm256_set1_pd(min_inclusive), _CMP_GE_OQ);
                    let m2b = _mm256_cmp_pd(vb, _mm256_set1_pd(max_exclusive), _CMP_LT_OQ);
                    *reg_a.get_unchecked_mut(dst as usize) =
                        _mm256_blendv_pd(outa, inra, _mm256_and_pd(m1a, m2a));
                    *reg_b.get_unchecked_mut(dst as usize) =
                        _mm256_blendv_pd(outb, inrb, _mm256_and_pd(m1b, m2b));
                }
                FlatOp::MulOrAdd { dst, a, is_add, argument } => {
                    let av = *reg_a.get_unchecked(a as usize);
                    let bv = *reg_b.get_unchecked(a as usize);
                    let (ra, rb) = if is_add {
                        (
                            _mm256_add_pd(av, _mm256_set1_pd(argument)),
                            _mm256_add_pd(bv, _mm256_set1_pd(argument)),
                        )
                    } else if argument == 0.0 {
                        (zero, zero)
                    } else {
                        (
                            _mm256_mul_pd(av, _mm256_set1_pd(argument)),
                            _mm256_mul_pd(bv, _mm256_set1_pd(argument)),
                        )
                    };
                    *reg_a.get_unchecked_mut(dst as usize) = ra;
                    *reg_b.get_unchecked_mut(dst as usize) = rb;
                }
            }
        }
        (
            *reg_a.get_unchecked(prog.result as usize),
            *reg_b.get_unchecked(prog.result as usize),
        )
    }

    /// Two-row runner: TWO 4-wide stacks (A = block_y_a row, B = block_y_b
    /// row) driven by ONE op dispatch loop — halves the interpreter dispatch
    /// while keeping every op the exact per-lane IEEE sequence of the scalar
    /// runner. rows8[slot] = [r1a, r2a, r1b, r2b].
    #[target_feature(enable = "avx2")]
    #[allow(clippy::too_many_arguments)]
    pub unsafe fn run_cellfill_2rows(
        prog: &[CellOp],
        rows8: &[f64],
        block_y_a: i32,
        block_y_b: i32,
        dzv: __m256d,
        ixp: &i32,
        stack_a: &mut [__m256d; 16],
        stack_b: &mut [__m256d; 16],
        sp: &mut usize,
    ) -> (__m256d, __m256d) {
        let zero = _mm256_setzero_pd();
        let sign_abs = _mm256_set1_pd(-0.0);
        *sp = 0;
        for op in prog {
            match op {
                CellOp::PushConst(v) => {
                    let c = _mm256_set1_pd(*v);
                    stack_a[*sp] = c;
                    stack_b[*sp] = c;
                    *sp += 1;
                }
                CellOp::Lerp3 { slot } => {
                    // rows8 layout [slot][k][ix] (k = r1a/r2a/r1b/r2b planes,
                    // ix = lane): r1a[ix] etc. are strided reads at +ix.
                    let base = *slot as usize * 16 + *ixp as usize;
                    let (r1a, r2a) = (
                        _mm256_set1_pd(rows8[base]),
                        _mm256_set1_pd(rows8[base + 4]),
                    );
                    let (r1b, r2b) = (
                        _mm256_set1_pd(rows8[base + 8]),
                        _mm256_set1_pd(rows8[base + 12]),
                    );
                    // a + t*(b-a)
                    stack_a[*sp] = _mm256_add_pd(r1a, _mm256_mul_pd(dzv, _mm256_sub_pd(r2a, r1a)));
                    stack_b[*sp] = _mm256_add_pd(r1b, _mm256_mul_pd(dzv, _mm256_sub_pd(r2b, r1b)));
                    *sp += 1;
                }
                CellOp::YGrad { from_y, to_y, from_value, to_value } => {
                    let grad = |by: i32| -> f64 {
                        let t = (by as f64 - *from_y as f64) / (*to_y as f64 - *from_y as f64);
                        if t < 0.0 {
                            *from_value
                        } else if t > 1.0 {
                            *to_value
                        } else {
                            *from_value + t * (*to_value - *from_value)
                        }
                    };
                    stack_a[*sp] = _mm256_set1_pd(grad(block_y_a));
                    stack_b[*sp] = _mm256_set1_pd(grad(block_y_b));
                    *sp += 1;
                }
                CellOp::Add => {
                    *sp -= 1;
                    let ba = stack_a[*sp];
                    let bb = stack_b[*sp];
                    *sp -= 1;
                    let aa = stack_a[*sp];
                    let ab = stack_b[*sp];
                    stack_a[*sp] = _mm256_add_pd(aa, ba);
                    stack_b[*sp] = _mm256_add_pd(ab, bb);
                    *sp += 1;
                }
                CellOp::Mul => {
                    *sp -= 1;
                    let ba = stack_a[*sp];
                    let bb = stack_b[*sp];
                    *sp -= 1;
                    let aa = stack_a[*sp];
                    let ab = stack_b[*sp];
                    let ma = _mm256_cmp_pd(aa, zero, _CMP_EQ_OQ);
                    let mb = _mm256_cmp_pd(ab, zero, _CMP_EQ_OQ);
                    stack_a[*sp] = _mm256_blendv_pd(_mm256_mul_pd(aa, ba), zero, ma);
                    stack_b[*sp] = _mm256_blendv_pd(_mm256_mul_pd(ab, bb), zero, mb);
                    *sp += 1;
                }
                CellOp::Min => {
                    *sp -= 1;
                    let ba = stack_a[*sp];
                    let bb = stack_b[*sp];
                    *sp -= 1;
                    let aa = stack_a[*sp];
                    let ab = stack_b[*sp];
                    stack_a[*sp] = _mm256_blendv_pd(ba, aa, _mm256_cmp_pd(aa, ba, _CMP_LT_OQ));
                    stack_b[*sp] = _mm256_blendv_pd(bb, ab, _mm256_cmp_pd(ab, bb, _CMP_LT_OQ));
                    *sp += 1;
                }
                CellOp::Max => {
                    *sp -= 1;
                    let ba = stack_a[*sp];
                    let bb = stack_b[*sp];
                    *sp -= 1;
                    let aa = stack_a[*sp];
                    let ab = stack_b[*sp];
                    stack_a[*sp] = _mm256_blendv_pd(ba, aa, _mm256_cmp_pd(aa, ba, _CMP_GT_OQ));
                    stack_b[*sp] = _mm256_blendv_pd(bb, ab, _mm256_cmp_pd(ab, bb, _CMP_GT_OQ));
                    *sp += 1;
                }
                CellOp::Abs => {
                    *sp -= 1;
                    let aa = stack_a[*sp];
                    let ab = stack_b[*sp];
                    stack_a[*sp] = _mm256_andnot_pd(sign_abs, aa);
                    stack_b[*sp] = _mm256_andnot_pd(sign_abs, ab);
                    *sp += 1;
                }
                CellOp::Square => {
                    *sp -= 1;
                    let aa = stack_a[*sp];
                    let ab = stack_b[*sp];
                    stack_a[*sp] = _mm256_mul_pd(aa, aa);
                    stack_b[*sp] = _mm256_mul_pd(ab, ab);
                    *sp += 1;
                }
                CellOp::Cube => {
                    *sp -= 1;
                    let aa = stack_a[*sp];
                    let ab = stack_b[*sp];
                    let a2a = _mm256_mul_pd(aa, aa);
                    let a2b = _mm256_mul_pd(ab, ab);
                    stack_a[*sp] = _mm256_mul_pd(a2a, aa);
                    stack_b[*sp] = _mm256_mul_pd(a2b, ab);
                    *sp += 1;
                }
                CellOp::HalfNeg => {
                    *sp -= 1;
                    let aa = stack_a[*sp];
                    let ab = stack_b[*sp];
                    stack_a[*sp] = _mm256_blendv_pd(
                        _mm256_mul_pd(aa, _mm256_set1_pd(0.5)),
                        aa,
                        _mm256_cmp_pd(aa, zero, _CMP_GT_OQ),
                    );
                    stack_b[*sp] = _mm256_blendv_pd(
                        _mm256_mul_pd(ab, _mm256_set1_pd(0.5)),
                        ab,
                        _mm256_cmp_pd(ab, zero, _CMP_GT_OQ),
                    );
                    *sp += 1;
                }
                CellOp::QuarterNeg => {
                    *sp -= 1;
                    let aa = stack_a[*sp];
                    let ab = stack_b[*sp];
                    stack_a[*sp] = _mm256_blendv_pd(
                        _mm256_mul_pd(aa, _mm256_set1_pd(0.25)),
                        aa,
                        _mm256_cmp_pd(aa, zero, _CMP_GT_OQ),
                    );
                    stack_b[*sp] = _mm256_blendv_pd(
                        _mm256_mul_pd(ab, _mm256_set1_pd(0.25)),
                        ab,
                        _mm256_cmp_pd(ab, zero, _CMP_GT_OQ),
                    );
                    *sp += 1;
                }
                CellOp::Squeeze => {
                    *sp -= 1;
                    let aa = stack_a[*sp];
                    let ab = stack_b[*sp];
                    let da = _mm256_min_pd(_mm256_max_pd(aa, _mm256_set1_pd(-1.0)), _mm256_set1_pd(1.0));
                    let db = _mm256_min_pd(_mm256_max_pd(ab, _mm256_set1_pd(-1.0)), _mm256_set1_pd(1.0));
                    let d3a = _mm256_mul_pd(_mm256_mul_pd(da, da), da);
                    let d3b = _mm256_mul_pd(_mm256_mul_pd(db, db), db);
                    stack_a[*sp] = _mm256_sub_pd(
                        _mm256_div_pd(da, _mm256_set1_pd(2.0)),
                        _mm256_div_pd(d3a, _mm256_set1_pd(24.0)),
                    );
                    stack_b[*sp] = _mm256_sub_pd(
                        _mm256_div_pd(db, _mm256_set1_pd(2.0)),
                        _mm256_div_pd(d3b, _mm256_set1_pd(24.0)),
                    );
                    *sp += 1;
                }
                CellOp::Clamp { min, max } => {
                    *sp -= 1;
                    let aa = stack_a[*sp];
                    let ab = stack_b[*sp];
                    let (miv, mav) = (_mm256_set1_pd(*min), _mm256_set1_pd(*max));
                    stack_a[*sp] = _mm256_min_pd(_mm256_max_pd(aa, miv), mav);
                    stack_b[*sp] = _mm256_min_pd(_mm256_max_pd(ab, miv), mav);
                    *sp += 1;
                }
                CellOp::RangeChoice { min_inclusive, max_exclusive } => {
                    *sp -= 1;
                    let outa = stack_a[*sp];
                    let outb = stack_b[*sp];
                    *sp -= 1;
                    let inra = stack_a[*sp];
                    let inrb = stack_b[*sp];
                    *sp -= 1;
                    let va = stack_a[*sp];
                    let vb = stack_b[*sp];
                    let (miv, mav) = (_mm256_set1_pd(*min_inclusive), _mm256_set1_pd(*max_exclusive));
                    stack_a[*sp] = _mm256_blendv_pd(
                        outa,
                        inra,
                        _mm256_and_pd(_mm256_cmp_pd(va, miv, _CMP_GE_OQ), _mm256_cmp_pd(va, mav, _CMP_LT_OQ)),
                    );
                    stack_b[*sp] = _mm256_blendv_pd(
                        outb,
                        inrb,
                        _mm256_and_pd(_mm256_cmp_pd(vb, miv, _CMP_GE_OQ), _mm256_cmp_pd(vb, mav, _CMP_LT_OQ)),
                    );
                    *sp += 1;
                }
                CellOp::MulOrAdd { is_add, argument } => {
                    *sp -= 1;
                    let aa = stack_a[*sp];
                    let ab = stack_b[*sp];
                    if *is_add {
                        let c = _mm256_set1_pd(*argument);
                        stack_a[*sp] = _mm256_add_pd(aa, c);
                        stack_b[*sp] = _mm256_add_pd(ab, c);
                    } else if *argument == 0.0 {
                        stack_a[*sp] = zero;
                        stack_b[*sp] = zero;
                    } else {
                        let c = _mm256_set1_pd(*argument);
                        stack_a[*sp] = _mm256_mul_pd(aa, c);
                        stack_b[*sp] = _mm256_mul_pd(ab, c);
                    };
                    *sp += 1;
                }
            }
        }
        (stack_a[0], stack_b[0])
    }

    /// Rows hoisting for the 2-row path, vectorized over the four ix lanes:
    /// per slot, 8 vector ops replace 4ix x 12 scalar lerps. rows8 layout
    /// [slot][ix][k] with k = (r1a, r2a, r1b, r2b). Bit-exact: same IEEE
    /// lerp sequences per (slot, ix).
    #[target_feature(enable = "avx2")]
    pub unsafe fn hoist_rows_2row(
        interp: &crate::density::InterpState,
        slots: &[u16],
        rows8: &mut [f64],
        dy_a: f64,
        dy_b: f64,
        dx_step: f64,
    ) {
        let dxv = _mm256_setr_pd(0.0, dx_step, dx_step + dx_step, dx_step + dx_step + dx_step);
        let dya = _mm256_set1_pd(dy_a);
        let dyb = _mm256_set1_pd(dy_b);
        for &s in slots {
            let n = interp.corners_pub(s);
            let n0 = _mm256_set1_pd(n[0]);
            let n1 = _mm256_set1_pd(n[1]);
            let n2 = _mm256_set1_pd(n[2]);
            let n3 = _mm256_set1_pd(n[3]);
            let n4 = _mm256_set1_pd(n[4]);
            let n5 = _mm256_set1_pd(n[5]);
            let n6 = _mm256_set1_pd(n[6]);
            let n7 = _mm256_set1_pd(n[7]);
            // x-level (shared by rows A and B): lerp(dx, na, nb)
            let x01 = _mm256_add_pd(n0, _mm256_mul_pd(dxv, _mm256_sub_pd(n1, n0)));
            let x23 = _mm256_add_pd(n2, _mm256_mul_pd(dxv, _mm256_sub_pd(n3, n2)));
            let x45 = _mm256_add_pd(n4, _mm256_mul_pd(dxv, _mm256_sub_pd(n5, n4)));
            let x67 = _mm256_add_pd(n6, _mm256_mul_pd(dxv, _mm256_sub_pd(n7, n6)));
            let base = s as usize * 16;
            let r1a = _mm256_add_pd(x01, _mm256_mul_pd(dya, _mm256_sub_pd(x23, x01)));
            _mm256_storeu_pd(rows8.as_mut_ptr().add(base), r1a);
            let r2a = _mm256_add_pd(x45, _mm256_mul_pd(dya, _mm256_sub_pd(x67, x45)));
            _mm256_storeu_pd(rows8.as_mut_ptr().add(base + 4), r2a);
            let r1b = _mm256_add_pd(x01, _mm256_mul_pd(dyb, _mm256_sub_pd(x23, x01)));
            _mm256_storeu_pd(rows8.as_mut_ptr().add(base + 8), r1b);
            let r2b = _mm256_add_pd(x45, _mm256_mul_pd(dyb, _mm256_sub_pd(x67, x45)));
            _mm256_storeu_pd(rows8.as_mut_ptr().add(base + 12), r2b);
        }
    }

    /// Uninitialized-equivalent stacks (zeroed once per iy-pair, reused
    /// across the 4 ix runs; the topologically-ordered program overwrites
    /// every register before it is read).
    pub fn fresh_stack512() -> [__m512d; 16] {
        unsafe { [_mm512_setzero_pd(); 16] }
    }

    pub fn fresh_stacks() -> ([__m256d; 16], [__m256d; 16], usize) {
        unsafe {
            (
                [_mm256_setzero_pd(); 16],
                [_mm256_setzero_pd(); 16],
                0,
            )
        }
    }

    /// dz vector [0, s, 2s, 3s] (same fractions for both rows).
    pub fn dz_vec(dx_step: f64) -> __m256d {
        unsafe { _mm256_setr_pd(0.0, dx_step, dx_step + dx_step, dx_step + dx_step + dx_step) }
    }
}
