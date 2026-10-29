//! Lane-vectorized pointwise passes over density volumes.
//!
//! Bit-exactness contract vs the scalar reference loops in df263:
//!   - NO FMA contraction (mul and add are separate rounds, like scalar Rust).
//!   - Min/Max reproduce the scalar `if` semantics including NaN: scalar
//!     `if rv < l { rv } else { l }` returns l when rv is NaN; MINPS returns
//!     the second operand. Fixup masks restore the scalar lane result.
//!   - Lerp keeps the exact-equality short-circuits (alpha == 0.0 / 1.0) via
//!     compare masks, then the plain f + a*(s-f) mul+add pair.
//!   - Round: Java Math.round = floor(v + 0.5) with half-away semantics,
//!     implemented as floor(|v| + 0.5) with the sign reapplied; Floor/Ceil/
//!     Truncate map to roundscale immediates. All f32-exact for |v| < 2^23
//!     (density magnitudes are far below; the CI bit-parity harness guards).
//!   - Log/Pow stay scalar per-lane (libm f64 calls, no vector libm).
//!
//! Width selection is process-global and sticky: AVX-512 f32x16 when the
//! runner has it (CI EPYC does), else AVX2 f32x8, else the scalar loops.
//! `set_enabled(false)` forces the scalar path — the A/B bit-parity section
//! in fill263bench flips it to diff vector vs scalar on identical inputs.

