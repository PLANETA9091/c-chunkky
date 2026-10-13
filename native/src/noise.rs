// Vanilla noise primitives, bit-exact with 1.21.8 (verified against NMS bytecode).

use crate::rng::{LegacyRandom, XoroshiroRandomSource};

pub const GRADIENT: [[i32; 3]; 16] = [
    [1, 1, 0],
    [-1, 1, 0],
    [1, -1, 0],
    [-1, -1, 0],
    [1, 0, 1],
    [-1, 0, 1],
    [1, 0, -1],
    [-1, 0, -1],
    [0, 1, 1],
    [0, -1, 1],
    [0, 1, -1],
    [0, -1, -1],
    [1, 1, 0],
    [0, -1, 1],
    [-1, 1, 0],
    [0, -1, -1],
];

/// Split &[f64; 8] into two zero-copy &[f64; 4] half views.
#[inline]
fn split8(a: &[f64; 8]) -> (&[f64; 4], &[f64; 4]) {
    unsafe {
        let p = a.as_ptr();
        (&*(p as *const [f64; 4]), &*(p.add(4) as *const [f64; 4]))
    }
}

#[inline]
fn smoothstep(t: f64) -> f64 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

#[inline]
fn lerp(t: f64, a: f64, b: f64) -> f64 {
    a + t * (b - a)
}

#[inline]
fn grad_dot(g: usize, x: f64, y: f64, z: f64) -> f64 {
    let gv = GRADIENT[g & 15];
    gv[0] as f64 * x + gv[1] as f64 * y + gv[2] as f64 * z
}

// ---- AVX2 lane-math helpers (strict IEEE, no FMA, no reassociation) ----
// Every op below performs the exact same per-lane IEEE operation sequence as
// the scalar helpers above.
#[cfg(target_arch = "x86_64")]
mod vec {
    #[cfg(target_arch = "x86_64")]
    use std::arch::x86_64::*;

    /// t*t*t*(t*(t*6.0-15.0)+10.0)
    #[inline]
    #[target_feature(enable = "avx2")]
    pub unsafe fn smoothstep_v(t: __m256d) -> __m256d {
        let t2 = _mm256_mul_pd(t, t);
        let t3 = _mm256_mul_pd(t2, t);
        let a = _mm256_mul_pd(t, _mm256_set1_pd(6.0));
        let b = _mm256_sub_pd(a, _mm256_set1_pd(15.0));
        let c = _mm256_mul_pd(b, t);
        let d = _mm256_add_pd(c, _mm256_set1_pd(10.0));
        _mm256_mul_pd(t3, d)
    }

    /// a + t*(b-a)
    #[inline]
    #[target_feature(enable = "avx2")]
    pub unsafe fn lerp_v(t: __m256d, a: __m256d, b: __m256d) -> __m256d {
        _mm256_add_pd(a, _mm256_mul_pd(t, _mm256_sub_pd(b, a)))
    }
}

#[cfg(target_arch = "x86_64")]
use vec::{lerp_v, smoothstep_v};

#[inline]
fn mth_floor(x: f64) -> i32 {
    let f = x.floor();
    f as i32
}

// ---------------- ImprovedNoise ----------------
#[derive(Clone)]
pub struct ImprovedNoise {
    pub xo: f64,
    pub yo: f64,
    pub zo: f64,
    pub p: [u8; 256],
    /// Unique per construction; indexes the per-thread lattice-cell memo.
    /// Clones share the id (identical p/xo/yo/zo -> identical pure function,
    /// so memo semantics carry over safely).
    pub id: u32,
}

// ---- per-octave lattice-cell memo (bit-exact pure-function cache) ----
// The 8-corner permutation walk depends only on (p-table, xi, yi, zi).
// Corner-plane points step y by cell_height (8) at fixed (x, z); for noise
// octaves whose input spacing < 1 cell, consecutive points share the same
// lattice cell and the walk (a short dependent-load chain, ~half of a
// perlin eval) can be reused. The cached value is a pure function of
// immutable state (p never changes after init), so a hit returns
// bit-identical bytes by construction. Per-thread storage: no locks, each
// plane worker / block loop keeps its own table.
static NEXT_NOISE_ID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

#[derive(Clone, Copy)]
struct CellEntry {
    xi: i32,
    yi: i32,
    zi: i32,
    grads: [u8; 8],
}

