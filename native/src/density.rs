// Density-function interpreter, bit-exact with vanilla 1.21.8.
//
// Two layers:
//   1. compile: WorldGen df_arena (raw JSON-shaped nodes) -> wired WNode arena.
//      Mirrors RandomState's NoiseWiringHelper + NoiseChunk::wrap: string refs
//      resolve, noise ids bind to NormalNoise instances, markers become cache
//      nodes (caches are pass-through: pure functions, vanilla values are
//      identical with or without memoization), BlendAlpha/BlendOffset/
//      BeardifierMarker become constants for Blender.empty().
//   2. eval: exact per-node arithmetic. f64 everywhere except splines, which
//      are pure f32 (vanilla CubicSpline.Multipoint.apply works in float).

use crate::noise::{BlendedNoise, NormalNoise};
use crate::rng::{XoroshiroRandomSource, XoroshiroPositionalRandomFactory};
use crate::worldgen::{DfNode, NoiseSource, SplineValue, WorldGen};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// public types
// ---------------------------------------------------------------------------

/// Evaluation point (block coords).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pt {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

/// How interpolated markers resolve at eval time.
#[derive(Clone, Copy)]
pub enum Mode {
    /// SinglePointContext (aquifer, prelim surface, oracle): interpolated
    /// markers evaluate their child directly (vanilla: context != NoiseChunk).
    Direct,
    /// Slice-fill corner (vanilla NoiseInterpolator.fillArray -> wrapped
    /// compute at cell corners).
    Slice,
    /// Block loop with fillingCell=false: interpolated markers return the
    /// y->x->z chain value held in InterpState.
    Block,
    /// Cell fill (fillingCell=true, vanilla CacheAllInCell): interpolated
    /// markers return lerp3 of the 8 corners at in-cell fractions.
    CellFill,
}

/// Instantiated noise objects for one RandomState (one seed).
pub struct NoiseInstances {
    /// NormalNoise per noise-params id ("minecraft:offset", ...).
    pub noises: HashMap<String, NormalNoise>,
    /// old_blended_noise ("minecraft:terrain" reseed).
    pub terrain: BlendedNoise,
}

/// Everything needed to evaluate density for one seed.
pub struct Env {
    pub wg: WorldGen,
    pub noise: NoiseInstances,
    pub aquifer_random: XoroshiroPositionalRandomFactory,
    pub ore_random: XoroshiroPositionalRandomFactory,
}

// ---------------------------------------------------------------------------
// wired arena
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MapKind {
    Abs,
    Square,
    Cube,
    HalfNegative,
    QuarterNegative,
    Squeeze,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Ap2Kind {
    Add,
    Mul,
    Min,
    Max,
}

pub enum SplineValueW {
    Const(f32),
    Node(u32),
}

pub enum WKind {
    Const(f64),
    Noise {
        noise_id: u32,
        xz_scale: f64,
        y_scale: f64,
    },
    ShiftedNoise {
        noise_id: u32,
        xz_scale: f64,
        y_scale: f64,
        sx: u32,
        sy: u32,
        sz: u32,
    },
    ShiftA {
        noise_id: u32,
    },
    ShiftB {
        noise_id: u32,
    },
    /// Baked params live in `NoiseInstances::terrain`.
    OldBlendedNoise,
    Clamp {
        input: u32,
        min: f64,
        max: f64,
    },
    Mapped {
        kind: MapKind,
        input: u32,
    },
    Ap2 {
        kind: Ap2Kind,
        a: u32,
        b: u32,
    },
    RangeChoice {
        input: u32,
        min_inclusive: f64,
        max_exclusive: f64,
        when_in_range: u32,
        when_out_of_range: u32,
    },
    YClampedGradient {
        from_y: i32,
        to_y: i32,
        from_value: f64,
        to_value: f64,
    },
    Spline {
        coordinate: u32,
        points: Vec<(f32, SplineValueW, f32)>,
    },
    WeirdScaledSampler {
        noise_id: u32,
        /// false = type_1 (getSpaghettiRarity3D), true = type_2 (getSphaghettiRarity2D)
        mapper_type2: bool,
        input: u32,
    },
    /// NoiseChunk::wrapNew -> NoiseInterpolator. `slot` indexes InterpState.
    Interpolated {
        child: u32,
        slot: u16,
    },
    EndIslands,
}

pub struct Compiled {
    pub arena: Vec<WKind>,
    /// Noise id strings by compiled noise index.
    pub noise_ids: Vec<String>,
    /// Wired router roots by field name ("final_density", "barrier", ...).
    pub router: HashMap<String, u32>,
    /// Number of Interpolated slots (== InterpState size).
    pub interp_slots: u16,
    /// Wrapped child arena index per Interpolated slot.
    pub interp_children: Vec<u32>,
}

// ---------------------------------------------------------------------------
// compile
// ---------------------------------------------------------------------------

struct Compiler<'a> {
    wg: &'a WorldGen,
    arena: Vec<WKind>,
    noise_ids: Vec<String>,
    noise_id_index: HashMap<String, u32>,
    interp_slots: u16,
    interp_children: Vec<u32>,
}