/// Lane ops implemented for every width. The scalar arm of each match IS the
/// reference semantics.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum V1 {
    Abs,
    Square,
    Cube,
    Sqrt,
    HalfNegative,
    QuarterNegative,
    Reciprocal,
    Negate,
    Squeeze,
    Sign,
    Floor,
    Round,
    Ceil,
    Truncate,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum V2 {
    Add,
    Sub,
    Mul,
    Div,
    Min,
    Max,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum VC {
    CAdd,
    CSub,
    CMul,
    CDiv,
    CMin,
    CMax,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum RType {
    Floor,
    Round,
    Ceil,
    Truncate,
}

// ---- global state ----------------------------------------------------------

#[cfg(target_arch = "x86_64")]
static STATE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
#[cfg(not(target_arch = "x86_64"))]
static STATE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(1);

// 0 = not probed, 1 = scalar, 2 = avx2, 3 = avx512
#[cfg(target_arch = "x86_64")]
fn state() -> u8 {
    match STATE.load(std::sync::atomic::Ordering::Relaxed) {
        0 => {
            let s = if std::arch::is_x86_feature_detected!("avx512f") {
                3
            } else if std::arch::is_x86_feature_detected!("avx2") {
                2
            } else {
                1
            };
            STATE.store(s, std::sync::atomic::Ordering::Relaxed);
            s
        }
        s => s,
    }
}

#[cfg(not(target_arch = "x86_64"))]
fn state() -> u8 {
    1
}

/// Force the scalar reference path (A/B bit-parity harness).
pub fn set_enabled(v: bool) {
    #[cfg(target_arch = "x86_64")]
    if !v {
        STATE.store(1, std::sync::atomic::Ordering::Relaxed);
    }
    #[cfg(target_arch = "x86_64")]
    if v {
        STATE.store(0, std::sync::atomic::Ordering::Relaxed);
    }
    let _ = v;
}

/// Effective lane width for kernels that vectorize beyond the pointwise
/// shell passes (the trilinear cell fill): 1 = scalar (forced or no AVX),
/// 2 = AVX2 f32x8, 3 = AVX-512 f32x16. Honors set_enabled(false) — the A/B
/// bit-parity harness flips this to diff vector vs scalar on identical
/// inputs, so every consumer must dispatch through this, not its own probe.
pub fn enabled_width() -> u8 {
    state()
}

#[inline]
fn lanes() -> usize {
    match state() {
        3 => 16,
        2 => 8,
        _ => 1,
    }
}

// ---- scalar reference semantics (shared by the fallback and the tests) -----

#[inline]
fn sc1(op: V1, v: f32) -> f32 {
    match op {
        V1::Abs => v.abs(),
        V1::Square => v * v,
        V1::Cube => v * v * v,
        V1::Sqrt => (v as f64).sqrt() as f32,
        V1::HalfNegative => {
            if v > 0.0 {
                v
            } else {
                v * 0.5
            }
        }
        V1::QuarterNegative => {
            if v > 0.0 {
                v
            } else {
                v * 0.25
            }
        }
        V1::Reciprocal => 1.0 / v,
        V1::Negate => -v,
        V1::Squeeze => {
            let c = if v < -1.0 {
                -1.0
            } else if v > 1.0 {
                1.0
            } else {
                v
            };
            c / 2.0 - (c * c * c) / 24.0
        }
        V1::Sign => {
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
        V1::Floor => (v as f64).floor() as f32,
        V1::Round => (v as f64).round() as f32,
        V1::Ceil => (v as f64).ceil() as f32,
        V1::Truncate => {
            if v > 0.0 {
                (v as f64).floor() as f32
            } else {
                (v as f64).ceil() as f32
            }
        }
    }
}

#[inline]
fn sc2(op: V2, l: f32, r: f32) -> f32 {
    match op {
        V2::Add => l + r,
        V2::Sub => l - r,
        V2::Mul => l * r,
        V2::Div => l / r,
        V2::Min => {
            if r < l {
                r
            } else {
                l
            }
        }
        V2::Max => {
            if r > l {
                r
            } else {
                l
            }
        }
    }
}

#[inline]
fn scc(op: VC, l: f32, c: f32) -> f32 {
    match op {
        VC::CAdd => l + c,
        VC::CSub => c - l,
        VC::CMul => l * c,
        VC::CDiv => c / l,
        // scalar reference: if c < v { c } else { v } — tie keeps v (== c bits)
        VC::CMin => {
            if c < l {
                c
            } else {
                l
            }
        }
        VC::CMax => {
            if c > l {
                c
            } else {
                l
            }
        }
    }
}

#[inline]
fn sc_round(ty: RType, v: f32) -> f32 {
    match ty {
        RType::Floor => (v as f64).floor() as f32,
        RType::Round => (v as f64).round() as f32,
        RType::Ceil => (v as f64).ceil() as f32,
        RType::Truncate => {
            if v > 0.0 {
                (v as f64).floor() as f32
            } else {
                (v as f64).ceil() as f32
            }
        }
    }
}

#[inline]
fn sc_clamp(v: f32, min: f32, max: f32) -> f32 {
    if v < min {
        min
    } else if v > max {
        max
    } else {
        v
    }
}

#[inline]
fn sc_lerp(a: f32, f: f32, s: f32) -> f32 {
    if a == 0.0 {
        f
    } else if a == 1.0 {
        s
    } else {
        f + a * (s - f)
    }
}

// ---- AVX-512 ---------------------------------------------------------------

#[cfg(target_arch = "x86_64")]
mod avx512 {
    use super::{RType, V1, V2, VC};
    use std::arch::x86_64::*;

    #[target_feature(enable = "avx512f")]
    pub unsafe fn round_pd(ty: RType, v: __m512) -> __m512 {
        match ty {
            RType::Floor => _mm512_roundscale_ps::<{ _MM_FROUND_TO_NEG_INF }>(v),
            RType::Ceil => _mm512_roundscale_ps::<{ _MM_FROUND_TO_POS_INF }>(v),
            RType::Truncate => _mm512_roundscale_ps::<{ _MM_FROUND_TO_ZERO }>(v),
            RType::Round => {
                // Java Math.round: floor(v+0.5), half away from zero.
                let sign = _mm512_and_ps(v, _mm512_set1_ps(-0.0f32));
                let a = _mm512_andnot_ps(_mm512_set1_ps(-0.0f32), v); // |v|
                let t = _mm512_roundscale_ps::<{ _MM_FROUND_TO_NEG_INF }>(_mm512_add_ps(a, _mm512_set1_ps(0.5)));
                _mm512_or_ps(t, sign)
            }
        }
    }

    #[target_feature(enable = "avx512f")]
    pub unsafe fn v1(op: V1, v: __m512) -> __m512 {
        match op {
            V1::Abs => _mm512_andnot_ps(_mm512_set1_ps(-0.0f32), v),
            V1::Square => _mm512_mul_ps(v, v),
            V1::Cube => _mm512_mul_ps(_mm512_mul_ps(v, v), v),
            V1::Sqrt => _mm512_sqrt_ps(v),
            V1::HalfNegative => {
                let m = _mm512_cmp_ps_mask(v, _mm512_set1_ps(0.0), _CMP_GT_OQ);
                _mm512_mask_mul_ps(v, !m, v, _mm512_set1_ps(0.5))
            }
            V1::QuarterNegative => {
                let m = _mm512_cmp_ps_mask(v, _mm512_set1_ps(0.0), _CMP_GT_OQ);
                _mm512_mask_mul_ps(v, !m, v, _mm512_set1_ps(0.25))
            }
            V1::Reciprocal => _mm512_div_ps(_mm512_set1_ps(1.0), v),
            V1::Negate => _mm512_xor_ps(v, _mm512_set1_ps(-0.0f32)),
            V1::Squeeze => {
                let c = _mm512_max_ps(
                    _mm512_min_ps(v, _mm512_set1_ps(1.0)),
                    _mm512_set1_ps(-1.0),
                );
                let c2 = _mm512_mul_ps(c, c);
                let c3 = _mm512_mul_ps(c2, c);
                _mm512_sub_ps(
                    _mm512_div_ps(c, _mm512_set1_ps(2.0)),
                    _mm512_div_ps(c3, _mm512_set1_ps(24.0)),
                )
            }
            V1::Sign => {
                let nan = _mm512_cmp_ps_mask(v, v, _CMP_UNORD_Q);
                let zero = _mm512_cmp_ps_mask(v, _mm512_set1_ps(0.0), _CMP_EQ_OQ);
                let pos = _mm512_cmp_ps_mask(v, _mm512_set1_ps(0.0), _CMP_GT_OQ);
                let r = _mm512_mask_blend_ps(pos, _mm512_set1_ps(-1.0), _mm512_set1_ps(1.0));
                let r = _mm512_mask_blend_ps(zero, r, _mm512_set1_ps(0.0));
                _mm512_mask_mov_ps(r, nan, v)
            }
            V1::Floor => _mm512_roundscale_ps::<{ _MM_FROUND_TO_NEG_INF }>(v),
            V1::Round => round_pd(RType::Round, v),
            V1::Ceil => _mm512_roundscale_ps::<{ _MM_FROUND_TO_POS_INF }>(v),
            V1::Truncate => _mm512_roundscale_ps::<{ _MM_FROUND_TO_ZERO }>(v),
        }
    }

    /// Scalar-semantic min: r < l ? r : l with NaN -> l.
    #[target_feature(enable = "avx512f")]
    pub unsafe fn min_sem(l: __m512, r: __m512) -> __m512 {
        let m = _mm512_min_ps(l, r);
        let isnan_r = _mm512_cmp_ps_mask(r, r, _CMP_UNORD_Q);
        let isnan_l = _mm512_cmp_ps_mask(l, l, _CMP_UNORD_Q);
        _mm512_mask_mov_ps(_mm512_mask_mov_ps(m, isnan_r, l), isnan_l, l)
    }

    /// Scalar-semantic max: r > l ? r : l with NaN -> l.
    #[target_feature(enable = "avx512f")]
    pub unsafe fn max_sem(l: __m512, r: __m512) -> __m512 {
        let m = _mm512_max_ps(l, r);
        let isnan_r = _mm512_cmp_ps_mask(r, r, _CMP_UNORD_Q);
        let isnan_l = _mm512_cmp_ps_mask(l, l, _CMP_UNORD_Q);
        _mm512_mask_mov_ps(_mm512_mask_mov_ps(m, isnan_r, l), isnan_l, l)
    }

    #[target_feature(enable = "avx512f")]
    pub unsafe fn v2(op: V2, l: __m512, r: __m512) -> __m512 {
        match op {
            V2::Add => _mm512_add_ps(l, r),
            V2::Sub => _mm512_sub_ps(l, r),
            V2::Mul => _mm512_mul_ps(l, r),
            V2::Div => _mm512_div_ps(l, r),
            V2::Min => min_sem(l, r),
            V2::Max => max_sem(l, r),
        }
    }

    #[target_feature(enable = "avx512f")]
    pub unsafe fn vc(op: VC, l: __m512, c: __m512) -> __m512 {
        match op {
            VC::CAdd => _mm512_add_ps(l, c),
            VC::CSub => _mm512_sub_ps(c, l),
            VC::CMul => _mm512_mul_ps(l, c),
            VC::CDiv => _mm512_div_ps(c, l),
            VC::CMin => min_sem(l, c),
            VC::CMax => max_sem(l, c),
        }
    }

    #[target_feature(enable = "avx512f")]
    pub unsafe fn clamp(v: __m512, min: __m512, max: __m512) -> __m512 {
        let m = _mm512_max_ps(_mm512_min_ps(v, max), min);
        let isnan = _mm512_cmp_ps_mask(v, v, _CMP_UNORD_Q);
        _mm512_mask_mov_ps(m, isnan, v)
    }

    /// f + a*(s-f) with the exact-equality short-circuits.
    #[target_feature(enable = "avx512f")]
    pub unsafe fn lerp(a: __m512, f: __m512, s: __m512) -> __m512 {
        let r = _mm512_add_ps(f, _mm512_mul_ps(a, _mm512_sub_ps(s, f)));
        let one = _mm512_cmp_ps_mask(a, _mm512_set1_ps(1.0), _CMP_EQ_OQ);
        let r = _mm512_mask_mov_ps(r, one, s);
        let zero = _mm512_cmp_ps_mask(a, _mm512_set1_ps(0.0), _CMP_EQ_OQ);
        _mm512_mask_mov_ps(r, zero, f)
    }

    /// r = m == 0 ? a : round_t(a / m) * m
    #[target_feature(enable = "avx512f")]
    pub unsafe fn round_mul(ty: RType, a: __m512, m: __m512) -> __m512 {
        let zero = _mm512_cmp_ps_mask(m, _mm512_set1_ps(0.0), _CMP_EQ_OQ);
        let q = _mm512_div_ps(a, m);
        let r = _mm512_mul_ps(round_pd(ty, q), m);
        _mm512_mask_mov_ps(r, zero, a)
    }
}

// ---- AVX2 ------------------------------------------------------------------

#[cfg(target_arch = "x86_64")]
mod avx2 {
    use super::{RType, V1, V2, VC};
    use std::arch::x86_64::*;

    #[target_feature(enable = "avx2")]
    pub unsafe fn round_ps(ty: RType, v: __m256) -> __m256 {
        match ty {
            RType::Floor => _mm256_floor_ps(v),
            RType::Ceil => _mm256_ceil_ps(v),
            RType::Truncate => _mm256_round_ps::<{ _MM_FROUND_TO_ZERO }>(v),
            RType::Round => {
                let sign = _mm256_and_ps(v, _mm256_set1_ps(-0.0f32));
                let a = _mm256_andnot_ps(_mm256_set1_ps(-0.0f32), v);
                let t = _mm256_floor_ps(_mm256_add_ps(a, _mm256_set1_ps(0.5)));
                _mm256_or_ps(t, sign)
            }
        }
    }

    #[target_feature(enable = "avx2")]
    pub unsafe fn v1(op: V1, v: __m256) -> __m256 {
        match op {
            V1::Abs => _mm256_andnot_ps(_mm256_set1_ps(-0.0f32), v),
            V1::Square => _mm256_mul_ps(v, v),
            V1::Cube => _mm256_mul_ps(_mm256_mul_ps(v, v), v),
            V1::Sqrt => _mm256_sqrt_ps(v),
            V1::HalfNegative => {
                let m = _mm256_cmp_ps(v, _mm256_set1_ps(0.0), _CMP_GT_OQ);
                let neg = _mm256_mul_ps(v, _mm256_set1_ps(0.5));
                _mm256_or_ps(_mm256_andnot_ps(m, neg), _mm256_and_ps(m, v))
            }
            V1::QuarterNegative => {
                let m = _mm256_cmp_ps(v, _mm256_set1_ps(0.0), _CMP_GT_OQ);
                let neg = _mm256_mul_ps(v, _mm256_set1_ps(0.25));
                _mm256_or_ps(_mm256_andnot_ps(m, neg), _mm256_and_ps(m, v))
            }
            V1::Reciprocal => _mm256_div_ps(_mm256_set1_ps(1.0), v),
            V1::Negate => _mm256_xor_ps(v, _mm256_set1_ps(-0.0f32)),
            V1::Squeeze => {
                let c = _mm256_max_ps(
                    _mm256_min_ps(v, _mm256_set1_ps(1.0)),
                    _mm256_set1_ps(-1.0),
                );
                let c2 = _mm256_mul_ps(c, c);
                let c3 = _mm256_mul_ps(c2, c);
                _mm256_sub_ps(
                    _mm256_div_ps(c, _mm256_set1_ps(2.0)),
                    _mm256_div_ps(c3, _mm256_set1_ps(24.0)),
                )
            }
            V1::Sign => {
                let nan = _mm256_cmp_ps(v, v, _CMP_UNORD_Q);
                let zero = _mm256_cmp_ps(v, _mm256_set1_ps(0.0), _CMP_EQ_OQ);
                let pos = _mm256_cmp_ps(v, _mm256_set1_ps(0.0), _CMP_GT_OQ);
                // r = pos ? 1 : -1; r = zero ? 0 : r; r = nan ? v : r
                let r = _mm256_or_ps(
                    _mm256_andnot_ps(pos, _mm256_set1_ps(-1.0)),
                    _mm256_and_ps(pos, _mm256_set1_ps(1.0)),
                );
                let r = _mm256_or_ps(
                    _mm256_andnot_ps(zero, r),
                    _mm256_and_ps(zero, _mm256_set1_ps(0.0)),
                );
                _mm256_or_ps(
                    _mm256_andnot_ps(nan, r),
                    _mm256_and_ps(nan, v),
                )
            }
            V1::Floor => _mm256_floor_ps(v),
            V1::Round => round_ps(RType::Round, v),
            V1::Ceil => _mm256_ceil_ps(v),
            V1::Truncate => _mm256_round_ps::<{ _MM_FROUND_TO_ZERO }>(v),
        }
    }

    /// Scalar-semantic min with NaN -> l (fixup around MINPS NaN behavior).
    #[target_feature(enable = "avx2")]
    pub unsafe fn min_sem(l: __m256, r: __m256) -> __m256 {
        let m = _mm256_min_ps(l, r);
        let isnan_r = _mm256_cmp_ps(r, r, _CMP_UNORD_Q);
        let isnan_l = _mm256_cmp_ps(l, l, _CMP_UNORD_Q);
        let bad = _mm256_or_ps(isnan_r, isnan_l);
        _mm256_or_ps(_mm256_andnot_ps(bad, m), _mm256_and_ps(bad, l))
    }

    #[target_feature(enable = "avx2")]
    pub unsafe fn max_sem(l: __m256, r: __m256) -> __m256 {
        let m = _mm256_max_ps(l, r);
        let isnan_r = _mm256_cmp_ps(r, r, _CMP_UNORD_Q);
        let isnan_l = _mm256_cmp_ps(l, l, _CMP_UNORD_Q);
        let bad = _mm256_or_ps(isnan_r, isnan_l);
        _mm256_or_ps(_mm256_andnot_ps(bad, m), _mm256_and_ps(bad, l))
    }

    #[target_feature(enable = "avx2")]
    pub unsafe fn v2(op: V2, l: __m256, r: __m256) -> __m256 {
        match op {
            V2::Add => _mm256_add_ps(l, r),
            V2::Sub => _mm256_sub_ps(l, r),
            V2::Mul => _mm256_mul_ps(l, r),
            V2::Div => _mm256_div_ps(l, r),
            V2::Min => min_sem(l, r),
            V2::Max => max_sem(l, r),
        }
    }

    #[target_feature(enable = "avx2")]
    pub unsafe fn vc(op: VC, l: __m256, c: __m256) -> __m256 {
        match op {
            VC::CAdd => _mm256_add_ps(l, c),
            VC::CSub => _mm256_sub_ps(c, l),
            VC::CMul => _mm256_mul_ps(l, c),
            VC::CDiv => _mm256_div_ps(c, l),
            VC::CMin => min_sem(l, c),
            VC::CMax => max_sem(l, c),
        }
    }

    #[target_feature(enable = "avx2")]
    pub unsafe fn clamp(v: __m256, min: __m256, max: __m256) -> __m256 {
        let m = _mm256_max_ps(_mm256_min_ps(v, max), min);
        let isnan = _mm256_cmp_ps(v, v, _CMP_UNORD_Q);
        _mm256_or_ps(_mm256_andnot_ps(isnan, m), _mm256_and_ps(isnan, v))
    }

    #[target_feature(enable = "avx2")]
    pub unsafe fn lerp(a: __m256, f: __m256, s: __m256) -> __m256 {
        let r = _mm256_add_ps(f, _mm256_mul_ps(a, _mm256_sub_ps(s, f)));
        let one = _mm256_cmp_ps(a, _mm256_set1_ps(1.0), _CMP_EQ_OQ);
        let r = _mm256_or_ps(_mm256_andnot_ps(one, r), _mm256_and_ps(one, s));
        let zero = _mm256_cmp_ps(a, _mm256_set1_ps(0.0), _CMP_EQ_OQ);
        _mm256_or_ps(_mm256_andnot_ps(zero, r), _mm256_and_ps(zero, f))
    }

    #[target_feature(enable = "avx2")]
    pub unsafe fn round_mul(ty: RType, a: __m256, m: __m256) -> __m256 {
        let zero = _mm256_cmp_ps(m, _mm256_set1_ps(0.0), _CMP_EQ_OQ);
        let q = _mm256_div_ps(a, m);
        let r = _mm256_mul_ps(round_ps(ty, q), m);
        _mm256_or_ps(_mm256_andnot_ps(zero, r), _mm256_and_ps(zero, a))
    }
}

// ---- public slice-level API ------------------------------------------------

/// In-place unary pass: out[i] = op(out[i]). Log/Pow stay in df263 (scalar).
pub fn map1(out: &mut [f32], op: V1) {
    let n = out.len();
    match state() {
        3 if n >= 16 => unsafe {
            let p = out.as_mut_ptr();
            let chunks = n / 16;
            for k in 0..chunks {
                let v = std::arch::x86_64::_mm512_loadu_ps(p.add(k * 16));
                std::arch::x86_64::_mm512_storeu_ps(p.add(k * 16), avx512::v1(op, v));
            }
            for i in chunks * 16..n {
                *p.add(i) = sc1(op, *p.add(i));
            }
        },
        2 if n >= 8 => unsafe {
            let p = out.as_mut_ptr();
            let chunks = n / 8;
            for k in 0..chunks {
                let v = std::arch::x86_64::_mm256_loadu_ps(p.add(k * 8));
                std::arch::x86_64::_mm256_storeu_ps(p.add(k * 8), avx2::v1(op, v));
            }
            for i in chunks * 8..n {
                *p.add(i) = sc1(op, *p.add(i));
            }
        },
        _ => {
            for v in out.iter_mut() {
                *v = sc1(op, *v);
            }
        }
    }
}

/// In-place binary pass: out[i] = op(out[i], b[i]).
pub fn map2(out: &mut [f32], b: &[f32], op: V2) {
    let n = out.len();
    debug_assert!(b.len() >= n);
    match state() {
        3 if n >= 16 => unsafe {
            let p = out.as_mut_ptr();
            let q = b.as_ptr();
            let chunks = n / 16;
            for k in 0..chunks {
                let l = std::arch::x86_64::_mm512_loadu_ps(p.add(k * 16));
                let r = std::arch::x86_64::_mm512_loadu_ps(q.add(k * 16));
                std::arch::x86_64::_mm512_storeu_ps(p.add(k * 16), avx512::v2(op, l, r));
            }
            for i in chunks * 16..n {
                *p.add(i) = sc2(op, *p.add(i), *q.add(i));
            }
        },
        2 if n >= 8 => unsafe {
            let p = out.as_mut_ptr();
            let q = b.as_ptr();
            let chunks = n / 8;
            for k in 0..chunks {
                let l = std::arch::x86_64::_mm256_loadu_ps(p.add(k * 8));
                let r = std::arch::x86_64::_mm256_loadu_ps(q.add(k * 8));
                std::arch::x86_64::_mm256_storeu_ps(p.add(k * 8), avx2::v2(op, l, r));
            }
            for i in chunks * 8..n {
                *p.add(i) = sc2(op, *p.add(i), *q.add(i));
            }
        },
        _ => {
            for (v, &rv) in out.iter_mut().zip(b.iter()) {
                *v = sc2(op, *v, rv);
            }
        }
    }
}

/// In-place const pass: out[i] = op(out[i], c).
pub fn mapc(out: &mut [f32], c: f32, op: VC) {
    let n = out.len();
    match state() {
        3 if n >= 16 => unsafe {
            let p = out.as_mut_ptr();
            let cv = std::arch::x86_64::_mm512_set1_ps(c);
            let chunks = n / 16;
            for k in 0..chunks {
                let l = std::arch::x86_64::_mm512_loadu_ps(p.add(k * 16));
                std::arch::x86_64::_mm512_storeu_ps(p.add(k * 16), avx512::vc(op, l, cv));
            }
            for i in chunks * 16..n {
                *p.add(i) = scc(op, *p.add(i), c);
            }
        },
        2 if n >= 8 => unsafe {
            let p = out.as_mut_ptr();
            let cv = std::arch::x86_64::_mm256_set1_ps(c);
            let chunks = n / 8;
            for k in 0..chunks {
                let l = std::arch::x86_64::_mm256_loadu_ps(p.add(k * 8));
                std::arch::x86_64::_mm256_storeu_ps(p.add(k * 8), avx2::vc(op, l, cv));
            }
            for i in chunks * 8..n {
                *p.add(i) = scc(op, *p.add(i), c);
            }
        },
        _ => {
            for v in out.iter_mut() {
                *v = scc(op, *v, c);
            }
        }
    }
}

pub fn clamp(out: &mut [f32], min: f32, max: f32) {
    let n = out.len();
    match state() {
        3 if n >= 16 => unsafe {
            let p = out.as_mut_ptr();
            let (mn, mx) = (
                std::arch::x86_64::_mm512_set1_ps(min),
                std::arch::x86_64::_mm512_set1_ps(max),
            );
            let chunks = n / 16;
            for k in 0..chunks {
                let v = std::arch::x86_64::_mm512_loadu_ps(p.add(k * 16));
                std::arch::x86_64::_mm512_storeu_ps(p.add(k * 16), avx512::clamp(v, mn, mx));
            }
            for i in chunks * 16..n {
                *p.add(i) = sc_clamp(*p.add(i), min, max);
            }
        },
        2 if n >= 8 => unsafe {
            let p = out.as_mut_ptr();
            let (mn, mx) = (
                std::arch::x86_64::_mm256_set1_ps(min),
                std::arch::x86_64::_mm256_set1_ps(max),
            );
            let chunks = n / 8;
            for k in 0..chunks {
                let v = std::arch::x86_64::_mm256_loadu_ps(p.add(k * 8));
                std::arch::x86_64::_mm256_storeu_ps(p.add(k * 8), avx2::clamp(v, mn, mx));
            }
            for i in chunks * 8..n {
                *p.add(i) = sc_clamp(*p.add(i), min, max);
            }
        },
        _ => {
            for v in out.iter_mut() {
                *v = sc_clamp(*v, min, max);
            }
        }
    }
}

/// out (alpha), fb, sb: out[i] = lerp semantics with exact short-circuits.
pub fn lerp3(out: &mut [f32], fb: &[f32], sb: &[f32]) {
    let n = out.len();
    match state() {
        3 if n >= 16 => unsafe {
            let p = out.as_mut_ptr();
            let (q, r) = (fb.as_ptr(), sb.as_ptr());
            let chunks = n / 16;
            for k in 0..chunks {
                let a = std::arch::x86_64::_mm512_loadu_ps(p.add(k * 16));
                let f = std::arch::x86_64::_mm512_loadu_ps(q.add(k * 16));
                let s = std::arch::x86_64::_mm512_loadu_ps(r.add(k * 16));
                std::arch::x86_64::_mm512_storeu_ps(p.add(k * 16), avx512::lerp(a, f, s));
            }
            for i in chunks * 16..n {
                *p.add(i) = sc_lerp(*p.add(i), *q.add(i), *r.add(i));
            }
        },
        2 if n >= 8 => unsafe {
            let p = out.as_mut_ptr();
            let (q, r) = (fb.as_ptr(), sb.as_ptr());
            let chunks = n / 8;
            for k in 0..chunks {
                let a = std::arch::x86_64::_mm256_loadu_ps(p.add(k * 8));
                let f = std::arch::x86_64::_mm256_loadu_ps(q.add(k * 8));
                let s = std::arch::x86_64::_mm256_loadu_ps(r.add(k * 8));
                std::arch::x86_64::_mm256_storeu_ps(p.add(k * 8), avx2::lerp(a, f, s));
            }
            for i in chunks * 8..n {
                *p.add(i) = sc_lerp(*p.add(i), *q.add(i), *r.add(i));
            }
        },
        _ => {
            for i in 0..n {
                out[i] = sc_lerp(out[i], fb[i], sb[i]);
            }
        }
    }
}

/// out (alpha) with const first: out[i] = lerp(a, F, sb[i]).
pub fn lerp_cf(out: &mut [f32], first: f32, sb: &[f32]) {
    let n = out.len();
    match state() {
        3 if n >= 16 => unsafe {
            let p = out.as_mut_ptr();
            let r = sb.as_ptr();
            let fv = std::arch::x86_64::_mm512_set1_ps(first);
            let chunks = n / 16;
            for k in 0..chunks {
                let a = std::arch::x86_64::_mm512_loadu_ps(p.add(k * 16));
                let s = std::arch::x86_64::_mm512_loadu_ps(r.add(k * 16));
                std::arch::x86_64::_mm512_storeu_ps(p.add(k * 16), avx512::lerp(a, fv, s));
            }
            for i in chunks * 16..n {
                *p.add(i) = sc_lerp(*p.add(i), first, *r.add(i));
            }
        },
        2 if n >= 8 => unsafe {
            let p = out.as_mut_ptr();
            let r = sb.as_ptr();
            let fv = std::arch::x86_64::_mm256_set1_ps(first);
            let chunks = n / 8;
            for k in 0..chunks {
                let a = std::arch::x86_64::_mm256_loadu_ps(p.add(k * 8));
                let s = std::arch::x86_64::_mm256_loadu_ps(r.add(k * 8));
                std::arch::x86_64::_mm256_storeu_ps(p.add(k * 8), avx2::lerp(a, fv, s));
            }
            for i in chunks * 8..n {
                *p.add(i) = sc_lerp(*p.add(i), first, *r.add(i));
            }
        },
        _ => {
            for i in 0..n {
                out[i] = sc_lerp(out[i], first, sb[i]);
            }
        }
    }
}

/// out (alpha) with const second: out[i] = lerp(a, fb[i], S).
pub fn lerp_cs(out: &mut [f32], fb: &[f32], second: f32) {
    let n = out.len();
    match state() {
        3 if n >= 16 => unsafe {
            let p = out.as_mut_ptr();
            let q = fb.as_ptr();
            let sv = std::arch::x86_64::_mm512_set1_ps(second);
            let chunks = n / 16;
            for k in 0..chunks {
                let a = std::arch::x86_64::_mm512_loadu_ps(p.add(k * 16));
                let f = std::arch::x86_64::_mm512_loadu_ps(q.add(k * 16));
                std::arch::x86_64::_mm512_storeu_ps(p.add(k * 16), avx512::lerp(a, f, sv));
            }
            for i in chunks * 16..n {
                *p.add(i) = sc_lerp(*p.add(i), *q.add(i), second);
            }
        },
        2 if n >= 8 => unsafe {
            let p = out.as_mut_ptr();
            let q = fb.as_ptr();
            let sv = std::arch::x86_64::_mm256_set1_ps(second);
            let chunks = n / 8;
            for k in 0..chunks {
                let a = std::arch::x86_64::_mm256_loadu_ps(p.add(k * 8));
                let f = std::arch::x86_64::_mm256_loadu_ps(q.add(k * 8));
                std::arch::x86_64::_mm256_storeu_ps(p.add(k * 8), avx2::lerp(a, f, sv));
            }
            for i in chunks * 8..n {
                *p.add(i) = sc_lerp(*p.add(i), *q.add(i), second);
            }
        },
        _ => {
            for i in 0..n {
                out[i] = sc_lerp(out[i], fb[i], second);
            }
        }
    }
}

/// out[i] = m[i] == 0 ? a[i] : round_ty(a[i]/m[i]) * m[i]
pub fn round_mul(out: &mut [f32], mb: &[f32], ty: RType) {
    let n = out.len();
    match state() {
        3 if n >= 16 => unsafe {
            let p = out.as_mut_ptr();
            let q = mb.as_ptr();
            let chunks = n / 16;
            for k in 0..chunks {
                let a = std::arch::x86_64::_mm512_loadu_ps(p.add(k * 16));
                let m = std::arch::x86_64::_mm512_loadu_ps(q.add(k * 16));
                std::arch::x86_64::_mm512_storeu_ps(
                    p.add(k * 16),
                    avx512::round_mul(ty, a, m),
                );
            }
            for i in chunks * 16..n {
                let m = *q.add(i);
                let a = *p.add(i);
                *p.add(i) = if m == 0.0 {
                    a
                } else {
                    sc_round(ty, a / m) * m
                };
            }
        },
        2 if n >= 8 => unsafe {
            let p = out.as_mut_ptr();
            let q = mb.as_ptr();
            let chunks = n / 8;
            for k in 0..chunks {
                let a = std::arch::x86_64::_mm256_loadu_ps(p.add(k * 8));
                let m = std::arch::x86_64::_mm256_loadu_ps(q.add(k * 8));
                std::arch::x86_64::_mm256_storeu_ps(p.add(k * 8), avx2::round_mul(ty, a, m));
            }
            for i in chunks * 8..n {
                let m = *q.add(i);
                let a = *p.add(i);
                *p.add(i) = if m == 0.0 {
                    a
                } else {
                    sc_round(ty, a / m) * m
                };
            }
        },
        _ => {
            for i in 0..n {
                let m = mb[i];
                let a = out[i];
                out[i] = if m == 0.0 {
                    a
                } else {
                    sc_round(ty, a / m) * m
                };
            }
        }
    }
}

pub fn round_ty(out: &mut [f32], ty: RType) {
    map1(
        out,
        match ty {
            RType::Floor => V1::Floor,
            RType::Round => V1::Round,
            RType::Ceil => V1::Ceil,
            RType::Truncate => V1::Truncate,
        },
    );
}

/// out holds in_range; where !(a >= min && a < max) it is replaced by ob[i].
pub fn range_choice(out: &mut [f32], ib: &[f32], ob: &[f32], min: f32, max: f32) {
    let n = out.len();
    for i in 0..n {
        let v = ib[i];
        if !(v >= min && v < max) {
            out[i] = ob[i];
        }
    }
}

/// out[i] = (a >= min && a < max) ? in_v : out_v
pub fn range_choice_const(out: &mut [f32], min: f32, max: f32, in_v: f32, out_v: f32) {
    let n = out.len();
    for v in out.iter_mut() {
        *v = if *v >= min && *v < max {
            in_v
        } else {
            out_v
        };
    }
    let _ = lanes();
}

/// out (input) replaced by the first branch whose threshold exceeds it:
/// picked = first ti with input < thresholds[ti], else last branch.
pub fn interval_select(out: &mut [f32], bufs: &[&[f32]], thresholds: &[f32]) {
    let n = out.len();
    let last = bufs.len() - 1;
    for i in 0..n {
        let input = out[i];
        let mut idx = last;
        for (ti, t) in thresholds.iter().enumerate() {
            if input < *t {
                idx = ti;
                break;
            }
        }
        out[i] = bufs[idx][i];
    }
}
