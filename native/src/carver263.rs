//! Vanilla 26.3 carvers — CaveWorldCarver + CanyonWorldCarver frozen from
//! reference/dec263/net/minecraft/world/level/levelgen/carver/*.java.
//!
//! 26.3 carver redesign: `carve` writes into a CarvingMask (CarverOutput) for
//! the CENTER chunk; the block application happens later in
//! NoiseBasedChunkGenerator.applyCarvingMask (aquifer.computeSubstance(0.0)
//! per masked cell + UNCARVABLE guard + grass/dirt topMaterial fix — the
//! topMaterial call is deferred until the MaterialSystem port, documented gap).
//!
//! Seeding (NoiseBasedChunkGenerator.generateCarvers 26.3): for each of the
//! 17x17 source chunks, per biome carver list index i:
//!   worldgenRandom.setLargeFeatureSeed(seed + i, sourceX, sourceZ)
//! with the 26.3 formula: setSeed(seed); xScale=nextLong(); zScale=nextLong();
//! setSeed(chunkX*xScale ^ chunkZ*zScale ^ seed).
//!
//! RNG: RandomSource.createThreadLocalInstance(seed) = SingleThreadedRandomSource
//! = the vanilla LCG (rng.rs LegacyRandom). Mth.sin/cos = 65536-entry float table.

use crate::rng::LegacyRandom;
use std::sync::OnceLock;

// ---------------------------------------------------------------- Mth table

fn sin_table() -> &'static [f32; 65536] {
    static SIN: OnceLock<[f32; 65536]> = OnceLock::new();
    SIN.get_or_init(|| {
        let mut t = [0f32; 65536];
        for i in 0..65536 {
            t[i] = (i as f64 * std::f64::consts::PI * 2.0 / 65536.0).sin() as f32;
        }
        t
    })
}

#[inline]
fn mth_sin(i: f64) -> f32 {
    sin_table()[((i * 10430.378350470453) as i64 & 65535) as usize]
}

#[inline]
fn mth_cos(i: f64) -> f32 {
    sin_table()[((i * 10430.378350470453 + 16384.0) as i64 & 65535) as usize]
}

#[inline]
fn mth_floor(v: f64) -> i32 {
    v.floor() as i32
}

// ---------------------------------------------------------------- providers

/// VerticalAnchor subset used by the vanilla carvers.
#[derive(Clone, Copy, Debug)]
pub enum Anchor263 {
    Absolute(i32),
    AboveBottom(i32),
}

impl Anchor263 {
    #[inline]
    fn resolve(self, min_gen_y: i32) -> i32 {
        match self {
            Anchor263::Absolute(v) => v,
            Anchor263::AboveBottom(v) => min_gen_y + v,
        }
    }
}

/// UniformHeight.
#[derive(Clone, Copy, Debug)]
pub struct HeightProvider263 {
    pub min: Anchor263,
    pub max: Anchor263,
}

