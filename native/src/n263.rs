//! Minecraft 26.3 noise core — bit-exact port of the vineflower decompile
//! (reference/dec263/net/minecraft/world/level/levelgen/synth/*).
//!
//! 26.3 rewrote the synth package:
//!   * ImprovedNoise  -> GradientNoise (byte[256] perms, 16-gradient table,
//!     FLOAT evaluation, offsets scaled by 256, far-lands wrap)
//!   * PerlinNoise    -> GradientNoise subclass with pAdj char-packing
//!     (p[n] | p[n+1]<<8) and float lerp3
//!   * NormalNoise    -> Parameters{base_amplitude, base_octave, octave_count,
//!     normalize, amplitude_modifiers}; buildOctaves + estimateDeviation
//!     normalization; create() forks TWO positional factories and emits a
//!     NoiseStack of PerlinNoise octave pairs
//!   * SmearedPerlinNoise -> PerlinNoise + fudgeYScale (y snap), used by
//!     BlendedNoise FBM
//!   * NoiseStack     -> sum of (noise, frequency, amplitude) layers
//!   * BlendedNoise   -> record; createFbmSet(-15/-15/-7) over
//!     SmearedPerlinNoise, lerp(clamp(main+0.5,0,1), min, max)
//!
//! RNG machinery is UNCHANGED vs 1.21.8 (verified: Xoroshiro128PlusPlus,
//! mixStafford13, upgradeSeedTo128bit, seedFromHashOf are rename-only diffs)
//! so rng.rs/md5.rs are reused directly.
//!
//! Parity oracle: a Java harness compiled against
//! server263/versions/26.3/paper-26.3.jar dumps golden values; see
//! scripts/oracle263_noise.rs + tests at the bottom of this file.

use crate::rng::{XoroshiroRandomSource, XoroshiroPositionalRandomFactory};

pub const HALF_ROUND_OFF: f64 = 16777215.999999998137354850769042968750; // Math.nextDown(1.6777216E7)
pub const ROUND_OFF: f64 = 33554432.0; // 2^25

/// GradientNoise.wrap: far-lands guard (vanilla keeps this ON unless
/// DEBUG_ENABLE_FARLANDS which is a dev flag — always wrap).
#[inline]
pub fn wrap(x: f64) -> f64 {
    if x >= -HALF_ROUND_OFF && x < HALF_ROUND_OFF {
        x
    } else {
        x - (x / ROUND_OFF + 0.5).floor() * ROUND_OFF
    }
}

#[inline]
pub fn smoothstep(x: f32) -> f32 {
    x * x * x * (x * (x * 6.0f32 - 15.0f32) + 10.0f32)
}

/// Mth.lerp3(float...) — note the float alpha path (p0 + a*(p1-p0)).
#[inline]
pub fn lerp3_f32(
    a1: f32,
    a2: f32,
    a3: f32,
    x000: f32,
    x100: f32,
    x010: f32,
    x110: f32,
    x001: f32,
    x101: f32,
    x011: f32,
    x111: f32,
) -> f32 {
    let lo = lerp2_f32(a1, a2, x000, x100, x010, x110);
    let hi = lerp2_f32(a1, a2, x001, x101, x011, x111);
    lo + a3 * (hi - lo)
}

#[inline]
pub fn lerp2_f32(a1: f32, a2: f32, x00: f32, x10: f32, x01: f32, x11: f32) -> f32 {
    let lo = x00 + a1 * (x10 - x00);
    let hi = x01 + a1 * (x11 - x01);
    lo + a2 * (hi - lo)
}

/// Mth.floor(double) -> int (vanilla semantics: (int)Math.floor).
#[inline]
pub fn mth_floor(x: f64) -> i32 {
    let f = x.floor() as i64;
    // Math.floor of anything within int range; values beyond clamp like Java's
    // implicit narrowing cast (Java: (int)Math.floor(x)).
    f as i32
}

/// The 16 static gradients of GradientNoise (i8 triples).
pub const GRADIENT: [(i32, i32, i32); 16] = [
    (1, 1, 0),
    (-1, 1, 0),
    (1, -1, 0),
    (-1, -1, 0),
    (1, 0, 1),
    (-1, 0, 1),
    (1, 0, -1),
    (-1, 0, -1),
    (0, 1, 1),
    (0, -1, 1),
    (0, 1, -1),
    (0, -1, -1),
    (1, 1, 0),
    (0, -1, 1),
    (-1, 1, 0),
    (0, -1, -1),
];

/// NoiseGradients.dot(hash, f32 x, y, z) — float math, flat table.
#[inline]
pub fn gradient_dot_f32(hash: usize, x: f32, y: f32, z: f32) -> f32 {
    let (gx, gy, gz) = GRADIENT[hash & 15];
    gx as f32 * x + gy as f32 * y + gz as f32 * z
}