impl<'a> Compiler<'a> {
    fn new(wg: &'a WorldGen) -> Compiler<'a> {
        Compiler {
            wg,
            arena: Vec::new(),
            noise_ids: Vec::new(),
            noise_id_index: HashMap::new(),
            interp_slots: 0,
            interp_children: Vec::new(),
        }
    }

    fn push(&mut self, k: WKind) -> u32 {
        self.arena.push(k);
        (self.arena.len() - 1) as u32
    }

    fn noise_ref(&mut self, id: &str) -> u32 {
        if let Some(i) = self.noise_id_index.get(id) {
            return *i;
        }
        let i = self.noise_ids.len() as u32;
        self.noise_ids.push(id.to_string());
        self.noise_id_index.insert(id.to_string(), i);
        i
    }

    fn compile(&mut self, idx: usize) -> u32 {
        // Guard against pathological deep recursion via explicit stack? The
        // overworld trees are < 60 deep; plain recursion is fine (verified
        // against the asset set).
        match &self.wg.df_arena[idx].clone() {
            DfNode::Constant(v) => self.push(WKind::Const(*v)),
            DfNode::Noise { noise, xz_scale, y_scale } => {
                let noise_id = self.noise_of(noise);
                self.push(WKind::Noise { noise_id, xz_scale: *xz_scale, y_scale: *y_scale })
            }
            DfNode::ShiftedNoise { noise, xz_scale, y_scale, shift_x, shift_y, shift_z } => {
                let noise_id = self.noise_of(noise);
                let sx = self.compile(*shift_x);
                let sy = self.compile(*shift_y);
                let sz = self.compile(*shift_z);
                self.push(WKind::ShiftedNoise { noise_id, xz_scale: *xz_scale, y_scale: *y_scale, sx, sy, sz })
            }
            DfNode::ShiftA { noise } => {
                let noise_id = self.noise_of(noise);
                self.push(WKind::ShiftA { noise_id })
            }
            DfNode::ShiftB { noise } => {
                let noise_id = self.noise_of(noise);
                self.push(WKind::ShiftB { noise_id })
            }
            // Blender.empty(): identity
            DfNode::BlendDensity { argument } => self.compile(*argument),
            // Blender.empty(): alpha=1.0, offset=0.0
            DfNode::BlendAlpha => self.push(WKind::Const(1.0)),
            DfNode::BlendOffset => self.push(WKind::Const(0.0)),
            // empty beardifier (no structure density in the noise takeover)
            DfNode::Beardifier => self.push(WKind::Const(0.0)),
            DfNode::EndIslands => self.push(WKind::EndIslands),
            DfNode::Clamp { input, min, max } => {
                let c = self.compile(*input);
                self.push(WKind::Clamp { input: c, min: *min, max: *max })
            }
            DfNode::Abs { argument } => {
                let c = self.compile(*argument);
                self.push(WKind::Mapped { kind: MapKind::Abs, input: c })
            }
            DfNode::Square { argument } => {
                let c = self.compile(*argument);
                self.push(WKind::Mapped { kind: MapKind::Square, input: c })
            }
            DfNode::Cube { argument } => {
                let c = self.compile(*argument);
                self.push(WKind::Mapped { kind: MapKind::Cube, input: c })
            }
            DfNode::HalfNegative { argument } => {
                let c = self.compile(*argument);
                self.push(WKind::Mapped { kind: MapKind::HalfNegative, input: c })
            }
            DfNode::QuarterNegative { argument } => {
                let c = self.compile(*argument);
                self.push(WKind::Mapped { kind: MapKind::QuarterNegative, input: c })
            }
            DfNode::Squeeze { argument } => {
                let c = self.compile(*argument);
                self.push(WKind::Mapped { kind: MapKind::Squeeze, input: c })
            }
            DfNode::Add { children } => self.fold(children, Ap2Kind::Add),
            DfNode::Mul { children } => self.fold(children, Ap2Kind::Mul),
            DfNode::Min { children } => self.fold(children, Ap2Kind::Min),
            DfNode::Max { children } => self.fold(children, Ap2Kind::Max),
            DfNode::RangeChoice { input, min_inclusive, max_exclusive, when_in_range, when_out_of_range } => {
                let i = self.compile(*input);
                let a = self.compile(*when_in_range);
                let b = self.compile(*when_out_of_range);
                self.push(WKind::RangeChoice {
                    input: i,
                    min_inclusive: *min_inclusive,
                    max_exclusive: *max_exclusive,
                    when_in_range: a,
                    when_out_of_range: b,
                })
            }
            DfNode::YClampedGradient { from_y, to_y, from_value, to_value } => self.push(WKind::YClampedGradient {
                from_y: *from_y,
                to_y: *to_y,
                from_value: *from_value,
                to_value: *to_value,
            }),
            DfNode::Interpolated { argument } => {
                let c = self.compile(*argument);
                let slot = self.interp_slots;
                self.interp_slots += 1;
                self.interp_children.push(c);
                self.push(WKind::Interpolated { child: c, slot })
            }
            // Pure memoization caches: values identical without them.
            DfNode::FlatCache { argument }
            | DfNode::Cache2d { argument }
            | DfNode::CacheOnce { argument }
            | DfNode::CacheAllInCell { argument } => self.compile(*argument),
            DfNode::CellScale { argument, .. } => {
                // Not present in 1.21.8 assets; treat as identity (documented).
                self.compile(*argument)
            }
            DfNode::OldBlendedNoise { .. } => self.push(WKind::OldBlendedNoise),
            DfNode::Spline { coordinate, points } => {
                let coord = self.compile(*coordinate);
                let pts = points
                    .iter()
                    .map(|p| {
                        let value = match &p.value {
                            SplineValue::Const(v) => SplineValueW::Const(*v as f32),
                            SplineValue::Node(n) => SplineValueW::Node(self.compile(*n)),
                        };
                        (p.location as f32, value, p.derivative as f32)
                    })
                    .collect();
                self.push(WKind::Spline { coordinate: coord, points: pts })
            }
            DfNode::WeirdScaledSampler { noise, rarity_value_mapper, input } => {
                let noise_id = self.noise_of(noise);
                let c = self.compile(*input);
                let mapper_type2 = rarity_value_mapper == "type_2";
                self.push(WKind::WeirdScaledSampler { noise_id, mapper_type2, input: c })
            }
            DfNode::WeightedList { distribution } => {
                // Weighted list is absent from 1.21.8 routers; fold as
                // left-associated add of (input * weight) defensively.
                let mut acc: Option<u32> = None;
                for (input, weight) in distribution {
                    let c = self.compile(*input);
                    let w = self.push(WKind::Const(*weight));
                    let mul = self.push(WKind::Ap2 { kind: Ap2Kind::Mul, a: w, b: c });
                    acc = Some(match acc {
                        None => mul,
                        Some(prev) => self.push(WKind::Ap2 { kind: Ap2Kind::Add, a: prev, b: mul }),
                    });
                }
                acc.unwrap_or_else(|| self.push(WKind::Const(0.0)))
            }
            DfNode::Bandlands => self.push(WKind::Const(0.0)),
        }
    }

    /// Left fold matching vanilla pairwise codec (no `arguments` arrays exist
    /// in 1.21.8 assets, so children are always [a, b]; fold is a safeguard).
    fn fold(&mut self, children: &[usize], kind: Ap2Kind) -> u32 {
        match children {
            [] => self.push(WKind::Const(0.0)),
            [one] => self.compile(*one),
            [a, b] => {
                let a = self.compile(*a);
                let b = self.compile(*b);
                self.push(WKind::Ap2 { kind, a, b })
            }
            many => {
                let mut acc = self.compile(many[0]);
                for &c in &many[1..] {
                    let c = self.compile(c);
                    acc = self.push(WKind::Ap2 { kind, a: acc, b: c });
                }
                acc
            }
        }
    }

    fn noise_of(&mut self, src: &NoiseSource) -> u32 {
        match src {
            NoiseSource::Id(id) => self.noise_ref(id),
            NoiseSource::Inline(p) => {
                let key = format!("__inline__{}", crate::worldgen::format_inline_key(p));
                self.noise_ref(&key)
            }
        }
    }
}

impl Env {
    /// Build the environment for one seed (vanilla RandomState semantics).
    pub fn new(seed: i64) -> Env {
        let wg = WorldGen::load_default("");
        Self::with_worldgen(wg, seed)
    }

