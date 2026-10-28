//! df263.rs — Minecraft 26.3 density-function engine (cont-48).
//! Volume (buffer algebra) + point paths, bit-exact vs Paper 26.3 b218.
//! Semantics frozen from reference/dec263 (see reference/port263_design.md).

use std::collections::HashMap;
use std::sync::Arc;

use crate::n263::{self, BlendedNoise263, Noise263, NormalNoise263, NormalNoiseParameters};
use crate::rng::XoroshiroPositionalRandomFactory;

// ---------------------------------------------------------------- volume model

#[derive(Clone, Debug, PartialEq)]
pub struct DensityVolume {
    pub size_x: i32,
    pub size_y: i32,
    pub size_z: i32,
    pub min_x: i32,
    pub min_y: i32,
    pub min_z: i32,
    pub step_x: i32,
    pub step_y: i32,
    pub step_z: i32,
}

impl DensityVolume {
    pub fn block_step(size: (i32, i32, i32), min: (i32, i32, i32)) -> Self {
        DensityVolume {
            size_x: size.0,
            size_y: size.1,
            size_z: size.2,
            min_x: min.0,
            min_y: min.1,
            min_z: min.2,
            step_x: 1,
            step_y: 1,
            step_z: 1,
        }
    }

    #[inline]
    pub fn index_unchecked(&self, x: i32, y: i32, z: i32) -> usize {
        (y + (x + z * self.size_x) * self.size_y) as usize
    }

    #[inline]
    pub fn block_x(&self, x: i32) -> i32 {
        self.min_x + x * self.step_x
    }

    #[inline]
    pub fn block_y(&self, y: i32) -> i32 {
        self.min_y + y * self.step_y
    }

    #[inline]
    pub fn block_z(&self, z: i32) -> i32 {
        self.min_z + z * self.step_z
    }

    #[inline]
    pub fn max_block_x(&self) -> i32 {
        self.min_x + self.size_x * self.step_x - 1
    }

    #[inline]
    pub fn max_block_y(&self) -> i32 {
        self.min_y + self.size_y * self.step_y - 1
    }

    #[inline]
    pub fn max_block_z(&self) -> i32 {
        self.min_z + self.size_z * self.step_z - 1
    }

    #[inline]
    pub fn size(&self) -> usize {
        (self.size_x * self.size_y * self.size_z) as usize
    }

    /// DensityVolume.indexOfBlock (26.3): -1 when outside / off-lattice.
    pub fn index_of_block(&self, bx: i32, by: i32, bz: i32) -> i64 {
        let rx = bx - self.min_x;
        let ry = by - self.min_y;
        let rz = bz - self.min_z;
        if self.step_x == 1 && self.step_y == 1 && self.step_z == 1 {
            if rx >= 0 && ry >= 0 && rz >= 0 && rx < self.size_x && ry < self.size_y && rz < self.size_z {
                return self.index_unchecked(rx, ry, rz) as i64;
            }
        } else if rx >= 0
            && ry >= 0
            && rz >= 0
            && rx < self.size_x * self.step_x
            && ry < self.size_y * self.step_y
            && rz < self.size_z * self.step_z
            && rx % self.step_x == 0
            && ry % self.step_y == 0
            && rz % self.step_z == 0
        {
            return self.index_unchecked(rx / self.step_x, ry / self.step_y, rz / self.step_z) as i64;
        }
        -1
    }
}

// ---------------------------------------------------------------- context

/// Buffer scratch pool (arena semantics; explicit acquire/release).
#[derive(Default)]
pub struct Scratch263 {
    pool: Vec<Vec<f32>>,
}

impl Scratch263 {
    pub fn acquire(&mut self, size: usize) -> Vec<f32> {
        match self.pool.pop() {
            Some(mut v) if v.capacity() >= size => {
                v.clear();
                v.resize(size, 0.0);
                v
            }
            _ => vec![0.0f32; size],
        }
    }

    pub fn release(&mut self, v: Vec<f32>) {
        if self.pool.len() < 32 {
            self.pool.push(v);
        }
    }
}

struct CacheCell263 {
    volume: Option<DensityVolume>,
    buffer: Option<Vec<f32>>,
    value_key: Option<(i32, i32, i32)>,
    value: f32,
}

impl CacheCell263 {
    fn new() -> Self {
        CacheCell263 {
            volume: None,
            buffer: None,
            value_key: None,
            value: f32::NAN,
        }
    }
}

/// SamplerContext 26.3: scratch arena + optional cache cells (caches disabled in
/// EMPTY_UNCACHED -> pass-through, mirroring Java null cacheCells).
/// `beard` carries the per-chunk Beardifier (Beardifier.CONTEXT_KEY user field;
/// EMPTY == the oracle's constant(0.0) fallback — identical values).
pub struct SamplerCtx263 {
    pub scratch: Scratch263,
    caches: Option<Vec<CacheCell263>>,
    pub beard: crate::beard263::Beardifier263,
}

impl SamplerCtx263 {
    pub fn uncached() -> Self {
        SamplerCtx263 {
            scratch: Scratch263::default(),
            caches: None,
            beard: crate::beard263::Beardifier263::empty(),
        }
    }

    pub fn cached() -> Self {
        SamplerCtx263 {
            scratch: Scratch263::default(),
            caches: Some(Vec::new()),
            beard: crate::beard263::Beardifier263::empty(),
        }
    }

    fn cell(&mut self, id: u32) -> Option<&mut CacheCell263> {
        let caches = self.caches.as_mut()?;
        if id as usize >= caches.len() {
            // Mth.roundToward(cacheId + 1, 16) = positiveCeilDiv * 16.
            let new_size = ((id as usize + 1) / 16 + usize::from((id as usize + 1) % 16 != 0)) * 16;
            caches.resize_with(new_size, CacheCell263::new);
        }
        Some(&mut caches[id as usize])
    }

    /// CachingDensitySampler.sampleVolume (26.3).
    fn sample_volume_cached(&mut self, id: u32, input: &DfSam263, out: &mut Vec<f32>, vol: &DensityVolume) {
        if self.caches.is_none() {
            return input.sample_volume(self, out, vol);
        }
        let need_recompute = {
            let cell = self.cell(id).unwrap();
            match (&cell.buffer, &cell.volume) {
                (Some(_), Some(v)) => v != vol,
                _ => true,
            }
        };
        if need_recompute {
            let mut buf = self.scratch.acquire(vol.size());
            input.sample_volume(self, &mut buf, vol);
            let cell = self.cell(id).unwrap();
            cell.volume = Some(vol.clone());
            cell.buffer = Some(buf);
        }
        let len = out.len();
        let buffer = self.cell(id).unwrap().buffer.as_ref().unwrap().clone();
        out.copy_from_slice(&buffer[..len]);
    }

    /// CachingDensitySampler.sampleValue (26.3): volume buffer hit -> point memo -> compute.
    fn sample_value_cached(&mut self, id: u32, input: &DfSam263, x: i32, y: i32, z: i32) -> f32 {
        if self.caches.is_none() {
            return input.sample_value(self, x, y, z);
        }
        let memo_hit = {
            let cell = self.cell(id).unwrap();
            match cell.value_key {
                Some((kx, ky, kz)) if (kx, ky, kz) == (x, y, z) && !cell.value.is_nan() => Some(cell.value),
                _ => None,
            }
        };
        if let Some(v) = memo_hit {
            return v;
        }
        let buf_hit = {
            let cell = self.cell(id).unwrap();
            match (&cell.buffer, &cell.volume) {
                (Some(buffer), Some(volume)) => {
                    let index = volume.index_of_block(x, y, z);
                    if index != -1 {
                        Some(buffer[index as usize])
                    } else {
                        None
                    }
                }
                _ => None,
            }
        };
        if let Some(v) = buf_hit {
            return v;
        }
        let value = input.sample_value(self, x, y, z);
        let cell = self.cell(id).unwrap();
        cell.value_key = Some((x, y, z));
        cell.value = value;
        value
    }
}

// ---------------------------------------------------------------- helpers

#[inline]
fn mth_lerp(alpha: f32, p0: f32, p1: f32) -> f32 {
    p0 + alpha * (p1 - p0)
}

#[inline]
fn mth_clamp(value: f32, min: f32, max: f32) -> f32 {
    if value < min {
        min
    } else if value > max {
        max
    } else {
        value
    }
}

#[inline]
fn mth_floor_f32(v: f32) -> i32 {
    (v as f64).floor() as i32
}

fn floor_div(a: i32, b: i32) -> i32 {
    (a as f64 / b as f64).floor() as i32
}

#[inline]
fn floor_mod(a: i32, b: i32) -> i32 {
    a - b * floor_div(a, b)
}

// ---------------------------------------------------------------- compiled samplers

pub type SamRef = Arc<DfSam263>;

// ---- volume-sample profiling (fill263bench: prof report) ----------------
pub static PROF_ON_263: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
/// Exclusive per-kind nanos: 0 shell, 1 noise-family, 2 blended, 3 spline,
/// 4 gradient, 5 beard, 6 slice, 7 cache, 8 interpolated-shell.
pub static PROF_KINDS: [std::sync::atomic::AtomicU64; 9] = [
    std::sync::atomic::AtomicU64::new(0),
    std::sync::atomic::AtomicU64::new(0),
    std::sync::atomic::AtomicU64::new(0),
    std::sync::atomic::AtomicU64::new(0),
    std::sync::atomic::AtomicU64::new(0),
    std::sync::atomic::AtomicU64::new(0),
    std::sync::atomic::AtomicU64::new(0),
    std::sync::atomic::AtomicU64::new(0),
    std::sync::atomic::AtomicU64::new(0),
];
/// Exact internal timers of the Interpolated volume path.
pub static PROF_INTERP_CELL: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);
pub static PROF_INTERP_FILL: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);
thread_local! { static PROF_CHILD_T: std::cell::Cell<u64> = std::cell::Cell::new(0); }

#[inline]
fn prof_now() -> u64 {
    use std::sync::atomic::AtomicU64;
    static EPOCH: AtomicU64 = AtomicU64::new(0);
    static TICK: AtomicU64 = AtomicU64::new(0);
    // monotonic micro-timer: nanos since first use via Instant elapsed
    let t = std::time::Instant::now();
    let base = EPOCH.compare_exchange(0, 1, std::sync::atomic::Ordering::Relaxed, std::sync::atomic::Ordering::Relaxed).is_ok();
    if base {
        // store nothing; we only need deltas — use a monotonic counter seeded once
    }
    let _ = &EPOCH;
    let _ = &TICK;
    // Use a cheap monotonic source: Instant elapsed since thread start is not
    // available; fall back to Instant::now() minus a lazy global epoch.
    static mut EPOCH_INST: Option<std::time::Instant> = None;
    unsafe {
        if let Some(e) = &*(&raw const EPOCH_INST) {
            t.duration_since(*e).as_nanos() as u64
        } else {
            EPOCH_INST = Some(t);
            0
        }
    }
}