/// NoiseGradients.dotXz.
#[inline]
pub fn gradient_dot_xz_f32(hash: usize, x: f32, z: f32) -> f32 {
    let (gx, _, gz) = GRADIENT[hash & 15];
    gx as f32 * x + gz as f32 * z
}

/// NoiseGradients.gradY.
#[inline]
pub fn gradient_y(hash: usize) -> f32 {
    GRADIENT[hash & 15].1 as f32
}

/// GradientNoise (base): byte[256] permutation + offsets.
pub struct GradientNoise263 {
    pub perms: [u8; 256],
    pub offset_x: f64,
    pub offset_y: f64,
    pub offset_z: f64,
}

impl GradientNoise263 {
    /// GradientNoise(RandomSource random, double noiseOffsetScale = 256.0).
    pub fn new(random: &mut XoroshiroRandomSource) -> Self {
        Self::with_scale(random, 256.0)
    }

    pub fn with_scale(random: &mut XoroshiroRandomSource, noise_offset_scale: f64) -> Self {
        let offset_x = random.next_f64() * noise_offset_scale;
        let offset_y = random.next_f64() * noise_offset_scale;
        let offset_z = random.next_f64() * noise_offset_scale;
        let mut perms = [0u8; 256];
        for i in 0..256u16 {
            perms[i as usize] = i as u8;
        }
        for i in 0..256usize {
            let offset = random.next_int_bound((256 - i) as i32) as usize;
            perms.swap(i, offset + i);
        }
        GradientNoise263 {
            perms,
            offset_x,
            offset_y,
            offset_z,
        }
    }

    #[inline]
    pub fn permute(&self, x: i32) -> usize {
        self.perms[(x & 0xFF) as usize] as usize
    }
}

/// PerlinNoise (26.3): pAdj packing + float sampling.
pub struct PerlinNoise263 {
    pub base: GradientNoise263,
    /// p[n] | p[n+1] << 8 (Java char = u16).
    pub p_adj: [u16; 256],
}

impl PerlinNoise263 {
    pub fn new(random: &mut XoroshiroRandomSource) -> Self {
        let base = GradientNoise263::new(random);
        let mut p_adj = [0u16; 256];
        for i in 0..256usize {
            let p0 = base.perms[i] as usize;
            // Java: p(i+1) — index i+1, &0xFF applied inside permute()
            let p1 = base.perms[(i + 1) & 0xFF] as usize;
            p_adj[i] = (p0 | (p1 << 8)) as u16;
        }
        PerlinNoise263 { base, p_adj }
    }

    /// PerlinNoise.get(x, y, z) — float result.
    pub fn get(&self, _x: f64, _y: f64, _z: f64) -> f32 {
        let x = wrap(_x) + self.base.offset_x;
        let y = wrap(_y) + self.base.offset_y;
        let z = wrap(_z) + self.base.offset_z;
        let floor_x = mth_floor(x);
        let floor_y = mth_floor(y);
        let floor_z = mth_floor(z);
        let relative_x = (x - floor_x as f64) as f32;
        let relative_y = (y - floor_y as f64) as f32;
        let relative_z = (z - floor_z as f64) as f32;
        self.sample_and_lerp(
            floor_x,
            floor_y,
            floor_z,
            relative_x,
            relative_y,
            relative_z,
            relative_y,
        )
    }

    /// PerlinNoise.get(x, y) — 2D: get(x, 0.0, y).
    pub fn get2(&self, _x: f64, _y: f64) -> f32 {
        self.get(wrap(_x), 0.0, wrap(_y))
    }