    pub fn with_worldgen(wg: WorldGen, seed: i64) -> Env {
        // RandomState: this.random = XoroshiroRandomSource(seed).forkPositional()
        let mut root = XoroshiroRandomSource::new(seed);
        let root_factory = root.fork_positional();
        // aquiferRandom = random.fromHashOf("minecraft:aquifer").forkPositional()
        let mut aq = root_factory.from_hash_of("minecraft:aquifer");
        let aquifer_random = aq.fork_positional();
        // oreRandom = random.fromHashOf("minecraft:ore").forkPositional()
        let mut ore = root_factory.from_hash_of("minecraft:ore");
        let ore_random = ore.fork_positional();
        // BlendedNoise.withNewRandom(random.fromHashOf("minecraft:terrain"))
        let blended = find_blended_params(&wg);
        let mut terrain_src = root_factory.from_hash_of("minecraft:terrain");
        let terrain = BlendedNoise::create(
            &mut terrain_src,
            blended.0, blended.1, blended.2, blended.3, blended.4,
        );
        // Noise instances: Noises.instantiate -> NormalNoise.create(
        //   random.fromHashOf(key.location()), params) — deterministic per id.
        let mut used: Vec<String> = Vec::new();
        collect_noise_refs(&wg, &mut used);
        used.sort();
        used.dedup();
        let mut noises = HashMap::new();
        for id in used {
            let params = wg.noise_params.get(&id).cloned().unwrap_or_else(|| {
                panic!("noise params missing for {id}");
            });
            let mut src = root_factory.from_hash_of(&id);
            noises.insert(id, NormalNoise::create(&mut src, params.first_octave, params.amplitudes));
        }
        Env {
            wg,
            noise: NoiseInstances { noises, terrain },
            aquifer_random,
            ore_random,
        }
    }

