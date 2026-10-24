//! Vanilla 26.3 `Aquifer.NoiseBasedAquifer` port — frozen from
//! reference/dec263/net/minecraft/world/level/levelgen/Aquifer.java (559 lines).
//!
//! 26.3 deltas vs 1.21.8 (cont-48 follow-up):
//!   - config moved from the noise router into noise_settings "aquifers"
//!     section {barrier, fluid_level_floodedness, fluid_level_spread, lava,
//!     exclusion, surface_level} — exclusion + surface_level are NEW;
//!   - surfaceLevel cache quantized at quart positions (QuartPos.fromBlock →
//!     toBlock), pre-filled by a strided (4,1,4) VOLUME sample of the
//!     surface_level DF (maxSurfaceLevel) — shares the SAME SamplerCtx263 as
//!     the fill so cached buffers converge exactly like the Java
//!     cachingSamplers;
//!   - skipSamplingAboveY precompute from the max adjusted surface level;
//!   - positionalRandomFactory = RandomState.getOrCreateRandomFactory
//!     ("minecraft:aquifer") = root.fromHashOf(name).forkPositional().
//!
//! DF samples are f32 (cast to f64 where vanilla casts); all pressure /
//! floodedness math is f64 exactly like the Java.

use crate::df263::{DfSam263, DensityVolume, SamplerCtx263, SamRef};
use crate::rng::XoroshiroPositionalRandomFactory;
use std::collections::HashMap;