    #[allow(clippy::too_many_arguments)]
    fn sample_and_lerp(
        &self,
        x: i32,
        y: i32,
        z: i32,
        relative_x: f32,
        relative_y: f32,
        relative_z: f32,
        original_relative_y: f32,
    ) -> f32 {
        let p = &self.p_adj;
        // Java char arithmetic: p[x & 0xFF] etc. — x may be negative,
        // &0xFF applies to the LOW byte of the int sum.
        let x0 = p[(x & 0xFF) as usize] as usize;
        let x1 = x0 >> 8;
        let xy00 = p[(x0.wrapping_add(y as usize)) & 0xFF] as usize;
        let xy01 = xy00 >> 8;
        let z000 = p[(xy00.wrapping_add(z as usize)) & 0xFF] as usize;
        let z001 = p[(xy01.wrapping_add(z as usize)) & 0xFF] as usize;
        let d000 = gradient_dot_f32(z000 & 15, relative_x, relative_y, relative_z);
        let d001 = gradient_dot_f32(z000 >> 8 & 15, relative_x, relative_y, relative_z - 1.0);
        let d010 = gradient_dot_f32(z001 & 15, relative_x, relative_y - 1.0, relative_z);
        let d011 = gradient_dot_f32(z001 >> 8 & 15, relative_x, relative_y - 1.0, relative_z - 1.0);
        let xy10 = p[(x1.wrapping_add(y as usize)) & 0xFF] as usize;
        let xy11 = xy10 >> 8;
        let z100 = p[(xy10.wrapping_add(z as usize)) & 0xFF] as usize;
        let z101 = p[(xy11.wrapping_add(z as usize)) & 0xFF] as usize;
        let d100 = gradient_dot_f32(z100 & 15, relative_x - 1.0, relative_y, relative_z);
        let d101 = gradient_dot_f32(z100 >> 8 & 15, relative_x - 1.0, relative_y, relative_z - 1.0);
        let d110 = gradient_dot_f32(z101 & 15, relative_x - 1.0, relative_y - 1.0, relative_z);
        let d111 = gradient_dot_f32(z101 >> 8 & 15, relative_x - 1.0, relative_y - 1.0, relative_z - 1.0);
        lerp3_f32(
            smoothstep(relative_x),
            smoothstep(original_relative_y),
            smoothstep(relative_z),
            d000,
            d100,
            d010,
            d110,
            d001,
            d101,
            d011,
            d111,
        )
    }

    /// PerlinNoise.noiseWithDerivative — used by gradient DFs.
    pub fn noise_with_derivative(
        &self,
        _x: f64,
        _y: f64,
        _z: f64,
        derivative_out: &mut [f32; 3],
    ) -> f32 {
        let x = wrap(_x) + self.base.offset_x;
        let y = wrap(_y) + self.base.offset_y;
        let z = wrap(_z) + self.base.offset_z;
        let floor_x = mth_floor(x);
        let floor_y = mth_floor(y);
        let floor_z = mth_floor(z);
        let relative_x = (x - floor_x as f64) as f32;
        let relative_y = (y - floor_y as f64) as f32;
        let relative_z = (z - floor_z as f64) as f32;
        self.sample_with_derivative(
            floor_x,
            floor_y,
            floor_z,
            relative_x,
            relative_y,
            relative_z,
            derivative_out,
        )
    }

    fn sample_with_derivative(
        &self,
        x: i32,
        y: i32,
        z: i32,
        relative_x: f32,
        relative_y: f32,
        relative_z: f32,
        derivative_out: &mut [f32; 3],
    ) -> f32 {
        // Vanilla 26.3 sampleWithDerivative: dot products with gradient
        // vectors, derivative = gradient contributions (all f32).
        let p = &self.base.perms;
        let x0 = p[(x & 0xFF) as usize] as usize;
        let x1 = self.permute(x + 1);
        let xy00 = self.permute(x0 as i32 + y);
        let xy01 = self.permute(xy00 as i32 + y + 1);
        let xy10 = self.permute(x1 as i32 + y);
        let xy11 = self.permute(xy10 as i32 + y + 1);
        let g000 = self.permute(xy00 as i32 + z);
        let g100 = self.permute(xy10 as i32 + z);
        let g010 = self.permute(xy01 as i32 + z);
        let g110 = self.permute(xy11 as i32 + z);
        let g001 = self.permute(xy00 as i32 + z + 1);
        let g101 = self.permute(xy10 as i32 + z + 1);
        let g011 = self.permute(xy01 as i32 + z + 1);
        let g111 = self.permute(xy11 as i32 + z + 1);
        let d000 = gradient_dot_f32(g000, relative_x, relative_y, relative_z);
        let d100 = gradient_dot_f32(g100, relative_x - 1.0, relative_y, relative_z);
        let d010 = gradient_dot_f32(g010, relative_x, relative_y - 1.0, relative_z);
        let d110 = gradient_dot_f32(g110, relative_x - 1.0, relative_y - 1.0, relative_z);
        let d001 = gradient_dot_f32(g001, relative_x, relative_y, relative_z - 1.0);
        let d101 = gradient_dot_f32(g101, relative_x - 1.0, relative_y, relative_z - 1.0);
        let d011 = gradient_dot_f32(g011, relative_x, relative_y - 1.0, relative_z - 1.0);
        let d111 = gradient_dot_f32(g111, relative_x - 1.0, relative_y - 1.0, relative_z - 1.0);
        let a = smoothstep(relative_x);
        let b = smoothstep(relative_y);
        let c = smoothstep(relative_z);
        let (gx000, gy000, gz000) = grad_xyz(g000);
        let (gx100, gy100, gz100) = grad_xyz(g100);
        let (gx010, gy010, gz010) = grad_xyz(g010);
        let (gx110, gy110, gz110) = grad_xyz(g110);
        let (gx001, gy001, gz001) = grad_xyz(g001);
        let (gx101, gy101, gz101) = grad_xyz(g101);
        let (gx011, gy011, gz011) = grad_xyz(g011);
        let (gx111, gy111, gz111) = grad_xyz(g111);
        derivative_out[0] =
            lerp3_f32(a, b, c, gx000, gx100, gx010, gx110, gx001, gx101, gx011, gx111);
        derivative_out[1] =
            lerp3_f32(a, b, c, gy000, gy100, gy010, gy110, gy001, gy101, gy011, gy111);
        derivative_out[2] =
            lerp3_f32(a, b, c, gz000, gz100, gz010, gz110, gz001, gz101, gz011, gz111);
        lerp3_f32(a, b, c, d000, d100, d010, d110, d001, d101, d011, d111)
    }

