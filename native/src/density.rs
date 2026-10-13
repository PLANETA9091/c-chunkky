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

use crate::noise::NormalNoise;
use crate::rng::{XoroshiroRandomSource, XoroshiroPositionalRandomFactory};
use crate::worldgen::{DfNode, NoiseSource, SplineValue, WorldGen};
use std::cell::Cell;
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

impl Mode {
    #[inline]
    pub fn bits(self) -> u64 {
        match self {
            Mode::Direct => 0,
            Mode::Slice => 1,
            Mode::Block => 2,
            Mode::CellFill => 3,
        }
    }
}

/// Per-point memoization context: turns the compiled arena tree into an
/// evaluated DAG. Every node computes AT MOST ONCE per (serial, mode).
/// Sound because every density node is a pure function of (pt, mode, interp).
/// The serial is bumped by the driver once per new evaluation context
/// (one corner point / one block / one prelim probe).
pub struct MemoCtx {
    stamp: Box<[Cell<u64>]>,
    value: Box<[Cell<f64>]>,
    serial: Cell<u64>,
}

impl MemoCtx {
    pub fn new(n: usize) -> MemoCtx {
        MemoCtx {
            stamp: vec![Cell::new(0); n].into_boxed_slice(),
            value: vec![Cell::new(0.0); n].into_boxed_slice(),
            serial: Cell::new(0),
        }
    }

    /// Begin a new evaluation context (one point / block).
    #[inline]
    pub fn next_serial(&self) {
        self.serial.set(self.serial.get() + 1);
    }

    #[inline]
    fn key(&self, idx: usize, mode: Mode) -> u64 {
        self.serial.get().wrapping_mul(8) + mode.bits() + 1
    }

    #[inline]
    fn hit(&self, idx: usize, mode: Mode) -> Option<f64> {
        let i = idx as usize;
        if self.stamp[i].get() == self.key(idx, mode) {
            Some(self.value[i].get())
        } else {
            None
        }
    }

    #[inline]
    fn store(&self, idx: usize, mode: Mode, v: f64) {
        let i = idx as usize;
        self.stamp[i].set(self.key(idx, mode));
        self.value[i].set(v);
    }
}

/// Instantiated noise objects for one RandomState (one seed).
pub struct NoiseInstances {
    /// Flat list of instances (hot-path access by index).
    pub list: Vec<NormalNoise>,
    /// name -> index into `list`.
    pub list_idx: HashMap<String, u32>,
}

/// Everything needed to evaluate density for one seed.
pub struct Env {
    pub seed: i64,
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
    /// Index into `Compiled::blended` (per-node params, terrain-seeded).
    OldBlendedNoise {
        instance: u32,
    },
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
    /// Pure memoization wrappers, kept for structural walk parity (values
    /// identical with or without the cache).
    Cache2d {
        child: u32,
    },
    CacheOnce {
        child: u32,
    },
    FlatCache {
        child: u32,
    },
    CacheAllInCell {
        child: u32,
    },
    /// Vanilla constant-fold of Ap2(ADD/MUL) with a Constant operand:
    /// argument1 = Constant(argument), argument2 = input.
    MulOrAdd {
        is_add: bool,
        input: u32,
        argument: f64,
    },
    EndIslands,
}

pub struct Compiled {
    pub arena: Vec<WKind>,
    /// Noise id strings by compiled noise index.
    pub noise_ids: Vec<String>,
    /// Instance list index per compiled noise index (hot-path resolution).
    pub noise_resolved: Vec<u32>,
    /// Per-params BlendedNoise instances (vanilla withNewRandom re-seeds each
    /// from the same "minecraft:terrain" factory hash).
    pub blended: Vec<crate::noise::BlendedNoise>,
    /// Wired router roots by field name ("final_density", "barrier", ...).
    pub router: HashMap<String, u32>,
    /// Number of Interpolated slots (== InterpState size).
    pub interp_slots: u16,
    /// Wrapped child arena index per Interpolated slot.
    pub interp_children: Vec<u32>,
    /// Column-cache slot per arena node, for Cache2d/FlatCache nodes whose
    /// subtree is provably y-independent (compile-time analysis); u32::MAX
    /// otherwise. A y-independent child returns identical f64 values for
    /// every y at a fixed (x, z), so caching one value per (x, z) column is
    /// bit-exact value reuse of a pure function.
    pub col_idx: Vec<u32>,
    /// Number of assigned column-cache slots (size of BatchMemo col storage).
    pub col_count: u32,
}

// ---------------------------------------------------------------------------
// compile
// ---------------------------------------------------------------------------