thread_local! {
    static CELL_MEMO: std::cell::RefCell<Vec<Option<CellEntry>>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

impl ImprovedNoise {
    /// 8 corner gradient bytes for the lattice cell (xi, yi, zi).
    /// Corner bit mapping: c = xbit | ybit<<1 | zbit<<2 where bit set means
    /// "delta - 1" on that axis (identical to the inline walks it replaces).
    /// NOTE: the per-thread TLS memo variant of this helper measured a net
    /// REGRESSION (TLS enter + borrow overhead on ~40 octave calls per point
    /// outweighed the walk savings) and is disabled — the walk itself is
    /// cheap and the y-invariant climate subtrees are handled at a much
    /// higher level (BatchMemo column cache in density.rs).
    #[inline]
    pub fn cell_grads(&self, xi: i32, yi: i32, zi: i32) -> [u8; 8] {
        self.walk_cell(xi, yi, zi)
    }

    /// Reserved for experiments: memoized lookup keyed by lattice cell.
    #[allow(dead_code)]
    fn cell_grads_memo(&self, xi: i32, yi: i32, zi: i32) -> [u8; 8] {
        CELL_MEMO.with(|m| {
            let mut m = m.borrow_mut();
            let id = self.id as usize;
            if m.len() <= id {
                m.resize(id + 1, None);
            }
            let slot = &mut m[id];
            if let Some(e) = slot {
                if e.xi == xi && e.yi == yi && e.zi == zi {
                    return e.grads;
                }
            }
            let grads = self.walk_cell(xi, yi, zi);
            *slot = Some(CellEntry { xi, yi, zi, grads });
            grads
        })
    }

    /// The un-memoized 8-corner permutation walk (bit-identical to the code
    /// inlined in sample_and_lerp / the AVX2 batch walk it replaces).
    #[inline]
    fn walk_cell(&self, xi: i32, yi: i32, zi: i32) -> [u8; 8] {
        let p0 = self.p(xi);
        let p1 = self.p(xi + 1);
        let i0 = self.p(p0 + yi);
        let i1 = self.p(p0 + yi + 1);
        let j0 = self.p(p1 + yi);
        let j1 = self.p(p1 + yi + 1);
        let zc = zi;
        let bases = [i0 + zc, j0 + zc, i1 + zc, j1 + zc]; // [X + 2*Y]
        let mut grads = [0u8; 8];
        for c in 0..8 {
            let xbit = c & 1;
            let ybit = (c >> 1) & 1;
            let zbit = (c >> 2) & 1;
            grads[c] = self.p(bases[ybit * 2 + xbit] + zbit as i32) as u8;
        }
        grads
    }
}

impl ImprovedNoise {
    pub fn new_xoroshiro(rng: &mut XoroshiroRandomSource) -> Self {
        let mut s = ImprovedNoise {
            xo: 0.0,
            yo: 0.0,
            zo: 0.0,
            p: [0; 256],
            id: NEXT_NOISE_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        };
        s.init(rng);
        s
    }

    pub fn new_legacy(rng: &mut LegacyRandom) -> Self {
        let mut s = ImprovedNoise {
            xo: 0.0,
            yo: 0.0,
            zo: 0.0,
            p: [0; 256],
            id: NEXT_NOISE_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        };
        s.init_legacy(rng);
        s
    }

    fn init(&mut self, rng: &mut XoroshiroRandomSource) {
        self.xo = rng.next_f64() * 256.0;
        self.yo = rng.next_f64() * 256.0;
        self.zo = rng.next_f64() * 256.0;
        for i in 0..256 {
            self.p[i] = i as u8;
        }
        for i in 0..256 {
            let j = rng.next_int_bound(256 - i as i32) as usize;
            self.p.swap(i, i + j);
        }
    }

    fn init_legacy(&mut self, rng: &mut LegacyRandom) {
        self.xo = rng.next_f64() * 256.0;
        self.yo = rng.next_f64() * 256.0;
        self.zo = rng.next_f64() * 256.0;
        for i in 0..256 {
            self.p[i] = i as u8;
        }
        for i in 0..256 {
            let j = rng.next_int_bound(256 - i as i32) as usize;
            self.p.swap(i, i + j);
        }
    }

    #[inline]
    fn p(&self, i: i32) -> i32 {
        (self.p[(i & 255) as usize] & 255) as i32
    }

    #[inline]
    fn sample_and_lerp(
        &self,
        x_cell: i32,
        y_cell: i32,
        z_cell: i32,
        dx: f64,
        weird_dy: f64,
        dz: f64,
        dy: f64,
    ) -> f64 {
        // Vanilla: gradDot uses (deltaX, weirdDeltaY, deltaZ); smoothstep uses (deltaX, deltaY, deltaZ)
        // (the permutation walk is memoized per lattice cell — bit-exact)
        let g = self.cell_grads(x_cell, y_cell, z_cell);
        let v000 = grad_dot((g[0] & 15) as usize, dx, weird_dy, dz);
        let v100 = grad_dot((g[1] & 15) as usize, dx - 1.0, weird_dy, dz);
        let v010 = grad_dot((g[2] & 15) as usize, dx, weird_dy - 1.0, dz);
        let v110 = grad_dot((g[3] & 15) as usize, dx - 1.0, weird_dy - 1.0, dz);
        let v001 = grad_dot((g[4] & 15) as usize, dx, weird_dy, dz - 1.0);
        let v101 = grad_dot((g[5] & 15) as usize, dx - 1.0, weird_dy, dz - 1.0);
        let v011 = grad_dot((g[6] & 15) as usize, dx, weird_dy - 1.0, dz - 1.0);
        let v111 = grad_dot((g[7] & 15) as usize, dx - 1.0, weird_dy - 1.0, dz - 1.0);
        let fx = smoothstep(dx);
        let fy = smoothstep(dy);
        let fz = smoothstep(dz);
        // Mth.lerp3(deltaX, deltaY, deltaZ, v000, v100, v010, v110, v001, v101, v011, v111)
        lerp(
            fz,
            lerp(
                fy,
                lerp(fx, v000, v100),
                lerp(fx, v010, v110),
            ),
            lerp(
                fy,
                lerp(fx, v001, v101),
                lerp(fx, v011, v111),
            ),
        )
    }

    /// noise(x, y, z, yScale, yMax) - full form with the y-quantization quirk
    /// Vanilla: sampleAndLerp(floorX, floorY, floorZ, d3, d4 - d7, d5, d4) where
    /// d7 = floor(min(dy, yMax) / yScale + 1.0E-7F) * yScale; gradDot sees d4-d7, smoothstep sees d4.
    #[inline]
    pub fn noise_5(&self, x: f64, y: f64, z: f64, y_scale: f64, y_max: f64) -> f64 {
        let x = x + self.xo;
        let y = y + self.yo;
        let z = z + self.zo;
        let xi = mth_floor(x);
        let yi = mth_floor(y);
        let zi = mth_floor(z);
        let dx = x - xi as f64;
        let dy = y - yi as f64;
        let dz = z - zi as f64;
        let mut d7 = 0.0f64;
        if y_scale != 0.0 {
            let d6 = if y_max >= 0.0 && y_max < dy { y_max } else { dy };
            d7 = (d6 / y_scale + 1.0000000116860974e-07).floor() * y_scale;
        }
        self.sample_and_lerp(xi, yi, zi, dx, dy - d7, dz, dy)
    }

    /// noise(x, y, z, yScale=0, yMax=0)
    #[inline]
    pub fn noise(&self, x: f64, y: f64, z: f64) -> f64 {
        self.noise_5(x, y, z, 0.0, 0.0)
    }

    /// 4-wide batch of noise(): lane-identical arithmetic (IEEE f64), the
    /// permutation gathers stay scalar per lane. Bit-exact vs noise().
    pub fn noise_batch(&self, x: &[f64; 4], y: &[f64; 4], z: &[f64; 4]) -> [f64; 4] {
        self.noise_5_batch(x, y, z, 0.0, &[0.0; 4])
    }

    pub fn noise_5_batch(
        &self,
        xin: &[f64; 4],
        yin: &[f64; 4],
        zin: &[f64; 4],
        y_scale: f64,
        y_max: &[f64; 4],
    ) -> [f64; 4] {
        #[cfg(target_arch = "x86_64")]
        {
            if std::arch::is_x86_feature_detected!("avx2") {
                return unsafe { self.noise_5_batch_avx2(xin, yin, zin, y_scale, y_max) };
            }
        }
        self.noise_5_batch_scalar(xin, yin, zin, y_scale, y_max)
    }

    fn noise_5_batch_scalar(
        &self,
        xin: &[f64; 4],
        yin: &[f64; 4],
        zin: &[f64; 4],
        y_scale: f64,
        y_max: &[f64; 4],
    ) -> [f64; 4] {
        let mut xi = [0i32; 4];
        let mut yi = [0i32; 4];
        let mut zi = [0i32; 4];
        let mut dx = [0f64; 4];
        let mut dy = [0f64; 4];
        let mut dz = [0f64; 4];
        let mut d7 = [0f64; 4];
        for l in 0..4 {
            let x = xin[l] + self.xo;
            let y = yin[l] + self.yo;
            let z = zin[l] + self.zo;
            let a = mth_floor(x);
            let b = mth_floor(y);
            let c = mth_floor(z);
            xi[l] = a;
            yi[l] = b;
            zi[l] = c;
            dx[l] = x - a as f64;
            dy[l] = y - b as f64;
            dz[l] = z - c as f64;
            if y_scale != 0.0 {
                let ym = y_max[l];
                let d6 = if ym >= 0.0 && ym < dy[l] { ym } else { dy[l] };
                d7[l] = (d6 / y_scale + 1.0000000116860974e-07).floor() * y_scale;
            }
        }
        let mut out = [0f64; 4];
        for l in 0..4 {
            out[l] = self.sample_and_lerp(
                xi[l], yi[l], zi[l], dx[l], dy[l] - d7[l], dz[l], dy[l],
            );
        }
        out
    }

    /// AVX2 core of noise_5_batch: the permutation walks stay scalar per lane
    /// (they are short dependent chains), all IEEE f64 math runs as __m256d
    /// lanes. Lane l of every vector op computes exactly the same operation
    /// sequence as the scalar path, so results are bit-identical (no FMA, no
    /// reassociation; mul/add are strict IEEE in both paths).
    #[cfg(target_arch = "x86_64")]
    #[target_feature(enable = "avx2")]
    unsafe fn noise_5_batch_avx2(
        &self,
        xin: &[f64; 4],
        yin: &[f64; 4],
        zin: &[f64; 4],
        y_scale: f64,
        y_max: &[f64; 4],
    ) -> [f64; 4] {
        use std::arch::x86_64::*;
        // ---- scalar cell setup (identical to the scalar path) ----
        // NOTE: a vectorized variant of this setup (vroundpd + cvtpd_epi32)
        // was measured as a REGRESSION (slice 2.07 -> 2.24): the scalar
        // floors overlap via OoO while the vector chain serializes the
        // dependency into the permutation walks. Kept scalar.
        let mut xi = [0i32; 4];
        let mut yi = [0i32; 4];
        let mut zi = [0i32; 4];
        let mut dx = [0f64; 4];
        let mut dy = [0f64; 4];
        let mut dz = [0f64; 4];
        let mut d7 = [0f64; 4];
        for l in 0..4 {
            let x = xin[l] + self.xo;
            let y = yin[l] + self.yo;
            let z = zin[l] + self.zo;
            let a = mth_floor(x);
            let b = mth_floor(y);
            let c = mth_floor(z);
            xi[l] = a;
            yi[l] = b;
            zi[l] = c;
            dx[l] = x - a as f64;
            dy[l] = y - b as f64;
            dz[l] = z - c as f64;
            if y_scale != 0.0 {
                let ym = y_max[l];
                let d6 = if ym >= 0.0 && ym < dy[l] { ym } else { dy[l] };
                d7[l] = (d6 / y_scale + 1.0000000116860974e-07).floor() * y_scale;
            }
        }

        let dxv = _mm256_loadu_pd(dx.as_ptr());
        let dyv = _mm256_loadu_pd(dy.as_ptr());
        let dzv = _mm256_loadu_pd(dz.as_ptr());
        let d7v = _mm256_loadu_pd(d7.as_ptr());
        let wdyv = _mm256_sub_pd(dyv, d7v); // weirdDeltaY = dy - d7
        let fx = smoothstep_v(dxv);
        let fy = smoothstep_v(dyv);
        let fz = smoothstep_v(dzv);

        // ---- per-lane permutation walk: 8 corner gradient bytes ----
        // Corner bit mapping (matches sample_and_lerp):
        //   bit0 = X (dx-1), bit1 = Y (weird_dy-1), bit2 = Z (dz-1)
        // Lane dedup: slice batches fill lanes with CONSECUTIVE y corners at
        // fixed (x, z); low-frequency octaves land several corners in the
        // same lattice cell, so a lane equal to its predecessor skips the
        // walk. Three integer compares on miss — bit-exact by pure-function
        // identity.
        let mut grad = [[0u8; 8]; 4];
        grad[0] = self.cell_grads(xi[0], yi[0], zi[0]);
        for l in 1..4 {
            grad[l] = if xi[l] == xi[l - 1] && yi[l] == yi[l - 1] && zi[l] == zi[l - 1] {
                grad[l - 1]
            } else {
                self.cell_grads(xi[l], yi[l], zi[l])
            };
        }

        // ---- 8 grad_dots across the 4 lanes ----
        let one = _mm256_set1_pd(1.0);
        let mut vc = [_mm256_setzero_pd(); 8];
        for c in 0..8 {
            let mut gxs = [0i32; 4];
            let mut gys = [0i32; 4];
            let mut gzs = [0i32; 4];
            for l in 0..4 {
                let gv = &GRADIENT[(grad[l][c] & 15) as usize];
                gxs[l] = gv[0];
                gys[l] = gv[1];
                gzs[l] = gv[2];
            }
            let gx = _mm256_cvtepi32_pd(_mm_loadu_si128(gxs.as_ptr() as *const __m128i));
            let gy = _mm256_cvtepi32_pd(_mm_loadu_si128(gys.as_ptr() as *const __m128i));
            let gz = _mm256_cvtepi32_pd(_mm_loadu_si128(gzs.as_ptr() as *const __m128i));
            let xv = if c & 1 != 0 { _mm256_sub_pd(dxv, one) } else { dxv };
            let yv = if c & 2 != 0 { _mm256_sub_pd(wdyv, one) } else { wdyv };
            let zv = if c & 4 != 0 { _mm256_sub_pd(dzv, one) } else { dzv };
            // gradDot: (gx*x + gy*y) + gz*z — same association as the scalar path
            let t0 = _mm256_mul_pd(gx, xv);
            let t1 = _mm256_mul_pd(gy, yv);
            let t2 = _mm256_mul_pd(gz, zv);
            vc[c] = _mm256_add_pd(_mm256_add_pd(t0, t1), t2);
        }

        // ---- Mth.lerp3 across corners ----
        let r0 = lerp_v(fx, vc[0], vc[1]);
        let r1 = lerp_v(fx, vc[2], vc[3]);
        let r01 = lerp_v(fy, r0, r1);
        let r2 = lerp_v(fx, vc[4], vc[5]);
        let r3 = lerp_v(fx, vc[6], vc[7]);
        let r23 = lerp_v(fy, r2, r3);
        let res = lerp_v(fz, r01, r23);
        let mut out = [0f64; 4];
        _mm256_storeu_pd(out.as_mut_ptr(), res);
        out
    }

    /// 8-wide batch: AVX-512 native when available, otherwise two 4-wide
    /// AVX2 halves (lane-identical arithmetic in both — bit-exact vs noise_5).
    pub fn noise_5_batch8(
        &self,
        xin: &[f64; 8],
        yin: &[f64; 8],
        zin: &[f64; 8],
        y_scale: f64,
        y_max: &[f64; 8],
    ) -> [f64; 8] {
        // AVX-512 is opt-in (CRUS_AVX512=1): on several x86 parts (SKX/CLX)
        // 512-bit f64 downclocks and measured SLOWER than 2x AVX2; the
        // 2x-256-bit path below is the safe default. Both are bit-exact.
        #[cfg(target_arch = "x86_64")]
        {
            if std::env::var("CRUS_AVX512").as_deref() == Ok("1")
                && std::arch::is_x86_feature_detected!("avx512f")
            {
                return unsafe { self.noise_5_batch_avx512(xin, yin, zin, y_scale, y_max) };
            }
        }
        let mut out = [0f64; 8];
        // zero-copy 4-wide half views (pointer math; halves never alias)
        let (xa, xb) = split8(xin);
        let (ya, yb) = split8(yin);
        let (za, zb) = split8(zin);
        let (ma, mb) = split8(y_max);
        let va = self.noise_5_batch(xa, ya, za, y_scale, ma);
        let vb = self.noise_5_batch(xb, yb, zb, y_scale, mb);
        out[..4].copy_from_slice(&va);
        out[4..].copy_from_slice(&vb);
        out
    }

    /// AVX-512 core: same structure as the AVX2 core with 8 lanes in __m512d.
    #[cfg(target_arch = "x86_64")]
    #[target_feature(enable = "avx512f")]
    unsafe fn noise_5_batch_avx512(
        &self,
        xin: &[f64; 8],
        yin: &[f64; 8],
        zin: &[f64; 8],
        y_scale: f64,
        y_max: &[f64; 8],
    ) -> [f64; 8] {
        use std::arch::x86_64::*;
        let mut xi = [0i32; 8];
        let mut yi = [0i32; 8];
        let mut zi = [0i32; 8];
        let mut dx = [0f64; 8];
        let mut dy = [0f64; 8];
        let mut dz = [0f64; 8];
        let mut d7 = [0f64; 8];
        for l in 0..8 {
            let x = xin[l] + self.xo;
            let y = yin[l] + self.yo;
            let z = zin[l] + self.zo;
            let a = mth_floor(x);
            let b = mth_floor(y);
            let c = mth_floor(z);
            xi[l] = a;
            yi[l] = b;
            zi[l] = c;
            dx[l] = x - a as f64;
            dy[l] = y - b as f64;
            dz[l] = z - c as f64;
            if y_scale != 0.0 {
                let ym = y_max[l];
                let d6 = if ym >= 0.0 && ym < dy[l] { ym } else { dy[l] };
                d7[l] = (d6 / y_scale + 1.0000000116860974e-07).floor() * y_scale;
            }
        }

        let dxv = _mm512_loadu_pd(dx.as_ptr());
        let dyv = _mm512_loadu_pd(dy.as_ptr());
        let dzv = _mm512_loadu_pd(dz.as_ptr());
        let d7v = _mm512_loadu_pd(d7.as_ptr());
        let wdyv = _mm512_sub_pd(dyv, d7v);
        // smoothstep(dx): t*t*t*(t*(t*6.0-15.0)+10.0)
        let smooth = |t: __m512d| -> __m512d {
            let t2 = _mm512_mul_pd(t, t);
            let t3 = _mm512_mul_pd(t2, t);
            let a = _mm512_mul_pd(t, _mm512_set1_pd(6.0));
            let b = _mm512_sub_pd(a, _mm512_set1_pd(15.0));
            let c = _mm512_mul_pd(b, t);
            let d = _mm512_add_pd(c, _mm512_set1_pd(10.0));
            _mm512_mul_pd(t3, d)
        };
        let fx = smooth(dxv);
        let fy = smooth(dyv);
        let fz = smooth(dzv);

        let mut grad = [[0u8; 8]; 8];
        grad[0] = self.cell_grads(xi[0], yi[0], zi[0]);
        for l in 1..8 {
            grad[l] = if xi[l] == xi[l - 1] && yi[l] == yi[l - 1] && zi[l] == zi[l - 1] {
                grad[l - 1]
            } else {
                self.cell_grads(xi[l], yi[l], zi[l])
            };
        }

        let one = _mm512_set1_pd(1.0);
        let mut vc = [_mm512_setzero_pd(); 8];
        for c in 0..8 {
            let mut gxs = [0i32; 8];
            let mut gys = [0i32; 8];
            let mut gzs = [0i32; 8];
            for l in 0..8 {
                let gv = &GRADIENT[(grad[l][c] & 15) as usize];
                gxs[l] = gv[0];
                gys[l] = gv[1];
                gzs[l] = gv[2];
            }
            let gx = _mm512_cvtepi32_pd(_mm256_loadu_si256(gxs.as_ptr() as *const __m256i));
            let gy = _mm512_cvtepi32_pd(_mm256_loadu_si256(gys.as_ptr() as *const __m256i));
            let gz = _mm512_cvtepi32_pd(_mm256_loadu_si256(gzs.as_ptr() as *const __m256i));
            let xv = if c & 1 != 0 { _mm512_sub_pd(dxv, one) } else { dxv };
            let yv = if c & 2 != 0 { _mm512_sub_pd(wdyv, one) } else { wdyv };
            let zv = if c & 4 != 0 { _mm512_sub_pd(dzv, one) } else { dzv };
            let t0 = _mm512_mul_pd(gx, xv);
            let t1 = _mm512_mul_pd(gy, yv);
            let t2 = _mm512_mul_pd(gz, zv);
            vc[c] = _mm512_add_pd(_mm512_add_pd(t0, t1), t2);
        }

        // Mth.lerp3: a + t*(b-a)
        let lp = |t: __m512d, a: __m512d, b: __m512d| -> __m512d {
            _mm512_add_pd(a, _mm512_mul_pd(t, _mm512_sub_pd(b, a)))
        };
        let r0 = lp(fx, vc[0], vc[1]);
        let r1 = lp(fx, vc[2], vc[3]);
        let r01 = lp(fy, r0, r1);
        let r2 = lp(fx, vc[4], vc[5]);
        let r3 = lp(fx, vc[6], vc[7]);
        let r23 = lp(fy, r2, r3);
        let res = lp(fz, r01, r23);
        let mut out = [0f64; 8];
        _mm512_storeu_pd(out.as_mut_ptr(), res);
        out
    }
}

// ---------------- PerlinNoise ----------------
pub struct PerlinNoise {
    pub first_octave: i32,
    pub amplitudes: Vec<f64>,
    pub noise_levels: Vec<Option<ImprovedNoise>>,
    pub lowest_freq_input_factor: f64,
    pub lowest_freq_value_factor: f64,
    pub max_value: f64,
}

fn wrap(d: f64) -> f64 {
    d - (d / 33554432.0 + 0.5).floor() * 33554432.0
}

impl PerlinNoise {
    /// Modern construction (useNewFactory=true): octaves seeded via
    /// factory.fromHashOf("octave_" + (firstOctave + i)).
    pub fn create(rng: &mut XoroshiroRandomSource, first_octave: i32, amplitudes: Vec<f64>) -> Self {
        let k = amplitudes.len();
        let mut noise_levels: Vec<Option<ImprovedNoise>> = Vec::with_capacity(k);
        let factory = rng.fork_positional();
        for i in 0..k {
            if amplitudes[i] != 0.0 {
                let j = first_octave + i as i32;
                let mut seed_src = factory.from_hash_of(&format!("octave_{}", j));
                noise_levels.push(Some(ImprovedNoise::new_xoroshiro(&mut seed_src)));
            } else {
                noise_levels.push(None);
            }
        }
        Self::finish(first_octave, amplitudes, noise_levels)
    }

    /// Legacy construction (createLegacyForBlendedNoise): octaves consumed sequentially
    /// from the same RandomSource; first ImprovedNoise goes to index -firstOctave.
    pub fn create_legacy_for_blended_noise(
        rng: &mut XoroshiroRandomSource,
        first_octave: i32,
        amplitudes: Vec<f64>,
    ) -> Self {
        // firstOctave = -15, amplitudes all 1.0, len = 16 (octaves -15..0)
        let k = amplitudes.len();
        let mut noise_levels: Vec<Option<ImprovedNoise>> = vec![None; k];
        let l = (-first_octave) as usize; // 15
        {
            let first = ImprovedNoise::new_xoroshiro(rng);
            if l < k && amplitudes[l] != 0.0 {
                noise_levels[l] = Some(first);
            }
        }
        let mut m = l as i32 - 1;
        while m >= 0 {
            if (m as usize) < k {
                if amplitudes[m as usize] != 0.0 {
                    noise_levels[m as usize] = Some(ImprovedNoise::new_xoroshiro(rng));
                } else {
                    // skipOctave: consumeCount(262)
                    for _ in 0..262 {
                        rng.next_i32();
                    }
                }
            } else {
                for _ in 0..262 {
                    rng.next_i32();
                }
            }
            m -= 1;
        }
        Self::finish(first_octave, amplitudes, noise_levels)
    }

    fn finish(
        first_octave: i32,
        amplitudes: Vec<f64>,
        noise_levels: Vec<Option<ImprovedNoise>>,
    ) -> Self {
        let k = amplitudes.len();
        // Vanilla: i = -firstOctave; lowestFreqInputFactor = 2^(-i) = 2^firstOctave
        let lowest_freq_input_factor = 2.0f64.powi(first_octave);
        let lowest_freq_value_factor =
            2.0f64.powi((k - 1) as i32) / (2.0f64.powi(k as i32) - 1.0);
        let mut max_value = 0.0;
        {
            // edgeValue(2.0)
            let mut v = 0.0;
            let mut vf = lowest_freq_value_factor;
            for i in 0..k {
                if noise_levels[i].is_some() {
                    v += amplitudes[i] * 2.0 * vf;
                }
                vf /= 2.0;
            }
            max_value = v;
        }
        PerlinNoise {
            first_octave,
            amplitudes,
            noise_levels,
            lowest_freq_input_factor,
            lowest_freq_value_factor,
            max_value,
        }
    }

    /// PerlinNoise.getValue(x, y, z) with yScale=0, yMax=0, useFixedY=false
    #[inline]
    pub fn get_value(&self, x: f64, y: f64, z: f64) -> f64 {
        let mut d = 0.0;
        let mut input_factor = self.lowest_freq_input_factor;
        let mut value_factor = self.lowest_freq_value_factor;
        for i in 0..self.noise_levels.len() {
            if let Some(nl) = &self.noise_levels[i] {
                let v = nl.noise(
                    wrap(x * input_factor),
                    wrap(y * input_factor),
                    wrap(z * input_factor),
                );
                d += self.amplitudes[i] * v * value_factor;
            }
            input_factor *= 2.0;
            value_factor /= 2.0;
        }
        d
    }

    /// 8-wide get_value: lane-identical octave loop (bit-exact vs get_value).
    pub fn get_value_batch8(
        &self,
        x: &[f64; 8],
        y: &[f64; 8],
        z: &[f64; 8],
    ) -> [f64; 8] {
        let mut d = [0f64; 8];
        let mut input_factor = self.lowest_freq_input_factor;
        let mut value_factor = self.lowest_freq_value_factor;
        for i in 0..self.noise_levels.len() {
            if let Some(nl) = &self.noise_levels[i] {
                let mut wx = [0f64; 8];
                let mut wy = [0f64; 8];
                let mut wz = [0f64; 8];
                for l in 0..8 {
                    wx[l] = wrap(x[l] * input_factor);
                    wy[l] = wrap(y[l] * input_factor);
                    wz[l] = wrap(z[l] * input_factor);
                }
                let v = nl.noise_5_batch8(&wx, &wy, &wz, 0.0, &[0.0; 8]);
                for l in 0..8 {
                    d[l] += self.amplitudes[i] * v[l] * value_factor;
                }
            }
            input_factor *= 2.0;
            value_factor /= 2.0;
        }
        d
    }

    /// Vanilla: getOctaveNoise(octave) = noiseLevels[noiseLevels.length - 1 - octave]
    pub fn get_octave_noise(&self, i: usize) -> Option<&ImprovedNoise> {
        self.noise_levels[self.noise_levels.len() - 1 - i].as_ref()
    }

    /// 4-wide get_value: lane-identical octave loop (bit-exact vs get_value).
    pub fn get_value_batch(
        &self,
        x: &[f64; 4],
        y: &[f64; 4],
        z: &[f64; 4],
    ) -> [f64; 4] {
        let mut d = [0f64; 4];
        let mut input_factor = self.lowest_freq_input_factor;
        let mut value_factor = self.lowest_freq_value_factor;
        for i in 0..self.noise_levels.len() {
            if let Some(nl) = &self.noise_levels[i] {
                let mut wx = [0f64; 4];
                let mut wy = [0f64; 4];
                let mut wz = [0f64; 4];
                for l in 0..4 {
                    wx[l] = wrap(x[l] * input_factor);
                    wy[l] = wrap(y[l] * input_factor);
                    wz[l] = wrap(z[l] * input_factor);
                }
                let v = nl.noise_batch(&wx, &wy, &wz);
                for l in 0..4 {
                    d[l] += self.amplitudes[i] * v[l] * value_factor;
                }
            }
            input_factor *= 2.0;
            value_factor /= 2.0;
        }
        d
    }
}

// ---------------- NormalNoise ----------------
pub struct NormalNoise {
    pub first: PerlinNoise,
    pub second: PerlinNoise,
    pub value_factor: f64,
    pub max_value: f64,
}

impl NormalNoise {
    pub fn create(rng: &mut XoroshiroRandomSource, first_octave: i32, amplitudes: Vec<f64>) -> Self {
        let first = PerlinNoise::create(rng, first_octave, amplitudes.clone());
        let second = PerlinNoise::create(rng, first_octave, amplitudes);
        Self::finish(first, second)
    }

    fn finish(first: PerlinNoise, second: PerlinNoise) -> Self {
        let amps = &first.amplitudes;
        let mut min_idx = i32::MAX;
        let mut max_idx = i32::MIN;
        for (i, a) in amps.iter().enumerate() {
            if *a != 0.0 {
                min_idx = min_idx.min(i as i32);
                max_idx = max_idx.max(i as i32);
            }
        }
        let expected_deviation = |i: i32| -> f64 { 0.1 * (1.0 + 1.0 / (i as f64 + 1.0)) };
        let value_factor = 0.16666666666666666 / expected_deviation(max_idx - min_idx);
        let max_value = (first.max_value + second.max_value) * value_factor;
        NormalNoise {
            first,
            second,
            value_factor,
            max_value,
        }
    }

    #[inline]
    pub fn get_value(&self, x: f64, y: f64, z: f64) -> f64 {
        (self.first.get_value(x, y, z)
            + self.second.get_value(
                x * 1.0181268882175227,
                y * 1.0181268882175227,
                z * 1.0181268882175227,
            )) * self.value_factor
    }

    /// 4-wide get_value (bit-exact per lane).
    pub fn get_value_batch(&self, x: &[f64; 4], y: &[f64; 4], z: &[f64; 4]) -> [f64; 4] {
        let a = self.first.get_value_batch(x, y, z);
        let mut x2 = [0f64; 4];
        let mut y2 = [0f64; 4];
        let mut z2 = [0f64; 4];
        for l in 0..4 {
            x2[l] = x[l] * 1.0181268882175227;
            y2[l] = y[l] * 1.0181268882175227;
            z2[l] = z[l] * 1.0181268882175227;
        }
        let b = self.second.get_value_batch(&x2, &y2, &z2);
        let mut out = [0f64; 4];
        for l in 0..4 {
            out[l] = (a[l] + b[l]) * self.value_factor;
        }
        out
    }

    /// 8-wide get_value (bit-exact per lane).
    pub fn get_value_batch8(&self, x: &[f64; 8], y: &[f64; 8], z: &[f64; 8]) -> [f64; 8] {
        let a = self.first.get_value_batch8(x, y, z);
        let mut x2 = [0f64; 8];
        let mut y2 = [0f64; 8];
        let mut z2 = [0f64; 8];
        for l in 0..8 {
            x2[l] = x[l] * 1.0181268882175227;
            y2[l] = y[l] * 1.0181268882175227;
            z2[l] = z[l] * 1.0181268882175227;
        }
        let b = self.second.get_value_batch8(&x2, &y2, &z2);
        let mut out = [0f64; 8];
        for l in 0..8 {
            out[l] = (a[l] + b[l]) * self.value_factor;
        }
        out
    }
}

// ---------------- BlendedNoise (old_blended_noise) ----------------
pub struct BlendedNoise {
    pub min_noise: PerlinNoise,
    pub max_noise: PerlinNoise,
    pub main_noise: PerlinNoise,
    pub xz_multiplier: f64, // 684.412 * xz_scale
    pub y_multiplier: f64,  // 684.412 * y_scale
    pub xz_factor: f64,
    pub y_factor: f64,
    pub smear_scale_multiplier: f64,
    pub max_value: f64,
}

impl BlendedNoise {
    pub fn create(
        rng: &mut XoroshiroRandomSource,
        xz_scale: f64,
        y_scale: f64,
        xz_factor: f64,
        y_factor: f64,
        smear_scale_multiplier: f64,
    ) -> Self {
        let amps16: Vec<f64> = vec![1.0; 16];
        let amps8: Vec<f64> = vec![1.0; 8];
        let min_limit = PerlinNoise::create_legacy_for_blended_noise(rng, -15, amps16.clone());
        let max_limit = PerlinNoise::create_legacy_for_blended_noise(rng, -15, amps16);
        let main = PerlinNoise::create_legacy_for_blended_noise(rng, -7, amps8);
        Self::finish(min_limit, max_limit, main, xz_scale, y_scale, xz_factor, y_factor, smear_scale_multiplier)
    }

    #[allow(clippy::too_many_arguments)]
    fn finish(
        min_limit_noise: PerlinNoise,
        max_limit_noise: PerlinNoise,
        main_noise: PerlinNoise,
        xz_scale: f64,
        y_scale: f64,
        xz_factor: f64,
        y_factor: f64,
        smear_scale_multiplier: f64,
    ) -> Self {
        let xz_multiplier = 684.412 * xz_scale;
        let y_multiplier = 684.412 * y_scale;
        // maxBrokenValue(yMultiplier) = edgeValue(yMultiplier + 2.0)
        let mut v = 0.0;
        let mut vf = min_limit_noise.lowest_freq_value_factor;
        for i in 0..min_limit_noise.noise_levels.len() {
            if min_limit_noise.noise_levels[i].is_some() {
                v += min_limit_noise.amplitudes[i] * (y_multiplier + 2.0) * vf;
            }
            vf /= 2.0;
        }
        let max_value = v;
        BlendedNoise {
            min_noise: min_limit_noise,
            max_noise: max_limit_noise,
            main_noise,
            xz_multiplier,
            y_multiplier,
            xz_factor,
            y_factor,
            smear_scale_multiplier,
            max_value,
        }
    }

    #[inline]
    pub fn compute(&self, block_x: i32, block_y: i32, block_z: i32) -> f64 {
        let d0 = block_x as f64 * self.xz_multiplier;
        let d1 = block_y as f64 * self.y_multiplier;
        let d2 = block_z as f64 * self.xz_multiplier;
        let d3 = d0 / self.xz_factor;
        let d4 = d1 / self.y_factor;
        let d5 = d2 / self.xz_factor;
        let d6 = self.y_multiplier * self.smear_scale_multiplier;
        let d7 = d6 / self.y_factor;

        // main noise loop, octaves 0..8 via getOctaveNoise (REVERSED: [7-i])
        let mut d10 = 0.0;
        let mut amp = 1.0;
        for i in 0..8usize {
            if let Some(oct) = self.main_noise.get_octave_noise(i) {
                d10 += oct.noise_5(
                    wrap(d3 * amp),
                    wrap(d4 * amp),
                    wrap(d5 * amp),
                    d7 * amp,
                    d4 * amp,
                ) / amp;
            }
            amp /= 2.0;
        }
        let d11b = (d10 / 10.0 + 1.0) / 2.0;
        let skip_min = d11b >= 1.0;
        let skip_max = d11b <= 0.0;

        let mut d8 = 0.0;
        let mut d9 = 0.0;
        let mut amp = 1.0;
        for i in 0..16usize {
            let dx = wrap(d0 * amp);
            let dy = wrap(d1 * amp);
            let dz = wrap(d2 * amp);
            let dyy = d6 * amp;
            if !skip_min {
                if let Some(oct) = self.min_noise.get_octave_noise(i) {
                    d8 += oct.noise_5(dx, dy, dz, dyy, d1 * amp) / amp;
                }
            }
            if !skip_max {
                if let Some(oct) = self.max_noise.get_octave_noise(i) {
                    d9 += oct.noise_5(dx, dy, dz, dyy, d1 * amp) / amp;
                }
            }
            amp /= 2.0;
        }
        // clampedLerp(d8/512, d9/512, d11b) / 128
        let t = d11b;
        let (a, b) = (d8 / 512.0, d9 / 512.0);
        let v = if t < 0.0 {
            a
        } else if t > 1.0 {
            b
        } else {
            a + t * (b - a)
        };
        v / 128.0
    }

    /// 4-wide compute: lane-identical arithmetic; per-lane y inputs; the
    /// skip_min/skip_max decision is per lane (skipped lanes keep their
    /// accumulator at 0.0, matching vanilla's never-run loop). Bit-exact.
    pub fn compute_batch(
        &self,
        bx: &[i32; 4],
        by: &[i32; 4],
        bz: &[i32; 4],
    ) -> [f64; 4] {
        let mut d0 = [0f64; 4];
        let mut d1 = [0f64; 4];
        let mut d2 = [0f64; 4];
        for l in 0..4 {
            d0[l] = bx[l] as f64 * self.xz_multiplier;
            d1[l] = by[l] as f64 * self.y_multiplier;
            d2[l] = bz[l] as f64 * self.xz_multiplier;
        }
        let mut d3 = [0f64; 4];
        let mut d4 = [0f64; 4];
        let mut d5 = [0f64; 4];
        for l in 0..4 {
            d3[l] = d0[l] / self.xz_factor;
            d4[l] = d1[l] / self.y_factor;
            d5[l] = d2[l] / self.xz_factor;
        }
        let d6 = self.y_multiplier * self.smear_scale_multiplier;
        let d7 = d6 / self.y_factor;

        // main noise loop, octaves 0..8 (REVERSED lookup [7-i])
        let mut d10 = [0f64; 4];
        let mut amp = 1.0f64;
        for i in 0..8usize {
            if let Some(oct) = self.main_noise.get_octave_noise(i) {
                let mut wx = [0f64; 4];
                let mut wy = [0f64; 4];
                let mut wz = [0f64; 4];
                let mut ymax = [0f64; 4];
                for l in 0..4 {
                    wx[l] = wrap(d3[l] * amp);
                    wy[l] = wrap(d4[l] * amp);
                    wz[l] = wrap(d5[l] * amp);
                    ymax[l] = d4[l] * amp;
                }
                let v = oct.noise_5_batch(&wx, &wy, &wz, d7 * amp, &ymax);
                for l in 0..4 {
                    d10[l] += v[l] / amp;
                }
            }
            amp /= 2.0;
        }
        let mut d11b = [0f64; 4];
        let mut skip_min = [false; 4];
        let mut skip_max = [false; 4];
        for l in 0..4 {
            d11b[l] = (d10[l] / 10.0 + 1.0) / 2.0;
            skip_min[l] = d11b[l] >= 1.0;
            skip_max[l] = d11b[l] <= 0.0;
        }

        let mut d8 = [0f64; 4];
        let mut d9 = [0f64; 4];
        let mut amp = 1.0f64;
        for i in 0..16usize {
            let mut dx = [0f64; 4];
            let mut dy = [0f64; 4];
            let mut dz = [0f64; 4];
            let mut dyy = [0f64; 4];
            let mut ymax = [0f64; 4];
            for l in 0..4 {
                dx[l] = wrap(d0[l] * amp);
                dy[l] = wrap(d1[l] * amp);
                dz[l] = wrap(d2[l] * amp);
                dyy[l] = d6 * amp;
                ymax[l] = d1[l] * amp;
            }
            if !skip_min.iter().all(|s| *s) {
                if let Some(oct) = self.min_noise.get_octave_noise(i) {
                    let v = oct.noise_5_batch(&dx, &dy, &dz, dyy[0], &ymax);
                    for l in 0..4 {
                        if !skip_min[l] {
                            d8[l] += v[l] / amp;
                        }
                    }
                }
            }
            if !skip_max.iter().all(|s| *s) {
                if let Some(oct) = self.max_noise.get_octave_noise(i) {
                    let v = oct.noise_5_batch(&dx, &dy, &dz, dyy[0], &ymax);
                    for l in 0..4 {
                        if !skip_max[l] {
                            d9[l] += v[l] / amp;
                        }
                    }
                }
            }
            amp /= 2.0;
        }
        let mut out = [0f64; 4];
        for l in 0..4 {
            let t = d11b[l];
            let (a, b) = (d8[l] / 512.0, d9[l] / 512.0);
            let v = if t < 0.0 {
                a
            } else if t > 1.0 {
                b
            } else {
                a + t * (b - a)
            };
            out[l] = v / 128.0;
        }
        out
    }

    /// 8-wide compute (bit-exact per lane).
    pub fn compute_batch8(
        &self,
        bx: &[i32; 8],
        by: &[i32; 8],
        bz: &[i32; 8],
    ) -> [f64; 8] {
        let mut d0 = [0f64; 8];
        let mut d1 = [0f64; 8];
        let mut d2 = [0f64; 8];
        for l in 0..8 {
            d0[l] = bx[l] as f64 * self.xz_multiplier;
            d1[l] = by[l] as f64 * self.y_multiplier;
            d2[l] = bz[l] as f64 * self.xz_multiplier;
        }
        let mut d3 = [0f64; 8];
        let mut d4 = [0f64; 8];
        let mut d5 = [0f64; 8];
        for l in 0..8 {
            d3[l] = d0[l] / self.xz_factor;
            d4[l] = d1[l] / self.y_factor;
            d5[l] = d2[l] / self.xz_factor;
        }
        let d6 = self.y_multiplier * self.smear_scale_multiplier;
        let d7 = d6 / self.y_factor;

        let mut d10 = [0f64; 8];
        let mut amp = 1.0f64;
        for i in 0..8usize {
            if let Some(oct) = self.main_noise.get_octave_noise(i) {
                let mut wx = [0f64; 8];
                let mut wy = [0f64; 8];
                let mut wz = [0f64; 8];
                let mut ymax = [0f64; 8];
                for l in 0..8 {
                    wx[l] = wrap(d3[l] * amp);
                    wy[l] = wrap(d4[l] * amp);
                    wz[l] = wrap(d5[l] * amp);
                    ymax[l] = d4[l] * amp;
                }
                let v = oct.noise_5_batch8(&wx, &wy, &wz, d7 * amp, &ymax);
                for l in 0..8 {
                    d10[l] += v[l] / amp;
                }
            }
            amp /= 2.0;
        }
        let mut d11b = [0f64; 8];
        let mut skip_min = [false; 8];
        let mut skip_max = [false; 8];
        for l in 0..8 {
            d11b[l] = (d10[l] / 10.0 + 1.0) / 2.0;
            skip_min[l] = d11b[l] >= 1.0;
            skip_max[l] = d11b[l] <= 0.0;
        }

        let mut d8 = [0f64; 8];
        let mut d9 = [0f64; 8];
        let mut amp = 1.0f64;
        for i in 0..16usize {
            let mut dx = [0f64; 8];
            let mut dy = [0f64; 8];
            let mut dz = [0f64; 8];
            let mut dyy = [0f64; 8];
            let mut ymax = [0f64; 8];
            for l in 0..8 {
                dx[l] = wrap(d0[l] * amp);
                dy[l] = wrap(d1[l] * amp);
                dz[l] = wrap(d2[l] * amp);
                dyy[l] = d6 * amp;
                ymax[l] = d1[l] * amp;
            }
            // per-half skip: halves are independent 4-wide batches, and an
            // all-skipped half keeps its accumulator at 0.0 exactly like the
            // never-run vanilla loop (mirrors compute_batch's group skip).
            if !skip_min.iter().take(4).all(|s| *s) {
                if let Some(oct) = self.min_noise.get_octave_noise(i) {
                    let va = oct.noise_5_batch(split8(&dx).0, split8(&dy).0, split8(&dz).0, dyy[0], split8(&ymax).0);
                    for l in 0..4 {
                        if !skip_min[l] {
                            d8[l] += va[l] / amp;
                        }
                    }
                }
            }
            if !skip_min.iter().skip(4).all(|s| *s) {
                if let Some(oct) = self.min_noise.get_octave_noise(i) {
                    let vb = oct.noise_5_batch(split8(&dx).1, split8(&dy).1, split8(&dz).1, dyy[0], split8(&ymax).1);
                    for l in 4..8 {
                        if !skip_min[l] {
                            d8[l] += vb[l - 4] / amp;
                        }
                    }
                }
            }
            if !skip_max.iter().take(4).all(|s| *s) {
                if let Some(oct) = self.max_noise.get_octave_noise(i) {
                    let va = oct.noise_5_batch(split8(&dx).0, split8(&dy).0, split8(&dz).0, dyy[0], split8(&ymax).0);
                    for l in 0..4 {
                        if !skip_max[l] {
                            d9[l] += va[l] / amp;
                        }
                    }
                }
            }
            if !skip_max.iter().skip(4).all(|s| *s) {
                if let Some(oct) = self.max_noise.get_octave_noise(i) {
                    let vb = oct.noise_5_batch(split8(&dx).1, split8(&dy).1, split8(&dz).1, dyy[0], split8(&ymax).1);
                    for l in 4..8 {
                        if !skip_max[l] {
                            d9[l] += vb[l - 4] / amp;
                        }
                    }
                }
            }
            amp /= 2.0;
        }
        let mut out = [0f64; 8];
        for l in 0..8 {
            let t = d11b[l];
            let (a, b) = (d8[l] / 512.0, d9[l] / 512.0);
            let v = if t < 0.0 {
                a
            } else if t > 1.0 {
                b
            } else {
                a + t * (b - a)
            };
            out[l] = v / 128.0;
        }
        out
    }
}

// ---------------- SIMD parity tests ----------------
#[cfg(test)]
mod simd_tests {
    use super::*;
    use crate::rng::XoroshiroRandomSource;

    struct SplitMix(u64);
    impl SplitMix {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9E3779B97F4A7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
            z ^ (z >> 31)
        }
        fn f64(&mut self) -> f64 {
            // [-4096, 4096) with full mantissa randomness
            (self.next() as i64 % 8192) as f64 - 4096.0 + (self.next() as f64 / u64::MAX as f64)
        }
        fn f64_pm1(&mut self) -> f64 {
            (self.next() as i64 % 2001) as f64 / 1000.0 - 1.0
        }
    }

    fn mk_noise(sm: &mut SplitMix) -> ImprovedNoise {
        // Build via a real XoroshiroRandomSource seeded from sm (matches prod path)
        let seed = sm.next();
        let mut rng = XoroshiroRandomSource::new(seed as i64);
        ImprovedNoise::new_xoroshiro(&mut rng)
    }

    #[test]
    fn noise_batch_simd_matches_scalar() {
        let mut sm = SplitMix(0xC0FFEE ^ 0xDEAD);
        for inst in 0..16 {
            let n = mk_noise(&mut sm);
            let y_scale = if inst % 3 == 0 { 0.0 } else { sm.f64_pm1().abs() * 2.0 };
            for iter in 0..2500 {
                let x = [sm.f64(), sm.f64(), sm.f64(), sm.f64()];
                let y = [sm.f64(), sm.f64(), sm.f64(), sm.f64()];
                let z = [sm.f64(), sm.f64(), sm.f64(), sm.f64()];
                let y_max = [sm.f64(), sm.f64(), sm.f64(), sm.f64()];
                let got = n.noise_5_batch(&x, &y, &z, y_scale, &y_max);
                for l in 0..4 {
                    let want = n.noise_5(x[l], y[l], z[l], y_scale, y_max[l]);
                    assert_eq!(
                        got[l].to_bits(),
                        want.to_bits(),
                        "inst {inst} iter {iter} lane {l} y_scale={y_scale} inputs=({}, {}, {}) ymax={}",
                        x[l], y[l], z[l], y_max[l]
                    );
                }
            }
        }
    }

    #[test]
    fn perlin_normal_batch_matches_scalar() {
        let mut sm = SplitMix(0xB105F00D);
        for inst in 0..8 {
            let seed = sm.next();
            let mut rng = XoroshiroRandomSource::new(seed as i64);
            let amps: Vec<f64> = (0..6).map(|_| if sm.next() % 5 == 0 { 0.0 } else { sm.f64_pm1() }).collect();
            let pn = PerlinNoise::create(&mut rng, -5, amps.clone());
            let nn = NormalNoise::create(&mut rng, -5, amps.clone());
            for _ in 0..1000 {
                let x = [sm.f64(), sm.f64(), sm.f64(), sm.f64()];
                let y = [sm.f64(), sm.f64(), sm.f64(), sm.f64()];
                let z = [sm.f64(), sm.f64(), sm.f64(), sm.f64()];
                let gb = pn.get_value_batch(&x, &y, &z);
                let nb = nn.get_value_batch(&x, &y, &z);
                for l in 0..4 {
                    assert_eq!(gb[l].to_bits(), pn.get_value(x[l], y[l], z[l]).to_bits(), "perlin inst {inst}");
                    assert_eq!(nb[l].to_bits(), nn.get_value(x[l], y[l], z[l]).to_bits(), "normal inst {inst}");
                }
            }
        }
    }

    #[test]
    fn blended_batch_matches_scalar() {
        let mut sm = SplitMix(0x5EED);
        for inst in 0..6 {
            let seed = sm.next();
            let mut rng = XoroshiroRandomSource::new(seed as i64);
            let xz_scale = 0.25 * sm.f64_pm1().abs() + 0.05;
            let y_scale = 0.5 * sm.f64_pm1().abs() + 0.05;
            let bn = BlendedNoise::create(&mut rng, xz_scale, y_scale, 1.0 + sm.f64_pm1().abs() * 2.0, 2.0, 1.0 + (inst % 3) as f64);
            for _ in 0..500 {
                let bx = [sm.next() as i32 % 100000, sm.next() as i32 % 100000, sm.next() as i32 % 100000, sm.next() as i32 % 100000];
                let by = [-100 + (sm.next() as i32 % 320); 4];
                let bz = [sm.next() as i32 % 100000, sm.next() as i32 % 100000, sm.next() as i32 % 100000, sm.next() as i32 % 100000];
                let got = bn.compute_batch(&bx, &by, &bz);
                for l in 0..4 {
                    assert_eq!(got[l].to_bits(), bn.compute(bx[l], by[l], bz[l]).to_bits(), "blended inst {inst}");
                }
            }
        }
    }
}