    fn permute(&self, x: i32) -> usize {
        self.base.permute(x)
    }
}

#[inline]
fn grad_xyz(hash: usize) -> (f32, f32, f32) {
    let (x, y, z) = GRADIENT[hash & 15];
    (x as f32, y as f32, z as f32)
}

/// SmearedPerlinNoise (26.3): fudgeYScale y-snap.
pub struct SmearedPerlinNoise263 {
    pub perlin: PerlinNoise263,
    pub fudge_y_scale: f64,
}

impl SmearedPerlinNoise263 {
    pub fn new(random: &mut XoroshiroRandomSource, fudge_y_scale: f64) -> Self {
        SmearedPerlinNoise263 {
            perlin: PerlinNoise263::new(random),
            fudge_y_scale,
        }
    }

    /// SmearedPerlinNoise.get(x, y, z).
    pub fn get(&self, _x: f64, _y: f64, _z: f64) -> f32 {
        let x = wrap(_x) + self.perlin.base.offset_x;
        let y = wrap(_y) + self.perlin.base.offset_y;
        let z = wrap(_z) + self.perlin.base.offset_z;
        let floor_x = mth_floor(x);
        let floor_y = mth_floor(y);
        let floor_z = mth_floor(z);
        let relative_x = (x - floor_x as f64) as f32;
        let relative_y = y - floor_y as f64; // NOTE: stays double in vanilla
        let relative_z = (z - floor_z as f64) as f32;
        let fudged_relative_y =
            (relative_y - self.compute_fudge_y(_y, relative_y)) as f32;
        self.perlin.sample_and_lerp(
            floor_x,
            floor_y,
            floor_z,
            relative_x,
            fudged_relative_y,
            relative_z,
            relative_y as f32,
        )
    }

    /// SmearedPerlinNoise.computeFudgeY.
    pub fn compute_fudge_y(&self, original_y: f64, relative_y: f64) -> f64 {
        let fudge_limit = if original_y >= 0.0 && original_y < relative_y {
            original_y
        } else {
            relative_y
        };
        (fudge_limit / self.fudge_y_scale + 1.0e-7f32 as f64).floor() * self.fudge_y_scale
    }
}

/// NoiseStack: layered sum. Layer = (noise, frequency, amplitude).
pub enum Noise263 {
    Perlin(Box<PerlinNoise263>),
    Smeared(Box<SmearedPerlinNoise263>),
    Stack(Box<NoiseStack263>),
}

impl Noise263 {
    pub fn get(&self, x: f64, y: f64, z: f64) -> f32 {
        match self {
            Noise263::Perlin(p) => p.get(x, y, z),
            Noise263::Smeared(s) => s.get(x, y, z),
            Noise263::Stack(s) => s.get(x, y, z),
        }
    }
}

pub struct NoiseStack263 {
    pub layers: Vec<(Noise263, f64, f32)>,
}

impl NoiseStack263 {
    pub fn get(&self, x: f64, y: f64, z: f64) -> f32 {
        let mut value = 0.0f32;
        for (noise, frequency, amplitude) in &self.layers {
            value += amplitude * noise.get(x * frequency, y * frequency, z * frequency);
        }
        value
    }
}

/// NormalNoise.Normalization
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Normalization {
    Disabled,
    Enabled,
    Legacy,
}

/// NormalNoise.Parameters (26.3 codec: base_amplitude/base_octave/
/// octave_count/normalize/amplitude_modifiers).
#[derive(Clone, Debug)]
pub struct NormalNoiseParameters {
    pub base_amplitude: f64,
    pub base_octave: i32,
    pub octave_count: i32,
    pub normalize: Normalization,
    pub amplitude_modifiers: Vec<f64>,
}