/// FNV-1a 64 over a byte iterator.
#[inline]
fn fnv64(init: u64, bytes: &[u8]) -> u64 {
    let mut h = init;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

#[inline]
fn fnv64_u64(init: u64, v: u64) -> u64 {
    fnv64(init, &v.to_le_bytes())
}

/// Structural hash for hash-consing. Children are interned BEFORE the parent,
/// so their arena indices are canonical identifiers — hashing them gives a
/// Merkle-style subtree hash.
fn hash_wkind(k: &WKind) -> u64 {
    use WKind as W;
    let base: u64 = 0xcbf29ce484222325;
    match k {
        W::Const(v) => fnv64_u64(fnv64_u64(base, 0), v.to_bits()),
        W::Noise { noise_id, xz_scale, y_scale } => {
            let h = fnv64_u64(base, 1);
            let h = fnv64_u64(h, *noise_id as u64);
            let h = fnv64_u64(h, xz_scale.to_bits());
            fnv64_u64(h, y_scale.to_bits())
        }
        W::ShiftedNoise { noise_id, xz_scale, y_scale, sx, sy, sz } => {
            let h = fnv64_u64(base, 2);
            let h = fnv64_u64(h, *noise_id as u64);
            let h = fnv64_u64(h, xz_scale.to_bits());
            let h = fnv64_u64(h, y_scale.to_bits());
            let h = fnv64_u64(h, *sx as u64);
            let h = fnv64_u64(h, *sy as u64);
            fnv64_u64(h, *sz as u64)
        }
        W::ShiftA { noise_id } => fnv64_u64(fnv64_u64(base, 3), *noise_id as u64),
        W::ShiftB { noise_id } => fnv64_u64(fnv64_u64(base, 4), *noise_id as u64),
        W::OldBlendedNoise { instance } => fnv64_u64(fnv64_u64(base, 5), *instance as u64),
        W::Clamp { input, min, max } => {
            let h = fnv64_u64(fnv64_u64(base, 6), *input as u64);
            let h = fnv64_u64(h, min.to_bits());
            fnv64_u64(h, max.to_bits())
        }
        W::Mapped { kind, input } => {
            let h = fnv64_u64(fnv64_u64(base, 7), *input as u64);
            fnv64_u64(h, *kind as u64)
        }
        W::Ap2 { kind, a, b } => {
            let h = fnv64_u64(fnv64_u64(base, 8), *a as u64);
            let h = fnv64_u64(h, *b as u64);
            fnv64_u64(h, *kind as u64)
        }
        W::RangeChoice { input, min_inclusive, max_exclusive, when_in_range, when_out_of_range } => {
            let h = fnv64_u64(fnv64_u64(base, 9), *input as u64);
            let h = fnv64_u64(h, min_inclusive.to_bits());
            let h = fnv64_u64(h, max_exclusive.to_bits());
            let h = fnv64_u64(h, *when_in_range as u64);
            fnv64_u64(h, *when_out_of_range as u64)
        }
        W::YClampedGradient { from_y, to_y, from_value, to_value } => {
            let h = fnv64_u64(fnv64_u64(base, 10), *from_y as u64);
            let h = fnv64_u64(h, *to_y as u64);
            let h = fnv64_u64(h, from_value.to_bits());
            fnv64_u64(h, to_value.to_bits())
        }
        W::Spline { coordinate, points } => {
            let mut h = fnv64_u64(fnv64_u64(base, 11), *coordinate as u64);
            h = fnv64_u64(h, points.len() as u64);
            for (a, v, b) in points {
                let h0 = fnv64_u64(h, (*a).to_bits() as u64);
                let h1 = match v {
                    SplineValueW::Const(c) => fnv64_u64(h0, 0)
                        .wrapping_add(fnv64_u64(0, (*c).to_bits() as u64)),
                    SplineValueW::Node(n) => fnv64_u64(h0, 1).wrapping_add(fnv64_u64(0, *n as u64)),
                };
                h = fnv64_u64(h1, (*b).to_bits() as u64)
            }
            h
        }
        W::WeirdScaledSampler { noise_id, mapper_type2, input } => {
            let h = fnv64_u64(fnv64_u64(base, 12), *noise_id as u64);
            let h = fnv64_u64(h, *mapper_type2 as u64);
            fnv64_u64(h, *input as u64)
        }
        // excluded from interning by push()
        W::Interpolated { child, slot } => {
            let h = fnv64_u64(fnv64_u64(base, 13), *child as u64);
            fnv64_u64(h, *slot as u64)
        }
        W::Cache2d { child } => fnv64_u64(fnv64_u64(base, 14), *child as u64),
        W::CacheOnce { child } => fnv64_u64(fnv64_u64(base, 15), *child as u64),
        W::FlatCache { child } => fnv64_u64(fnv64_u64(base, 16), *child as u64),
        W::CacheAllInCell { child } => fnv64_u64(fnv64_u64(base, 17), *child as u64),
        W::MulOrAdd { is_add, input, argument } => {
            let h = fnv64_u64(fnv64_u64(base, 18), *is_add as u64);
            let h = fnv64_u64(h, *input as u64);
            fnv64_u64(h, argument.to_bits())
        }
        W::EndIslands => fnv64_u64(base, 19),
    }
}

/// Exact structural equality (guards hash collisions; bit-exactness mandate).
fn wkind_eq(a: &WKind, b: &WKind) -> bool {
    use WKind as W;
    match (a, b) {
        (W::Const(x), W::Const(y)) => x.to_bits() == y.to_bits(),
        (W::Noise { noise_id: a1, xz_scale: a2, y_scale: a3 }, W::Noise { noise_id: b1, xz_scale: b2, y_scale: b3 }) => {
            a1 == b1 && a2.to_bits() == b2.to_bits() && a3.to_bits() == b3.to_bits()
        }
        (W::ShiftedNoise { noise_id: a1, xz_scale: a2, y_scale: a3, sx: a4, sy: a5, sz: a6 }, W::ShiftedNoise { noise_id: b1, xz_scale: b2, y_scale: b3, sx: b4, sy: b5, sz: b6 }) => {
            a1 == b1 && a2.to_bits() == b2.to_bits() && a3.to_bits() == b3.to_bits() && a4 == b4 && a5 == b5 && a6 == b6
        }
        (W::ShiftA { noise_id: a1 }, W::ShiftA { noise_id: b1 }) => a1 == b1,
        (W::ShiftB { noise_id: a1 }, W::ShiftB { noise_id: b1 }) => a1 == b1,
        (W::OldBlendedNoise { instance: a1 }, W::OldBlendedNoise { instance: b1 }) => a1 == b1,
        (W::Clamp { input: a1, min: a2, max: a3 }, W::Clamp { input: b1, min: b2, max: b3 }) => {
            a1 == b1 && a2.to_bits() == b2.to_bits() && a3.to_bits() == b3.to_bits()
        }
        (W::Mapped { kind: a1, input: a2 }, W::Mapped { kind: b1, input: b2 }) => {
            a1 == b1 && a2 == b2
        }
        (W::Ap2 { kind: a1, a: a2, b: a3 }, W::Ap2 { kind: b1, a: b2, b: b3 }) => {
            a1 == b1 && a2 == b2 && a3 == b3
        }
        (W::RangeChoice { input: a1, min_inclusive: a2, max_exclusive: a3, when_in_range: a4, when_out_of_range: a5 }, W::RangeChoice { input: b1, min_inclusive: b2, max_exclusive: b3, when_in_range: b4, when_out_of_range: b5 }) => {
            a1 == b1 && a2.to_bits() == b2.to_bits() && a3.to_bits() == b3.to_bits() && a4 == b4 && a5 == b5
        }
        (W::YClampedGradient { from_y: a1, to_y: a2, from_value: a3, to_value: a4 }, W::YClampedGradient { from_y: b1, to_y: b2, from_value: b3, to_value: b4 }) => {
            a1 == b1 && a2 == b2 && a3.to_bits() == b3.to_bits() && a4.to_bits() == b4.to_bits()
        }
        (W::Spline { coordinate: a1, points: a2 }, W::Spline { coordinate: b1, points: b2 }) => {
            if a1 != b1 || a2.len() != b2.len() {
                return false;
            }
            a2.iter().zip(b2.iter()).all(|((a, va, b), (c, vb, d))| {
                a.to_bits() == c.to_bits()
                    && b.to_bits() == d.to_bits()
                    && match (va, vb) {
                        (SplineValueW::Const(x), SplineValueW::Const(y)) => x.to_bits() == y.to_bits(),
                        (SplineValueW::Node(x), SplineValueW::Node(y)) => x == y,
                        _ => false,
                    }
            })
        }
        (W::WeirdScaledSampler { noise_id: a1, mapper_type2: a2, input: a3 }, W::WeirdScaledSampler { noise_id: b1, mapper_type2: b2, input: b3 }) => {
            a1 == b1 && a2 == b2 && a3 == b3
        }
        (W::Interpolated { child: a1, slot: a2 }, W::Interpolated { child: b1, slot: b2 }) => {
            a1 == b1 && a2 == b2
        }
        (W::Cache2d { child: a1 }, W::Cache2d { child: b1 })
        | (W::CacheOnce { child: a1 }, W::CacheOnce { child: b1 })
        | (W::FlatCache { child: a1 }, W::FlatCache { child: b1 })
        | (W::CacheAllInCell { child: a1 }, W::CacheAllInCell { child: b1 }) => a1 == b1,
        (W::MulOrAdd { is_add: a1, input: a2, argument: a3 }, W::MulOrAdd { is_add: b1, input: b2, argument: b3 }) => {
            a1 == b1 && a2 == b2 && a3.to_bits() == b3.to_bits()
        }
        (W::EndIslands, W::EndIslands) => true,
        _ => false,
    }
}

struct Compiler<'a> {
    wg: &'a WorldGen,
    arena: Vec<WKind>,
    noise_ids: Vec<String>,
    noise_id_index: HashMap<String, u32>,
    interp_slots: u16,
    interp_children: Vec<u32>,
    blended_params: Vec<(f64, f64, f64, f64, f64)>,
    blended_index: HashMap<(u64, u64, u64, u64, u64), u32>,
    /// Structural hash-consing (CSE): subtree hash -> arena index. All nodes
    /// are pure functions of (pt, mode, interp), so merging structurally
    /// identical subtrees is value-identical. Interpolated markers are
    /// EXCLUDED (vanilla allocates one interpolator per marker).
    intern: HashMap<u64, u32>,
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
            blended_params: Vec::new(),
            blended_index: HashMap::new(),
            intern: HashMap::new(),
        }
    }

    fn push(&mut self, k: WKind) -> u32 {
        if matches!(k, WKind::Interpolated { .. }) {
            // one interpolator per marker, never merged
            self.arena.push(k);
            return (self.arena.len() - 1) as u32;
        }
        let h = hash_wkind(&k);
        if let Some(&i) = self.intern.get(&h) {
            if wkind_eq(&self.arena[i as usize], &k) {
                return i;
            }
        }
        self.arena.push(k);
        let idx = (self.arena.len() - 1) as u32;
        self.intern.insert(h, idx);
        idx
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
            // Pure memoization wrappers: kept for walk parity, pass-through eval.
            DfNode::FlatCache { argument } => {
                let c = self.compile(*argument);
                self.push(WKind::FlatCache { child: c })
            }
            DfNode::Cache2d { argument } => {
                let c = self.compile(*argument);
                self.push(WKind::Cache2d { child: c })
            }
            DfNode::CacheOnce { argument } => {
                let c = self.compile(*argument);
                self.push(WKind::CacheOnce { child: c })
            }
            DfNode::CacheAllInCell { argument } => {
                let c = self.compile(*argument);
                self.push(WKind::CacheAllInCell { child: c })
            }
            DfNode::CellScale { argument, .. } => {
                // Not present in 1.21.8 assets; treat as identity (documented).
                self.compile(*argument)
            }
            DfNode::OldBlendedNoise { xz_scale, y_scale, xz_factor, y_factor, smear_scale_multiplier } => {
                let key = (
                    xz_scale.to_bits(),
                    y_scale.to_bits(),
                    xz_factor.to_bits(),
                    y_factor.to_bits(),
                    smear_scale_multiplier.to_bits(),
                );
                let instance = if let Some(i) = self.blended_index.get(&key) {
                    *i
                } else {
                    let i = self.blended_params.len() as u32;
                    self.blended_params.push((
                        *xz_scale,
                        *y_scale,
                        *xz_factor,
                        *y_factor,
                        *smear_scale_multiplier,
                    ));
                    self.blended_index.insert(key, i);
                    i
                };
                self.push(WKind::OldBlendedNoise { instance })
            }
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
                // NOTE: vanilla does NOT fold JSON-parsed trees — inline JSON
                // numbers arrive as HolderHolder(Constant), so the
                // `instanceof Constant` fold in TwoArgumentSimpleFunction.create
                // never fires (verified: zero MulOrAdd in the live tree walk).
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

    fn as_const(&self, idx: u32) -> (bool, f64) {
        match &self.arena[idx as usize] {
            WKind::Const(v) => (true, *v),
            _ => (false, 0.0),
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
        // Noise instances: Noises.instantiate -> NormalNoise.create(
        //   random.fromHashOf(key.location()), params) — deterministic per id.
        let mut used: Vec<String> = Vec::new();
        collect_noise_refs(&wg, &mut used);
        used.sort();
        used.dedup();
        let mut list = Vec::new();
        let mut list_idx = HashMap::new();
        for id in used {
            let params = wg.noise_params.get(&id).cloned().unwrap_or_else(|| {
                panic!("noise params missing for {id}");
            });
            let mut src = root_factory.from_hash_of(&id);
            let inst = NormalNoise::create(&mut src, params.first_octave, params.amplitudes);
            list_idx.insert(id, list.len() as u32);
            list.push(inst);
        }
        Env {
            seed,
            wg,
            noise: NoiseInstances { list, list_idx },
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
        // BlendedNoise per unique params (each re-seeded from "minecraft:terrain")
        let mut blended = Vec::with_capacity(c.blended_params.len());
        for (xz, y, xzf, yf, smear) in &c.blended_params {
            let mut src = root_terrain_factory(&self.wg, self.seed);
            blended.push(crate::noise::BlendedNoise::create(&mut src, *xz, *y, *xzf, *yf, *smear));
        }
        // resolve compiled noise indices -> instance list indices ONCE
        let noise_resolved = c
            .noise_ids
            .iter()
            .map(|id| {
                *self
                    .noise
                    .list_idx
                    .get(id)
                    .unwrap_or_else(|| panic!("noise instance missing: {id}"))
            })
            .collect();
        // column-cache slots for y-independent Cache2d/FlatCache subtrees
        let (col_idx, col_count) = compute_col_caches(&c.arena);
        Compiled {
            arena: c.arena,
            noise_ids: c.noise_ids,
            noise_resolved,
            blended,
            router,
            interp_slots: c.interp_slots,
            interp_children: c.interp_children,
            col_idx,
            col_count,
        }
    }

}

fn seed_unused() -> i64 {
    0
}

/// Compile-time y-invariance analysis over the compiled arena (DAG-safe,
/// iterative). Returns (col_idx, col_count): a Cache2d/FlatCache node whose
/// child subtree is provably y-independent gets a column-cache slot.
///
/// y-invariance rules (conservative): Const / ShiftA / ShiftB are invariant;
/// pure wrappers and arithmetic propagate; Noise is invariant iff y_scale==0;
/// ShiftedNoise iff y_scale==0 and all three shift inputs are invariant;
/// everything touching y directly (OldBlendedNoise, YClampedGradient,
/// WeirdScaledSampler, Spline, Interpolated, EndIslands) is variant.
fn compute_col_caches(arena: &[WKind]) -> (Vec<u32>, u32) {
    // 0 = unknown, 1 = in progress, 2 = y-invariant, 3 = y-variant
    let mut state = vec![0u8; arena.len()];
    let mut col_idx = vec![u32::MAX; arena.len()];
    let mut next_slot = 0u32;

    let children = |i: usize| -> Vec<u32> {
        match &arena[i] {
            WKind::Const(_)
            | WKind::ShiftA { .. }
            | WKind::ShiftB { .. }
            | WKind::OldBlendedNoise { .. }
            | WKind::YClampedGradient { .. }
            | WKind::EndIslands
            | WKind::Noise { .. }
            | WKind::Interpolated { .. }
            | WKind::WeirdScaledSampler { .. }
            | WKind::Spline { .. } => Vec::new(),
            WKind::Clamp { input, .. }
            | WKind::Mapped { input, .. }
            | WKind::MulOrAdd { input, .. }
            | WKind::Cache2d { child: input }
            | WKind::CacheOnce { child: input }
            | WKind::FlatCache { child: input }
            | WKind::CacheAllInCell { child: input } => vec![*input],
            WKind::Ap2 { a, b, .. } => vec![*a, *b],
            WKind::RangeChoice {
                input,
                when_in_range,
                when_out_of_range,
                ..
            } => vec![*input, *when_in_range, *when_out_of_range],
            WKind::ShiftedNoise { sx, sy, sz, .. } => vec![*sx, *sy, *sz],
        }
    };

    for root in 0..arena.len() {
        if state[root] != 0 {
            continue;
        }
        let mut stack = vec![(root as u32, false)];
        while let Some((i, expanded)) = stack.pop() {
            let iu = i as usize;
            if !expanded {
                if state[iu] != 0 {
                    continue;
                }
                let ch = children(iu);
                state[iu] = 1;
                stack.push((i, true));
                for c in ch {
                    if state[c as usize] == 0 {
                        stack.push((c, false));
                    }
                }
                continue;
            }
            if state[iu] != 1 {
                continue; // computed via a shared node earlier in this walk
            }
            let yinv = match &arena[iu] {
                WKind::Const(_) | WKind::ShiftA { .. } | WKind::ShiftB { .. } => true,
                WKind::MulOrAdd { input, .. }
                | WKind::Clamp { input, .. }
                | WKind::Mapped { input, .. }
                | WKind::Cache2d { child: input }
                | WKind::CacheOnce { child: input }
                | WKind::FlatCache { child: input }
                | WKind::CacheAllInCell { child: input } => state[*input as usize] == 2,
                WKind::Ap2 { a, b, .. } => {
                    state[*a as usize] == 2 && state[*b as usize] == 2
                }
                WKind::RangeChoice {
                    input,
                    when_in_range,
                    when_out_of_range,
                    ..
                } => {
                    state[*input as usize] == 2
                        && state[*when_in_range as usize] == 2
                        && state[*when_out_of_range as usize] == 2
                }
                WKind::ShiftedNoise {
                    y_scale, sx, sy, sz, ..
                } => {
                    *y_scale == 0.0
                        && state[*sx as usize] == 2
                        && state[*sy as usize] == 2
                        && state[*sz as usize] == 2
                }
                WKind::Noise { y_scale, .. } => *y_scale == 0.0,
                _ => false,
            };
            state[iu] = if yinv { 2 } else { 3 };
            match &arena[iu] {
                WKind::Cache2d { child } | WKind::FlatCache { child } => {
                    if state[*child as usize] == 2 {
                        col_idx[iu] = next_slot;
                        next_slot += 1;
                    }
                }
                _ => {}
            }
        }
    }
    (col_idx, next_slot)
}

/// The "minecraft:terrain" XoroshiroRandomSource for the given worldgen seed:
/// root = Xoroshiro(seed).forkPositional(); terrain = root.fromHashOf("minecraft:terrain").
fn root_terrain_factory(_wg: &WorldGen, seed: i64) -> crate::rng::XoroshiroRandomSource {
    let mut root = crate::rng::XoroshiroRandomSource::new(seed);
    let root_factory = root.fork_positional();
    root_factory.from_hash_of("minecraft:terrain")
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
#[derive(Clone)]
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

    /// Hoisted lerp3 inner rows: rows depend only on (dx, dy), so the driver
    /// computes them once per (iy, ix) and calls cellfill_row_z per iz.
    /// Values are bit-identical to cellfill_value (same expressions).
    #[inline]
    pub fn cellfill_rows(&self, slot: u16, dx: f64, dy: f64) -> (f64, f64) {
        let n = self.corners[slot as usize];
        let row1 = lerp(dy, lerp(dx, n[0], n[1]), lerp(dx, n[2], n[3]));
        let row2 = lerp(dy, lerp(dx, n[4], n[5]), lerp(dx, n[6], n[7]));
        (row1, row2)
    }

    #[inline]
    pub fn cellfill_row_z(row1: f64, row2: f64, dz: f64) -> f64 {
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

    pub fn slice0_get_pub(&self, slot: u16, z: usize, y: usize) -> f64 {
        let stride = self.slice_rows * self.slice_cols;
        self.slice0[slot as usize * stride + z * self.slice_cols + y]
    }
    pub fn slice1_get_pub(&self, slot: u16, z: usize, y: usize) -> f64 {
        let stride = self.slice_rows * self.slice_cols;
        self.slice1[slot as usize * stride + z * self.slice_cols + y]
    }
    pub fn slice1_set(&mut self, slot: u16, z: usize, y: usize, v: f64) {
        let stride = self.slice_rows * self.slice_cols;
        self.slice1[slot as usize * stride + z * self.slice_cols + y] = v;
    }
}

#[inline]
fn lerp(t: f64, a: f64, b: f64) -> f64 {
    a + t * (b - a)
}

// ---------------------------------------------------------------------------
// 4-wide batch evaluation (Slice mode, uncached) — bit-exact per lane.
// ---------------------------------------------------------------------------

pub type V4 = [f64; 4];

/// 4-lane memo for eval4: stamps the [f64; 4] result of every node within
/// one eval4 call (one point group). Shared DAG references (splines, ap2
/// chains) evaluate once per group instead of once per reference — pure
/// functions, so stamping cannot change values, only cost.
pub struct BatchMemo<const N: usize> {
    stamp: Box<[Cell<u64>]>,
    value: Box<[Cell<[f64; N]>]>,
    serial: Cell<u64>,
    /// Column-cache storage: one entry per y-independent Cache2d/FlatCache
    /// node (slot = Compiled::col_idx[node]). Valid across the whole plane
    /// eval — the memo is created per plane, so entries never go stale.
    col: Box<[Cell<ColEntry>]>,
}

#[derive(Clone, Copy)]
pub struct ColEntry {
    pub ok: bool,
    pub x: i32,
    pub z: i32,
    pub v: f64,
}

impl<const N: usize> BatchMemo<N> {
    pub fn new(n: usize) -> BatchMemo<N> {
        BatchMemo {
            stamp: vec![Cell::new(0); n].into_boxed_slice(),
            value: vec![Cell::new([0.0; N]); n].into_boxed_slice(),
            serial: Cell::new(0),
            col: Box::new([]),
        }
    }

    /// Allocate column-cache storage for a plane eval (col_count slots).
    pub fn with_cols(mut self, cols: usize) -> Self {
        self.col = vec![Cell::new(ColEntry { ok: false, x: 0, z: 0, v: 0.0 }); cols]
            .into_boxed_slice();
        self
    }

    #[inline]
    pub fn next_serial(&self) {
        self.serial.set(self.serial.get() + 1);
    }

    #[inline]
    fn key(&self) -> u64 {
        self.serial.get().wrapping_mul(8) + 1
    }
}

pub type V4Memo = BatchMemo<4>;
pub type V8Memo = BatchMemo<8>;

/// [f64; N] -> [f64; 4] view for the 4-wide noise kernels (callers guarantee
/// N == 4 via the const-folded dispatch branch).
#[inline]
fn arr4(a: &[f64], b: &[f64], c: &[f64]) -> ([f64; 4], [f64; 4], [f64; 4]) {
    let mut x = [0.0f64; 4];
    let mut y = [0.0f64; 4];
    let mut z = [0.0f64; 4];
    x.copy_from_slice(a);
    y.copy_from_slice(b);
    z.copy_from_slice(c);
    (x, y, z)
}

/// [f64; N] -> [f64; 8] view for the 8-wide noise kernels (callers guarantee
/// N == 8 via the const-folded dispatch branch).
#[inline]
fn arr8(a: &[f64], b: &[f64], c: &[f64]) -> ([f64; 8], [f64; 8], [f64; 8]) {
    let mut x = [0.0f64; 8];
    let mut y = [0.0f64; 8];
    let mut z = [0.0f64; 8];
    x.copy_from_slice(a);
    y.copy_from_slice(b);
    z.copy_from_slice(c);
    (x, y, z)
}

/// [i32; N] -> ([i32; 4]; ...) view.
#[inline]
fn arri4(a: &[i32], b: &[i32], c: &[i32]) -> ([i32; 4], [i32; 4], [i32; 4]) {
    let mut x = [0i32; 4];
    let mut y = [0i32; 4];
    let mut z = [0i32; 4];
    x.copy_from_slice(a);
    y.copy_from_slice(b);
    z.copy_from_slice(c);
    (x, y, z)
}

/// [i32; N] -> ([i32; 8]; ...) view.
#[inline]
fn arri8(a: &[i32], b: &[i32], c: &[i32]) -> ([i32; 8], [i32; 8], [i32; 8]) {
    let mut x = [0i32; 8];
    let mut y = [0i32; 8];
    let mut z = [0i32; 8];
    x.copy_from_slice(a);
    y.copy_from_slice(b);
    z.copy_from_slice(c);
    (x, y, z)
}

/// Identity view &[f64; N] -> &[f64; M]: only used inside const-folded
/// branches where N == M is guaranteed (dead branch is never executed).
#[inline]
fn narrow_ref<T, const N: usize, const M: usize>(a: &[T; N]) -> &[T; M] {
    debug_assert_eq!(N, M);
    unsafe { &*(a as *const [T; N] as *const [T; M]) }
}

/// Noise-node dispatch: route [f64; N] arrays into the 4-wide or 8-wide
/// batch kernels (bit-exact per lane in both). N is a const-generic known at
/// monomorphization time, so the dead branch is compiled out (the runtime
/// len checks in copy_from_slice never fire).
/// Diagnostic phase counters (CRUS_PROFILE=1): AVX noise batch time inside
/// the slice tree walk. Zero-cost when profiling is off.
pub static PROF_NOISE_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static PROF_BLEND_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static PROF_NOISE_CALLS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static PROF_BLEND_CALLS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static PROF_ON: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[inline]
fn noise_batch_n<const N: usize>(
    n: &crate::noise::NormalNoise,
    x: &[f64; N],
    y: &[f64; N],
    z: &[f64; N],
) -> [f64; N] {
    // try_into on &[f64; N] is an identity cast when N matches the target —
    // zero copies at monomorphization.
    let mut out = [0.0f64; N];
    let t0 = if crate::density::PROF_ON.load(std::sync::atomic::Ordering::Relaxed) {
        Some(std::time::Instant::now())
    } else {
        None
    };
    if N == 8 {
        let v = n.get_value_batch8(narrow_ref(x), narrow_ref(y), narrow_ref(z));
        out.copy_from_slice(&v);
    } else {
        let v = n.get_value_batch(narrow_ref(x), narrow_ref(y), narrow_ref(z));
        out.copy_from_slice(&v);
    }
    if let Some(t0) = t0 {
        PROF_NOISE_NS.fetch_add(
            t0.elapsed().as_nanos() as u64,
            std::sync::atomic::Ordering::Relaxed,
        );
    }
    if PROF_ON.load(std::sync::atomic::Ordering::Relaxed) {
        PROF_NOISE_CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    out
}

#[inline]
fn blended_batch_n<const N: usize>(
    blended: &[crate::noise::BlendedNoise],
    instance: usize,
    bx: &[i32; N],
    by: &[i32; N],
    bz: &[i32; N],
) -> [f64; N] {
    let mut out = [0.0f64; N];
    let t0 = if PROF_ON.load(std::sync::atomic::Ordering::Relaxed) {
        Some(std::time::Instant::now())
    } else {
        None
    };
    if N == 8 {
        let v = blended[instance].compute_batch8(
            narrow_ref(bx),
            narrow_ref(by),
            narrow_ref(bz),
        );
        out.copy_from_slice(&v);
    } else {
        let v = blended[instance].compute_batch(
            narrow_ref(bx),
            narrow_ref(by),
            narrow_ref(bz),
        );
        out.copy_from_slice(&v);
    }
    if let Some(t0) = t0 {
        PROF_BLEND_NS.fetch_add(
            t0.elapsed().as_nanos() as u64,
            std::sync::atomic::Ordering::Relaxed,
        );
    }
    if PROF_ON.load(std::sync::atomic::Ordering::Relaxed) {
        PROF_BLEND_CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    out
}

impl Env {
    /// Evaluate `idx` at N points simultaneously (Mode::Slice semantics).
    /// Lane arithmetic is identical to the scalar eval, so results are
    /// bit-exact. Each call gets a fresh memo serial: shared subtrees are
    /// computed once per call (per point group).
    pub fn eval_batch<const N: usize>(
        &self,
        c: &Compiled,
        idx: u32,
        pts: &[Pt; N],
        mode: Mode,
        interp: &InterpState,
        memo: &BatchMemo<N>,
    ) -> [f64; N] {
        memo.next_serial();
        self.eval_batch_uncached(c, idx, pts, mode, interp, memo)
    }

    /// 4-lane convenience wrapper (legacy API).
    pub fn eval4(
        &self,
        c: &Compiled,
        idx: u32,
        pts: &[Pt; 4],
        mode: Mode,
        interp: &InterpState,
        memo: &V4Memo,
    ) -> V4 {
        self.eval_batch(c, idx, pts, mode, interp, memo)
    }

    /// 8-lane convenience wrapper.
    pub fn eval8(
        &self,
        c: &Compiled,
        idx: u32,
        pts: &[Pt; 8],
        mode: Mode,
        interp: &InterpState,
        memo: &V8Memo,
    ) -> [f64; 8] {
        self.eval_batch(c, idx, pts, mode, interp, memo)
    }

    fn eval_batch_uncached<const N: usize>(
        &self,
        c: &Compiled,
        idx: u32,
        pts: &[Pt; N],
        mode: Mode,
        interp: &InterpState,
        memo: &BatchMemo<N>,
    ) -> [f64; N] {
        let i = idx as usize;
        if memo.stamp[i].get() == memo.key() {
            return memo.value[i].get();
        }
        let v = self.eval_batch_recurse(c, idx, pts, mode, interp, memo);
        memo.stamp[i].set(memo.key());
        memo.value[i].set(v);
        v
    }

    fn eval_batch_recurse<const N: usize>(
        &self,
        c: &Compiled,
        idx: u32,
        pts: &[Pt; N],
        mode: Mode,
        interp: &InterpState,
        memo: &BatchMemo<N>,
    ) -> [f64; N] {
        let node = &c.arena[idx as usize];
        match node {
            WKind::Const(v) => [*v; N],
            WKind::Noise { noise_id, xz_scale, y_scale } => {
                let n = &self.noise.list[c.noise_resolved[*noise_id as usize] as usize];
                let mut x = [0f64; N];
                let mut y = [0f64; N];
                let mut z = [0f64; N];
                for l in 0..N {
                    x[l] = pts[l].x as f64 * xz_scale;
                    y[l] = pts[l].y as f64 * y_scale;
                    z[l] = pts[l].z as f64 * xz_scale;
                }
                let v = noise_batch_n(n, &x, &y, &z);
                let mut out = [0f64; N];
                out.copy_from_slice(&v);
                out
            }
            WKind::ShiftedNoise { noise_id, xz_scale, y_scale, sx, sy, sz } => {
                let sxa = self.eval_batch_uncached(c, *sx, pts, mode, interp, memo);
                let sya = self.eval_batch_uncached(c, *sy, pts, mode, interp, memo);
                let sza = self.eval_batch_uncached(c, *sz, pts, mode, interp, memo);
                let n = &self.noise.list[c.noise_resolved[*noise_id as usize] as usize];
                let mut x = [0f64; N];
                let mut y = [0f64; N];
                let mut z = [0f64; N];
                for l in 0..N {
                    x[l] = pts[l].x as f64 * xz_scale + sxa[l];
                    y[l] = pts[l].y as f64 * y_scale + sya[l];
                    z[l] = pts[l].z as f64 * xz_scale + sza[l];
                }
                let v = noise_batch_n(n, &x, &y, &z);
                let mut out = [0f64; N];
                out.copy_from_slice(&v);
                out
            }
            WKind::ShiftA { noise_id } => {
                let n = &self.noise.list[c.noise_resolved[*noise_id as usize] as usize];
                let mut x = [0f64; N];
                let mut z = [0f64; N];
                for l in 0..N {
                    x[l] = pts[l].x as f64 * 0.25;
                    z[l] = pts[l].z as f64 * 0.25;
                }
                let y = [0.0f64; N];
                let vb = noise_batch_n(n, &x, &y, &z);
                let mut out = [0f64; N];
                for l in 0..N {
                    out[l] = vb[l] * 4.0;
                }
                out
            }
            WKind::ShiftB { noise_id } => {
                let n = &self.noise.list[c.noise_resolved[*noise_id as usize] as usize];
                // vanilla ShiftNoise.compute(z, x, 0): x_arg = z*0.25,
                // y_arg = x*0.25, z_arg = 0 (matches the scalar arm — the
                // x/z SWAP reading was wrong and diverged from eval!)
                let mut x = [0f64; N];
                let mut y = [0f64; N];
                for l in 0..N {
                    x[l] = pts[l].z as f64 * 0.25;
                    y[l] = pts[l].x as f64 * 0.25;
                }
                let z = [0.0f64; N];
                let vb = noise_batch_n(n, &x, &y, &z);
                let mut out = [0f64; N];
                for l in 0..N {
                    out[l] = vb[l] * 4.0;
                }
                out
            }
            WKind::OldBlendedNoise { instance } => {
                let mut bx = [0i32; N];
                let mut by = [0i32; N];
                let mut bz = [0i32; N];
                for l in 0..N {
                    bx[l] = pts[l].x;
                    by[l] = pts[l].y;
                    bz[l] = pts[l].z;
                }
                let v = blended_batch_n(&c.blended, *instance as usize, &bx, &by, &bz);
                let mut out = [0f64; N];
                out.copy_from_slice(&v);
                out
            }
            WKind::Clamp { input, min, max } => {
                let v = self.eval_batch_uncached(c, *input, pts, mode, interp, memo);
                let mut out = [0f64; N];
                for l in 0..N {
                    out[l] = mth_clamp(v[l], *min, *max);
                }
                out
            }
            WKind::Mapped { kind, input } => {
                let v = self.eval_batch_uncached(c, *input, pts, mode, interp, memo);
                let mut out = [0f64; N];
                match kind {
                    MapKind::Abs => {
                        for l in 0..N { out[l] = v[l].abs(); }
                    }
                    MapKind::Square => {
                        for l in 0..N { out[l] = v[l] * v[l]; }
                    }
                    MapKind::Cube => {
                        for l in 0..N { out[l] = v[l] * v[l] * v[l]; }
                    }
                    MapKind::HalfNegative => {
                        for l in 0..N { out[l] = if v[l] > 0.0 { v[l] } else { v[l] * 0.5 }; }
                    }
                    MapKind::QuarterNegative => {
                        for l in 0..N { out[l] = if v[l] > 0.0 { v[l] } else { v[l] * 0.25 }; }
                    }
                    MapKind::Squeeze => {
                        for l in 0..N {
                            let d = mth_clamp(v[l], -1.0, 1.0);
                            out[l] = d / 2.0 - d * d * d / 24.0;
                        }
                    }
                }
                out
            }
            WKind::Ap2 { kind, a, b } => {
                let va = self.eval_batch_uncached(c, *a, pts, mode, interp, memo);
                match kind {
                    Ap2Kind::Add => {
                        let vb = self.eval_batch_uncached(c, *b, pts, mode, interp, memo);
                        let mut out = [0f64; N];
                        for l in 0..N { out[l] = va[l] + vb[l]; }
                        out
                    }
                    Ap2Kind::Mul => {
                        let mut out = [0f64; N];
                        // vanilla short-circuit per lane
                        let mut need = [false; N];
                        for l in 0..N {
                            if va[l] == 0.0 {
                                out[l] = 0.0;
                            } else {
                                need[l] = true;
                            }
                        }
                        if need.iter().any(|n| *n) {
                            let vb = self.eval_batch_uncached(c, *b, pts, mode, interp, memo);
                            for l in 0..N {
                                if need[l] {
                                    out[l] = va[l] * vb[l];
                                }
                            }
                        }
                        out
                    }
                    Ap2Kind::Min => {
                        let vb = self.eval_batch_uncached(c, *b, pts, mode, interp, memo);
                        let mut out = [0f64; N];
                        for l in 0..N { out[l] = if va[l] < vb[l] { va[l] } else { vb[l] }; }
                        out
                    }
                    Ap2Kind::Max => {
                        let vb = self.eval_batch_uncached(c, *b, pts, mode, interp, memo);
                        let mut out = [0f64; N];
                        for l in 0..N { out[l] = if va[l] > vb[l] { va[l] } else { vb[l] }; }
                        out
                    }
                }
            }
            WKind::RangeChoice { input, min_inclusive, max_exclusive, when_in_range, when_out_of_range } => {
                let v = self.eval_batch_uncached(c, *input, pts, mode, interp, memo);
                let mut in_mask = [false; N];
                let mut any_in = false;
                let mut any_out = false;
                for l in 0..N {
                    let m = v[l] >= *min_inclusive && v[l] < *max_exclusive;
                    in_mask[l] = m;
                    if m { any_in = true; } else { any_out = true; }
                }
                let vin = if any_in { self.eval_batch_uncached(c, *when_in_range, pts, mode, interp, memo) } else { [0.0; N] };
                let vout = if any_out { self.eval_batch_uncached(c, *when_out_of_range, pts, mode, interp, memo) } else { [0.0; N] };
                let mut out = [0f64; N];
                for l in 0..N {
                    out[l] = if in_mask[l] { vin[l] } else { vout[l] };
                }
                out
            }
            WKind::YClampedGradient { from_y, to_y, from_value, to_value } => {
                let mut out = [0f64; N];
                for l in 0..N {
                    let t = (pts[l].y as f64 - *from_y as f64) / (*to_y as f64 - *from_y as f64);
                    out[l] = if t < 0.0 {
                        *from_value
                    } else if t > 1.0 {
                        *to_value
                    } else {
                        lerp(t, *from_value, *to_value)
                    };
                }
                out
            }
            WKind::Spline { coordinate, points } => {
                let f4 = self.eval_batch_uncached(c, *coordinate, pts, mode, interp, memo);
                let mut f = [0f32; N];
                for l in 0..N {
                    f[l] = f4[l] as f32;
                }
                let mut out = [0f64; N];
                for l in 0..N {
                    out[l] = spline_apply(self, c, points, f[l], pts[l], mode, interp, None) as f64;
                }
                out
            }
            WKind::WeirdScaledSampler { noise_id, mapper_type2, input } => {
                let v = self.eval_batch_uncached(c, *input, pts, mode, interp, memo);
                let n = &self.noise.list[c.noise_resolved[*noise_id as usize] as usize];
                let mut d = [1.0f64; N];
                for l in 0..N {
                    d[l] = if *mapper_type2 {
                        if v[l] < -0.75 { 0.5 }
                        else if v[l] < -0.5 { 0.75 }
                        else if v[l] < 0.5 { 1.0 }
                        else if v[l] < 0.75 { 2.0 }
                        else { 3.0 }
                    } else {
                        if v[l] < -0.5 { 0.75 }
                        else if v[l] < 0.0 { 1.0 }
                        else if v[l] < 0.5 { 1.5 }
                        else { 2.0 }
                    };
                }
                let mut x = [0f64; N];
                let mut y = [0f64; N];
                let mut z = [0f64; N];
                for l in 0..N {
                    x[l] = pts[l].x as f64 / d[l];
                    y[l] = pts[l].y as f64 / d[l];
                    z[l] = pts[l].z as f64 / d[l];
                }
                let nv = noise_batch_n(n, &x, &y, &z);
                let mut out = [0f64; N];
                for l in 0..N {
                    out[l] = d[l] * nv[l].abs();
                }
                out
            }
            WKind::Cache2d { child } | WKind::FlatCache { child }
                if c.col_idx[idx as usize] != u32::MAX =>
            {
                // y-independent child (compile-time proven): all lanes share
                // the same (x, z) in a slice batch, so one value serves the
                // whole column — bit-exact reuse of a pure function.
                let ci = c.col_idx[idx as usize] as usize;
                let x0 = pts[0].x;
                let z0 = pts[0].z;
                let mut uniform = true;
                for l in 1..N {
                    if pts[l].x != x0 || pts[l].z != z0 {
                        uniform = false;
                        break;
                    }
                }
                if uniform && ci < memo.col.len() {
                    let e = memo.col[ci].get();
                    if e.ok && e.x == x0 && e.z == z0 {
                        let mut out = [0.0f64; N];
                        out.fill(e.v);
                        return out;
                    }
                    let v = self.eval_batch_uncached(c, *child, pts, mode, interp, memo);
                    memo.col[ci].set(ColEntry { ok: true, x: x0, z: z0, v: v[0] });
                    return v;
                }
                self.eval_batch_uncached(c, *child, pts, mode, interp, memo)
            }
            WKind::Cache2d { child }
            | WKind::CacheOnce { child }
            | WKind::FlatCache { child }
            | WKind::CacheAllInCell { child } => self.eval_batch_uncached(c, *child, pts, mode, interp, memo),
            WKind::MulOrAdd { is_add, input, argument } => {
                let v = self.eval_batch_uncached(c, *input, pts, mode, interp, memo);
                let mut out = [0f64; N];
                for l in 0..N {
                    out[l] = if *is_add {
                        v[l] + argument
                    } else if *argument == 0.0 {
                        0.0
                    } else {
                        v[l] * argument
                    };
                }
                out
            }
            WKind::Interpolated { child, .. } => {
                // Slice mode: direct child evaluation (same as scalar)
                self.eval_batch_uncached(c, *child, pts, mode, interp, memo)
            }
            WKind::EndIslands => [0.0; N],
            _ => {
                // Unreachable on the overworld Slice path (CellFill-only
                // markers); fall back to scalar lanes for safety.
                let mut out = [0f64; N];
                for l in 0..N {
                    out[l] = self.eval_uncached(c, idx, pts[l], mode, interp, None);
                }
                out
            }
        }
    }
}

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
    pub fn eval(
        &self,
        c: &Compiled,
        idx: u32,
        pt: Pt,
        mode: Mode,
        interp: &InterpState,
        mem: Option<&MemoCtx>,
    ) -> f64 {
        if let Some(m) = mem {
            if let Some(v) = m.hit(idx as usize, mode) {
                return v;
            }
            let v = self.eval_uncached(c, idx, pt, mode, interp, mem);
            m.store(idx as usize, mode, v);
            return v;
        }
        self.eval_uncached(c, idx, pt, mode, interp, mem)
    }

    fn eval_uncached(
        &self,
        c: &Compiled,
        idx: u32,
        pt: Pt,
        mode: Mode,
        interp: &InterpState,
        mem: Option<&MemoCtx>,
    ) -> f64 {
        let node = &c.arena[idx as usize];
        match node {
            WKind::Const(v) => *v,
            WKind::Noise { noise_id, xz_scale, y_scale } => {
                let n = &self.noise.list[c.noise_resolved[*noise_id as usize] as usize];
                n.get_value(pt.x as f64 * xz_scale, pt.y as f64 * y_scale, pt.z as f64 * xz_scale)
            }
            WKind::ShiftedNoise { noise_id, xz_scale, y_scale, sx, sy, sz } => {
                let dx = pt.x as f64 * xz_scale + self.eval(c, *sx, pt, mode, interp, mem);
                let dy = pt.y as f64 * y_scale + self.eval(c, *sy, pt, mode, interp, mem);
                let dz = pt.z as f64 * xz_scale + self.eval(c, *sz, pt, mode, interp, mem);
                let n = &self.noise.list[c.noise_resolved[*noise_id as usize] as usize];
                n.get_value(dx, dy, dz)
            }
            WKind::ShiftA { noise_id } => {
                let n = &self.noise.list[c.noise_resolved[*noise_id as usize] as usize];
                // ShiftNoise.compute(x, 0, z): noise(x*0.25, 0, z*0.25) * 4
                n.get_value(pt.x as f64 * 0.25, 0.0, pt.z as f64 * 0.25) * 4.0
            }
            WKind::ShiftB { noise_id } => {
                let n = &self.noise.list[c.noise_resolved[*noise_id as usize] as usize];
                // ShiftNoise.compute(z, x, 0)
                n.get_value(pt.z as f64 * 0.25, pt.x as f64 * 0.25, 0.0) * 4.0
            }
            WKind::OldBlendedNoise { instance } => {
                c.blended[*instance as usize].compute(pt.x, pt.y, pt.z)
            }
            WKind::Clamp { input, min, max } => mth_clamp(self.eval(c, *input, pt, mode, interp, mem), *min, *max),
            WKind::Mapped { kind, input } => {
                let v = self.eval(c, *input, pt, mode, interp, mem);
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
                let va = self.eval(c, *a, pt, mode, interp, mem);
                match kind {
                    Ap2Kind::Add => va + self.eval(c, *b, pt, mode, interp, mem),
                    Ap2Kind::Mul => {
                        if va == 0.0 {
                            0.0
                        } else {
                            va * self.eval(c, *b, pt, mode, interp, mem)
                        }
                    }
                    Ap2Kind::Min => {
                        let vb = self.eval(c, *b, pt, mode, interp, mem);
                        if va < vb {
                            va
                        } else {
                            vb
                        }
                    }
                    Ap2Kind::Max => {
                        let vb = self.eval(c, *b, pt, mode, interp, mem);
                        if va > vb {
                            va
                        } else {
                            vb
                        }
                    }
                }
            }
            WKind::RangeChoice { input, min_inclusive, max_exclusive, when_in_range, when_out_of_range } => {
                let v = self.eval(c, *input, pt, mode, interp, mem);
                if v >= *min_inclusive && v < *max_exclusive {
                    self.eval(c, *when_in_range, pt, mode, interp, mem)
                } else {
                    self.eval(c, *when_out_of_range, pt, mode, interp, mem)
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
                let f = self.eval(c, *coordinate, pt, mode, interp, mem) as f32;
                spline_apply(self, c, points, f, pt, mode, interp, mem) as f64
            }
            WKind::WeirdScaledSampler { noise_id, mapper_type2, input } => {
                let v = self.eval(c, *input, pt, mode, interp, mem);
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
                let n = &self.noise.list[c.noise_resolved[*noise_id as usize] as usize];
                let v = n.get_value(
                    pt.x as f64 / d,
                    pt.y as f64 / d,
                    pt.z as f64 / d,
                );
                d * v.abs()
            }
            WKind::Cache2d { child }
            | WKind::CacheOnce { child }
            | WKind::FlatCache { child }
            | WKind::CacheAllInCell { child } => self.eval(c, *child, pt, mode, interp, mem),
            WKind::MulOrAdd { is_add, input, argument } => {
                let v = self.eval(c, *input, pt, mode, interp, mem);
                if *is_add {
                    v + argument
                } else if *argument == 0.0 {
                    // vanilla Ap2 MUL short-circuit on the constant side
                    0.0
                } else {
                    v * argument
                }
            }
            WKind::Interpolated { child, slot } => match mode {
                Mode::Direct | Mode::Slice => self.eval(c, *child, pt, mode, interp, mem),
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
    mem: Option<&MemoCtx>,
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
                let d = env.eval(c, *node, pt, mode, interp, mem);
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

impl WKind {
    /// Java class simple name for walk-parity output.
    pub fn java_kind(&self) -> &'static str {
        match self {
            WKind::Const(_) => "Constant",
            WKind::Noise { .. } => "Noise",
            WKind::ShiftedNoise { .. } => "ShiftedNoise",
            WKind::ShiftA { .. } => "ShiftA",
            WKind::ShiftB { .. } => "ShiftB",
            WKind::OldBlendedNoise { .. } => "BlendedNoise",
            WKind::Clamp { .. } => "Clamp",
            WKind::Mapped { kind, .. } => match kind {
                MapKind::Abs => "Mapped",
                MapKind::Square => "Mapped",
                MapKind::Cube => "Mapped",
                MapKind::HalfNegative => "Mapped",
                MapKind::QuarterNegative => "Mapped",
                MapKind::Squeeze => "Mapped",
            },
            WKind::Ap2 { .. } => "Ap2",
            WKind::MulOrAdd { .. } => "MulOrAdd",
            WKind::RangeChoice { .. } => "RangeChoice",
            WKind::YClampedGradient { .. } => "YClampedGradient",
            WKind::Spline { .. } => "Spline",
            WKind::WeirdScaledSampler { .. } => "WeirdScaledSampler",
            WKind::Interpolated { .. } => "NoiseInterpolator",
            WKind::Cache2d { .. } => "Cache2D",
            WKind::CacheOnce { .. } => "CacheOnce",
            WKind::FlatCache { .. } => "FlatCache",
            WKind::CacheAllInCell { .. } => "CacheAllInCell",
            WKind::EndIslands => "EndIslandDensityFunction",
        }
    }

    /// Single child index for PureTransformer-shaped nodes.
    pub fn child_of(&self) -> Option<u32> {
        match self {
            WKind::Clamp { input, .. } | WKind::Mapped { input, .. } => Some(*input),
            _ => None,
        }
    }
}

impl InterpState {
    /// Copy helpers for the cross-chunk corner-plane cache (raw f64 copy of
    /// the slice0 buffer; layout is slot-major and stable).
    pub fn copy_slice0_from(&mut self, src: &[f64]) {
        let n = self.slice0.len().min(src.len());
        self.slice0[..n].copy_from_slice(&src[..n]);
    }

    pub fn copy_slice0_into(&self, dst: &mut Vec<f64>) {
        dst.clear();
        dst.extend_from_slice(&self.slice0);
    }
}

// ---------------------------------------------------------------------------
// compiled CellFill program (straight-line replay of the density tree for
// the CacheAllInCell hot path)
// ---------------------------------------------------------------------------

/// One operation of the CellFill program (post-order = exact vanilla
/// evaluation order). Only PURE nodes are compilable; anything that samples
/// noise per block aborts compilation and the driver falls back to the
/// interpreter. Values are bit-identical to Env::eval in Mode::CellFill.
#[derive(Debug, Clone)]
pub enum CellOp {
    PushConst(f64),
    /// lerp3 of corners[slot] at the in-cell fractions (vanilla
    /// NoiseInterpolator.compute with fillingCell=true).
    Lerp3 { slot: u16 },
    /// YClampedGradient at the current block y.
    YGrad { from_y: i32, to_y: i32, from_value: f64, to_value: f64 },
    Add,
    Mul,
    Min,
    Max,
    Abs,
    Square,
    Cube,
    HalfNeg,
    QuarterNeg,
    Squeeze,
    Clamp { min: f64, max: f64 },
    RangeChoice { min_inclusive: f64, max_exclusive: f64 },
    MulOrAdd { is_add: bool, argument: f64 },
}

/// Compile the CellFill program for `root`, or None if the tree contains
/// nodes that require per-block noise sampling (raw noises, splines with
/// node values, weird samplers, end islands).
pub fn compile_cellfill(arena: &[WKind], root: u32) -> Option<Vec<CellOp>> {
    let mut prog = Vec::new();
    let mut stack = vec![(root, false)];
    while let Some((idx, expanded)) = stack.pop() {
        if expanded {
            // emit the parent op after its children
            match &arena[idx as usize] {
                WKind::Const(v) => prog.push(CellOp::PushConst(*v)),
                WKind::Clamp { min, max, .. } => prog.push(CellOp::Clamp { min: *min, max: *max }),
                WKind::Mapped { kind, .. } => prog.push(match kind {
                    MapKind::Abs => CellOp::Abs,
                    MapKind::Square => CellOp::Square,
                    MapKind::Cube => CellOp::Cube,
                    MapKind::HalfNegative => CellOp::HalfNeg,
                    MapKind::QuarterNegative => CellOp::QuarterNeg,
                    MapKind::Squeeze => CellOp::Squeeze,
                }),
                WKind::Ap2 { kind, .. } => prog.push(match kind {
                    Ap2Kind::Add => CellOp::Add,
                    Ap2Kind::Mul => CellOp::Mul,
                    Ap2Kind::Min => CellOp::Min,
                    Ap2Kind::Max => CellOp::Max,
                }),
                WKind::RangeChoice { min_inclusive, max_exclusive, .. } => prog.push(CellOp::RangeChoice {
                    min_inclusive: *min_inclusive,
                    max_exclusive: *max_exclusive,
                }),
                WKind::YClampedGradient { from_y, to_y, from_value, to_value } => {
                    prog.push(CellOp::YGrad {
                        from_y: *from_y,
                        to_y: *to_y,
                        from_value: *from_value,
                        to_value: *to_value,
                    })
                }
                WKind::Interpolated { slot, .. } => prog.push(CellOp::Lerp3 { slot: *slot }),
                WKind::MulOrAdd { is_add, argument, .. } => {
                    prog.push(CellOp::MulOrAdd { is_add: *is_add, argument: *argument })
                }
                // pass-through wrappers: child already emitted, emit nothing
                WKind::Cache2d { .. } | WKind::CacheOnce { .. } | WKind::FlatCache { .. }
                | WKind::CacheAllInCell { .. } => {}
                // per-block noise sampling / unsupported: abort
                _ => return None,
            }
            continue;
        }
        let node = &arena[idx as usize];
        match node {
            WKind::Const(_) | WKind::Interpolated { .. } | WKind::YClampedGradient { .. } => {
                stack.push((idx, true));
            }
            WKind::Clamp { input, .. }
            | WKind::Mapped { input, .. }
            | WKind::MulOrAdd { input, .. }
            | WKind::Cache2d { child: input }
            | WKind::CacheOnce { child: input }
            | WKind::FlatCache { child: input }
            | WKind::CacheAllInCell { child: input } => {
                stack.push((idx, true));
                stack.push((*input, false));
            }
            WKind::Ap2 { a, b, .. } => {
                stack.push((idx, true));
                stack.push((*b, false));
                stack.push((*a, false));
            }
            WKind::RangeChoice { input, when_in_range, when_out_of_range, .. } => {
                stack.push((idx, true));
                stack.push((*when_out_of_range, false));
                stack.push((*when_in_range, false));
                stack.push((*input, false));
            }
            // unsupported in the compiled program
            _ => return None,
        }
    }
    Some(prog)
}

/// Flat (static-stack-slot) form of a CellFill program: the postfix program's
/// stack depth is a static property, so every op's destination and operand
/// slots are resolved once at compile time. The AVX2 runner indexes its
/// register file directly — no stack-pointer dependency chain, half the µops.
#[derive(Clone, Copy, Debug)]
pub enum FlatOp {
    Const { dst: u8, v: f64 },
    Lerp3 { dst: u8, slot: u16 },
    YGrad { dst: u8, from_y: i32, to_y: i32, from_value: f64, to_value: f64 },
    Add { dst: u8, a: u8, b: u8 },
    Mul { dst: u8, a: u8, b: u8 },
    Min { dst: u8, a: u8, b: u8 },
    Max { dst: u8, a: u8, b: u8 },
    Abs { dst: u8, a: u8 },
    Square { dst: u8, a: u8 },
    Cube { dst: u8, a: u8 },
    HalfNeg { dst: u8, a: u8 },
    QuarterNeg { dst: u8, a: u8 },
    Squeeze { dst: u8, a: u8 },
    Clamp { dst: u8, a: u8, min: f64, max: f64 },
    RangeChoice { dst: u8, a: u8, min_inclusive: f64, max_exclusive: f64 },
    MulOrAdd { dst: u8, a: u8, is_add: bool, argument: f64 },
}

#[derive(Clone, Debug)]
pub struct FlatProg {
    pub ops: Vec<FlatOp>,
    /// Stack slot of the program result (the last op's destination).
    pub result: u8,
}

/// Resolve static stack slots. Returns None if the program's stack exceeds
/// the runner's 16-slot register file (same bound as the stack machine).
pub fn flatten_cellfill(prog: &[CellOp]) -> Option<FlatProg> {
    let mut sp = 0usize;
    let mut ops = Vec::with_capacity(prog.len());
    let mut result = 0u8;
    for op in prog {
        let f = match *op {
            CellOp::PushConst(v) => {
                let f = FlatOp::Const { dst: sp as u8, v };
                sp += 1;
                f
            }
            CellOp::Lerp3 { slot } => {
                let f = FlatOp::Lerp3 { dst: sp as u8, slot };
                sp += 1;
                f
            }
            CellOp::YGrad { from_y, to_y, from_value, to_value } => {
                let f = FlatOp::YGrad { dst: sp as u8, from_y, to_y, from_value, to_value };
                sp += 1;
                f
            }
            // binary ops pop 2 push 1: with sp already decremented to S-1,
            // the operands sit at b = sp (old top) and a = sp - 1, dst = sp - 1
            CellOp::Add => {
                sp -= 1;
                FlatOp::Add { dst: (sp - 1) as u8, a: (sp - 1) as u8, b: sp as u8 }
            }
            CellOp::Mul => {
                sp -= 1;
                FlatOp::Mul { dst: (sp - 1) as u8, a: (sp - 1) as u8, b: sp as u8 }
            }
            CellOp::Min => {
                sp -= 1;
                FlatOp::Min { dst: (sp - 1) as u8, a: (sp - 1) as u8, b: sp as u8 }
            }
            CellOp::Max => {
                sp -= 1;
                FlatOp::Max { dst: (sp - 1) as u8, a: (sp - 1) as u8, b: sp as u8 }
            }
            CellOp::Abs => FlatOp::Abs { dst: (sp - 1) as u8, a: (sp - 1) as u8 },
            CellOp::Square => FlatOp::Square { dst: (sp - 1) as u8, a: (sp - 1) as u8 },
            CellOp::Cube => FlatOp::Cube { dst: (sp - 1) as u8, a: (sp - 1) as u8 },
            CellOp::HalfNeg => FlatOp::HalfNeg { dst: (sp - 1) as u8, a: (sp - 1) as u8 },
            CellOp::QuarterNeg => FlatOp::QuarterNeg { dst: (sp - 1) as u8, a: (sp - 1) as u8 },
            CellOp::Squeeze => FlatOp::Squeeze { dst: (sp - 1) as u8, a: (sp - 1) as u8 },
            CellOp::Clamp { min, max } => {
                FlatOp::Clamp { dst: (sp - 1) as u8, a: (sp - 1) as u8, min, max }
            }
            CellOp::RangeChoice { min_inclusive, max_exclusive } => {
                // stack order: input, when_in, when_out (top)
                FlatOp::RangeChoice {
                    dst: (sp - 3) as u8,
                    a: (sp - 3) as u8,
                    min_inclusive,
                    max_exclusive,
                }
            }
            CellOp::MulOrAdd { is_add, argument } => {
                FlatOp::MulOrAdd { dst: (sp - 1) as u8, a: (sp - 1) as u8, is_add, argument }
            }
        };
        match f {
            FlatOp::RangeChoice { .. } => sp -= 2,
            _ => {}
        }
        if sp > 16 {
            return None;
        }
        result = match f {
            FlatOp::Const { dst, .. }
            | FlatOp::Lerp3 { dst, .. }
            | FlatOp::YGrad { dst, .. }
            | FlatOp::Add { dst, .. }
            | FlatOp::Mul { dst, .. }
            | FlatOp::Min { dst, .. }
            | FlatOp::Max { dst, .. }
            | FlatOp::Abs { dst, .. }
            | FlatOp::Square { dst, .. }
            | FlatOp::Cube { dst, .. }
            | FlatOp::HalfNeg { dst, .. }
            | FlatOp::QuarterNeg { dst, .. }
            | FlatOp::Squeeze { dst, .. }
            | FlatOp::Clamp { dst, .. }
            | FlatOp::RangeChoice { dst, .. }
            | FlatOp::MulOrAdd { dst, .. } => dst,
        };
        ops.push(f);
    }
    if prog.is_empty() {
        return None;
    }
    Some(FlatProg { ops, result })
}

impl InterpState {
    /// cellfill_value with explicit fractions (no cell_frac dependency).
    #[inline]
    pub fn cellfill_value_at(&self, slot: u16, dx: f64, dy: f64, dz: f64) -> f64 {
        let n = self.corners[slot as usize];
        let row1 = lerp(dy, lerp(dx, n[0], n[1]), lerp(dx, n[2], n[3]));
        let row2 = lerp(dy, lerp(dx, n[4], n[5]), lerp(dx, n[6], n[7]));
        lerp(dz, row1, row2)
    }
}

impl InterpState {
    /// Read-only corner access for the cellfill rows pass.
    #[inline]
    pub fn corners_pub(&self, slot: u16) -> &[f64; 8] {
        &self.corners[slot as usize]
    }

    /// Number of interpolated slots (cellfill row-hoisting scratch sizing).
    #[inline]
    pub fn slot_count(&self) -> usize {
        self.corners.len()
    }
}

#[cfg(test)]
mod eval4_parity_tests {
    use super::*;

    /// Regression: eval4 must stay bit-identical to scalar eval. A past
    /// ShiftB argument-order bug silently corrupted pipeline corners on
    /// 4-core machines (eval_plane_into path) while serial fills stayed
    /// correct — this test pins the invariant over the judged region's
    /// corner grid.
    #[test]
    fn eval4_matches_scalar_eval() {
        let seed: i64 = 3257840388504953787;
        let env: &'static Env = Box::leak(Box::new(Env::new(seed)));
        let gen = crate::nochunk::ChunkGen::new(env);
        let compiled = &gen.compiled;
        let children = compiled.interp_children.clone();
        let interp = InterpState::new(compiled.interp_slots, 4, 48);

        let mut points: Vec<Pt> = Vec::new();
        for i in 0..=4i32 {
            for j in 0..=4i32 {
                for y in (0..48i32).step_by(3) {
                    points.push(Pt {
                        x: 48 + i * 4,
                        y: (y - 8) * 8,
                        z: -32 + j * 4,
                    });
                }
            }
        }
        assert!(points.len() % 4 == 0);

        let mut checked = 0usize;
        let memo = V4Memo::new(compiled.arena.len());
        for chunk in points.chunks(4) {
            let pts = [chunk[0], chunk[1], chunk[2], chunk[3]];
            for &child in &children {
                let b = env.eval4(compiled, child, &pts, Mode::Slice, &interp, &memo);
                for l in 0..4 {
                    let s = env.eval(compiled, child, pts[l], Mode::Slice, &interp, None);
                    assert_eq!(
                        s.to_bits(),
                        b[l].to_bits(),
                        "eval4 diverged from eval at {:?} child {child}",
                        pts[l]
                    );
                    checked += 1;
                }
            }
        }
        // eval8 parity vs scalar (8-lane const-generic path)
        let memo8 = V8Memo::new(compiled.arena.len());
        for chunk in points.chunks(8) {
            let mut pts = [chunk[0]; 8];
            for (k, pt) in chunk.iter().enumerate() {
                pts[k] = *pt;
            }
            let n = chunk.len();
            for &child in &children {
                let b = env.eval8(compiled, child, &pts, Mode::Slice, &interp, &memo8);
                for l in 0..n {
                    let sc = env.eval(compiled, child, pts[l], Mode::Slice, &interp, None);
                    assert_eq!(
                        sc.to_bits(),
                        b[l].to_bits(),
                        "eval8 diverged from eval at {:?} child {child} lane {l}",
                        pts[l]
                    );
                    checked += 1;
                }
            }
        }
        assert!(checked > 3_000, "coverage too low: {checked}");
    }
}