impl DfSam263 {
    /// Profiling class id (see PROF_KINDS).
    pub fn prof_kind(&self) -> usize {
        match self {
            DfSam263::Noise { .. }
            | DfSam263::ShiftedXz { .. }
            | DfSam263::ShiftB { .. }
            | DfSam263::Blended { .. } => 1,
            DfSam263::Spline { .. } => 3,
            DfSam263::Gradient { .. } => 4,
            DfSam263::Beard => 5,
            DfSam263::SliceX { .. }
            | DfSam263::SliceY { .. }
            | DfSam263::SliceZ { .. }
            | DfSam263::SliceXz { .. } => 6,
            DfSam263::Cache { .. } => 7,
            DfSam263::Interpolated { .. } => 8,
            _ => 0,
        }
    }
}

pub const PROF_KIND_NAMES: [&str; 9] = [
    "shell(plain ops)",
    "noise-family",
    "blended",
    "spline",
    "gradient",
    "beard",
    "slice",
    "cache",
    "interpolated-shell",
];

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Min,
    Max,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum UnOp {
    Abs,
    Square,
    Cube,
    Sqrt,
    HalfNegative,
    QuarterNegative,
    Reciprocal,
    Negate,
    Squeeze,
    Log,
    Sign,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum RoundType {
    Floor,
    Round,
    Ceil,
    Truncate,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Axis {
    X,
    Y,
    Z,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Tiling {
    ClampToEdge,
    Repeat,
    MirroredRepeat,
}

/// Compiled density sampler (mirrors the Java compileSampler products).
pub enum DfSam263 {
    Constant(f32),
    /// NoiseFunction.Sampler (no shifts).
    Noise {
        noise: Arc<N263>,
        xz_scale: f64,
        y_scale: f64,
    },
    /// NoiseFunction.ShiftedXzSampler (shiftY == 0; overworld usage).
    ShiftedXz {
        shift_x: SamRef,
        shift_z: SamRef,
        noise: Arc<N263>,
        xz_scale: f64,
        y_scale: f64,
    },
    Binary {
        op: BinOp,
        l: SamRef,
        r: SamRef,
        /// right.range().min() for Min / .max() for Max point shortcuts (f32 range math).
        r_bound: f32,
    },
    ConstAdd {
        a: SamRef,
        c: f32,
    },
    /// ConstSubSampler: left constant minus right sampler (c - a).
    ConstSub {
        c: f32,
        a: SamRef,
    },
    ConstMul {
        a: SamRef,
        c: f32,
    },
    /// ConstDivSampler: constant left divided by right sampler (c / a).
    ConstDiv {
        c: f32,
        a: SamRef,
    },
    ConstMin {
        a: SamRef,
        c: f32,
    },
    ConstMax {
        a: SamRef,
        c: f32,
    },
    Unary {
        op: UnOp,
        a: SamRef,
    },
    Clamp {
        a: SamRef,
        min: f32,
        max: f32,
    },
    Lerp {
        alpha: SamRef,
        first: SamRef,
        second: SamRef,
    },
    LerpConstFirst {
        alpha: SamRef,
        first: f32,
        second: SamRef,
    },
    LerpConstSecond {
        alpha: SamRef,
        first: SamRef,
        second: f32,
    },
    PowConstBase {
        base: f64,
        exp: SamRef,
    },
    PowConstExp {
        a: SamRef,
        exp: f64,
    },
    Pow {
        a: SamRef,
        b: SamRef,
    },
    RoundInt {
        ty: RoundType,
        a: SamRef,
    },
    Round {
        ty: RoundType,
        a: SamRef,
        mul: SamRef,
    },
    RangeChoice {
        a: SamRef,
        min: f32,
        max: f32,
        in_range: SamRef,
        out_of_range: SamRef,
    },
    /// RangeChoice with both branch constants (Java ConstSampler).
    RangeChoiceConst {
        a: SamRef,
        min: f32,
        max: f32,
        in_v: f32,
        out_v: f32,
    },
    IntervalSelect {
        a: SamRef,
        thresholds: Vec<f32>,
        branches: Vec<SamRef>,
    },
    Cache {
        id: u32,
        a: SamRef,
    },
    SliceX {
        a: SamRef,
        x: i32,
    },
    SliceY {
        a: SamRef,
        y: i32,
    },
    SliceZ {
        a: SamRef,
        z: i32,
    },
    SliceXz {
        a: SamRef,
        x: i32,
        z: i32,
    },
    Gradient {
        axis: Axis,
        tiling: Tiling,
        from_coord: i32,
        /// min(from,to) for clamp / range for repeat / mirrored.
        aux: i32,
        from_value: f32,
        factor: f32,
    },
    Interpolated {
        a: SamRef,
        cell_xz: i32,
        cell_y: i32,
        inv_xz: f32,
        inv_y: f32,
    },
    FindTopSurface {
        density: SamRef,
        upper_bound: SamRef,
        lower_bound: i32,
        cell_height: i32,
    },
    BlendDensity {
        a: SamRef,
    },
    /// ShiftNoiseFunction.ShiftB compiled: noise.get(z*0.25, x*0.25, 0)*4
    /// (transposed volume path).
    ShiftB {
        noise: Arc<N263>,
    },
    /// BlendedNoise 26.3 compiled tree: Lerp(clamp(main+0.5,0,1), min, max)
    /// over three FBM stacks, all f32 lerp short-circuits.
    Blended {
        min_stack: Arc<Noise263>,
        max_stack: Arc<Noise263>,
        main_stack: Arc<Noise263>,
        xz_multiplier: f64,
        y_multiplier: f64,
        main_xz_div: f64,
        main_y_div: f64,
    },
    /// ContextBoundSampler(Beardifier.CONTEXT_KEY, fallback 0.0) — the beard
    /// contribution from the ctx (EMPTY beardifier == 0.0 everywhere).
    Beard,
    Spline {
        coord: SamRef,
        locations: Vec<f32>,
        derivatives: Vec<f32>,
        values: Vec<SplineRef>,
    },
}

/// CubicSpline 26.3 (f32 throughout). Multi nodes carry a coord_slot: the DFS
/// pre-order index of their coordinate among all coordinates of the ROOT spline
/// (slot 0 = root coordinate). Volume path samples every slot into a buffer
/// once (BufferSplineInput); point path caches per slot (PointSplineInput).
pub enum SplineRef {
    Constant(f32),
    Multi {
        coord_slot: usize,
        coord: SamRef,
        locations: Vec<f32>,
        derivatives: Vec<f32>,
        values: Vec<SplineRef>,
    },
}

impl Clone for SplineRef {
    fn clone(&self) -> Self {
        match self {
            SplineRef::Constant(v) => SplineRef::Constant(*v),
            SplineRef::Multi {
                coord_slot,
                coord,
                locations,
                derivatives,
                values,
            } => SplineRef::Multi {
                coord_slot: *coord_slot,
                coord: coord.clone(),
                locations: locations.clone(),
                derivatives: derivatives.clone(),
                values: values.clone(),
            },
        }
    }
}

impl SplineRef {
    /// Assign DFS pre-order coordinate slots; returns next free slot.
    pub fn assign_slots(&mut self, next: usize) -> usize {
        match self {
            SplineRef::Constant(_) => next,
            SplineRef::Multi {
                coord_slot,
                coord: _,
                values,
                ..
            } => {
                *coord_slot = next;
                let mut n = next + 1;
                for v in values.iter_mut() {
                    n = v.assign_slots(n);
                }
                n
            }
        }
    }

    pub fn collect_coords(&self, coords: &mut Vec<SamRef>) {
        match self {
            SplineRef::Constant(_) => {}
            SplineRef::Multi {
                coord, values, ..
            } => {
                coords.push(coord.clone());
                for v in values {
                    v.collect_coords(coords);
                }
            }
        }
    }
}

/// Noise instance (26.3): NormalNoise / BlendedNoise stacks / plain Noise263.
pub enum N263 {
    Normal(NormalNoise263),
    Plain(Noise263),
}

impl N263 {
    #[inline]
    pub fn get(&self, x: f64, y: f64, z: f64) -> f32 {
        match self {
            N263::Normal(n) => n.get(x, y, z),
            N263::Plain(n) => n.get(x, y, z),
        }
    }

    #[inline]
    pub fn get_batch8(&self, x: &[f64; 8], y: &[f64; 8], z: &[f64; 8]) -> [f32; 8] {
        match self {
            N263::Normal(n) => n.get_batch8(x, y, z),
            N263::Plain(n) => n.get_batch8(x, y, z),
        }
    }
}

// ---------------------------------------------------------------- noise factory

/// Mirrors RandomState: root positional factory + memoized noise instances
/// (Noises.instantiate = params.create(root.fromHashOf("minecraft:<name>"))).
pub struct NoiseFactory263 {
    root: XoroshiroPositionalRandomFactory,
    cache: HashMap<String, Arc<N263>>,
}

impl NoiseFactory263 {
    pub fn new(seed: i64) -> Self {
        let mut src = crate::rng::XoroshiroRandomSource::new(seed);
        let root = src.fork_positional();
        NoiseFactory263 {
            root,
            cache: HashMap::new(),
        }
    }

    pub fn normal(&mut self, name: &str, params: NormalNoiseParameters) -> Arc<N263> {
        let key = format!("minecraft:{name}");
        if let Some(n) = self.cache.get(&key) {
            return n.clone();
        }
        let mut random = self.root.from_hash_of(&key);
        let noise = NormalNoise263::new(params, &mut random);
        let rc = Arc::new(N263::Normal(noise));
        self.cache.insert(key, rc.clone());
        rc
    }

    /// RandomState.getOrCreateRandomFactory(name) step 1:
    /// random.fromHashOf(name) — the caller forkPositional()s the result.
    pub fn root_from_hash_of(&self, name: &str) -> crate::rng::XoroshiroRandomSource {
        self.root.from_hash_of(name)
    }

    /// BlendedNoise 26.3: createFbmSet(context.createRandom("minecraft:terrain")).
    pub fn blended(
        &mut self,
        xz_scale: f64,
        y_scale: f64,
        xz_factor: f64,
        y_factor: f64,
        smear_scale_multiplier: f64,
    ) -> (Arc<Noise263>, Arc<Noise263>, Arc<Noise263>, f64, f64, f64, f64) {
        let mut random = self.root.from_hash_of("minecraft:terrain");
        let blended = BlendedNoise263::new(
            xz_scale,
            y_scale,
            xz_factor,
            y_factor,
            smear_scale_multiplier,
            &mut random,
        );
        let min_stack = Arc::new(Noise263::Stack(Box::new(blended.min_limit)));
        let max_stack = Arc::new(Noise263::Stack(Box::new(blended.max_limit)));
        let main = Arc::new(Noise263::Stack(Box::new(blended.main)));
        let xz_multiplier = 684.412 * xz_scale;
        let y_multiplier = 684.412 * y_scale;
        (
            min_stack,
            max_stack,
            main,
            xz_multiplier,
            y_multiplier,
            xz_factor,
            y_factor,
        )
    }
}

// ---------------------------------------------------------------- sampler impls

impl DfSam263 {
    #[inline]
    pub fn sample_volume(&self, ctx: &mut SamplerCtx263, out: &mut Vec<f32>, vol: &DensityVolume) {
        if !PROF_ON_263.load(std::sync::atomic::Ordering::Relaxed) {
            // hot path: single dispatch, no wrapper overhead
            return self.sample_volume_inner(ctx, out, vol);
        }
        let t0 = prof_now();
        let child_before = PROF_CHILD_T.with(|c| c.get());
        PROF_CHILD_T.with(|c| c.set(0));
        self.sample_volume_inner(ctx, out, vol);
        let total = prof_now() - t0;
        let child = PROF_CHILD_T.with(|c| c.get());
        PROF_KINDS[self.prof_kind()].fetch_add(total - child, std::sync::atomic::Ordering::Relaxed);
        PROF_CHILD_T.with(|c| c.set(child_before + total));
    }

    fn sample_volume_inner(&self, ctx: &mut SamplerCtx263, out: &mut Vec<f32>, vol: &DensityVolume) {
        match self {
            DfSam263::Constant(c) => out.fill(*c),
            DfSam263::Beard => ctx.beard.fill_volume(vol, out),
            DfSam263::Noise {
                noise,
                xz_scale,
                y_scale,
            } => {
                // NoiseFunction.Sampler volume: fill(0) then addToVolume(amplitude*get).
                // addToVolume fills index-order per point; emulate directly.
                // NOTE: the 8-wide AVX2 kernel (get_batch8) measured 2x SLOWER
                // than this scalar loop — perlin is lookup-bound (9 dependent
                // p_adj table reads per point), the SIMD arithmetic does not
                // pay for the per-lane prep. A column-state kernel (hoist the
                // x/z lattice + hashes per column, like the legacy
                // noise_col4 kernels) is the future direction; the batch
                // kernel + bit-exact tests stay in n263.rs for that work.
                if *y_scale == 0.0 {
                    // 2D noise (cache_2d semantics): the sampled y is
                    // block_y(y)*0.0 = ±0.0 for EVERY row of a column — the
                    // same value by construction (sign-of-zero only; every
                    // downstream comparison treats them equal). Evaluate
                    // once per (x,z) column, broadcast over y — vanilla's
                    // Cache2D turns 49 redundant perlin evals per column
                    // into 1, the same way.
                    let by0 = vol.block_y(0) as f64 * y_scale;
                    let mut index = 0usize;
                    for z in 0..vol.size_z {
                        let bz = vol.block_z(z);
                        for x in 0..vol.size_x {
                            let bx = vol.block_x(x);
                            let v = noise.get(bx as f64 * xz_scale, by0, bz as f64 * xz_scale);
                            for _y in 0..vol.size_y {
                                out[index] = v;
                                index += 1;
                            }
                        }
                    }
                } else {
                let mut index = 0usize;
                for z in 0..vol.size_z {
                    let bz = vol.block_z(z);
                    for x in 0..vol.size_x {
                        let bx = vol.block_x(x);
                        for y in 0..vol.size_y {
                            let by = vol.block_y(y);
                            out[index] = noise.get(bx as f64 * xz_scale, by as f64 * y_scale, bz as f64 * xz_scale);
                            index += 1;
                        }
                    }
                }
                }
            }
            DfSam263::ShiftedXz {
                shift_x,
                shift_z,
                noise,
                xz_scale,
                y_scale,
            } => {
                shift_x.sample_volume(ctx, out, vol);
                let mut sz = ctx.scratch.acquire(vol.size());
                shift_z.sample_volume(ctx, &mut sz, vol);
                if *y_scale == 0.0 {
                    // 2D shifted noise (cache_2d semantics, see the Noise
                    // branch): the shift children of THIS tree are y-invariant
                    // by construction (router shift_x/shift_z wrap y_scale=0.0
                    // noises / the transposed ShiftB), so the y0 row carries
                    // the whole column. One noise.get per (x,z), broadcast.
                    let by0 = vol.block_y(0) as f64 * y_scale;
                    let mut index = 0usize;
                    for z in 0..vol.size_z {
                        let base_z = vol.block_z(z) as f64 * xz_scale;
                        for x in 0..vol.size_x {
                            let base_x = vol.block_x(x) as f64 * xz_scale;
                            let nx = base_x + out[index] as f64;
                            let nz = base_z + sz[index] as f64;
                            let v = noise.get(nx, by0, nz);
                            for _y in 0..vol.size_y {
                                out[index] = v;
                                index += 1;
                            }
                        }
                    }
                } else {
                let mut index = 0usize;
                for z in 0..vol.size_z {
                    let base_z = vol.block_z(z) as f64 * xz_scale;
                    for x in 0..vol.size_x {
                        let base_x = vol.block_x(x) as f64 * xz_scale;
                        for y in 0..vol.size_y {
                            let nx = base_x + out[index] as f64;
                            let ny = vol.block_y(y) as f64 * y_scale;
                            let nz = base_z + sz[index] as f64;
                            out[index] = noise.get(nx, ny, nz);
                            index += 1;
                        }
                    }
                }
                }
                ctx.scratch.release(sz);
            }
            DfSam263::Binary { op, l, r, .. } => {
                l.sample_volume(ctx, out, vol);
                let mut rb = ctx.scratch.acquire(vol.size());
                r.sample_volume(ctx, &mut rb, vol);
                for i in 0..out.len() {
                    let l = out[i];
                    let rv = rb[i];
                    out[i] = match op {
                        BinOp::Add => l + rv,
                        BinOp::Sub => l - rv,
                        BinOp::Mul => l * rv,
                        BinOp::Div => l / rv,
                        BinOp::Min => {
                            if rv < l {
                                rv
                            } else {
                                l
                            }
                        }
                        BinOp::Max => {
                            if rv > l {
                                rv
                            } else {
                                l
                            }
                        }
                    };
                }
                ctx.scratch.release(rb);
            }
            DfSam263::ConstAdd { a, c } => {
                a.sample_volume(ctx, out, vol);
                for v in out.iter_mut() {
                    *v += *c;
                }
            }
            DfSam263::ConstSub { c, a } => {
                a.sample_volume(ctx, out, vol);
                for v in out.iter_mut() {
                    *v = *c - *v;
                }
            }
            DfSam263::ConstMul { a, c } => {
                a.sample_volume(ctx, out, vol);
                for v in out.iter_mut() {
                    *v *= *c;
                }
            }
            DfSam263::ConstDiv { c, a } => {
                a.sample_volume(ctx, out, vol);
                for v in out.iter_mut() {
                    *v = *c / *v;
                }
            }
            DfSam263::ConstMin { a, c } => {
                a.sample_volume(ctx, out, vol);
                for v in out.iter_mut() {
                    if *c < *v {
                        *v = *c;
                    }
                }
            }
            DfSam263::ConstMax { a, c } => {
                a.sample_volume(ctx, out, vol);
                for v in out.iter_mut() {
                    if *c > *v {
                        *v = *c;
                    }
                }
            }
            DfSam263::Unary { op, a } => {
                a.sample_volume(ctx, out, vol);
                for v in out.iter_mut() {
                    *v = apply_unary(*op, *v);
                }
            }
            DfSam263::Clamp { a, min, max } => {
                a.sample_volume(ctx, out, vol);
                for v in out.iter_mut() {
                    *v = mth_clamp(*v, *min, *max);
                }
            }
            DfSam263::Lerp {
                alpha,
                first,
                second,
            } => {
                alpha.sample_volume(ctx, out, vol);
                let mut fb = ctx.scratch.acquire(vol.size());
                first.sample_volume(ctx, &mut fb, vol);
                let mut sb = ctx.scratch.acquire(vol.size());
                second.sample_volume(ctx, &mut sb, vol);
                for i in 0..out.len() {
                    let a = out[i];
                    out[i] = if a == 0.0 {
                        fb[i]
                    } else if a == 1.0 {
                        sb[i]
                    } else {
                        mth_lerp(a, fb[i], sb[i])
                    };
                }
                ctx.scratch.release(sb);
                ctx.scratch.release(fb);
            }
            DfSam263::LerpConstFirst {
                alpha,
                first,
                second,
            } => {
                alpha.sample_volume(ctx, out, vol);
                let mut sb = ctx.scratch.acquire(vol.size());
                second.sample_volume(ctx, &mut sb, vol);
                for i in 0..out.len() {
                    let a = out[i];
                    out[i] = if a == 0.0 {
                        *first
                    } else if a == 1.0 {
                        sb[i]
                    } else {
                        mth_lerp(a, *first, sb[i])
                    };
                }
                ctx.scratch.release(sb);
            }
            DfSam263::LerpConstSecond {
                alpha,
                first,
                second,
            } => {
                alpha.sample_volume(ctx, out, vol);
                let mut fb = ctx.scratch.acquire(vol.size());
                first.sample_volume(ctx, &mut fb, vol);
                for i in 0..out.len() {
                    let a = out[i];
                    out[i] = if a == 0.0 {
                        fb[i]
                    } else if a == 1.0 {
                        *second
                    } else {
                        mth_lerp(a, fb[i], *second)
                    };
                }
                ctx.scratch.release(fb);
            }
            DfSam263::PowConstBase { base, exp } => {
                exp.sample_volume(ctx, out, vol);
                for v in out.iter_mut() {
                    *v = (*base).powf(*v as f64) as f32;
                }
            }
            DfSam263::PowConstExp { a, exp } => {
                a.sample_volume(ctx, out, vol);
                for v in out.iter_mut() {
                    *v = (*v as f64).powf(*exp) as f32;
                }
            }
            DfSam263::Pow { a, b } => {
                a.sample_volume(ctx, out, vol);
                let mut bb = ctx.scratch.acquire(vol.size());
                b.sample_volume(ctx, &mut bb, vol);
                for i in 0..out.len() {
                    out[i] = (out[i] as f64).powf(bb[i] as f64) as f32;
                }
                ctx.scratch.release(bb);
            }
            DfSam263::RoundInt { ty, a } => {
                a.sample_volume(ctx, out, vol);
                for v in out.iter_mut() {
                    *v = apply_round(*ty, *v);
                }
            }
            DfSam263::Round { ty, a, mul } => {
                a.sample_volume(ctx, out, vol);
                let mut mb = ctx.scratch.acquire(vol.size());
                mul.sample_volume(ctx, &mut mb, vol);
                for i in 0..out.len() {
                    let m = mb[i];
                    out[i] = if m == 0.0 {
                        out[i]
                    } else {
                        apply_round(*ty, out[i] / m) * m
                    };
                }
                ctx.scratch.release(mb);
            }
            DfSam263::RangeChoice {
                a,
                min,
                max,
                in_range,
                out_of_range,
            } => {
                in_range.sample_volume(ctx, out, vol);
                let mut ib = ctx.scratch.acquire(vol.size());
                a.sample_volume(ctx, &mut ib, vol);
                let mut ob = ctx.scratch.acquire(vol.size());
                out_of_range.sample_volume(ctx, &mut ob, vol);
                for i in 0..out.len() {
                    let v = ib[i];
                    if !(v >= *min && v < *max) {
                        out[i] = ob[i];
                    }
                }
                ctx.scratch.release(ob);
                ctx.scratch.release(ib);
            }
            DfSam263::RangeChoiceConst {
                a,
                min,
                max,
                in_v,
                out_v,
            } => {
                a.sample_volume(ctx, out, vol);
                for v in out.iter_mut() {
                    *v = if *v >= *min && *v < *max { *in_v } else { *out_v };
                }
            }
            DfSam263::IntervalSelect {
                a,
                thresholds,
                branches,
            } => {
                a.sample_volume(ctx, out, vol);
                let mut bufs = Vec::with_capacity(branches.len());
                for b in branches {
                    let mut bb = ctx.scratch.acquire(vol.size());
                    b.sample_volume(ctx, &mut bb, vol);
                    bufs.push(bb);
                }
                for i in 0..out.len() {
                    let input = out[i];
                    let mut idx = branches.len() - 1;
                    for (ti, t) in thresholds.iter().enumerate() {
                        if input < *t {
                            idx = ti;
                            break;
                        }
                    }
                    out[i] = bufs[idx][i];
                }
                for bb in bufs {
                    ctx.scratch.release(bb);
                }
            }
            DfSam263::Cache { id, a } => {
                ctx.sample_volume_cached(*id, a, out, vol);
            }
            DfSam263::SliceX { a, x } => slice_volume_axis(a, ctx, out, vol, Axis::X, *x),
            DfSam263::SliceY { a, y } => slice_volume_axis(a, ctx, out, vol, Axis::Y, *y),
            DfSam263::SliceZ { a, z } => slice_volume_axis(a, ctx, out, vol, Axis::Z, *z),
            DfSam263::SliceXz { a, x, z } => {
                if vol.size_x == 1 && vol.size_z == 1 && vol.min_x == *x && vol.min_z == *z {
                    a.sample_volume(ctx, out, vol);
                } else {
                    let input_volume = DensityVolume {
                        size_x: 1,
                        size_y: vol.size_y,
                        size_z: 1,
                        min_x: *x,
                        min_y: vol.min_y,
                        min_z: *z,
                        step_x: vol.step_x,
                        step_y: vol.step_y,
                        step_z: vol.step_z,
                    };
                    let mut ib = ctx.scratch.acquire(input_volume.size());
                    a.sample_volume(ctx, &mut ib, &input_volume);
                    for yy in 0..vol.size_y {
                        let input = ib[input_volume.index_unchecked(0, yy, 0)];
                        let mut index = vol.index_unchecked(0, yy, 0);
                        for _z in 0..vol.size_z {
                            for _x in 0..vol.size_x {
                                out[index] = input;
                                index += vol.size_y as usize;
                            }
                        }
                    }
                    ctx.scratch.release(ib);
                }
            }
            DfSam263::Gradient {
                axis,
                tiling,
                from_coord,
                aux,
                from_value,
                factor,
            } => {
                let compute = |coordinate: i32| -> f32 {
                    match tiling {
                        Tiling::ClampToEdge => {
                            let clamped = coordinate.clamp((*from_coord).min(*aux), (*from_coord).max(*aux));
                            *from_value + (clamped - *from_coord) as f32 * *factor
                        }
                        Tiling::Repeat => {
                            let range = *aux;
                            let rel = coordinate - *from_coord;
                            *from_value + floor_mod(rel, range) as f32 * *factor
                        }
                        Tiling::MirroredRepeat => {
                            let range = *aux;
                            let rel = coordinate - *from_coord;
                            let tile_index = floor_div(rel, range);
                            let local = rel - tile_index * range;
                            if tile_index & 1 == 0 {
                                *from_value + local as f32 * *factor
                            } else {
                                *from_value + (range - local) as f32 * *factor
                            }
                        }
                    }
                };
                match axis {
                    Axis::X => {
                        for x in 0..vol.size_x {
                            let value = compute(vol.block_x(x));
                            for z in 0..vol.size_z {
                                let base = vol.index_unchecked(x, 0, z);
                                for yy in 0..vol.size_y {
                                    out[base + yy as usize] = value;
                                }
                            }
                        }
                    }
                    Axis::Y => {
                        for y in 0..vol.size_y {
                            let value = compute(vol.block_y(y));
                            for z in 0..vol.size_z {
                                for x in 0..vol.size_x {
                                    out[vol.index_unchecked(x, y, z)] = value;
                                }
                            }
                        }
                    }
                    Axis::Z => {
                        for z in 0..vol.size_z {
                            let value = compute(vol.block_z(z));
                            let base = vol.index_unchecked(0, 0, z);
                            let span = (vol.size_x * vol.size_y) as usize;
                            for i in 0..span {
                                out[base + i] = value;
                            }
                        }
                    }
                }
            }
            DfSam263::Interpolated {
                a,
                cell_xz,
                cell_y,
                inv_xz,
                inv_y,
            } => interpolated_volume(a, ctx, out, vol, *cell_xz, *cell_y, *inv_xz, *inv_y),
            DfSam263::FindTopSurface {
                density,
                upper_bound,
                lower_bound,
                cell_height,
            } => {
                // compileSampler wraps this sampler in YSampler(0): evaluate at y=0
                // slice and broadcast across every Y row of the requested volume.
                let slice = DensityVolume {
                    size_x: vol.size_x,
                    size_y: 1,
                    size_z: vol.size_z,
                    min_x: vol.min_x,
                    min_y: 0,
                    min_z: vol.min_z,
                    step_x: vol.step_x,
                    step_y: 1,
                    step_z: vol.step_z,
                };
                let mut sb = ctx.scratch.acquire(slice.size());
                upper_bound.sample_volume(ctx, &mut sb, &slice);
                let mut index = 0usize;
                for z in 0..slice.size_z {
                    for x in 0..slice.size_x {
                        let ub = sb[index];
                        sb[index] = find_surface_from(
                            density, ctx, slice.block_x(x), slice.block_z(z), ub, *lower_bound, *cell_height,
                        ) as f32;
                        index += 1;
                    }
                }
                // broadcast rows (y fastest)
                let mut i = 0usize;
                for _z in 0..vol.size_z {
                    for _x in 0..vol.size_x {
                        let value = sb[vol.size_x as usize * _z as usize + _x as usize];
                        for _y in 0..vol.size_y {
                            out[i] = value;
                            i += 1;
                        }
                    }
                }
                ctx.scratch.release(sb);
            }
            DfSam263::BlendDensity { a } => a.sample_volume(ctx, out, vol),
            DfSam263::ShiftB { noise } => {
                // Transposed volume: thin (sizeZ, sizeX, 1) buffer at (minZ, minX, 0).
                let transposed = DensityVolume {
                    size_x: vol.size_z,
                    size_y: vol.size_x,
                    size_z: 1,
                    min_x: vol.min_z,
                    min_y: vol.min_x,
                    min_z: 0,
                    step_x: vol.step_z,
                    step_y: vol.step_x,
                    step_z: 1,
                };
                let mut tb = ctx.scratch.acquire(transposed.size());
                tb.fill(0.0);
                let mut index = 0usize;
                for z in 0..transposed.size_z {
                    for x in 0..transposed.size_x {
                        for y in 0..transposed.size_y {
                            let bx = transposed.block_x(x) as f64 * 0.25;
                            let by = transposed.block_y(y) as f64 * 0.25;
                            let bz = transposed.block_z(z) as f64;
                            tb[index] += 4.0f32 * noise.get(bx, by, bz);
                            index += 1;
                        }
                    }
                }
                for z in 0..vol.size_z {
                    for x in 0..vol.size_x {
                        let value = tb[transposed.index_unchecked(z, x, 0)];
                        let base = vol.index_unchecked(x, 0, z);
                        for yy in 0..vol.size_y {
                            out[base + yy as usize] = value;
                        }
                    }
                }
                ctx.scratch.release(tb);
            }
            DfSam263::Blended {
                min_stack,
                max_stack,
                main_stack,
                xz_multiplier,
                y_multiplier,
                main_xz_div,
                main_y_div,
            } => {
                // 26.3 compiled BlendedNoise = NoiseFunction.Samplers: the z
                // coordinate is scaled by xzMultiplier (NOT bare z). Scalar
                // loop: the batch kernel measured slower (lookup-bound).
                let mut index = 0usize;
                let bxz = xz_multiplier;
                let bym = y_multiplier;
                let mdiv = main_xz_div;
                let mydiv = main_y_div;
                for z in 0..vol.size_z {
                    let bz = vol.block_z(z) as f64 * bxz;
                    for x in 0..vol.size_x {
                        let bx = vol.block_x(x) as f64 * bxz;
                        let bxm = bx / mdiv;
                        let bzm = bz / mdiv;
                        for y in 0..vol.size_y {
                            let by = vol.block_y(y);
                            let yv = by as f64 * bym;
                            let ymv = yv / mydiv;
                            let min = min_stack.get(bx, yv, bz);
                            let max = max_stack.get(bx, yv, bz);
                            let main = main_stack.get(bxm, ymv, bzm);
                            let choice = mth_clamp(main + 0.5, 0.0, 1.0);
                            out[index] = if choice == 0.0 {
                                min
                            } else if choice == 1.0 {
                                max
                            } else {
                                mth_lerp(choice, min, max)
                            };
                            index += 1;
                        }
                    }
                }
            }
            DfSam263::Spline {
                coord,
                locations,
                derivatives,
                values,
            } => {
                // BufferSplineInput: slot 0 = root coordinate, DFS pre-order slots.
                let mut coords: Vec<SamRef> = Vec::new();
                coords.push(coord.clone());
                for v in values {
                    v.collect_coords(&mut coords);
                }
                let mut coord_bufs: Vec<Vec<f32>> = Vec::with_capacity(coords.len());
                for c in &coords {
                    let mut b = ctx.scratch.acquire(vol.size());
                    c.sample_volume(ctx, &mut b, vol);
                    coord_bufs.push(b);
                }
                if std::env::var("CRUS_SPLINE_DEBUG").is_ok() {
                    let mut maxslot = 0usize;
                    fn maxs(v: &SplineRef, m: &mut usize) {
                        if let SplineRef::Multi { coord_slot, values: nested, .. } = v {
                            *m = (*m).max(*coord_slot);
                            for n in nested { maxs(n, m); }
                        }
                    }
                    for v in values { maxs(v, &mut maxslot); }
                    eprintln!(
                        "[spline-debug] node={:p} coords={} maxslot={} vol=({},{},{})",
                        locations.as_ptr(), coords.len(), maxslot, vol.size_x, vol.size_y, vol.size_z
                    );
                    if coords.len() <= maxslot {
                        eprintln!("[spline-debug] *** BROKEN NODE TREE ***");
                        fn dumpv(v: &SplineRef, d: usize, out: &mut Vec<String>) {
                            match v {
                                SplineRef::Constant(_) => out.push(format!("{}const", "  ".repeat(d))),
                                SplineRef::Multi { coord_slot, values: nested, .. } => {
                                    out.push(format!("{}multi slot={coord_slot}", "  ".repeat(d)));
                                    for n in nested { dumpv(n, d + 1, out); }
                                }
                            }
                        }
                        let mut lines = Vec::new();
                        for v in values { dumpv(v, 1, &mut lines); }
                        for l in lines.iter().take(70) { eprintln!("[spline-debug] {}", l); }
                    }
                }
                for i in 0..out.len() {
                    let input = coord_bufs[0][i];
                    out[i] = spline_sample_vol(locations, derivatives, values, &coord_bufs, i, input);
                }
                for b in coord_bufs {
                    ctx.scratch.release(b);
                }
            }
        }
    }

    pub fn sample_value(&self, ctx: &mut SamplerCtx263, x: i32, y: i32, z: i32) -> f32 {
        match self {
            DfSam263::Constant(c) => *c,
            DfSam263::Beard => ctx.beard.sample_value(x, y, z),
            DfSam263::Noise {
                noise,
                xz_scale,
                y_scale,
            } => noise.get(x as f64 * xz_scale, y as f64 * y_scale, z as f64 * xz_scale),
            DfSam263::ShiftedXz {
                shift_x,
                shift_z,
                noise,
                xz_scale,
                y_scale,
            } => {
                let nx = x as f64 * xz_scale + shift_x.sample_value(ctx, x, y, z) as f64;
                let ny = y as f64 * y_scale;
                let nz = z as f64 * xz_scale + shift_z.sample_value(ctx, x, y, z) as f64;
                noise.get(nx, ny, nz)
            }
            DfSam263::Binary { op, l, r, r_bound } => {
                let lv = l.sample_value(ctx, x, y, z);
                match op {
                    BinOp::Add => lv + r.sample_value(ctx, x, y, z),
                    BinOp::Sub => lv - r.sample_value(ctx, x, y, z),
                    BinOp::Mul => {
                        if lv == 0.0 {
                            0.0
                        } else {
                            lv * r.sample_value(ctx, x, y, z)
                        }
                    }
                    BinOp::Div => {
                        if lv == 0.0 {
                            0.0
                        } else {
                            lv / r.sample_value(ctx, x, y, z)
                        }
                    }
                    BinOp::Min => {
                        if lv <= *r_bound {
                            lv
                        } else {
                            mth_f32_min(lv, r.sample_value(ctx, x, y, z))
                        }
                    }
                    BinOp::Max => {
                        if lv >= *r_bound {
                            lv
                        } else {
                            mth_f32_max(lv, r.sample_value(ctx, x, y, z))
                        }
                    }
                }
            }
            DfSam263::ConstAdd { a, c } => a.sample_value(ctx, x, y, z) + *c,
            DfSam263::ConstSub { c, a } => *c - a.sample_value(ctx, x, y, z),
            DfSam263::ConstMul { a, c } => a.sample_value(ctx, x, y, z) * *c,
            DfSam263::ConstDiv { c, a } => *c / a.sample_value(ctx, x, y, z),
            DfSam263::ConstMin { a, c } => mth_f32_min(a.sample_value(ctx, x, y, z), *c),
            DfSam263::ConstMax { a, c } => mth_f32_max(a.sample_value(ctx, x, y, z), *c),
            DfSam263::Unary { op, a } => apply_unary(*op, a.sample_value(ctx, x, y, z)),
            DfSam263::Clamp { a, min, max } => mth_clamp(a.sample_value(ctx, x, y, z), *min, *max),
            DfSam263::Lerp {
                alpha,
                first,
                second,
            } => {
                let a = alpha.sample_value(ctx, x, y, z);
                if a == 0.0 {
                    first.sample_value(ctx, x, y, z)
                } else if a == 1.0 {
                    second.sample_value(ctx, x, y, z)
                } else {
                    let f = first.sample_value(ctx, x, y, z);
                    let s = second.sample_value(ctx, x, y, z);
                    mth_lerp(a, f, s)
                }
            }
            DfSam263::LerpConstFirst {
                alpha,
                first,
                second,
            } => {
                let a = alpha.sample_value(ctx, x, y, z);
                if a == 0.0 {
                    *first
                } else if a == 1.0 {
                    second.sample_value(ctx, x, y, z)
                } else {
                    mth_lerp(a, *first, second.sample_value(ctx, x, y, z))
                }
            }
            DfSam263::LerpConstSecond {
                alpha,
                first,
                second,
            } => {
                let a = alpha.sample_value(ctx, x, y, z);
                if a == 0.0 {
                    first.sample_value(ctx, x, y, z)
                } else if a == 1.0 {
                    *second
                } else {
                    mth_lerp(a, first.sample_value(ctx, x, y, z), *second)
                }
            }
            DfSam263::PowConstBase { base, exp } => (*base).powf(exp.sample_value(ctx, x, y, z) as f64) as f32,
            DfSam263::PowConstExp { a, exp } => (a.sample_value(ctx, x, y, z) as f64).powf(*exp) as f32,
            DfSam263::Pow { a, b } => (a.sample_value(ctx, x, y, z) as f64)
                .powf(b.sample_value(ctx, x, y, z) as f64) as f32,
            DfSam263::RoundInt { ty, a } => apply_round(*ty, a.sample_value(ctx, x, y, z)),
            DfSam263::Round { ty, a, mul } => {
                let input = a.sample_value(ctx, x, y, z);
                let m = mul.sample_value(ctx, x, y, z);
                if m == 0.0 {
                    input
                } else {
                    apply_round(*ty, input / m) * m
                }
            }
            DfSam263::RangeChoice {
                a,
                min,
                max,
                in_range,
                out_of_range,
            } => {
                let v = a.sample_value(ctx, x, y, z);
                if v >= *min && v < *max {
                    in_range.sample_value(ctx, x, y, z)
                } else {
                    out_of_range.sample_value(ctx, x, y, z)
                }
            }
            DfSam263::RangeChoiceConst {
                a,
                min,
                max,
                in_v,
                out_v,
            } => {
                let v = a.sample_value(ctx, x, y, z);
                if v >= *min && v < *max {
                    *in_v
                } else {
                    *out_v
                }
            }
            DfSam263::IntervalSelect {
                a,
                thresholds,
                branches,
            } => {
                let input = a.sample_value(ctx, x, y, z);
                let mut idx = branches.len() - 1;
                for (ti, t) in thresholds.iter().enumerate() {
                    if input < *t {
                        idx = ti;
                        break;
                    }
                }
                branches[idx].sample_value(ctx, x, y, z)
            }
            DfSam263::Cache { id, a } => ctx.sample_value_cached(*id, a, x, y, z),
            DfSam263::SliceX { a, x: sx } => a.sample_value(ctx, *sx, y, z),
            DfSam263::SliceY { a, y: sy } => a.sample_value(ctx, x, *sy, z),
            DfSam263::SliceZ { a, z: sz } => a.sample_value(ctx, x, y, *sz),
            DfSam263::SliceXz { a, x: sx, z: sz } => a.sample_value(ctx, *sx, y, *sz),
            DfSam263::Gradient {
                axis,
                tiling,
                from_coord,
                aux,
                from_value,
                factor,
            } => {
                let coordinate = match axis {
                    Axis::X => x,
                    Axis::Y => y,
                    Axis::Z => z,
                };
                compute_gradient(*tiling, coordinate, *from_coord, *aux, *from_value, *factor)
            }
            DfSam263::Interpolated {
                a,
                cell_xz,
                cell_y,
                inv_xz: _,
                inv_y: _,
            } => interpolated_point(a, ctx, x, y, z, *cell_xz, *cell_y),
            DfSam263::FindTopSurface {
                density,
                upper_bound,
                lower_bound,
                cell_height,
            } => {
                let ub = upper_bound.sample_value(ctx, x, 0, z);
                find_surface_from(density, ctx, x, z, ub, *lower_bound, *cell_height) as f32
            }
            DfSam263::BlendDensity { a } => a.sample_value(ctx, x, y, z),
            DfSam263::ShiftB { noise } => {
                noise.get(z as f64 * 0.25, x as f64 * 0.25, 0.0) * 4.0
            }
            DfSam263::Blended {
                min_stack,
                max_stack,
                main_stack,
                xz_multiplier,
                y_multiplier,
                main_xz_div,
                main_y_div,
            } => {
                let min = min_stack.get(x as f64 * xz_multiplier, y as f64 * y_multiplier, z as f64 * xz_multiplier);
                let max = max_stack.get(x as f64 * xz_multiplier, y as f64 * y_multiplier, z as f64 * xz_multiplier);
                let main = main_stack.get(
                    x as f64 * xz_multiplier / main_xz_div,
                    y as f64 * y_multiplier / main_y_div,
                    z as f64 * xz_multiplier / main_xz_div,
                );
                let choice = mth_clamp(main + 0.5, 0.0, 1.0);
                if choice == 0.0 {
                    min
                } else if choice == 1.0 {
                    max
                } else {
                    mth_lerp(choice, min, max)
                }
            }
            DfSam263::Spline {
                coord,
                locations,
                derivatives,
                values,
            } => {
                let input = coord.sample_value(ctx, x, y, z);
                let mut point_cache: Vec<Option<f32>> = Vec::new();
                point_cache.push(Some(input));
                spline_sample_point(locations, derivatives, values, ctx, x, y, z, &mut point_cache, input)
            }
        }
    }
}

#[inline]
fn mth_f32_min(a: f32, b: f32) -> f32 {
    if a < b {
        a
    } else {
        b
    }
}

#[inline]
fn mth_f32_max(a: f32, b: f32) -> f32 {
    if a > b {
        a
    } else {
        b
    }
}

fn apply_unary(op: UnOp, v: f32) -> f32 {
    match op {
        UnOp::Abs => v.abs(),
        UnOp::Square => v * v,
        UnOp::Cube => v * v * v,
        UnOp::Sqrt => (v as f64).sqrt() as f32,
        UnOp::HalfNegative => {
            if v > 0.0 {
                v
            } else {
                v * 0.5
            }
        }
        UnOp::QuarterNegative => {
            if v > 0.0 {
                v
            } else {
                v * 0.25
            }
        }
        UnOp::Reciprocal => 1.0 / v,
        UnOp::Negate => -v,
        UnOp::Squeeze => {
            let clamped = mth_clamp(v, -1.0, 1.0);
            clamped / 2.0 - (clamped * clamped * clamped) / 24.0
        }
        UnOp::Log => (v as f64).ln() as f32,
        UnOp::Sign => {
            if v.is_nan() {
                f32::NAN
            } else if v == 0.0 {
                0.0
            } else if v > 0.0 {
                1.0
            } else {
                -1.0
            }
        }
    }
}

fn apply_round(ty: RoundType, v: f32) -> f32 {
    match ty {
        RoundType::Floor => (v as f64).floor() as f32,
        RoundType::Round => (v as f64).round() as f32, // Java Math.round(float): floor(v+0.5) via int; f64 round equals for in-range values
        RoundType::Ceil => (v as f64).ceil() as f32,
        RoundType::Truncate => {
            if v > 0.0 {
                (v as f64).floor() as f32
            } else {
                (v as f64).ceil() as f32
            }
        }
    }
}

fn compute_gradient(tiling: Tiling, coordinate: i32, from_coord: i32, aux: i32, from_value: f32, factor: f32) -> f32 {
    match tiling {
        Tiling::ClampToEdge => {
            let clamped = coordinate.clamp(from_coord.min(aux), from_coord.max(aux));
            from_value + (clamped - from_coord) as f32 * factor
        }
        Tiling::Repeat => from_value + floor_mod(coordinate - from_coord, aux) as f32 * factor,
        Tiling::MirroredRepeat => {
            let rel = coordinate - from_coord;
            let tile_index = floor_div(rel, aux);
            let local = rel - tile_index * aux;
            if tile_index & 1 == 0 {
                from_value + local as f32 * factor
            } else {
                from_value + (aux - local) as f32 * factor
            }
        }
    }
}

/// SliceFunction volume broadcast (X/Z thin-slice eval + broadcast; Y setRange).
fn slice_volume_axis(
    a: &DfSam263,
    ctx: &mut SamplerCtx263,
    out: &mut Vec<f32>,
    vol: &DensityVolume,
    axis: Axis,
    coord: i32,
) {
    match axis {
        Axis::X => {
            if vol.size_x == 1 && vol.min_x == coord {
                a.sample_volume(ctx, out, vol);
            } else {
                let input_volume = DensityVolume {
                    size_x: 1,
                    size_y: vol.size_y,
                    size_z: vol.size_z,
                    min_x: coord,
                    min_y: vol.min_y,
                    min_z: vol.min_z,
                    step_x: vol.step_x,
                    step_y: vol.step_y,
                    step_z: vol.step_z,
                };
                let mut ib = ctx.scratch.acquire(input_volume.size());
                a.sample_volume(ctx, &mut ib, &input_volume);
                let mut index = 0usize;
                for z in 0..vol.size_z {
                    for _x in 0..vol.size_x {
                        for y in 0..vol.size_y {
                            out[index] = ib[input_volume.index_unchecked(0, y, z)];
                            index += 1;
                        }
                    }
                }
                ctx.scratch.release(ib);
            }
        }
        Axis::Z => {
            if vol.size_z == 1 && vol.min_z == coord {
                a.sample_volume(ctx, out, vol);
            } else {
                let input_volume = DensityVolume {
                    size_x: vol.size_x,
                    size_y: vol.size_y,
                    size_z: 1,
                    min_x: vol.min_x,
                    min_y: vol.min_y,
                    min_z: coord,
                    step_x: vol.step_x,
                    step_y: vol.step_y,
                    step_z: vol.step_z,
                };
                let mut ib = ctx.scratch.acquire(input_volume.size());
                a.sample_volume(ctx, &mut ib, &input_volume);
                let mut index = 0usize;
                for _z in 0..vol.size_z {
                    for x in 0..vol.size_x {
                        for y in 0..vol.size_y {
                            out[index] = ib[input_volume.index_unchecked(x, y, 0)];
                            index += 1;
                        }
                    }
                }
                ctx.scratch.release(ib);
            }
        }
        Axis::Y => {
            if vol.size_y == 1 && vol.min_y == coord {
                a.sample_volume(ctx, out, vol);
            } else {
                let input_volume = DensityVolume {
                    size_x: vol.size_x,
                    size_y: 1,
                    size_z: vol.size_z,
                    min_x: vol.min_x,
                    min_y: coord,
                    min_z: vol.min_z,
                    step_x: vol.step_x,
                    step_y: vol.step_y,
                    step_z: vol.step_z,
                };
                let mut ib = ctx.scratch.acquire(input_volume.size());
                a.sample_volume(ctx, &mut ib, &input_volume);
                for z in 0..vol.size_z {
                    for x in 0..vol.size_x {
                        let input = ib[input_volume.index_unchecked(x, 0, z)];
                        let base = vol.index_unchecked(x, 0, z);
                        for yy in 0..vol.size_y {
                            out[base + yy as usize] = input;
                        }
                    }
                }
                ctx.scratch.release(ib);
            }
        }
    }
}

/// InterpolatedFunction.sampleWithBlockStep + fillCell (26.3, exact).
fn interpolated_volume(
    a: &DfSam263,
    ctx: &mut SamplerCtx263,
    out: &mut Vec<f32>,
    vol: &DensityVolume,
    cell_xz: i32,
    cell_y: i32,
    inv_xz: f32,
    inv_y: f32,
) {
    // Fastpath: aligned volume at cell step -> pass through.
    if (vol.step_x == cell_xz || vol.size_x == 1)
        && (vol.step_y == cell_y || vol.size_y == 1)
        && (vol.step_z == cell_xz || vol.size_z == 1)
        && floor_mod(vol.min_x, cell_xz) == 0
        && floor_mod(vol.min_y, cell_y) == 0
        && floor_mod(vol.min_z, cell_xz) == 0
    {
        return a.sample_volume(ctx, out, vol);
    }
    debug_assert_eq!(vol.step_x, 1);
    debug_assert_eq!(vol.step_y, 1);
    debug_assert_eq!(vol.step_z, 1);
    let min_cell_x = floor_div(vol.min_x, cell_xz);
    let min_cell_y = floor_div(vol.min_y, cell_y);
    let min_cell_z = floor_div(vol.min_z, cell_xz);
    let max_cell_x = floor_div(vol.max_block_x(), cell_xz);
    let max_cell_y = floor_div(vol.max_block_y(), cell_y);
    let max_cell_z = floor_div(vol.max_block_z(), cell_xz);
    let cell_count_x = max_cell_x - min_cell_x + 1;
    let cell_count_y = max_cell_y - min_cell_y + 1;
    let cell_count_z = max_cell_z - min_cell_z + 1;
    let cell_volume = DensityVolume {
        size_x: if floor_mod(vol.max_block_x(), cell_xz) == 0 {
            cell_count_x
        } else {
            cell_count_x + 1
        },
        size_y: if floor_mod(vol.max_block_y(), cell_y) == 0 {
            cell_count_y
        } else {
            cell_count_y + 1
        },
        size_z: if floor_mod(vol.max_block_z(), cell_xz) == 0 {
            cell_count_z
        } else {
            cell_count_z + 1
        },
        min_x: min_cell_x * cell_xz,
        min_y: min_cell_y * cell_y,
        min_z: min_cell_z * cell_xz,
        step_x: cell_xz,
        step_y: cell_y,
        step_z: cell_xz,
    };

    let mut cell_buf = ctx.scratch.acquire(cell_volume.size());
    let prof_t = if PROF_ON_263.load(std::sync::atomic::Ordering::Relaxed) {
        prof_now()
    } else {
        0
    };
    a.sample_volume(ctx, &mut cell_buf, &cell_volume);
    if prof_t != 0 {
        PROF_INTERP_CELL.fetch_add(prof_now() - prof_t, std::sync::atomic::Ordering::Relaxed);
    }
    let prof_t2 = if PROF_ON_263.load(std::sync::atomic::Ordering::Relaxed) {
        prof_now()
    } else {
        0
    };

    let fill_done = std::cell::Cell::new(false);
    for cell_z in 0..cell_count_z {
        let next_cell_z = (cell_z + 1).min(cell_volume.size_z - 1);
        for cell_x in 0..cell_count_x {
            let next_cell_x = (cell_x + 1).min(cell_volume.size_x - 1);
            let mut v000 = cell_buf[cell_volume.index_unchecked(cell_x, 0, cell_z)];
            let mut v100 = cell_buf[cell_volume.index_unchecked(next_cell_x, 0, cell_z)];
            let mut v001 = cell_buf[cell_volume.index_unchecked(cell_x, 0, next_cell_z)];
            let mut v101 = cell_buf[cell_volume.index_unchecked(next_cell_x, 0, next_cell_z)];
            for cell_yy in 0..cell_count_y {
                let next_cell_y = (cell_yy + 1).min(cell_volume.size_y - 1);
                let v010 = cell_buf[cell_volume.index_unchecked(cell_x, next_cell_y, cell_z)];
                let v110 = cell_buf[cell_volume.index_unchecked(next_cell_x, next_cell_y, cell_z)];
                let v011 = cell_buf[cell_volume.index_unchecked(cell_x, next_cell_y, next_cell_z)];
                let v111 = cell_buf[cell_volume.index_unchecked(next_cell_x, next_cell_y, next_cell_z)];
                fill_cell(
                    out, vol, &cell_volume, cell_x, cell_yy, cell_z, v000, v100, v010, v110, v001, v101, v011, v111,
                    cell_xz, cell_y, inv_xz, inv_y,
                );
                v000 = v010;
                v100 = v110;
                v001 = v011;
                v101 = v111;
            }
        }
    }
    if prof_t2 != 0 {
        PROF_INTERP_FILL.fetch_add(prof_now() - prof_t2, std::sync::atomic::Ordering::Relaxed);
    }
    ctx.scratch.release(cell_buf);
}

#[allow(clippy::too_many_arguments)]
fn fill_cell(
    out: &mut Vec<f32>,
    out_vol: &DensityVolume,
    cell_volume: &DensityVolume,
    cell_x: i32,
    cell_y: i32,
    cell_z: i32,
    v000: f32,
    v100: f32,
    v010: f32,
    v110: f32,
    v001: f32,
    v101: f32,
    v011: f32,
    v111: f32,
    cell_xz: i32,
    cell_size_y: i32,
    inv_xz: f32,
    inv_y: f32,
) {
    let cell_output_x = cell_volume.block_x(cell_x) - out_vol.min_x;
    let cell_output_y = cell_volume.block_y(cell_y) - out_vol.min_y;
    let cell_output_z = cell_volume.block_z(cell_z) - out_vol.min_z;
    let x0 = (-cell_output_x).max(0);
    let y0 = (-cell_output_y).max(0);
    let z0 = (-cell_output_z).max(0);
    let x1 = (cell_xz).min(out_vol.size_x - cell_output_x) - 1;
    let y1 = (cell_size_y).min(out_vol.size_y - cell_output_y) - 1;
    let z1 = (cell_xz).min(out_vol.size_z - cell_output_z) - 1;

    for z in z0..=z1 {
        let output_z = cell_output_z + z;
        let alpha_z = z as f32 * inv_xz;
        let v00_ = mth_lerp(alpha_z, v000, v001);
        let v01_ = mth_lerp(alpha_z, v010, v011);
        let v10_ = mth_lerp(alpha_z, v100, v101);
        let v11_ = mth_lerp(alpha_z, v110, v111);
        for x in x0..=x1 {
            let output_x = cell_output_x + x;
            let alpha_x = x as f32 * inv_xz;
            let v_0_ = mth_lerp(alpha_x, v00_, v10_);
            let v_1_ = mth_lerp(alpha_x, v01_, v11_);
            let value_step = (v_1_ - v_0_) * inv_y;
            let mut value = v_0_ + value_step * y0 as f32;
            let mut output_index = out_vol.index_unchecked(output_x, cell_output_y + y0, output_z);
            for _y in y0..=y1 {
                out[output_index] = value;
                output_index += 1;
                value += value_step;
            }
        }
    }
}

/// InterpolatedFunction.sampleValue (point path).
fn interpolated_point(a: &DfSam263, ctx: &mut SamplerCtx263, x: i32, y: i32, z: i32, cell_xz: i32, cell_y: i32) -> f32 {
    let x_in_cell = floor_mod(x, cell_xz);
    let y_in_cell = floor_mod(y, cell_y);
    let z_in_cell = floor_mod(z, cell_xz);
    if x_in_cell == 0 && y_in_cell == 0 && z_in_cell == 0 {
        return a.sample_value(ctx, x, y, z);
    }
    // The 8 cell-corner values MUST come from the inner sampler's POINT
    // path: Java's interpolated point sampler evaluates the inner function
    // at the corners directly (DensitySampler.sampleValue). A 2x2x2 volume
    // pass would run the inner's VOLUME path — for FindTopSurface that path
    // is per-(x,z)-block over a sizeY==1 volume (Java throws on sizeY != 1)
    // and yields different numbers. chunk_surface_level = interpolated(
    // FindTopSurface, 16, 1) makes this exact distinction observable.
    let (x0, y0, z0) = (x - x_in_cell, y - y_in_cell, z - z_in_cell);
    let (x1, y1, z1) = (x0 + cell_xz, y0 + cell_y, z0 + cell_xz);
    let v000 = a.sample_value(ctx, x0, y0, z0);
    let v100 = a.sample_value(ctx, x1, y0, z0);
    let v010 = a.sample_value(ctx, x0, y1, z0);
    let v110 = a.sample_value(ctx, x1, y1, z0);
    let v001 = a.sample_value(ctx, x0, y0, z1);
    let v101 = a.sample_value(ctx, x1, y0, z1);
    let v011 = a.sample_value(ctx, x0, y1, z1);
    let v111 = a.sample_value(ctx, x1, y1, z1);
    n263::lerp3_f32(
        x_in_cell as f32 / cell_xz as f32,
        y_in_cell as f32 / cell_y as f32,
        z_in_cell as f32 / cell_xz as f32,
        v000,
        v100,
        v010,
        v110,
        v001,
        v101,
        v011,
        v111,
    )
}

fn find_surface_from(
    density: &DfSam263,
    ctx: &mut SamplerCtx263,
    x: i32,
    z: i32,
    upper_bound: f32,
    lower_bound: i32,
    cell_height: i32,
) -> i32 {
    let top_y = mth_floor_f32(upper_bound / cell_height as f32) * cell_height;
    if top_y <= lower_bound {
        return lower_bound;
    }
    let mut probe_y = top_y;
    while probe_y >= lower_bound {
        if density.sample_value(ctx, x, probe_y, z) > 0.0 {
            return probe_y;
        }
        probe_y -= cell_height;
    }
    lower_bound
}

// ---- spline plumbing ----

#[inline]
fn spline_eval_vol(
    locations: &[f32],
    derivatives: &[f32],
    values: &[SplineRef],
    coord_bufs: &[Vec<f32>],
    i: usize,
    input: f32,
) -> f32 {
    let start = find_interval_start(locations, input);
    let last_index = locations.len() as i32 - 1;
    if start < 0 {
        let value = spline_value_vol(values, 0, coord_bufs, i);
        return linear_extend(input, locations, value, derivatives, 0);
    } else if start == last_index {
        let value = spline_value_vol(values, last_index as usize, coord_bufs, i);
        return linear_extend(input, locations, value, derivatives, last_index as usize);
    }
    let x1 = locations[start as usize];
    let x2 = locations[start as usize + 1];
    let t = (input - x1) / (x2 - x1);
    let y1 = spline_value_vol(values, start as usize, coord_bufs, i);
    let y2 = spline_value_vol(values, start as usize + 1, coord_bufs, i);
    let d1 = derivatives[start as usize];
    let d2 = derivatives[start as usize + 1];
    let aa = d1 * (x2 - x1) - (y2 - y1);
    let b = -d2 * (x2 - x1) + (y2 - y1);
    mth_lerp(t, y1, y2) + t * (1.0 - t) * mth_lerp(t, aa, b)
}

fn spline_value_vol(values: &[SplineRef], index: usize, coord_bufs: &[Vec<f32>], i: usize) -> f32 {
    match &values[index] {
        SplineRef::Constant(v) => *v,
        SplineRef::Multi {
            coord_slot,
            locations,
            derivatives,
            values: nested,
            ..
        } => {
            let input = coord_bufs[*coord_slot][i];
            spline_eval_vol(locations, derivatives, nested, coord_bufs, i, input)
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn spline_sample_vol(
    locations: &[f32],
    derivatives: &[f32],
    values: &[SplineRef],
    coord_bufs: &[Vec<f32>],
    i: usize,
    input: f32,
) -> f32 {
    spline_eval_vol(locations, derivatives, values, coord_bufs, i, input)
}

fn linear_extend(input: f32, locations: &[f32], value: f32, derivatives: &[f32], index: usize) -> f32 {
    let derivative = derivatives[index];
    if derivative == 0.0 {
        value
    } else {
        value + derivative * (input - locations[index])
    }
}

fn find_interval_start(locations: &[f32], input: f32) -> i32 {
    // Mth.binarySearch(0, len, i -> input < locations[i]) - 1
    let mut lo = 0i32;
    let mut hi = locations.len() as i32;
    while lo < hi {
        let mid = (lo + hi) / 2;
        if input < locations[mid as usize] {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    lo - 1
}

/// Point path for splines: per-point coordinate cache (PointSplineInput).
fn spline_sample_point(
    locations: &[f32],
    derivatives: &[f32],
    values: &[SplineRef],
    ctx: &mut SamplerCtx263,
    x: i32,
    y: i32,
    z: i32,
    cache: &mut Vec<Option<f32>>,
    input: f32,
) -> f32 {
    let start = find_interval_start(locations, input);
    let last_index = locations.len() as i32 - 1;
    if start < 0 {
        let value = spline_value_point(values, 0, ctx, x, y, z, cache);
        return linear_extend(input, locations, value, derivatives, 0);
    } else if start == last_index {
        let value = spline_value_point(values, last_index as usize, ctx, x, y, z, cache);
        return linear_extend(input, locations, value, derivatives, last_index as usize);
    }
    let x1 = locations[start as usize];
    let x2 = locations[start as usize + 1];
    let t = (input - x1) / (x2 - x1);
    let y1 = spline_value_point(values, start as usize, ctx, x, y, z, cache);
    let y2 = spline_value_point(values, start as usize + 1, ctx, x, y, z, cache);
    let d1 = derivatives[start as usize];
    let d2 = derivatives[start as usize + 1];
    let aa = d1 * (x2 - x1) - (y2 - y1);
    let b = -d2 * (x2 - x1) + (y2 - y1);
    mth_lerp(t, y1, y2) + t * (1.0 - t) * mth_lerp(t, aa, b)
}

fn spline_value_point(
    values: &[SplineRef],
    index: usize,
    ctx: &mut SamplerCtx263,
    x: i32,
    y: i32,
    z: i32,
    cache: &mut Vec<Option<f32>>,
) -> f32 {
    match &values[index] {
        SplineRef::Constant(v) => *v,
        SplineRef::Multi {
            coord_slot,
            coord,
            locations,
            derivatives,
            values: nested,
        } => {
            let slot = *coord_slot;
            while cache.len() <= slot {
                cache.push(None);
            }
            if let Some(v) = cache[slot] {
                return v;
            }
            let input = coord.sample_value(ctx, x, y, z);
            cache[slot] = Some(input);
            spline_sample_point(locations, derivatives, nested, ctx, x, y, z, cache, input)
        }
    }
}


// ---------------------------------------------------------------------------
// Pointwise fusion: flat stack program for index-independent subtrees.
//
// The tree-walk sample_volume materializes a full-volume buffer at EVERY
// pointwise node (binary ops, const ops, unary, clamp, lerp, range choice,
// round, interval select) — memory-bound passes over 98k+ f32 per node.
// FuseProg compiles the pointwise spine once, materializes only barrier
// leaves (noise family, interpolated, cache, slices, gradient, spline, ...),
// then runs ONE pass over indices with a register stack. Per-index op order
// is identical to the buffer path (each step a separate f32 op, no FMA
// contraction — Rust never enables fp-contract), so results are bit-exact.
// ---------------------------------------------------------------------------

const FUSE_MAX_STACK: usize = 48;
const FUSE_MAX_BRANCHES: usize = 16;

/// One flat program step. Stack effects: PushConst/Load push 1; the n-ary
/// ops pop their operands in reverse push order and push 1.
#[derive(Clone, Debug)]
pub enum FuseOp {
    PushConst(f32),
    Load(usize),
    Add,
    Sub,
    Mul,
    Div,
    Min,
    Max,
    ConstAdd(f32),
    ConstSub(f32),
    ConstMul(f32),
    ConstDiv(f32),
    ConstMin(f32),
    ConstMax(f32),
    Unary(UnOp),
    Clamp(f32, f32),
    /// pops second, first, alpha (alpha pushed first).
    Lerp,
    /// pops second, alpha (const first).
    LerpConstFirst(f32),
    /// pops first, alpha (const second).
    LerpConstSecond(f32),
    PowConstBase(f64),
    PowConstExp(f64),
    Pow,
    RoundInt(RoundType),
    /// pops mul, a (a pushed first): if mul==0 { a } else { round(a/mul)*mul }.
    Round(RoundType),
    /// pops out_of_range, in_range, a (a pushed first).
    RangeChoice(f32, f32),
    RangeChoiceConst { min: f32, max: f32, in_v: f32, out_v: f32 },
    /// pops branches in reverse push order, then a (a pushed FIRST).
    IntervalSelect(Arc<Vec<f32>>),
}

/// Compiled pointwise spine of a DfSam263 subtree. Barrier leaves are kept
/// as shared SamRefs and materialized into slot buffers once per volume.
pub struct FuseProg {
    pub ops: Vec<FuseOp>,
    pub slots: Vec<SamRef>,
}

impl FuseProg {
    /// Compile a subtree into a flat program. Returns None when the subtree
    /// exceeds the fixed stack/branch budget (caller falls back to the walk).
    pub fn compile(root: &SamRef) -> Option<FuseProg> {
        let mut ops = Vec::new();
        let mut slots: Vec<SamRef> = Vec::new();
        let mut depth = 0usize;
        Self::compile_into(root, &mut ops, &mut slots, &mut depth)?;
        Some(FuseProg { ops, slots })
    }

    fn compile_into(
        node: &SamRef,
        ops: &mut Vec<FuseOp>,
        slots: &mut Vec<SamRef>,
        depth: &mut usize,
    ) -> Option<()> {
        // +1 per stack push; bail out if the fixed budget would overflow.
        macro_rules! push {
            ($op:expr) => {{
                *depth += 1;
                if *depth > FUSE_MAX_STACK {
                    return None;
                }
                ops.push($op);
            }};
        }
        // pop `$pop` operands (net after the op's own push of 1).
        macro_rules! pop_net {
            ($op:expr, $pop:expr) => {{
                *depth = depth.saturating_sub($pop);
                ops.push($op);
            }};
        }
        match &**node {
            DfSam263::Constant(c) => push!(FuseOp::PushConst(*c)),
            DfSam263::Binary { op, l, r, .. } => {
                Self::compile_into(l, ops, slots, depth)?;
                Self::compile_into(r, ops, slots, depth)?;
                pop_net!(
                    match op {
                        BinOp::Add => FuseOp::Add,
                        BinOp::Sub => FuseOp::Sub,
                        BinOp::Mul => FuseOp::Mul,
                        BinOp::Div => FuseOp::Div,
                        BinOp::Min => FuseOp::Min,
                        BinOp::Max => FuseOp::Max,
                    },
                    1
                );
            }
            DfSam263::ConstAdd { a, c } => {
                Self::compile_into(a, ops, slots, depth)?;
                pop_net!(FuseOp::ConstAdd(*c), 0);
            }
            DfSam263::ConstSub { c, a } => {
                Self::compile_into(a, ops, slots, depth)?;
                pop_net!(FuseOp::ConstSub(*c), 0);
            }
            DfSam263::ConstMul { a, c } => {
                Self::compile_into(a, ops, slots, depth)?;
                pop_net!(FuseOp::ConstMul(*c), 0);
            }
            DfSam263::ConstDiv { c, a } => {
                Self::compile_into(a, ops, slots, depth)?;
                pop_net!(FuseOp::ConstDiv(*c), 0);
            }
            DfSam263::ConstMin { a, c } => {
                Self::compile_into(a, ops, slots, depth)?;
                pop_net!(FuseOp::ConstMin(*c), 0);
            }
            DfSam263::ConstMax { a, c } => {
                Self::compile_into(a, ops, slots, depth)?;
                pop_net!(FuseOp::ConstMax(*c), 0);
            }
            DfSam263::Unary { op, a } => {
                Self::compile_into(a, ops, slots, depth)?;
                pop_net!(FuseOp::Unary(*op), 0);
            }
            DfSam263::Clamp { a, min, max } => {
                Self::compile_into(a, ops, slots, depth)?;
                pop_net!(FuseOp::Clamp(*min, *max), 0);
            }
            DfSam263::Lerp { alpha, first, second } => {
                Self::compile_into(alpha, ops, slots, depth)?;
                Self::compile_into(first, ops, slots, depth)?;
                Self::compile_into(second, ops, slots, depth)?;
                pop_net!(FuseOp::Lerp, 2);
            }
            DfSam263::LerpConstFirst { alpha, first, second } => {
                Self::compile_into(alpha, ops, slots, depth)?;
                Self::compile_into(second, ops, slots, depth)?;
                pop_net!(FuseOp::LerpConstFirst(*first), 1);
            }
            DfSam263::LerpConstSecond { alpha, first, second } => {
                Self::compile_into(alpha, ops, slots, depth)?;
                Self::compile_into(first, ops, slots, depth)?;
                pop_net!(FuseOp::LerpConstSecond(*second), 1);
            }
            DfSam263::PowConstBase { base, exp } => {
                Self::compile_into(exp, ops, slots, depth)?;
                pop_net!(FuseOp::PowConstBase(*base), 0);
            }
            DfSam263::PowConstExp { a, exp } => {
                Self::compile_into(a, ops, slots, depth)?;
                pop_net!(FuseOp::PowConstExp(*exp), 0);
            }
            DfSam263::Pow { a, b } => {
                Self::compile_into(a, ops, slots, depth)?;
                Self::compile_into(b, ops, slots, depth)?;
                pop_net!(FuseOp::Pow, 1);
            }
            DfSam263::RoundInt { ty, a } => {
                Self::compile_into(a, ops, slots, depth)?;
                pop_net!(FuseOp::RoundInt(*ty), 0);
            }
            DfSam263::Round { ty, a, mul } => {
                Self::compile_into(a, ops, slots, depth)?;
                Self::compile_into(mul, ops, slots, depth)?;
                pop_net!(FuseOp::Round(*ty), 1);
            }
            DfSam263::RangeChoice { a, min, max, in_range, out_of_range } => {
                Self::compile_into(a, ops, slots, depth)?;
                Self::compile_into(in_range, ops, slots, depth)?;
                Self::compile_into(out_of_range, ops, slots, depth)?;
                pop_net!(FuseOp::RangeChoice(*min, *max), 2);
            }
            DfSam263::RangeChoiceConst { a, min, max, in_v, out_v } => {
                Self::compile_into(a, ops, slots, depth)?;
                pop_net!(
                    FuseOp::RangeChoiceConst { min: *min, max: *max, in_v: *in_v, out_v: *out_v },
                    0
                );
            }
            DfSam263::IntervalSelect { a, thresholds, branches } => {
                if branches.is_empty() || branches.len() > FUSE_MAX_BRANCHES {
                    return None;
                }
                Self::compile_into(a, ops, slots, depth)?;
                for b in branches {
                    Self::compile_into(b, ops, slots, depth)?;
                }
                // op pops (branches.len()+1), pushes 1 -> net -= branches.len()
                *depth = depth.saturating_sub(branches.len());
                ops.push(FuseOp::IntervalSelect(Arc::new(thresholds.clone())));
            }
            // ---- barriers: materialize into a slot buffer, Load per index ----
            _ => {
                let slot = slots.len();
                slots.push(node.clone());
                push!(FuseOp::Load(slot));
            }
        }
        Some(())
    }

    /// Run the flat program over a volume. Bit-exact vs the tree walk: every
    /// slot is materialized through the unmodified tree path (cache cells,
    /// sub-volumes, ctx state all behave identically), then one pass per
    /// index applies the same f32 op sequence the buffer loops would.
    pub fn run(&self, ctx: &mut SamplerCtx263, out: &mut Vec<f32>, vol: &DensityVolume) {
        let n = vol.size();
        let mut slot_bufs: Vec<Vec<f32>> = Vec::with_capacity(self.slots.len());
        for s in &self.slots {
            let mut b = ctx.scratch.acquire(n);
            s.sample_volume(ctx, &mut b, vol);
            slot_bufs.push(b);
        }
        let slot_ptrs: Vec<*const f32> = slot_bufs.iter().map(|b| b.as_ptr()).collect();
        let mut stack = [0f32; FUSE_MAX_STACK];
        let base = out.as_mut_ptr();
        for i in 0..n {
            let mut sp = 0usize;
            for op in &self.ops {
                match op {
                    FuseOp::PushConst(c) => {
                        stack[sp] = *c;
                        sp += 1;
                    }
                    FuseOp::Load(s) => {
                        stack[sp] = unsafe { *(*slot_ptrs.as_ptr().add(*s)).add(i) };
                        sp += 1;
                    }
                    FuseOp::Add => {
                        sp -= 1;
                        let r = stack[sp];
                        sp -= 1;
                        stack[sp] = stack[sp] + r;
                        sp += 1;
                    }
                    FuseOp::Sub => {
                        sp -= 1;
                        let r = stack[sp];
                        sp -= 1;
                        stack[sp] = stack[sp] - r;
                        sp += 1;
                    }
                    FuseOp::Mul => {
                        sp -= 1;
                        let r = stack[sp];
                        sp -= 1;
                        stack[sp] = stack[sp] * r;
                        sp += 1;
                    }
                    FuseOp::Div => {
                        sp -= 1;
                        let r = stack[sp];
                        sp -= 1;
                        stack[sp] = stack[sp] / r;
                        sp += 1;
                    }
                    FuseOp::Min => {
                        sp -= 1;
                        let r = stack[sp];
                        sp -= 1;
                        let l = stack[sp];
                        stack[sp] = if r < l { r } else { l };
                        sp += 1;
                    }
                    FuseOp::Max => {
                        sp -= 1;
                        let r = stack[sp];
                        sp -= 1;
                        let l = stack[sp];
                        stack[sp] = if r > l { r } else { l };
                        sp += 1;
                    }
                    FuseOp::ConstAdd(c) => {
                        sp -= 1;
                        stack[sp] = stack[sp] + *c;
                        sp += 1;
                    }
                    FuseOp::ConstSub(c) => {
                        sp -= 1;
                        stack[sp] = *c - stack[sp];
                        sp += 1;
                    }
                    FuseOp::ConstMul(c) => {
                        sp -= 1;
                        stack[sp] = stack[sp] * *c;
                        sp += 1;
                    }
                    FuseOp::ConstDiv(c) => {
                        sp -= 1;
                        stack[sp] = *c / stack[sp];
                        sp += 1;
                    }
                    FuseOp::ConstMin(c) => {
                        sp -= 1;
                        let v = stack[sp];
                        stack[sp] = if *c < v { *c } else { v };
                        sp += 1;
                    }
                    FuseOp::ConstMax(c) => {
                        sp -= 1;
                        let v = stack[sp];
                        stack[sp] = if *c > v { *c } else { v };
                        sp += 1;
                    }
                    FuseOp::Unary(u) => {
                        sp -= 1;
                        stack[sp] = apply_unary(*u, stack[sp]);
                        sp += 1;
                    }
                    FuseOp::Clamp(min, max) => {
                        sp -= 1;
                        stack[sp] = mth_clamp(stack[sp], *min, *max);
                        sp += 1;
                    }
                    FuseOp::Lerp => {
                        sp -= 1;
                        let sb = stack[sp];
                        sp -= 1;
                        let fb = stack[sp];
                        sp -= 1;
                        let a = stack[sp];
                        stack[sp] = if a == 0.0 {
                            fb
                        } else if a == 1.0 {
                            sb
                        } else {
                            mth_lerp(a, fb, sb)
                        };
                        sp += 1;
                    }
                    FuseOp::LerpConstFirst(f) => {
                        sp -= 1;
                        let sb = stack[sp];
                        sp -= 1;
                        let a = stack[sp];
                        stack[sp] = if a == 0.0 {
                            *f
                        } else if a == 1.0 {
                            sb
                        } else {
                            mth_lerp(a, *f, sb)
                        };
                        sp += 1;
                    }
                    FuseOp::LerpConstSecond(s2) => {
                        sp -= 1;
                        let fb = stack[sp];
                        sp -= 1;
                        let a = stack[sp];
                        stack[sp] = if a == 0.0 {
                            fb
                        } else if a == 1.0 {
                            *s2
                        } else {
                            mth_lerp(a, fb, *s2)
                        };
                        sp += 1;
                    }
                    FuseOp::PowConstBase(b) => {
                        sp -= 1;
                        stack[sp] = (*b).powf(stack[sp] as f64) as f32;
                        sp += 1;
                    }
                    FuseOp::PowConstExp(exp) => {
                        sp -= 1;
                        stack[sp] = (stack[sp] as f64).powf(*exp) as f32;
                        sp += 1;
                    }
                    FuseOp::Pow => {
                        sp -= 1;
                        let b = stack[sp] as f64;
                        sp -= 1;
                        stack[sp] = (stack[sp] as f64).powf(b) as f32;
                        sp += 1;
                    }
                    FuseOp::RoundInt(ty) => {
                        sp -= 1;
                        stack[sp] = apply_round(*ty, stack[sp]);
                        sp += 1;
                    }
                    FuseOp::Round(ty) => {
                        sp -= 1;
                        let m = stack[sp];
                        sp -= 1;
                        let a = stack[sp];
                        stack[sp] = if m == 0.0 {
                            a
                        } else {
                            apply_round(*ty, a / m) * m
                        };
                        sp += 1;
                    }
                    FuseOp::RangeChoice(min, max) => {
                        sp -= 1;
                        let ob = stack[sp];
                        sp -= 1;
                        let ir = stack[sp];
                        sp -= 1;
                        let a = stack[sp];
                        stack[sp] = if a >= *min && a < *max { ir } else { ob };
                        sp += 1;
                    }
                    FuseOp::RangeChoiceConst { min, max, in_v, out_v } => {
                        sp -= 1;
                        let a = stack[sp];
                        stack[sp] = if a >= *min && a < *max { *in_v } else { *out_v };
                        sp += 1;
                    }
                    FuseOp::IntervalSelect(thresholds) => {
                        let nb = thresholds.len() + 1;
                        let mut vals = [0f32; FUSE_MAX_BRANCHES + 1];
                        for k in (0..nb).rev() {
                            sp -= 1;
                            vals[k] = stack[sp];
                        }
                        sp -= 1;
                        let input = stack[sp];
                        let mut picked = nb - 1;
                        for (ti, t) in thresholds.iter().enumerate() {
                            if input < *t {
                                picked = ti;
                                break;
                            }
                        }
                        stack[sp] = vals[picked];
                        sp += 1;
                    }
                }
            }
            debug_assert_eq!(sp, 1);
            unsafe {
                *base.add(i) = stack[0];
            }
        }
        for b in slot_bufs {
            ctx.scratch.release(b);
        }
    }
}