impl Default for NormalNoiseParameters {
    fn default() -> Self {
        NormalNoiseParameters {
            base_amplitude: 1.0,
            base_octave: 0,
            octave_count: 1,
            normalize: Normalization::Enabled,
            amplitude_modifiers: Vec::new(),
        }
    }
}

struct OctaveInfo {
    octave_index: i32,
    frequency: f64,
    amplitude: f64,
}

/// NormalNoise (26.3): parameters + octave list + normalization; the
/// evaluation is a NoiseStack built in create().
pub struct NormalNoise263 {
    pub parameters: NormalNoiseParameters,
    /// The compiled evaluation stack (create() result).
    pub stack: NoiseStack263,
    pub range: f32,
}

impl NormalNoise263 {
    pub fn new(parameters: NormalNoiseParameters, random: &mut XoroshiroRandomSource) -> Self {
        let octaves = build_octaves(
            parameters.base_octave,
            parameters.base_amplitude,
            parameters.octave_count,
            parameters.normalize != Normalization::Disabled,
            &parameters.amplitude_modifiers,
        );
        let target_amplitude: f64 = octaves.iter().map(|o| o.amplitude.abs()).sum();
        let mut normalization_factor = compute_normalization_factor(target_amplitude, &octaves);
        if parameters.normalize == Normalization::Legacy && normalization_factor != 0.0 {
            let parity = compute_parity_normalization_factor(
                parameters.base_amplitude,
                parameters.octave_count,
                &parameters.amplitude_modifiers,
            );
            normalization_factor = parity;
        }

        // NormalNoise.create(random): two independent positional factories.
        let first_random: XoroshiroPositionalRandomFactory = random.fork_positional();
        let second_random: XoroshiroPositionalRandomFactory = random.fork_positional();
        let mut layers: Vec<(Noise263, f64, f32)> = Vec::with_capacity(octaves.len() * 2);
        for octave in &octaves {
            let octave_seed = format!("octave_{}", octave.octave_index);
            let first = PerlinNoise263::new(&mut first_random.from_hash_of(&octave_seed));
            let second = PerlinNoise263::new(&mut second_random.from_hash_of(&octave_seed));
            let value_factor = (normalization_factor * octave.amplitude) as f32;
            layers.push((Noise263::Perlin(Box::new(first)), octave.frequency, value_factor));
            layers.push((
                Noise263::Perlin(Box::new(second)),
                octave.frequency * 1.0181268882175227,
                value_factor,
            ));
        }

        let range = (target_amplitude * 0.3333333333333333 * 6.0) as f32;
        NormalNoise263 {
            parameters,
            stack: NoiseStack263 { layers },
            range,
        }
    }

    pub fn get(&self, x: f64, y: f64, z: f64) -> f32 {
        self.stack.get(x, y, z)
    }
}

fn build_octaves(
    base_octave: i32,
    base_amplitude: f64,
    octave_count: i32,
    normalize: bool,
    amplitude_modifiers: &[f64],
) -> Vec<OctaveInfo> {
    let mut frequency = 2.0f64.powi(base_octave);
    let mut amplitude = base_amplitude;
    if normalize {
        amplitude = base_amplitude
            * (0.5f64.powf(-(octave_count as f64 - 1.0))
                / (0.5f64.powf(-(octave_count as f64)) - 1.0));
    }
    let mut octaves = Vec::with_capacity(octave_count as usize);
    for i in 0..octave_count {
        let modifier = get_amplitude_modifier(amplitude_modifiers, i);
        if modifier != 0.0 {
            octaves.push(OctaveInfo {
                octave_index: base_octave + i,
                frequency,
                amplitude: amplitude * modifier,
            });
        }
        frequency *= 2.0;
        // bytecode-verified: amplitude halves EVERY iteration (even when the
        // octave is skipped by a zero modifier) — sum of |amp| over a fully
        // normalized stack equals base_amplitude.
        amplitude *= 0.5;
    }
    octaves
}

#[inline]
fn get_amplitude_modifier(modifiers: &[f64], index: i32) -> f64 {
    if modifiers.is_empty() {
        1.0
    } else {
        modifiers[index as usize]
    }
}

fn compute_normalization_factor(target_amplitude: f64, octaves: &[OctaveInfo]) -> f64 {
    let input_deviation = estimate_deviation(octaves);
    if input_deviation == 0.0 {
        0.0
    } else {
        let input_sum_deviation = input_deviation * 2.0f64.sqrt();
        let target_deviation = target_amplitude * 0.3333333333333333;
        target_deviation / input_sum_deviation
    }
}