pub const CELL_AIR: u8 = 0;
pub const CELL_WATER: u8 = 1;
pub const CELL_LAVA: u8 = 2;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FluidTy {
    Air,
    Water,
    Lava,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FluidStatus {
    pub fluid_level: i32,
    pub ty: FluidTy,
}

impl FluidStatus {
    /// FluidStatus.at(blockY) — below the level it is the fluid, above it air.
    #[inline]
    pub fn at(&self, block_y: i32) -> FluidTy {
        if block_y < self.fluid_level {
            self.ty
        } else {
            FluidTy::Air
        }
    }
}

const X_RANGE: i32 = 10;
const Y_RANGE: i32 = 9;
const Z_RANGE: i32 = 10;

/// similarity(Mth.square(10), Mth.square(12)) = 1.0 - 44.0/25.0 (const-folded
/// exactly like the Java static initializer's IEEE ops).
const FLOWING_UPDATE_SIMULARITY: f64 = 1.0 - 44.0f64 / 25.0f64;

/// similarity(d1, d2) = 1.0 - (d2 - d1) / 25.0
#[inline]
fn similarity_raw(_d1: f64, _d2: f64) -> f64 {
    1.0 - (_d2 - _d1) / 25.0
}

#[inline]
fn similarity(d1: i32, d2: i32) -> f64 {
    1.0 - (d2 - d1) as f64 / 25.0
}

/// The 26.3 surface sampling offsets (SURFACE_SAMPLING_OFFSETS_IN_CHUNKS).
const SURFACE_OFFSETS: [(i32, i32); 13] = [
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

/// The aquifer config DF handles (from the noise_settings "aquifers" section).
pub struct AquiferConfig263 {
    pub barrier: SamRef,
    pub floodedness: SamRef,
    pub spread: SamRef,
    pub lava: SamRef,
    pub exclusion: SamRef,
    pub surface_level: SamRef,
}

pub struct Aquifer263 {
    cfg: AquiferConfig263,
    factory: XoroshiroPositionalRandomFactory,
    aquifer_cache: Vec<Option<FluidStatus>>,
    location_cache: Vec<i64>,
    min_grid_x: i32,
    min_grid_y: i32,
    min_grid_z: i32,
    grid_size_x: i32,
    grid_size_y: i32,
    grid_size_z: i32,
    sea_level: i32,
    pub skip_sampling_above_y: i32,
    surface_level_cache: HashMap<i64, i32>,
    should_schedule: bool,
}

#[inline]
fn grid_x(block_coord: i32) -> i32 {
    block_coord >> 4
}
#[inline]
fn from_grid_x(grid: i32, offset: i32) -> i32 {
    (grid << 4) + offset
}
#[inline]
fn grid_y(block_coord: i32) -> i32 {
    block_coord.div_euclid(12)
}
#[inline]
fn from_grid_y(grid: i32, offset: i32) -> i32 {
    grid * 12 + offset
}
#[inline]
fn grid_z(block_coord: i32) -> i32 {
    block_coord >> 4
}
#[inline]
fn from_grid_z(grid: i32, offset: i32) -> i32 {
    (grid << 4) + offset
}

/// BlockPos.asLong + getX/getY/getZ unpack (26.3 layout: x<<38 | z<<12 | y).
#[inline]
pub fn pos_as_long(x: i32, y: i32, z: i32) -> i64 {
    ((x as i64 & 0x3FFFFFF) << 38) | ((z as i64 & 0x3FFFFFF) << 12) | (y as i64 & 0xFFF)
}
#[inline]
fn pos_x(l: i64) -> i32 {
    (l >> 38) as i32
}
#[inline]
fn pos_y(l: i64) -> i32 {
    ((l << 52) >> 52) as i32
}
#[inline]
fn pos_z(l: i64) -> i32 {
    ((l << 26) >> 38) as i32
}

/// ChunkPos.pack(x, z) — key for the surface level cache.
#[inline]
fn chunk_pack(x: i32, z: i32) -> i64 {
    (x as i64 & 0xFFFFFFFF) | ((z as i64 & 0xFFFFFFFF) << 32)
}

impl Aquifer263 {
    /// NoiseBasedAquifer constructor (needs a mutable ctx for the
    /// maxSurfaceLevel strided volume sample).
    pub fn new(
        ctx: &mut SamplerCtx263,
        cfg: AquiferConfig263,
        factory: XoroshiroPositionalRandomFactory,
        volume: &DensityVolume,
        sea_level: i32,
    ) -> Self {
        let min_grid_x = grid_x(volume.min_x + -5);
        let max_grid_x = grid_x(volume.max_block_x() + -5) + 1;
        let grid_size_x = max_grid_x - min_grid_x + 1;
        let min_grid_y = grid_y(volume.min_y + 1) + -1;
        let max_grid_y = grid_y(volume.max_block_y() + 1) + 1;
        let grid_size_y = max_grid_y - min_grid_y + 1;
        let min_grid_z = grid_z(volume.min_z + -5);
        let max_grid_z = grid_z(volume.max_block_z() + -5) + 1;
        let grid_size_z = max_grid_z - min_grid_z + 1;
        let total = (grid_size_x * grid_size_y * grid_size_z) as usize;
        let mut aq = Aquifer263 {
            cfg,
            factory,
            aquifer_cache: vec![None; total],
            location_cache: vec![i64::MAX; total],
            min_grid_x,
            min_grid_y,
            min_grid_z,
            grid_size_x,
            grid_size_y,
            grid_size_z,
            sea_level,
            skip_sampling_above_y: i32::MAX,
            surface_level_cache: HashMap::new(),
            should_schedule: false,
        };
        let max_surface = aq.max_surface_level(
            ctx,
            from_grid_x(min_grid_x, 0),
            from_grid_z(min_grid_z, 0),
            from_grid_x(max_grid_x, 9),
            from_grid_z(max_grid_z, 9),
        );
        let max_adjusted = aq.adjust_surface_level(max_surface);
        let skip_grid_y = grid_y(max_adjusted + 12) - -1;
        aq.skip_sampling_above_y = from_grid_y(skip_grid_y, 11) - 1;
        aq
    }

    /// maxSurfaceLevel — strided (4,1,4) volume sample of the surface_level
    /// DF over the quart grid; fills the surface level cache and returns the
    /// max floor'ed level.
    fn max_surface_level(&mut self, ctx: &mut SamplerCtx263, min_x: i32, min_z: i32, max_x: i32, max_z: i32) -> i32 {
        let min_quart_x = min_x.div_euclid(4);
        let max_quart_x = max_x.div_euclid(4);
        let min_quart_z = min_z.div_euclid(4);
        let max_quart_z = max_z.div_euclid(4);
        let volume = DensityVolume {
            size_x: max_quart_x - min_quart_x + 1,
            size_y: 1,
            size_z: max_quart_z - min_quart_z + 1,
            min_x: min_quart_x * 4,
            min_y: 0,
            min_z: min_quart_z * 4,
            step_x: 4,
            step_y: 1,
            step_z: 4,
        };
        let mut buf = ctx.scratch.acquire(volume.size());
        (self.cfg.surface_level.clone()).sample_volume(ctx, &mut buf, &volume);
        let mut max_y = i32::MIN;
        for z in 0..volume.size_z {
            for x in 0..volume.size_x {
                let level = mth_floor(buf[volume.index_unchecked(x, 0, z)]);
                self.surface_level_cache
                    .insert(chunk_pack(volume.block_x(x), volume.block_z(z)), level);
                if level > max_y {
                    max_y = level;
                }
            }
        }
        ctx.scratch.release(buf);
        max_y
    }

    #[inline]
    fn adjust_surface_level(&self, level: i32) -> i32 {
        level + 8
    }

    /// surfaceLevel(blockX, blockZ) — quart-quantized, cached, floor'd.
    fn surface_level(&mut self, ctx: &mut SamplerCtx263, block_x: i32, block_z: i32) -> i32 {
        let qx = block_x.div_euclid(4) * 4;
        let qz = block_z.div_euclid(4) * 4;
        let key = chunk_pack(qx, qz);
        if let Some(&v) = self.surface_level_cache.get(&key) {
            return v;
        }
        let v = mth_floor(
            self.cfg
                .surface_level
                .sample_value(ctx, qx, 0, qz),
        );
        self.surface_level_cache.insert(key, v);
        v
    }

    fn get_index(&self, gx: i32, gy: i32, gz: i32) -> usize {
        (((gy - self.min_grid_y) * self.grid_size_z + (gz - self.min_grid_z)) * self.grid_size_x
            + (gx - self.min_grid_x)) as usize
    }

    /// Global fluid picker (createFluidPicker, DEBUG_DISABLE_FLUID_GENERATION off):
    /// y < min(-54, seaLevel) ? LAVA(-54) : sea(seaLevel, WATER).
    #[inline]
    fn global_fluid(&self, _x: i32, y: i32, _z: i32) -> FluidStatus {
        if y < i32::min(-54, self.sea_level) {
            FluidStatus {
                fluid_level: -54,
                ty: FluidTy::Lava,
            }
        } else {
            FluidStatus {
                fluid_level: self.sea_level,
                ty: FluidTy::Water,
            }
        }
    }

    /// computeSubstance — returns Ok(None) = vanilla default block (stone),
    /// Ok(Some(ty)) = the fluid/air cell to write.
    pub fn compute_substance(
        &mut self,
        ctx: &mut SamplerCtx263,
        block_x: i32,
        block_y: i32,
        block_z: i32,
        density: f64,
    ) -> Option<FluidTy> {
        if density > 0.0 {
            self.should_schedule = false;
            return None;
        }
        let global_fluid = self.global_fluid(block_x, block_y, block_z);
        if block_y > self.skip_sampling_above_y {
            self.should_schedule = false;
            return Some(global_fluid.at(block_y));
        }
        if global_fluid.at(block_y) == FluidTy::Lava {
            self.should_schedule = false;
            return Some(FluidTy::Lava);
        }
        let x_anchor = grid_x(block_x + -5);
        let y_anchor = grid_y(block_y + 1);
        let z_anchor = grid_z(block_z + -5);
        let mut distance_sqr1 = i32::MAX;
        let mut distance_sqr2 = i32::MAX;
        let mut distance_sqr3 = i32::MAX;
        let mut distance_sqr4 = i32::MAX;
        let mut closest_index1 = 0usize;
        let mut closest_index2 = 0usize;
        let mut closest_index3 = 0usize;
        let mut closest_index4 = 0usize;

        for x1 in 0..=1 {
            for y1 in -1..=1 {
                for z1 in 0..=1 {
                    let spaced_x = x_anchor + x1;
                    let spaced_y = y_anchor + y1;
                    let spaced_z = z_anchor + z1;
                    let index = self.get_index(spaced_x, spaced_y, spaced_z);
                    let location = if self.location_cache[index] != i64::MAX {
                        self.location_cache[index]
                    } else {
                        let mut random = self.factory.at(spaced_x, spaced_y, spaced_z);
                        let location = pos_as_long(
                            from_grid_x(spaced_x, random.next_int_bound(X_RANGE)),
                            from_grid_y(spaced_y, random.next_int_bound(Y_RANGE)),
                            from_grid_z(spaced_z, random.next_int_bound(Z_RANGE)),
                        );
                        self.location_cache[index] = location;
                        location
                    };
                    let dx = pos_x(location) - block_x;
                    let dy = pos_y(location) - block_y;
                    let dz = pos_z(location) - block_z;
                    let new_distance = dx * dx + dy * dy + dz * dz;
                    if distance_sqr1 >= new_distance {
                        closest_index4 = closest_index3;
                        closest_index3 = closest_index2;
                        closest_index2 = closest_index1;
                        closest_index1 = index;
                        distance_sqr4 = distance_sqr3;
                        distance_sqr3 = distance_sqr2;
                        distance_sqr2 = distance_sqr1;
                        distance_sqr1 = new_distance;
                    } else if distance_sqr2 >= new_distance {
                        closest_index4 = closest_index3;
                        closest_index3 = closest_index2;
                        closest_index2 = index;
                        distance_sqr4 = distance_sqr3;
                        distance_sqr3 = distance_sqr2;
                        distance_sqr2 = new_distance;
                    } else if distance_sqr3 >= new_distance {
                        closest_index4 = closest_index3;
                        closest_index3 = index;
                        distance_sqr4 = distance_sqr3;
                        distance_sqr3 = new_distance;
                    } else if distance_sqr4 >= new_distance {
                        closest_index4 = index;
                        distance_sqr4 = new_distance;
                    }
                }
            }
        }

        let closest_status1 = self.get_aquifer_status(ctx, closest_index1);
        let similarity12 = similarity(distance_sqr1, distance_sqr2);
        let fluid_ty = closest_status1.at(block_y);
        if similarity12 <= 0.0 {
            if similarity12 >= FLOWING_UPDATE_SIMULARITY {
                let closest_status2 = self.get_aquifer_status(ctx, closest_index2);
                self.should_schedule = closest_status1 != closest_status2;
            } else {
                self.should_schedule = false;
            }
            return Some(fluid_ty);
        }
        if fluid_ty == FluidTy::Water
            && self.global_fluid(block_x, block_y - 1, block_z).at(block_y - 1) == FluidTy::Lava
        {
            self.should_schedule = true;
            return Some(fluid_ty);
        }
        // barrier path
        let mut barrier_memo = f64::NAN;
        let closest_status2 = self.get_aquifer_status(ctx, closest_index2);
        let barrier12 = similarity12
            * self.calculate_pressure(
                ctx,
                block_x,
                block_y,
                block_z,
                &mut barrier_memo,
                closest_status1,
                closest_status2,
            );
        if density + barrier12 > 0.0 {
            self.should_schedule = false;
            return None;
        }
        let closest_status3 = self.get_aquifer_status(ctx, closest_index3);
        let similarity13 = similarity(distance_sqr1, distance_sqr3);
        if similarity13 > 0.0 {
            let barrier13 = similarity12
                * similarity13
                * self.calculate_pressure(
                    ctx,
                    block_x,
                    block_y,
                    block_z,
                    &mut barrier_memo,
                    closest_status1,
                    closest_status3,
                );
            if density + barrier13 > 0.0 {
                self.should_schedule = false;
                return None;
            }
        }
        let similarity23 = similarity(distance_sqr2, distance_sqr3);
        if similarity23 > 0.0 {
            let barrier23 = similarity12
                * similarity23
                * self.calculate_pressure(
                    ctx,
                    block_x,
                    block_y,
                    block_z,
                    &mut barrier_memo,
                    closest_status2,
                    closest_status3,
                );
            if density + barrier23 > 0.0 {
                self.should_schedule = false;
                return None;
            }
        }
        let may_flow12 = closest_status1 != closest_status2;
        let may_flow23 = similarity23 >= FLOWING_UPDATE_SIMULARITY && closest_status2 != closest_status3;
        let may_flow13 = similarity13 >= FLOWING_UPDATE_SIMULARITY && closest_status1 != closest_status3;
        if !may_flow12 && !may_flow23 && !may_flow13 {
            self.should_schedule = similarity13 >= FLOWING_UPDATE_SIMULARITY
                && similarity(distance_sqr1, distance_sqr4) >= FLOWING_UPDATE_SIMULARITY
                && closest_status1 != self.get_aquifer_status(ctx, closest_index4);
        } else {
            self.should_schedule = true;
        }
        Some(fluid_ty)
    }

    pub fn should_schedule_fluid_update(&self) -> bool {
        self.should_schedule
    }

    fn get_aquifer_status(&mut self, ctx: &mut SamplerCtx263, index: usize) -> FluidStatus {
        if let Some(s) = self.aquifer_cache[index] {
            return s;
        }
        let location = self.location_cache[index];
        let status = self.compute_fluid(ctx, pos_x(location), pos_y(location), pos_z(location));
        self.aquifer_cache[index] = Some(status);
        status
    }

    /// computeFluid — the 13-offset surface scan + floodedness math.
    fn compute_fluid(&mut self, ctx: &mut SamplerCtx263, x: i32, y: i32, z: i32) -> FluidStatus {
        let global_fluid = self.global_fluid(x, y, z);
        let mut lowest_preliminary_surface = i32::MAX;
        let top_of_aquifer_cell = y + 12;
        let bottom_of_aquifer_cell = y - 12;
        let mut surface_at_center_under_global = false;

        for (ox, oz) in SURFACE_OFFSETS {
            let sample_x = x + ox * 16;
            let sample_z = z + oz * 16;
            let surface_level = self.surface_level(ctx, sample_x, sample_z);
            let adjusted = self.adjust_surface_level(surface_level);
            let start = ox == 0 && oz == 0;
            if start && bottom_of_aquifer_cell > adjusted {
                return global_fluid;
            }
            let pokes_above = top_of_aquifer_cell > adjusted;
            if pokes_above || start {
                let global_at_surface = self.global_fluid(sample_x, adjusted, sample_z);
                if global_at_surface.at(adjusted) != FluidTy::Air {
                    if start {
                        surface_at_center_under_global = true;
                    }
                    if pokes_above {
                        return global_at_surface;
                    }
                }
            }
            if surface_level < lowest_preliminary_surface {
                lowest_preliminary_surface = surface_level;
            }
        }

        let fluid_surface_level = self.compute_surface_level(
            ctx,
            x,
            y,
            z,
            global_fluid,
            lowest_preliminary_surface,
            surface_at_center_under_global,
        );
        let fluid_ty = self.compute_fluid_type(ctx, x, y, z, global_fluid, fluid_surface_level);
        FluidStatus {
            fluid_level: fluid_surface_level,
            ty: fluid_ty,
        }
    }

    fn calculate_pressure(
        &mut self,
        ctx: &mut SamplerCtx263,
        block_x: i32,
        block_y: i32,
        block_z: i32,
        barrier_noise_value: &mut f64,
        status1: FluidStatus,
        status2: FluidStatus,
    ) -> f64 {
        let ty1 = status1.at(block_y);
        let ty2 = status2.at(block_y);
        if !((ty1 == FluidTy::Lava && ty2 == FluidTy::Water)
            || (ty1 == FluidTy::Water && ty2 == FluidTy::Lava))
        {
            let fluid_y_diff = status1.fluid_level.abs_diff(status2.fluid_level);
            if fluid_y_diff == 0 {
                return 0.0;
            }
            let average_fluid_y = 0.5 * (status1.fluid_level + status2.fluid_level) as f64;
            let how_far_above = block_y as f64 + 0.5 - average_fluid_y;
            let base_value = fluid_y_diff as f64 / 2.0;
            let distance_from_barrier_edge = base_value - how_far_above.abs();
            let gradient;
            if how_far_above > 0.0 {
                let center_point = 0.0 + distance_from_barrier_edge;
                gradient = if center_point > 0.0 {
                    center_point / 1.5
                } else {
                    center_point / 2.5
                };
            } else {
                let center_point = 3.0 + distance_from_barrier_edge;
                gradient = if center_point > 0.0 {
                    center_point / 3.0
                } else {
                    center_point / 10.0
                };
            }
            let noise_value: f64 = if !(gradient < -2.0) && !(gradient > 2.0) {
                if barrier_noise_value.is_nan() {
                    let barrier_noise = self.cfg.barrier.sample_value(ctx, block_x, block_y, block_z) as f64;
                    *barrier_noise_value = barrier_noise;
                    barrier_noise
                } else {
                    *barrier_noise_value
                }
            } else {
                0.0
            };
            2.0 * (noise_value + gradient)
        } else {
            2.0
        }
    }

    fn compute_surface_level(
        &mut self,
        ctx: &mut SamplerCtx263,
        x: i32,
        y: i32,
        z: i32,
        global_fluid: FluidStatus,
        lowest_surface_level: i32,
        surface_at_center_under_global: bool,
    ) -> i32 {
        let partially_floodedness;
        let fully_floodedness;
        if self.cfg.exclusion.sample_value(ctx, x, y, z) as f64 > 0.0 {
            partially_floodedness = -1.0;
            fully_floodedness = -1.0;
        } else {
            let distance_below_surface = self.adjust_surface_level(lowest_surface_level) - y;
            let floodedness_factor: f64 = if surface_at_center_under_global {
                mth_clamped_map(distance_below_surface as f64, 0.0, 64.0, 1.0, 0.0)
            } else {
                0.0
            };
            let floodedness_noise_value =
                mth_clamp(self.cfg.floodedness.sample_value(ctx, x, y, z) as f64, -1.0, 1.0);
            let fully_flooded_threshold = mth_map(floodedness_factor, 1.0, 0.0, -0.3, 0.8);
            let partially_flooded_threshold = mth_map(floodedness_factor, 1.0, 0.0, -0.8, 0.4);
            partially_floodedness = floodedness_noise_value - partially_flooded_threshold;
            fully_floodedness = floodedness_noise_value - fully_flooded_threshold;
        }
        if fully_floodedness > 0.0 {
            global_fluid.fluid_level
        } else if partially_floodedness > 0.0 {
            self.compute_randomized_fluid_surface_level(ctx, x, y, z, lowest_surface_level)
        } else {
            -32512
        }
    }

    fn compute_randomized_fluid_surface_level(
        &mut self,
        ctx: &mut SamplerCtx263,
        x: i32,
        y: i32,
        z: i32,
        lowest_surface_level: i32,
    ) -> i32 {
        let cell_x = x.div_euclid(16);
        let cell_y = y.div_euclid(40);
        let cell_z = z.div_euclid(16);
        let cell_middle_y = cell_y * 40 + 20;
        let spread = (self.cfg.spread.sample_value(ctx, cell_x, cell_y, cell_z) * 10.0f32) as f64;
        let quantized = mth_quantize(spread, 3);
        let target = cell_middle_y + quantized;
        i32::min(lowest_surface_level, target)
    }

    fn compute_fluid_type(
        &mut self,
        ctx: &mut SamplerCtx263,
        x: i32,
        y: i32,
        z: i32,
        global_fluid: FluidStatus,
        fluid_surface_level: i32,
    ) -> FluidTy {
        let mut ty = global_fluid.ty;
        if fluid_surface_level <= -10
            && fluid_surface_level != -32512
            && global_fluid.ty != FluidTy::Lava
        {
            let cell_x = x.div_euclid(64);
            let cell_y = y.div_euclid(40);
            let cell_z = z.div_euclid(64);
            let lava_noise = self.cfg.lava.sample_value(ctx, cell_x, cell_y, cell_z) as f64;
            if lava_noise.abs() > 0.3 {
                ty = FluidTy::Lava;
            }
        }
        ty
    }
}

// ---------------------------------------------------------------- Mth (f64)

#[inline]
fn mth_floor(v: f32) -> i32 {
    (v as f64).floor() as i32
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

/// Mth.map(double): lerp(inverseLerp(value, fromMin, fromMax), toMin, toMax).
#[inline]
fn mth_map(value: f64, from_min: f64, from_max: f64, to_min: f64, to_max: f64) -> f64 {
    to_min + inverse_lerp(value, from_min, from_max) * (to_max - to_min)
}

/// Mth.clampedMap(double).
#[inline]
fn mth_clamped_map(value: f64, from_min: f64, from_max: f64, to_min: f64, to_max: f64) -> f64 {
    if value <= from_min {
        to_min
    } else if value >= from_max {
        to_max
    } else {
        mth_map(value, from_min, from_max, to_min, to_max)
    }
}

#[inline]
fn inverse_lerp(value: f64, from_min: f64, from_max: f64) -> f64 {
    (value - from_min) / (from_max - from_min)
}

/// Mth.quantize(double, int) = floor(value / resolution) * resolution.
#[inline]
fn mth_quantize(value: f64, resolution: i32) -> i32 {
    (value / resolution as f64).floor() as i32 * resolution
}

// Keep the SamRef import used even if cfg fields change.
#[allow(dead_code)]
fn _assert_sam(_: &SamRef) {}

#[allow(dead_code)]
fn _assert_dfsam(_: &DfSam263) {}