    /// Compile the wired evaluator for this env's worldgen.
    pub fn compile(&self) -> Compiled {
        let mut c = Compiler::new(&self.wg);
        let mut router = HashMap::new();
        // deterministic order
        let mut keys: Vec<&String> = self.wg.router_indices.keys().collect();
        keys.sort();
        for k in keys {
            let idx = self.wg.router_indices[k];
            let root = c.compile(idx);
            router.insert(k.clone(), root);
        }
        Compiled {
            arena: c.arena,
            noise_ids: c.noise_ids,
            router,
            interp_slots: c.interp_slots,
            interp_children: c.interp_children,
        }
    }

    #[inline]
    fn normal(&self, id: &str) -> &NormalNoise {
        self.noise.noises.get(id).unwrap_or_else(|| panic!("noise instance missing: {id}"))
    }
}

fn find_blended_params(wg: &WorldGen) -> (f64, f64, f64, f64, f64) {
    for node in &wg.df_arena {
        if let DfNode::OldBlendedNoise { xz_scale, y_scale, xz_factor, y_factor, smear_scale_multiplier } = node {
            return (*xz_scale, *y_scale, *xz_factor, *y_factor, *smear_scale_multiplier);
        }
    }
    (1.0, 1.0, 80.0, 160.0, 4.0)
}

fn collect_noise_refs(wg: &WorldGen, out: &mut Vec<String>) {
    for node in &wg.df_arena {
        match node {
            DfNode::Noise { noise, .. }
            | DfNode::ShiftA { noise }
            | DfNode::ShiftB { noise }
            | DfNode::ShiftedNoise { noise, .. }
            | DfNode::WeirdScaledSampler { noise, .. } => match noise {
                NoiseSource::Id(id) => out.push(id.clone()),
                NoiseSource::Inline(p) => out.push(format!("__inline__{}", crate::worldgen::format_inline_key(p))),
            },
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// eval
// ---------------------------------------------------------------------------

/// Interpolated-node runtime state for the block loop (per NoiseChunk).
/// Slot order matches WKind::Interpolated{slot}.
pub struct InterpState {
    /// slice0[z][y] and slice1[z][y], z in 0..=cellCountXZ, y in 0..=cellCountY.
    pub slice0: Vec<f64>,
    pub slice1: Vec<f64>,
    slice_rows: usize, // cellCountXZ + 1
    slice_cols: usize, // cellCountY + 1
    /// Per slot: 8 corners (n000, n100, n010, n110, n001, n101, n011, n111)
    /// in vanilla selectCellYZ order.
    corners: Vec<[f64; 8]>,
    /// Per slot chain values (valueXZ00, valueXZ10, valueXZ01, valueXZ11, valueZ0, valueZ1, value).
    chain: Vec<[f64; 7]>,
    /// Current in-cell fractions for Mode::CellFill lerp3 (set by the driver).
    pub cell_frac: (f64, f64, f64),
}

impl InterpState {
    pub fn new(slots: u16, cell_count_xz: usize, cell_count_y: usize) -> InterpState {
        InterpState {
            slice0: vec![0.0; slots as usize * (cell_count_xz + 1) * (cell_count_y + 1)],
            slice1: vec![0.0; slots as usize * (cell_count_xz + 1) * (cell_count_y + 1)],
            slice_rows: cell_count_xz + 1,
            slice_cols: cell_count_y + 1,
            corners: vec![[0.0; 8]; slots as usize],
            chain: vec![[0.0; 7]; slots as usize],
            cell_frac: (0.0, 0.0, 0.0),
        }
    }

    #[inline]
    fn slice_get(&self, s: &[f64], slot: u16, z: usize, y: usize) -> f64 {
        s[slot as usize * self.slice_rows * self.slice_cols + z * self.slice_cols + y]
    }

    #[inline]
    fn slice_set(&mut self, s: &mut [f64], slot: u16, z: usize, y: usize, v: f64) {
        s[slot as usize * self.slice_rows * self.slice_cols + z * self.slice_cols + y] = v;
    }

    /// Vanilla NoiseInterpolator.selectCellYZ.
    #[inline]
    pub fn select_cell_yz(&mut self, y: usize, z: usize) {
        for slot in 0..self.corners.len() {
            self.corners[slot] = [
                self.slice_get(&self.slice0, slot as u16, z, y),     // n000
                self.slice_get(&self.slice1, slot as u16, z, y),     // n100
                self.slice_get(&self.slice0, slot as u16, z, y + 1), // n010
                self.slice_get(&self.slice1, slot as u16, z, y + 1), // n110
                self.slice_get(&self.slice0, slot as u16, z + 1, y), // n001
                self.slice_get(&self.slice1, slot as u16, z + 1, y), // n101
                self.slice_get(&self.slice0, slot as u16, z + 1, y + 1), // n011
                self.slice_get(&self.slice1, slot as u16, z + 1, y + 1), // n111
            ];
        }
    }

    /// Vanilla NoiseInterpolator.updateForY.
    #[inline]
    pub fn update_for_y(&mut self, y_frac: f64) {
        for slot in 0..self.chain.len() {
            let n = self.corners[slot];
            // n = [n000, n100, n010, n110, n001, n101, n011, n111]
            self.chain[slot] = [
                lerp(y_frac, n[0], n[2]), // valueXZ00 = lerp(y, n000, n010)
                lerp(y_frac, n[1], n[3]), // valueXZ10 = lerp(y, n100, n110)
                lerp(y_frac, n[4], n[6]), // valueXZ01 = lerp(y, n001, n011)
                lerp(y_frac, n[5], n[7]), // valueXZ11 = lerp(y, n101, n111)
                0.0,
                0.0,
                0.0,
            ];
        }
    }

    /// Vanilla NoiseInterpolator.updateForX.
    #[inline]
    pub fn update_for_x(&mut self, x_frac: f64) {
        for slot in 0..self.chain.len() {
            let c = &mut self.chain[slot];
            c[4] = lerp(x_frac, c[0], c[1]); // valueZ0 = lerp(x, valueXZ00, valueXZ10)
            c[5] = lerp(x_frac, c[2], c[3]); // valueZ1 = lerp(x, valueXZ01, valueXZ11)
        }
    }

    /// Vanilla NoiseInterpolator.updateForZ.
    #[inline]
    pub fn update_for_z(&mut self, z_frac: f64) {
        for slot in 0..self.chain.len() {
            let c = &mut self.chain[slot];
            c[6] = lerp(z_frac, c[4], c[5]); // value = lerp(z, valueZ0, valueZ1)
        }
    }

    #[inline]
    pub fn chain_value(&self, slot: u16) -> f64 {
        self.chain[slot as usize][6]
    }

    /// Vanilla NoiseInterpolator.compute with fillingCell=true:
    /// Mth.lerp3(inCellX/cw, inCellY/ch, inCellZ/cw, n000, n100, n010, n110,
    ///           n001, n101, n011, n111)
    #[inline]
    pub fn cellfill_value(&self, slot: u16) -> f64 {
        let n = self.corners[slot as usize];
        let (dx, dy, dz) = self.cell_frac;
        let row1 = lerp(dy, lerp(dx, n[0], n[1]), lerp(dx, n[2], n[3]));
        let row2 = lerp(dy, lerp(dx, n[4], n[5]), lerp(dx, n[6], n[7]));
        lerp(dz, row1, row2)
    }

    #[inline]
    pub fn swap_slices(&mut self) {
        std::mem::swap(&mut self.slice0, &mut self.slice1);
    }

    #[inline]
    pub fn slice0_set(&mut self, slot: u16, z: usize, y: usize, v: f64) {
        let stride = self.slice_rows * self.slice_cols;
        self.slice0[slot as usize * stride + z * self.slice_cols + y] = v;
    }

    #[inline]
    pub fn slice1_set(&mut self, slot: u16, z: usize, y: usize, v: f64) {
        let stride = self.slice_rows * self.slice_cols;
        self.slice1[slot as usize * stride + z * self.slice_cols + y] = v;
    }
}

#[inline]
fn lerp(t: f64, a: f64, b: f64) -> f64 {
    a + t * (b - a)
}

#[inline]
fn mth_clamp(v: f64, min: f64, max: f64) -> f64 {
    if v < min {
        min
    } else if v > max {
        max
    } else {
        v
    }
}

impl Env {
    pub fn eval(&self, c: &Compiled, idx: u32, pt: Pt, mode: Mode, interp: &InterpState) -> f64 {
        let node = &c.arena[idx as usize];
        match node {
            WKind::Const(v) => *v,
            WKind::Noise { noise_id, xz_scale, y_scale } => {
                let id: &str = &c.noise_ids[*noise_id as usize];
                self.normal(id).get_value(pt.x as f64 * xz_scale, pt.y as f64 * y_scale, pt.z as f64 * xz_scale)
            }
            WKind::ShiftedNoise { noise_id, xz_scale, y_scale, sx, sy, sz } => {
                let dx = pt.x as f64 * xz_scale + self.eval(c, *sx, pt, mode, interp);
                let dy = pt.y as f64 * y_scale + self.eval(c, *sy, pt, mode, interp);
                let dz = pt.z as f64 * xz_scale + self.eval(c, *sz, pt, mode, interp);
                let id: &str = &c.noise_ids[*noise_id as usize];
                self.normal(id).get_value(dx, dy, dz)
            }
            WKind::ShiftA { noise_id } => {
                let id: &str = &c.noise_ids[*noise_id as usize];
                // ShiftNoise.compute(x, 0, z): noise(x*0.25, 0, z*0.25) * 4
                self.normal(id).get_value(pt.x as f64 * 0.25, 0.0, pt.z as f64 * 0.25) * 4.0
            }
            WKind::ShiftB { noise_id } => {
                let id: &str = &c.noise_ids[*noise_id as usize];
                // ShiftNoise.compute(z, x, 0)
                self.normal(id).get_value(pt.z as f64 * 0.25, pt.x as f64 * 0.25, 0.0) * 4.0
            }
            WKind::OldBlendedNoise => self.noise.terrain.compute(pt.x, pt.y, pt.z),
            WKind::Clamp { input, min, max } => mth_clamp(self.eval(c, *input, pt, mode, interp), *min, *max),
            WKind::Mapped { kind, input } => {
                let v = self.eval(c, *input, pt, mode, interp);
                match kind {
                    MapKind::Abs => v.abs(),
                    MapKind::Square => v * v,
                    MapKind::Cube => v * v * v,
                    MapKind::HalfNegative => {
                        if v > 0.0 {
                            v
                        } else {
                            v * 0.5
                        }
                    }
                    MapKind::QuarterNegative => {
                        if v > 0.0 {
                            v
                        } else {
                            v * 0.25
                        }
                    }
                    MapKind::Squeeze => {
                        let d = mth_clamp(v, -1.0, 1.0);
                        d / 2.0 - d * d * d / 24.0
                    }
                }
            }
            WKind::Ap2 { kind, a, b } => {
                let va = self.eval(c, *a, pt, mode, interp);
                match kind {
                    Ap2Kind::Add => va + self.eval(c, *b, pt, mode, interp),
                    Ap2Kind::Mul => {
                        if va == 0.0 {
                            0.0
                        } else {
                            va * self.eval(c, *b, pt, mode, interp)
                        }
                    }
                    Ap2Kind::Min => {
                        let vb = self.eval(c, *b, pt, mode, interp);
                        if va < vb {
                            va
                        } else {
                            vb
                        }
                    }
                    Ap2Kind::Max => {
                        let vb = self.eval(c, *b, pt, mode, interp);
                        if va > vb {
                            va
                        } else {
                            vb
                        }
                    }
                }
            }
            WKind::RangeChoice { input, min_inclusive, max_exclusive, when_in_range, when_out_of_range } => {
                let v = self.eval(c, *input, pt, mode, interp);
                if v >= *min_inclusive && v < *max_exclusive {
                    self.eval(c, *when_in_range, pt, mode, interp)
                } else {
                    self.eval(c, *when_out_of_range, pt, mode, interp)
                }
            }
            WKind::YClampedGradient { from_y, to_y, from_value, to_value } => {
                // Mth.clampedMap(blockY, fromY, toY, fromValue, toValue)
                let t = (pt.y as f64 - *from_y as f64) / (*to_y as f64 - *from_y as f64);
                if t < 0.0 {
                    *from_value
                } else if t > 1.0 {
                    *to_value
                } else {
                    lerp(t, *from_value, *to_value)
                }
            }
            WKind::Spline { coordinate, points } => {
                let f = self.eval(c, *coordinate, pt, mode, interp) as f32;
                spline_apply(self, c, points, f, pt, mode, interp) as f64
            }
            WKind::WeirdScaledSampler { noise_id, mapper_type2, input } => {
                let v = self.eval(c, *input, pt, mode, interp);
                let d = if *mapper_type2 {
                    // getSphaghettiRarity2D
                    if v < -0.75 {
                        0.5
                    } else if v < -0.5 {
                        0.75
                    } else if v < 0.5 {
                        1.0
                    } else if v < 0.75 {
                        2.0
                    } else {
                        3.0
                    }
                } else {
                    // getSpaghettiRarity3D
                    if v < -0.5 {
                        0.75
                    } else if v < 0.0 {
                        1.0
                    } else if v < 0.5 {
                        1.5
                    } else {
                        2.0
                    }
                };
                let id: &str = &c.noise_ids[*noise_id as usize];
                let n = self.normal(id).get_value(
                    pt.x as f64 / d,
                    pt.y as f64 / d,
                    pt.z as f64 / d,
                );
                d * n.abs()
            }
            WKind::Interpolated { child, slot } => match mode {
                Mode::Direct | Mode::Slice => self.eval(c, *child, pt, mode, interp),
                Mode::Block => interp.chain_value(*slot),
                Mode::CellFill => interp.cellfill_value(*slot),
            },
            WKind::EndIslands => {
                // Not on the overworld path; keep exact-zero like empty end
                // router usage. (End islands would need SimplexNoise.)
                0.0
            }
        }
    }
}

/// CubicSpline.Multipoint.apply — all f32, exact vanilla order.
fn spline_apply(
    env: &Env,
    c: &Compiled,
    points: &[(f32, SplineValueW, f32)],
    f: f32,
    pt: Pt,
    mode: Mode,
    interp: &InterpState,
) -> f32 {
    // locations are sorted ascending in vanilla data; findIntervalStart:
    // first index i where f < locations[i], minus 1.
    let n = points.len();
    let mut lo: usize = 0;
    let mut hi: usize = n;
    while lo < hi {
        let mid = (lo + hi) / 2;
        if f < points[mid].0 {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    let i = lo as isize - 1; // interval start; -1 if f < locations[0]
    let last = n - 1;
    let point_value = |env: &Env, v: &SplineValueW| -> f32 {
        match v {
            SplineValueW::Const(k) => *k,
            SplineValueW::Node(node) => {
                // Leaf coordinate: (float) density compute. Nested splines are
                // DfNode::Spline -> WKind::Spline, handled by eval recursion.
                let d = env.eval(c, *node, pt, mode, interp);
                d as f32
            }
        }
    };
    if i < 0 {
        // linearExtend(f, locations, values[0], derivatives, 0)
        let v = point_value(env, &points[0].1);
        let der = points[0].2;
        if der == 0.0 {
            v
        } else {
            v + der * (f - points[0].0)
        }
    } else if i as usize == last {
        let v = point_value(env, &points[last].1);
        let der = points[last].2;
        if der == 0.0 {
            v
        } else {
            v + der * (f - points[last].0)
        }
    } else {
        let i = i as usize;
        let f1 = points[i].0;
        let f2 = points[i + 1].0;
        let f3 = (f - f1) / (f2 - f1);
        let f4 = points[i].2;
        let f5 = points[i + 1].2;
        let f6 = point_value(env, &points[i].1);
        let f7 = point_value(env, &points[i + 1].1);
        let f8 = f4 * (f2 - f1) - (f7 - f6);
        let f9 = -f5 * (f2 - f1) + (f7 - f6);
        let l0 = f6 + f3 * (f7 - f6); // Mth.lerp(f3, f6, f7) in f32
        let l1 = f8 + f3 * (f9 - f8); // Mth.lerp(f3, f8, f9) in f32
        l0 + f3 * (1.0f32 - f3) * l1
    }
}