fn estimate_deviation(octaves: &[OctaveInfo]) -> f64 {
    let mut variance = 0.0f64;
    for octave in octaves {
        let layer_deviation = 0.2702247831245211 * octave.amplitude.abs();
        variance += layer_deviation * layer_deviation;
    }
    variance.sqrt()
}

fn compute_parity_normalization_factor(
    base_amplitude: f64,
    octave_count: i32,
    amplitude_modifiers: &[f64],
) -> f64 {
    let mut min_octave = i32::MAX;
    let mut max_octave = i32::MIN;
    for i in 0..octave_count {
        let modifier = get_amplitude_modifier(amplitude_modifiers, i);
        if modifier != 0.0 {
            min_octave = min_octave.min(i);
            max_octave = max_octave.max(i);
        }
    }
    base_amplitude * 0.5 * 0.3333333333333333 / parity_expected_deviation(max_octave - min_octave)
}

fn parity_expected_deviation(octave_span: i32) -> f64 {
    0.1 * (1.0 + 1.0 / (octave_span as f64 + 1.0))
}

/// BlendedNoise (26.3 record): xz_scale/y_scale/xz_factor/y_factor/
/// smear_scale_multiplier; compiled to lerp(clamp(main+0.5,0,1), min, max).
pub struct BlendedNoise263 {
    pub xz_scale: f64,
    pub y_scale: f64,
    pub xz_factor: f64,
    pub y_factor: f64,
    pub smear_scale_multiplier: f64,
    pub min_limit: NoiseStack263,
    pub max_limit: NoiseStack263,
    pub main: NoiseStack263,
}

impl BlendedNoise263 {
    pub const NOISE_SEED: &'static str = "terrain";

    pub fn new(
        xz_scale: f64,
        y_scale: f64,
        xz_factor: f64,
        y_factor: f64,
        smear_scale_multiplier: f64,
        random: &mut XoroshiroRandomSource,
    ) -> Self {
        let xz_multiplier = 684.412 * xz_scale;
        let y_multiplier = 684.412 * y_scale;
        let limit_smear_scale_y = y_multiplier * smear_scale_multiplier;
        let main_smear_scale_y = limit_smear_scale_y / y_factor;
        let min_limit = create_fbm(random, -15, limit_smear_scale_y, 0.99998474f64);
        let max_limit = create_fbm(random, -15, limit_smear_scale_y, 0.99998474f64);
        let main = create_fbm(random, -7, main_smear_scale_y, 12.75f64);
        BlendedNoise263 {
            xz_scale,
            y_scale,
            xz_factor,
            y_factor,
            smear_scale_multiplier,
            min_limit,
            max_limit,
            main,
        }
    }

    /// BlendedNoise.compileSampler evaluation:
    /// lerp(clamp(main + 0.5, 0, 1), min, max).
    pub fn get(&self, x: f64, y: f64, z: f64) -> f32 {
        let xz_multiplier = 684.412 * self.xz_scale;
        let y_multiplier = 684.412 * self.y_scale;
        let min = self.min_limit.get(x * xz_multiplier, y * y_multiplier, z);
        let max = self.max_limit.get(x * xz_multiplier, y * y_multiplier, z);
        let main = self
            .main
            .get(x * xz_multiplier / self.xz_factor, y * y_multiplier / self.y_factor, z);
        let choice = clamp_f32(main + 0.5, 0.0, 1.0);
        min + choice * (max - min)
    }
}

#[inline]
fn clamp_f32(v: f32, lo: f32, hi: f32) -> f32 {
    if v < lo {
        lo
    } else if v > hi {
        hi
    } else {
        v
    }
}

