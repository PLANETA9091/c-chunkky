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
}

impl ImprovedNoise {
    pub fn new_xoroshiro(rng: &mut XoroshiroRandomSource) -> Self {
        let mut s = ImprovedNoise {
            xo: 0.0,
            yo: 0.0,
            zo: 0.0,
            p: [0; 256],
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
        let p0 = self.p(x_cell);
        let p1 = self.p(x_cell + 1);
        let i0 = self.p(p0 + y_cell);
        let i1 = self.p(p0 + y_cell + 1);
        let j0 = self.p(p1 + y_cell);
        let j1 = self.p(p1 + y_cell + 1);
        let v000 = grad_dot(self.p(i0 + z_cell) as usize, dx, weird_dy, dz);
        let v100 = grad_dot(self.p(j0 + z_cell) as usize, dx - 1.0, weird_dy, dz);
        let v010 = grad_dot(self.p(i1 + z_cell) as usize, dx, weird_dy - 1.0, dz);
        let v110 = grad_dot(self.p(j1 + z_cell) as usize, dx - 1.0, weird_dy - 1.0, dz);
        let v001 = grad_dot(self.p(i0 + z_cell + 1) as usize, dx, weird_dy, dz - 1.0);
        let v101 = grad_dot(self.p(j0 + z_cell + 1) as usize, dx - 1.0, weird_dy, dz - 1.0);
        let v011 = grad_dot(self.p(i1 + z_cell + 1) as usize, dx, weird_dy - 1.0, dz - 1.0);
        let v111 = grad_dot(self.p(j1 + z_cell + 1) as usize, dx - 1.0, weird_dy - 1.0, dz - 1.0);
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
}

// ---------------- PerlinNoise ----------------
pub struct PerlinNoise {
    pub first_octave: i32,
    pub amplitudes: Vec<f64>,
    pub noise_levels: Vec<Option<ImprovedNoise>>,
    lowest_freq_input_factor: f64,
    lowest_freq_value_factor: f64,
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
    first: PerlinNoise,
    second: PerlinNoise,
    value_factor: f64,
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
}