impl HeightProvider263 {
    #[inline]
    fn sample(&self, rng: &mut LegacyRandom, min_gen_y: i32) -> i32 {
        let lo = self.min.resolve(min_gen_y);
        let hi = self.max.resolve(min_gen_y);
        if lo > hi {
            lo
        } else {
            lo + rng.next_int_bound(hi - lo + 1)
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum FProv263 {
    Constant(f32),
    /// [min, max) — Mth.randomBetween.
    Uniform(f32, f32),
    /// Trapezoid (min, max, plateau).
    Trapezoid(f32, f32, f32),
}

impl FProv263 {
    #[inline]
    fn sample(&self, rng: &mut LegacyRandom) -> f32 {
        match *self {
            FProv263::Constant(v) => v,
            FProv263::Uniform(min, max) => rng.next_f32() * (max - min) + min,
            FProv263::Trapezoid(min, max, plateau) => {
                let range = max - min;
                let plateau_start = (range - plateau) / 2.0f32;
                let plateau_end = range - plateau_start;
                min + rng.next_f32() * plateau_end + rng.next_f32() * plateau_start
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum IProv263 {
    Constant(i32),
    /// UniformInt — randomBetweenInclusive.
    Uniform(i32, i32),
    /// VeryBiasedToBottomInt 26.3: min + nextInt(nextInt(nextInt(range+1)+1)+1).
    VeryBiasedToBottom(i32, i32),
}

impl IProv263 {
    #[inline]
    fn sample(&self, rng: &mut LegacyRandom) -> i32 {
        match *self {
            IProv263::Constant(v) => v,
            IProv263::Uniform(min, max) => rng.next_int_bound(max - min + 1) + min,
            IProv263::VeryBiasedToBottom(min, max) => {
                let a = rng.next_int_bound(max - min + 1);
                let b = rng.next_int_bound(a + 1);
                min + rng.next_int_bound(b + 1)
            }
        }
    }
}

// ---------------------------------------------------------------- configs

#[derive(Clone, Copy, Debug)]
pub struct CaveCarver263 {
    pub probability: f32,
    pub y: HeightProvider263,
    pub count: IProv263,
    pub thickness: FProv263,
    pub weird_thickness_bias: bool,
    pub room_vertical_radius_multiplier: FProv263,
    pub horizontal_radius_multiplier: FProv263,
    pub vertical_radius_multiplier: FProv263,
    pub start_vertical_radius_multiplier: FProv263,
    pub floor_level: FProv263,
}

#[derive(Clone, Copy, Debug)]
pub struct CanyonShape263 {
    pub distance_factor: FProv263,
    pub thickness: FProv263,
    pub width_smoothness: i32,
    pub horizontal_radius_factor: FProv263,
    pub vertical_radius_default_factor: f32,
    pub vertical_radius_center_factor: f32,
    pub y_scale: FProv263,
}

#[derive(Clone, Copy, Debug)]
pub struct CanyonCarver263 {
    pub probability: f32,
    pub y: HeightProvider263,
    pub vertical_rotation: FProv263,
    pub shape: CanyonShape263,
}

#[derive(Clone, Copy, Debug)]
pub enum Carver263 {
    Cave(CaveCarver263),
    Canyon(CanyonCarver263),
}

impl Carver263 {
    #[inline]
    fn probability(&self) -> f32 {
        match self {
            Carver263::Cave(c) => c.probability,
            Carver263::Canyon(c) => c.probability,
        }
    }
}

/// The three vanilla overworld carvers in biome-JSON order
/// ([cave, cave_extra_underground, canyon] — uniform across overworld biomes).
pub fn overworld_carvers() -> [Carver263; 3] {
    let cave = CaveCarver263 {
        probability: 0.15,
        y: HeightProvider263 {
            min: Anchor263::AboveBottom(8),
            max: Anchor263::Absolute(180),
        },
        count: IProv263::VeryBiasedToBottom(0, 14),
        thickness: FProv263::Trapezoid(0.0, 3.0, 1.0),
        weird_thickness_bias: true,
        room_vertical_radius_multiplier: FProv263::Uniform(0.1, 0.9),
        horizontal_radius_multiplier: FProv263::Uniform(0.7, 1.4),
        vertical_radius_multiplier: FProv263::Uniform(0.8, 1.3),
        start_vertical_radius_multiplier: FProv263::Constant(1.0),
        floor_level: FProv263::Uniform(-1.0, -0.4),
    };
    let cave_extra = CaveCarver263 {
        probability: 0.07,
        y: HeightProvider263 {
            min: Anchor263::AboveBottom(8),
            max: Anchor263::Absolute(47),
        },
        count: IProv263::VeryBiasedToBottom(0, 14),
        thickness: FProv263::Trapezoid(0.0, 3.0, 1.0),
        weird_thickness_bias: true,
        room_vertical_radius_multiplier: FProv263::Uniform(0.1, 0.9),
        horizontal_radius_multiplier: FProv263::Uniform(0.7, 1.4),
        vertical_radius_multiplier: FProv263::Uniform(0.8, 1.3),
        start_vertical_radius_multiplier: FProv263::Constant(1.0),
        floor_level: FProv263::Uniform(-1.0, -0.4),
    };
    let canyon = Carver263::Canyon(CanyonCarver263 {
        probability: 0.01,
        y: HeightProvider263 {
            min: Anchor263::Absolute(10),
            max: Anchor263::Absolute(67),
        },
        vertical_rotation: FProv263::Uniform(-0.125, 0.125),
        shape: CanyonShape263 {
            distance_factor: FProv263::Uniform(0.75, 1.0),
            thickness: FProv263::Trapezoid(0.0, 6.0, 2.0),
            width_smoothness: 3,
            horizontal_radius_factor: FProv263::Uniform(0.75, 1.0),
            vertical_radius_default_factor: 1.0,
            vertical_radius_center_factor: 0.0,
            y_scale: FProv263::Constant(3.0),
        },
    });
    [
        Carver263::Cave(cave),
        Carver263::Cave(cave_extra),
        canyon,
    ]
}

// ---------------------------------------------------------------- mask

/// CarvingMask 26.3 — bit per (x, y, z) of the CENTER chunk:
/// index = (y - minY) + (z + (x << 4)) * height.
pub struct CarvingMask263 {
    min_y: i32,
    height: i32,
    bits: Vec<u64>,
}

impl CarvingMask263 {
    pub fn new(min_y: i32, max_y: i32) -> Self {
        let height = max_y - min_y + 1;
        CarvingMask263 {
            min_y,
            height,
            bits: vec![0u64; ((256 * height as usize) + 63) / 64],
        }
    }

    #[inline]
    fn index(&self, x: i32, y: i32, z: i32) -> usize {
        let i = (y - self.min_y + (z + (x << 4)) * self.height) as usize;
        debug_assert!(i < self.bits.len() * 64);
        i
    }

    #[inline]
    fn set(&mut self, x: i32, y: i32, z: i32) {
        let i = self.index(x, y, z);
        self.bits[i / 64] |= 1u64 << (i % 64);
    }

    pub fn is_empty(&self) -> bool {
        self.bits.iter().all(|&b| b == 0)
    }

    /// CarvingMask.visit — ascending bit order, split into per-column
    /// (x, z, bottomY, topY) segments. `f` must return false to stop early.
    pub fn visit(&self, mut f: impl FnMut(i32, i32, i32, i32) -> bool) {
        let total = self.bits.len() * 64;
        let mut start = 0usize;
        while start < total {
            // next set bit from start
            while start < total && self.bits[start / 64] >> (start % 64) & 1 == 0 {
                start += 1;
            }
            if start >= total {
                break;
            }
            let mut end = start;
            while end + 1 < total && self.bits[(end + 1) / 64] >> ((end + 1) % 64) & 1 == 1 {
                end += 1;
            }
            // visitSegment
            let start_column = start / self.height as usize;
            let end_column = end / self.height as usize;
            for column in start_column..=end_column {
                let column_x = (column >> 4) & 15;
                let column_z = column & 15;
                let column_base = column * self.height as usize;
                let bottom_y = (start.saturating_sub(column_base)) as i32 + self.min_y;
                let top_y = (end - column_base).min(self.height as usize - 1) as i32 + self.min_y;
                if !f(column_x as i32, column_z as i32, bottom_y, top_y) {
                    return;
                }
            }
            start = end + 1;
        }
    }
}

// ---------------------------------------------------------------- carvers

/// WorldCarver.canReach.
#[inline]
fn can_reach(
    mid_x: f64,
    mid_z: f64,
    x: f64,
    z: f64,
    current_step: i32,
    total_steps: i32,
    thickness: f32,
) -> bool {
    let xd = x - mid_x;
    let zd = z - mid_z;
    let remaining = (total_steps - current_step) as f64;
    let rr = (thickness + 2.0f32 + 16.0f32) as f64;
    xd * xd + zd * zd - remaining * remaining <= rr * rr
}

/// WorldCarver.carveEllipsoid — writes mask bits for the center chunk.
fn carve_ellipsoid(
    min_x: i32,
    min_z: i32,
    mask: &mut CarvingMask263,
    x: f64,
    y: f64,
    z: f64,
    horizontal_radius: f64,
    vertical_radius: f64,
    skip: &dyn Fn(f64, f64, f64, i32) -> bool,
) {
    let center_x = min_x as f64 + 8.0;
    let center_z = min_z as f64 + 8.0;
    let max_delta = 16.0 + horizontal_radius * 2.0;
    if !((x - center_x).abs() > max_delta) && !((z - center_z).abs() > max_delta) {
        let max_x_index = mth_floor(x + horizontal_radius) - min_x;
        let max_z_index = mth_floor(z + horizontal_radius) - min_z;
        let min_x_index = i32::max(mth_floor(x - horizontal_radius) - min_x - 1, 0);
        let max_x_index = i32::min(max_x_index, 15);
        let min_y = i32::max(mth_floor(y - vertical_radius) - 1, mask.min_y);
        let max_y = i32::min(mth_floor(y + vertical_radius) + 1, mask.min_y + mask.height - 1);
        let min_z_index = i32::max(mth_floor(z - horizontal_radius) - min_z - 1, 0);
        let max_z_index = i32::min(max_z_index, 15);

        for x_index in min_x_index..=max_x_index {
            let world_x = min_x + x_index;
            let xd = (world_x as f64 + 0.5 - x) / horizontal_radius;
            for z_index in min_z_index..=max_z_index {
                let world_z = min_z + z_index;
                let zd = (world_z as f64 + 0.5 - z) / horizontal_radius;
                if !(xd * xd + zd * zd >= 1.0) {
                    for world_y in (min_y + 1..=max_y).rev() {
                        let yd = (world_y as f64 - 0.5 - y) / vertical_radius;
                        if !skip(xd, yd, zd, world_y) {
                            mask.set(x_index, world_y, z_index);
                        }
                    }
                }
            }
        }
    }
}

fn cave_should_skip(xd: f64, yd: f64, zd: f64, floor_level: f64) -> bool {
    yd <= floor_level || xd * xd + yd * yd + zd * zd >= 1.0
}

impl CaveCarver263 {
    fn carve(
        &self,
        min_x: i32,
        min_z: i32,
        mask: &mut CarvingMask263,
        rng: &mut LegacyRandom,
        min_gen_y: i32,
        source_chunk_pos: (i32, i32),
    ) {
        let max_distance = (4 * 2 - 1) * 16; // SectionPos.sectionToBlockCoord(7)
        let cave_count = self.count.sample(rng);
        for _cave in 0..cave_count {
            let x = (source_chunk_pos.0 * 16 + rng.next_int_bound(16)) as f64;
            let y = self.y.sample(rng, min_gen_y) as f64;
            let z = (source_chunk_pos.1 * 16 + rng.next_int_bound(16)) as f64;
            let horizontal_radius_multiplier = self.horizontal_radius_multiplier.sample(rng) as f64;
            let vertical_radius_multiplier = self.vertical_radius_multiplier.sample(rng) as f64;
            let start_vertical_radius_multiplier =
                self.start_vertical_radius_multiplier.sample(rng) as f64;
            let floor_level = self.floor_level.sample(rng) as f64;
            let skip = |xd: f64, yd: f64, zd: f64, _wy: i32| {
                cave_should_skip(xd, yd, zd, floor_level)
            };
            let mut tunnels = 1i32;
            if rng.next_int_bound(4) == 0 {
                let y_scale = self.room_vertical_radius_multiplier.sample(rng) as f64;
                let thickness = 1.0f32 + rng.next_f32() * 6.0f32;
                self.create_room(min_x, min_z, mask, x, y, z, thickness, y_scale, &skip);
                tunnels += rng.next_int_bound(4);
            }
            for _i in 0..tunnels {
                let horizontal_rotation = rng.next_f32() * (std::f64::consts::PI as f32 * 2.0);
                let vertical_rotation = (rng.next_f32() - 0.5f32) / 4.0f32;
                let thickness = self.get_thickness(rng);
                let distance = max_distance - rng.next_int_bound(max_distance / 4);
                self.create_tunnel(
                    min_x,
                    min_z,
                    mask,
                    rng.next_long(),
                    x,
                    y,
                    z,
                    horizontal_radius_multiplier,
                    vertical_radius_multiplier,
                    thickness,
                    horizontal_rotation,
                    vertical_rotation,
                    0,
                    distance,
                    start_vertical_radius_multiplier,
                    &skip,
                );
            }
        }
    }

    fn get_thickness(&self, rng: &mut LegacyRandom) -> f32 {
        let mut thickness = self.thickness.sample(rng);
        if self.weird_thickness_bias && rng.next_int_bound(10) == 0 {
            thickness *= rng.next_f32() * rng.next_f32() * 3.0f32 + 1.0f32;
        }
        thickness
    }

    fn create_room(
        &self,
        min_x: i32,
        min_z: i32,
        mask: &mut CarvingMask263,
        x: f64,
        y: f64,
        z: f64,
        thickness: f32,
        y_scale: f64,
        skip: &dyn Fn(f64, f64, f64, i32) -> bool,
    ) {
        let horizontal_radius =
            1.5 + (mth_sin((std::f64::consts::FRAC_PI_2 as f32) as f64) * thickness) as f64;
        let vertical_radius = horizontal_radius * y_scale;
        carve_ellipsoid(
            min_x,
            min_z,
            mask,
            x + 1.0,
            y,
            z,
            horizontal_radius,
            vertical_radius,
            skip,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn create_tunnel(
        &self,
        min_x: i32,
        min_z: i32,
        mask: &mut CarvingMask263,
        tunnel_seed: i64,
        mut x: f64,
        mut y: f64,
        mut z: f64,
        horizontal_radius_multiplier: f64,
        vertical_radius_multiplier: f64,
        thickness: f32,
        mut horizontal_rotation: f32,
        mut vertical_rotation: f32,
        step: i32,
        dist: i32,
        y_scale: f64,
        skip: &dyn Fn(f64, f64, f64, i32) -> bool,
    ) {
        let mut rng = LegacyRandom::new(tunnel_seed);
        let split_point = rng.next_int_bound(dist / 2) + dist / 4;
        let steep = rng.next_int_bound(6) == 0;
        let mut y_rota = 0.0f32;
        let mut x_rota = 0.0f32;
        let mut current_step = step;
        while current_step < dist {
            let horizontal_radius = 1.5
                + (mth_sin(((std::f64::consts::PI as f32) * current_step as f32 / dist as f32) as f64)
                    * thickness) as f64;
            let vertical_radius = horizontal_radius * y_scale;
            let cos_x = mth_cos(vertical_rotation as f64);
            x += (mth_cos(horizontal_rotation as f64) * cos_x) as f64;
            y += mth_sin(vertical_rotation as f64) as f64;
            z += (mth_sin(horizontal_rotation as f64) * cos_x) as f64;
            vertical_rotation *= if steep { 0.92 } else { 0.7 };
            vertical_rotation += x_rota * 0.1;
            horizontal_rotation += y_rota * 0.1;
            x_rota *= 0.9;
            y_rota *= 0.75;
            x_rota += (rng.next_f32() - rng.next_f32()) * rng.next_f32() * 2.0;
            y_rota += (rng.next_f32() - rng.next_f32()) * rng.next_f32() * 4.0;
            if current_step == split_point && thickness > 1.0 {
                self.create_tunnel(
                    min_x,
                    min_z,
                    mask,
                    rng.next_long(),
                    x,
                    y,
                    z,
                    horizontal_radius_multiplier,
                    vertical_radius_multiplier,
                    rng.next_f32() * 0.5 + 0.5,
                    horizontal_rotation - (std::f64::consts::PI as f32 / 2.0),
                    vertical_rotation / 3.0,
                    current_step,
                    dist,
                    1.0,
                    skip,
                );
                self.create_tunnel(
                    min_x,
                    min_z,
                    mask,
                    rng.next_long(),
                    x,
                    y,
                    z,
                    horizontal_radius_multiplier,
                    vertical_radius_multiplier,
                    rng.next_f32() * 0.5 + 0.5,
                    horizontal_rotation + (std::f64::consts::PI as f32 / 2.0),
                    vertical_rotation / 3.0,
                    current_step,
                    dist,
                    1.0,
                    skip,
                );
                return;
            }
            if rng.next_int_bound(4) != 0 {
                if !can_reach(
                    min_x as f64 + 8.0,
                    min_z as f64 + 8.0,
                    x,
                    z,
                    current_step,
                    dist,
                    thickness,
                ) {
                    return;
                }
                carve_ellipsoid(
                    min_x,
                    min_z,
                    mask,
                    x,
                    y,
                    z,
                    horizontal_radius * horizontal_radius_multiplier,
                    vertical_radius * vertical_radius_multiplier,
                    skip,
                );
            }
            current_step += 1;
        }
    }
}

impl CanyonCarver263 {
    fn carve(
        &self,
        min_x: i32,
        min_z: i32,
        mask: &mut CarvingMask263,
        rng: &mut LegacyRandom,
        min_gen_y: i32,
        gen_depth: i32,
        source_chunk_pos: (i32, i32),
    ) {
        let max_distance = (4 * 2 - 1) * 16;
        let x = (source_chunk_pos.0 * 16 + rng.next_int_bound(16)) as f64;
        let y = self.y.sample(rng, min_gen_y);
        let z = (source_chunk_pos.1 * 16 + rng.next_int_bound(16)) as f64;
        let mut horizontal_rotation = rng.next_f32() * (std::f64::consts::PI as f32 * 2.0);
        let mut vertical_rotation = self.vertical_rotation.sample(rng);
        let y_scale = self.shape.y_scale.sample(rng) as f64;
        let thickness = self.shape.thickness.sample(rng);
        let distance = (max_distance as f32 * self.shape.distance_factor.sample(rng)) as i32;
        self.do_carve(
            min_x,
            min_z,
            mask,
            rng.next_long(),
            x,
            y as f64,
            z,
            thickness,
            horizontal_rotation,
            vertical_rotation,
            0,
            distance,
            y_scale,
            min_gen_y,
            gen_depth,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn do_carve(
        &self,
        min_x: i32,
        min_z: i32,
        mask: &mut CarvingMask263,
        tunnel_seed: i64,
        mut x: f64,
        mut y: f64,
        mut z: f64,
        thickness: f32,
        mut horizontal_rotation: f32,
        mut vertical_rotation: f32,
        step: i32,
        distance: i32,
        y_scale: f64,
        min_gen_y: i32,
        gen_depth: i32,
    ) {
        let mut rng = LegacyRandom::new(tunnel_seed);
        let width_factor_per_height = self.init_width_factors(&mut rng, gen_depth);
        let mut y_rota = 0.0f32;
        let mut x_rota = 0.0f32;
        let mut current_step = step;
        while current_step < distance {
            let mut horizontal_radius = 1.5
                + (mth_sin((current_step as f32 * (std::f64::consts::PI as f32) / distance as f32)
                    as f64)
                    * thickness) as f64;
            let mut vertical_radius = horizontal_radius * y_scale;
            horizontal_radius *= self.shape.horizontal_radius_factor.sample(&mut rng) as f64;
            vertical_radius = self.update_vertical_radius(&mut rng, vertical_radius, distance as f32, current_step as f32);
            let xc = mth_cos(vertical_rotation as f64);
            let xs = mth_sin(vertical_rotation as f64);
            x += (mth_cos(horizontal_rotation as f64) * xc) as f64;
            y += xs as f64;
            z += (mth_sin(horizontal_rotation as f64) * xc) as f64;
            vertical_rotation *= 0.7;
            vertical_rotation += x_rota * 0.05;
            horizontal_rotation += y_rota * 0.05;
            x_rota *= 0.8;
            y_rota *= 0.5;
            x_rota += (rng.next_f32() - rng.next_f32()) * rng.next_f32() * 2.0;
            y_rota += (rng.next_f32() - rng.next_f32()) * rng.next_f32() * 4.0;
            if rng.next_int_bound(4) != 0 {
                if !can_reach(
                    min_x as f64 + 8.0,
                    min_z as f64 + 8.0,
                    x,
                    z,
                    current_step,
                    distance,
                    thickness,
                ) {
                    return;
                }
                let wf = &width_factor_per_height;
                let skip = |xd: f64, _yd: f64, zd: f64, wy: i32| {
                    let y_index = wy - min_gen_y;
                    if y_index < 1 || y_index as usize >= wf.len() {
                        // vanilla indexes [yIndex - 1]; y == minGenY would be
                        // out of bounds — the mask range excludes it in practice.
                        return false;
                    }
                    (xd * xd + zd * zd) * wf[(y_index - 1) as usize] as f64 + _yd * _yd / 6.0 >= 1.0
                };
                carve_ellipsoid(
                    min_x,
                    min_z,
                    mask,
                    x,
                    y,
                    z,
                    horizontal_radius,
                    vertical_radius,
                    &skip,
                );
            }
            current_step += 1;
        }
    }

    fn init_width_factors(&self, rng: &mut LegacyRandom, gen_depth: i32) -> Vec<f32> {
        let mut arr = vec![0f32; gen_depth as usize];
        let mut width_factor = 1.0f32;
        for y_index in 0..gen_depth as usize {
            if y_index == 0 || rng.next_int_bound(self.shape.width_smoothness) == 0 {
                width_factor = 1.0 + rng.next_f32() * rng.next_f32();
            }
            arr[y_index] = width_factor * width_factor;
        }
        arr
    }

    fn update_vertical_radius(
        &self,
        rng: &mut LegacyRandom,
        vertical_radius: f64,
        distance: f32,
        current_step: f32,
    ) -> f64 {
        let vertical_multiplier = 1.0 - (0.5 - current_step / distance).abs() * 2.0;
        let factor = self.shape.vertical_radius_default_factor
            + self.shape.vertical_radius_center_factor * vertical_multiplier;
        // Mth.randomBetween(random, 0.75F, 1.0F)
        (factor as f64) * vertical_radius * (rng.next_f32() * (1.0f32 - 0.75f32) + 0.75f32) as f64
    }
}

// ---------------------------------------------------------------- driver

/// NoiseBasedChunkGenerator.generateCarvers 26.3 — the mask pass over the
/// 17x17 source chunks. All overworld biomes share the same carver list
/// ([cave, cave_extra_underground, canyon] — biome-JSON order) so no biome
/// data is needed. Mask range: [minGenY + 1, minGenY + genDepth - 1 - 7]
/// (protectedBlocksOnTop = 7, not upgrading).
pub fn generate_carver_mask(seed: i64, cx: i32, cz: i32, min_gen_y: i32, gen_depth: i32) -> CarvingMask263 {
    let carvers = overworld_carvers();
    let mut mask = CarvingMask263::new(min_gen_y + 1, min_gen_y + gen_depth - 1 - 7);
    let mut rng = LegacyRandom::new(0);
    for dx in -8..=8 {
        for dz in -8..=8 {
            let source_x = cx + dx;
            let source_z = cz + dz;
            for (index, carver) in carvers.iter().enumerate() {
                // WorldgenRandom.setLargeFeatureSeed(seed + index, sx, sz) — 26.3:
                // setSeed(s); xScale=nextLong(); zScale=nextLong();
                // setSeed(sx*xScale ^ sz*zScale ^ s)
                let s = seed + index as i64;
                rng.set_seed(s);
                let x_scale = rng.next_long();
                let z_scale = rng.next_long();
                rng.set_seed(
                    (source_x as i64).wrapping_mul(x_scale)
                        ^ (source_z as i64).wrapping_mul(z_scale)
                        ^ s,
                );
                if rng.next_f32() <= carver.probability() {
                    match carver {
                        Carver263::Cave(c) => {
                            c.carve(cx * 16, cz * 16, &mut mask, &mut rng, min_gen_y, (source_x, source_z))
                        }
                        Carver263::Canyon(c) => {
                            c.carve(cx * 16, cz * 16, &mut mask, &mut rng, min_gen_y, gen_depth, (source_x, source_z))
                        }
                    }
                }
            }
        }
    }
    mask
}