/// BlendedNoise.createFbm (26.3): SmearedPerlinNoise octave chain.
pub fn create_fbm(
    random: &mut XoroshiroRandomSource,
    first_octave: i32,
    smear_scale_y: f64,
    mut value_factor: f64,
) -> NoiseStack263 {
    assert!(first_octave <= 0, "firstOctave>0");
    let octaves = -first_octave + 1;
    let mut factor = 1.0f64;
    value_factor /= 2.0f64.powi(octaves) - 1.0;
    let mut layers = Vec::with_capacity(octaves as usize);
    let mut i = octaves - 1;
    while i >= 0 {
        let noise = SmearedPerlinNoise263::new(random, smear_scale_y * factor);
        layers.push((Noise263::Smeared(Box::new(noise)), factor, value_factor as f32));
        factor /= 2.0;
        value_factor *= 2.0;
        i -= 1;
    }
    NoiseStack263 { layers }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gradient_table_matches_vanilla() {
        assert_eq!(GRADIENT.len(), 16);
        assert_eq!(GRADIENT[0], (1, 1, 0));
        assert_eq!(GRADIENT[12], (1, 1, 0));
        assert_eq!(GRADIENT[14], (-1, 1, 0));
    }

    #[test]
    fn wrap_matches_vanilla() {
        assert_eq!(wrap(100.0), 100.0);
        assert_eq!(wrap(-100.0), -100.0);
        // outside the band: folds modulo 2^25
        let x = 1.7e7;
        let expected = x - (x / ROUND_OFF + 0.5).floor() * ROUND_OFF;
        assert_eq!(wrap(x), expected);
    }

    #[test]
    fn perlin_is_deterministic_from_seed() {
        let mut r = XoroshiroRandomSource::new(1234567890);
        let p = PerlinNoise263::new(&mut r);
        let a = p.get(1.25, 2.5, 3.75);
        let mut r2 = XoroshiroRandomSource::new(1234567890);
        let p2 = PerlinNoise263::new(&mut r2);
        let b = p2.get(1.25, 2.5, 3.75);
        assert_eq!(a, b);
        assert!(a.is_finite());
    }

    #[test]
    fn normalnoise_octaves_and_seed_chain_deterministic() {
        let params = NormalNoiseParameters {
            base_amplitude: 1.0383104856073737,
            base_octave: -16,
            octave_count: 16,
            normalize: Normalization::Enabled,
            amplitude_modifiers: vec![],
        };
        let mut r = XoroshiroRandomSource::new(42);
        let n = NormalNoise263::new(params.clone(), &mut r);
        let v1 = n.get(10.5, 20.25, 30.125);
        let mut r2 = XoroshiroRandomSource::new(42);
        let n2 = NormalNoise263::new(params, &mut r2);
        let v2 = n2.get(10.5, 20.25, 30.125);
        assert_eq!(v1, v2);
    }

    #[test]
    fn blended_noise_scales() {
        let mut r = XoroshiroRandomSource::new(99);
        let b = BlendedNoise263::new(1.0, 1.0, 80.0, 160.0, 1.0, &mut r);
        let v = b.get(0.5, 0.25, 0.75);
        assert!(v.is_finite());
    }

    #[test]
    fn fudge_y_computation() {
        let mut r = XoroshiroRandomSource::new(7);
        let s = SmearedPerlinNoise263::new(&mut r, 4.277575000000001);
        // computeFudgeY(10.3, 0.3): originalY >= 0 and originalY(10.3) >=
        // relativeY(0.3) -> limit=relativeY; floor(0.3/4.277575 + 1e-7)*4.277575
        let f = s.compute_fudge_y(10.3, 0.3);
        assert!((f - (0.3f64 / 4.277575000000001 + 1.0e-7f64).floor() * 4.277575000000001).abs() < 1e-12);
        // originalY in [0, relativeY): limit = originalY
        let f2 = s.compute_fudge_y(0.1, 0.3);
        assert!((f2 - (0.1f64 / 4.277575000000001 + 1.0e-7f64).floor() * 4.277575000000001).abs() < 1e-12);
    }

    /// Golden bit-parity against oracle263/NoiseOracle263.java run on the
    /// real Paper 26.3 jar (reference/oracle263_noise.txt).
    #[test]
    fn golden_parity_vs_paper263() {
        let golden = include_str!("../../reference/oracle263_noise.txt");
        let seed = 3257840388504953787i64;
        let mut checked = 0;

        let mut r_perlin = XoroshiroRandomSource::new(seed);
        let perlin = PerlinNoise263::new(&mut r_perlin);
        let mut r_normal = XoroshiroRandomSource::new(seed);
        let normal = NormalNoise263::new(
            NormalNoiseParameters {
                base_amplitude: 1.7,
                base_octave: -7,
                octave_count: 4,
                normalize: Normalization::Enabled,
                amplitude_modifiers: vec![],
            },
            &mut r_normal,
        );
        let mut r_parity = XoroshiroRandomSource::new(seed);
        // createParity(-14, 16x1.0): baseOctave=-14, count=16, base_amplitude
        // = computeParityBaseAmplitude(-14, 16x1.0), all modifiers == 1.0
        // (only !=1.0 modifiers are set -> empty list).
        let parity_params = parity_base_amplitude_params(-14, vec![1.0; 16]);
        let normal_parity = NormalNoise263::new(parity_params, &mut r_parity);
        let mut r_smeared = XoroshiroRandomSource::new(seed);
        let smeared = SmearedPerlinNoise263::new(&mut r_smeared, 4.277575000000001);
        let mut r_blend = XoroshiroRandomSource::new(seed);
        let blended = BlendedNoise263::new(1.0, 1.0, 80.0, 160.0, 1.0, &mut r_blend);

        for line in golden.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            match parts[0] {
                "perlin" => {
                    let x: f64 = parts[1].parse().unwrap();
                    let y: f64 = parts[2].parse().unwrap();
                    let z: f64 = parts[3].parse().unwrap();
                    let want: f32 = parts[4].parse().unwrap();
                    let got = perlin.get(x, y, z);
                    assert_eq!(got.to_bits(), want.to_bits(), "perlin {x} {y} {z}: {got} != {want}");
                    checked += 1;
                }
                "normal" | "normalparity" => {
                    let x: f64 = parts[1].parse().unwrap();
                    let y: f64 = parts[2].parse().unwrap();
                    let z: f64 = parts[3].parse().unwrap();
                    let want: f32 = parts[4].parse().unwrap();
                    let got = if parts[0] == "normal" {
                        normal.get(x, y, z)
                    } else {
                        normal_parity.get(x, y, z)
                    };
                    assert_eq!(got.to_bits(), want.to_bits(), "{} {x} {y} {z}: {got} != {want}", parts[0]);
                    checked += 1;
                }
                "smeared" => {
                    let x: f64 = parts[1].parse().unwrap();
                    let y: f64 = parts[2].parse().unwrap();
                    let z: f64 = parts[3].parse().unwrap();
                    let want: f32 = parts[4].parse().unwrap();
                    let got = smeared.get(x, y, z);
                    assert_eq!(got.to_bits(), want.to_bits(), "smeared {x} {y} {z}: {got} != {want}");
                    checked += 1;
                }
                "blended" => {
                    let x: f64 = parts[1].parse().unwrap();
                    let y: f64 = parts[2].parse().unwrap();
                    let z: f64 = parts[3].parse().unwrap();
                    let want_min: f32 = parts[4].parse().unwrap();
                    let want_max: f32 = parts[5].parse().unwrap();
                    let want_main: f32 = parts[6].parse().unwrap();
                    let xzm = 684.412f64;
                    let ym = 684.412f64;
                    let got_min = blended.min_limit.get(x * xzm, y * ym, z);
                    let got_max = blended.max_limit.get(x * xzm, y * ym, z);
                    let got_main = blended.main.get(x * xzm / 80.0, y * ym / 160.0, z);
                    assert_eq!(got_min.to_bits(), want_min.to_bits(), "blended min {x} {y} {z}");
                    assert_eq!(got_max.to_bits(), want_max.to_bits(), "blended max {x} {y} {z}");
                    assert_eq!(got_main.to_bits(), want_main.to_bits(), "blended main {x} {y} {z}");
                    checked += 3;
                }
                "forkA" | "forkB" => {
                    // positional factory chain parity
                    let mut r = XoroshiroRandomSource::new(seed);
                    let f1 = r.fork_positional();
                    let f2 = r.fork_positional();
                    let mut a = f1.from_hash_of("octave_-7");
                    let mut b = f2.from_hash_of("octave_-7");
                    let va = a.next_long() as i64;
                    let vb = b.next_long() as i64;
                    assert_eq!(va, -8619392257271187495i64, "forkA");
                    assert_eq!(vb, -2497823444401815363i64, "forkB");
                    checked += 2;
                }
                _ => panic!("unknown golden line: {line}"),
            }
        }
        assert!(checked >= 20, "too few golden checks: {checked}");
    }
}

/// NormalNoise.createParity(firstOctave, amplitudes) — parity construction
/// used by the 1.21-style noise JSON path (legacy worlds + our goldens).
pub fn parity_base_amplitude_params(first_octave: i32, amplitudes: Vec<f64>) -> NormalNoiseParameters {
    let octave_count = amplitudes.len() as i32;
    // computeParityBaseAmplitude: base_amplitude such that the NEW
    // normalization equals the OLD (legacy) one.
    let octaves = build_octaves(first_octave, 1.0, octave_count, true, &amplitudes);
    let target_amplitude: f64 = octaves.iter().map(|o| o.amplitude.abs()).sum();
    let new_factor = compute_normalization_factor(target_amplitude, &octaves);
    let base_amplitude = if new_factor == 0.0 {
        1.0
    } else {
        let old_factor =
            compute_parity_normalization_factor(1.0, octave_count, &amplitudes);
        old_factor / new_factor
    };
    // Builder semantics: modifiers list stays EMPTY unless some != 1.0;
    // once set, it is a FULL-length list (1.0-filled) with index overrides.
    let modifiers: Vec<f64> = if amplitudes.iter().any(|&m| m != 1.0) {
        amplitudes.clone()
    } else {
        Vec::new()
    };
    NormalNoiseParameters {
        base_amplitude,
        base_octave: first_octave,
        octave_count,
        normalize: Normalization::Enabled,
        amplitude_modifiers: modifiers,
    }
}
