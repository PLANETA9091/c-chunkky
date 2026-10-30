//! df263_router.rs — vanilla 26.3 overworld router build (NoiseRouterData +
//! TerrainProvider mirrored in f32) and oracle goldens (cont-48).

use std::sync::Arc;

use crate::df263::{
    Axis, BinOp, DfSam263, N263, NoiseFactory263, SamRef, SplineRef, Tiling, UnOp,
};
use crate::n263::{Normalization, NormalNoiseParameters};

fn params(base_amplitude: f64, base_octave: i32, octave_count: i32, mods: &[f64]) -> NormalNoiseParameters {
    NormalNoiseParameters {
        base_amplitude,
        base_octave,
        octave_count,
        normalize: Normalization::Enabled,
        amplitude_modifiers: mods.to_vec(),
    }
}

type NP = (f64, i32, i32, &'static [f64]);

/// Vanilla 26.3 worldgen/noise parameters (transcribed from the jar datapack).
pub struct NoiseParams263;

impl NoiseParams263 {
    pub const OFFSET: NP = (0.9381732587751005, -3, 4, &[1.0, 1.0, 1.0, 0.0]);
    pub const TEMPERATURE: NP = (1.2453007926713473, -10, 6, &[1.5, 0.0, 1.0, 0.0, 0.0, 0.0]);
    pub const VEGETATION: NP = (0.9494731054427978, -8, 6, &[1.0, 1.0, 0.0, 0.0, 0.0, 0.0]);
    pub const CONTINENTALNESS: NP = (0.8880832896205223, -9, 9, &[1.0, 1.0, 2.0, 2.0, 2.0, 1.0, 1.0, 1.0, 1.0]);
    pub const EROSION: NP = (1.063180125160734, -9, 5, &[1.0, 1.0, 0.0, 1.0, 1.0]);
    pub const RIDGE: NP = (0.9147152149950137, -7, 6, &[1.0, 2.0, 1.0, 0.0, 0.0, 0.0]);
    pub const JAGGED: NP = (1.0383104856073737, -16, 16, &[]);
    pub const CAVE_ENTRANCE: NP = (0.8500634887071167, -7, 3, &[0.4, 0.5, 1.0]);
    pub const CAVE_LAYER: NP = (0.955388882960065, -8, 1, &[]);
    pub const CAVE_CHEESE: NP = (0.8361300524356068, -8, 9, &[0.5, 1.0, 2.0, 1.0, 2.0, 1.0, 0.0, 2.0, 0.0]);
    pub const SPAGHETTI: NP = (0.955388882960065, -7, 1, &[]);
    pub const SPAGHETTI_M11: NP = (0.955388882960065, -11, 1, &[]);
    pub const SPAGHETTI_M8: NP = (0.955388882960065, -8, 1, &[]);
    pub const SPAGHETTI_M5: NP = (0.955388882960065, -5, 1, &[]);
    pub const NOODLE: NP = (0.955388882960065, -8, 1, &[]);
    pub const NOODLE_RIDGE: NP = (0.955388882960065, -7, 1, &[]);
    pub const PILLAR: NP = (0.9494731054427981, -7, 2, &[]);
}

// ---------------------------------------------------------------- build helpers

fn sam(v: DfSam263) -> SamRef {
    Arc::new(v)
}

fn constant(v: f32) -> SamRef {
    sam(DfSam263::Constant(v))
}

fn is_const(s: &SamRef) -> Option<f32> {
    match &**s {
        DfSam263::Constant(v) => Some(*v),
        _ => None,
    }
}

/// BinaryFunction.compileSampler folding (MIN/MAX range collapse intentionally
/// omitted — verified non-firing for the overworld tree; point shortcuts use
/// inf bounds = always evaluate right, value-identical).
fn binary(op: BinOp, l: SamRef, r: SamRef) -> SamRef {
    match op {
        BinOp::Add => {
            if let Some(c) = is_const(&l) {
                return if let Some(d) = is_const(&r) {
                    constant(c + d)
                } else {
                    sam(DfSam263::ConstAdd { a: r, c })
                };
            }
            if let Some(c) = is_const(&r) {
                return sam(DfSam263::ConstAdd { a: l, c });
            }
        }
        BinOp::Sub => {
            if let Some(c) = is_const(&l) {
                return if let Some(d) = is_const(&r) {
                    constant(c - d)
                } else {
                    sam(DfSam263::ConstSub { c, a: r })
                };
            }
            if let Some(c) = is_const(&r) {
                return sam(DfSam263::ConstAdd { a: l, c: -c });
            }
        }
        BinOp::Mul => {
            if let Some(c) = is_const(&l) {
                return if let Some(d) = is_const(&r) {
                    constant(c * d)
                } else {
                    sam(DfSam263::ConstMul { a: r, c })
                };
            }
            if let Some(c) = is_const(&r) {
                return sam(DfSam263::ConstMul { a: l, c });
            }
        }
        BinOp::Div => {
            if let Some(c) = is_const(&l) {
                return if let Some(d) = is_const(&r) {
                    constant(c / d)
                } else {
                    sam(DfSam263::ConstDiv { c, a: r })
                };
            }
            if let Some(c) = is_const(&r) {
                return sam(DfSam263::ConstMul { a: l, c: 1.0 / c });
            }
        }
        BinOp::Min | BinOp::Max => {
            if let Some(c) = is_const(&l) {
                if let Some(d) = is_const(&r) {
                    return constant(if op == BinOp::Min { c.min(d) } else { c.max(d) });
                }
                return if op == BinOp::Min {
                    sam(DfSam263::ConstMin { a: r, c })
                } else {
                    sam(DfSam263::ConstMax { a: r, c })
                };
            }
            if let Some(c) = is_const(&r) {
                return if op == BinOp::Min {
                    sam(DfSam263::ConstMin { a: l, c })
                } else {
                    sam(DfSam263::ConstMax { a: l, c })
                };
            }
            let bound = if op == BinOp::Min {
                f32::NEG_INFINITY
            } else {
                f32::INFINITY
            };
            return sam(DfSam263::Binary { op, l, r, r_bound: bound });
        }
    }
    sam(DfSam263::Binary {
        op,
        l,
        r,
        r_bound: 0.0,
    })
}

fn unary(op: UnOp, a: SamRef) -> SamRef {
    sam(DfSam263::Unary { op, a })
}

fn clamp_fn(a: SamRef, min: f32, max: f32) -> SamRef {
    sam(DfSam263::Clamp { a, min, max })
}

fn lerp(alpha: SamRef, first: SamRef, second: SamRef) -> SamRef {
    if let Some(f) = is_const(&first) {
        return sam(DfSam263::LerpConstFirst {
            alpha,
            first: f,
            second,
        });
    }
    if let Some(s) = is_const(&second) {
        return sam(DfSam263::LerpConstSecond {
            alpha,
            first,
            second: s,
        });
    }
    sam(DfSam263::Lerp { alpha, first, second })
}

fn y_clamped_gradient(from_y: i32, to_y: i32, from_value: f32, to_value: f32) -> SamRef {
    gradient(
        Axis::Y,
        Tiling::ClampToEdge,
        from_y,
        to_y,
        from_value,
        to_value,
    )
}

fn gradient(
    axis: Axis,
    tiling: Tiling,
    from_coord: i32,
    to_coord: i32,
    from_value: f32,
    to_value: f32,
) -> SamRef {
    let range = to_coord - from_coord;
    let factor = (to_value - from_value) / range as f32;
    let aux = match tiling {
        Tiling::ClampToEdge => to_coord,
        Tiling::Repeat | Tiling::MirroredRepeat => range,
    };
    sam(DfSam263::Gradient {
        axis,
        tiling,
        from_coord,
        aux,
        from_value,
        factor,
    })
}

fn noise_fn(noise: Arc<N263>, xz_scale: f64, y_scale: f64) -> SamRef {
    sam(DfSam263::Noise {
        noise,
        xz_scale,
        y_scale,
    })
}

fn interpolated(a: SamRef, cell_xz: i32, cell_y: i32) -> SamRef {
    sam(DfSam263::Interpolated {
        a,
        cell_xz,
        cell_y,
        inv_xz: 1.0 / cell_xz as f32,
        inv_y: 1.0 / cell_y as f32,
    })
}

/// DensityFunctions.remap (build-time f32).
fn remap(input: SamRef, from_min: f32, from_max: f32, to_min: f32, to_max: f32) -> SamRef {
    let factor = (to_max - to_min) / (from_max - from_min);
    let offset = to_min - from_min * factor;
    if offset == 0.0 {
        binary(BinOp::Mul, input, constant(factor))
    } else {
        binary(
            BinOp::Add,
            binary(BinOp::Mul, input, constant(factor)),
            constant(offset),
        )
    }
}

/// mappedNoise 3-arg: (noise, minTarget, maxTarget) = xz 1.0, y 1.0.
fn mapped_noise(noise: Arc<N263>, min_target: f32, max_target: f32) -> SamRef {
    remap(noise_fn(noise, 1.0, 1.0), -1.0, 1.0, min_target, max_target)
}

/// mappedNoise 5-arg: (noise, xzScale, yScale, minTarget, maxTarget).
fn mapped_noise_scaled(noise: Arc<N263>, xz_scale: f64, y_scale: f64, min_target: f32, max_target: f32) -> SamRef {
    remap(noise_fn(noise, xz_scale, y_scale), -1.0, 1.0, min_target, max_target)
}

// ---------------------------------------------------------------- spline builder

struct SplineBuilder {
    coord: SamRef,
    points: Vec<(f32, SplineRef, f32)>,
}

impl SplineBuilder {
    fn new(coord: SamRef) -> Self {
        SplineBuilder {
            coord,
            points: Vec::new(),
        }
    }

    fn add_const(&mut self, location: f32, value: f32) -> &mut Self {
        self.points.push((location, SplineRef::Constant(value), 0.0));
        self
    }

    fn add_const_d(&mut self, location: f32, value: f32, derivative: f32) -> &mut Self {
        self.points
            .push((location, SplineRef::Constant(value), derivative));
        self
    }

    fn add_spline(&mut self, location: f32, value: SplineRef) -> &mut Self {
        self.points.push((location, value, 0.0));
        self
    }

    fn build(self) -> SplineRef {
        SplineRef::Multi {
            coord_slot: 0,
            coord: self.coord,
            locations: self.points.iter().map(|p| p.0).collect(),
            derivatives: self.points.iter().map(|p| p.2).collect(),
            values: self.points.into_iter().map(|p| p.1).collect(),
        }
    }
}

fn spline_df(spline: SplineRef) -> SamRef {
    let mut s = spline;
    s.assign_slots(0); // root Multi slot 0 aligns with coord_bufs[0] = root coord
    let (coord, locations, derivatives, values) = match &s {
        SplineRef::Multi {
            coord,
            locations,
            derivatives,
            values,
            ..
        } => (coord.clone(), locations.clone(), derivatives.clone(), values.clone()),
        SplineRef::Constant(_) => unreachable!("root spline must be Multi"),
    };
    sam(DfSam263::Spline {
        coord,
        locations,
        derivatives,
        values,
    })
}

// ---------------------------------------------------------------- TerrainProvider 26.3

fn peaks_and_valleys(weirdness: f32) -> f32 {
    -((weirdness.abs() - 0.6666667f32).abs() - 0.33333334f32) * 3.0
}

fn mountain_continentalness(ridge: f32, modulation: f32, allow_rivers_below: f32) -> f32 {
    let ridge_slope = 1.0 - (1.0 - modulation) * 0.5;
    let ridge_intersect = 0.5 * (1.0 - modulation);
    let adjusted_ridge_height = (ridge + 1.17) * 0.46082947;
    let continentalness = adjusted_ridge_height * ridge_slope - ridge_intersect;
    if ridge < allow_rivers_below {
        continentalness.max(-0.2222)
    } else {
        continentalness.max(0.0)
    }
}

fn calculate_mountain_ridge_zero_continentalness_point(modulation: f32) -> f32 {
    let ridge_slope = 1.0 - (1.0 - modulation) * 0.5;
    let ridge_intersect = 0.5 * (1.0 - modulation);
    ridge_intersect / (0.46082947 * ridge_slope) - 1.17
}

fn calculate_slope(y1: f32, y2: f32, x1: f32, x2: f32) -> f32 {
    (y2 - y1) / (x2 - x1)
}

fn build_mountain_ridge_spline_with_points(ridges: SamRef, modulation: f32, saddle: bool) -> SplineRef {
    let mut build = SplineBuilder::new(ridges);
    let min_point_continentalness = mountain_continentalness(-1.0, modulation, -0.7);
    let max_point_continentalness = mountain_continentalness(1.0, modulation, -0.7);
    let ridge_zero_point = calculate_mountain_ridge_zero_continentalness_point(modulation);
    if -0.65 < ridge_zero_point && ridge_zero_point < 1.0 {
        let after_river_threshold_continentalness = mountain_continentalness(-0.65, modulation, -0.7);
        let before_river_threshold_continentalness = mountain_continentalness(-0.75, modulation, -0.7);
        let min_point_derivative =
            calculate_slope(min_point_continentalness, before_river_threshold_continentalness, -1.0, -0.75);
        build.add_const_d(-1.0, min_point_continentalness, min_point_derivative);
        build.add_const(-0.75, before_river_threshold_continentalness);
        build.add_const(-0.65, after_river_threshold_continentalness);
        let ridge_zero_point_continentalness = mountain_continentalness(ridge_zero_point, modulation, -0.7);
        let max_point_derivative =
            calculate_slope(ridge_zero_point_continentalness, max_point_continentalness, ridge_zero_point, 1.0);
        build.add_const(ridge_zero_point - 0.01, ridge_zero_point_continentalness);
        build.add_const_d(ridge_zero_point, ridge_zero_point_continentalness, max_point_derivative);
        build.add_const_d(1.0, max_point_continentalness, max_point_derivative);
    } else {
        let simple_derivative = calculate_slope(min_point_continentalness, max_point_continentalness, -1.0, 1.0);
        if saddle {
            build.add_const(-1.0, 0.2f32.max(min_point_continentalness));
            let lerp_mid = min_point_continentalness + 0.5f32 * (max_point_continentalness - min_point_continentalness);
            build.add_const_d(0.0, lerp_mid, simple_derivative);
        } else {
            build.add_const_d(-1.0, min_point_continentalness, simple_derivative);
        }
        build.add_const_d(1.0, max_point_continentalness, simple_derivative);
    }
    build.build()
}

fn ridge_spline(
    ridges: SamRef,
    valley: f32,
    low: f32,
    mid: f32,
    high: f32,
    peaks: f32,
    min_valley_steepness: f32,
) -> SplineRef {
    let d1 = (0.5 * (low - valley)).max(min_valley_steepness);
    let d2 = 5.0 * (mid - low);
    let mut b = SplineBuilder::new(ridges);
    b.add_const_d(-1.0, valley, d1);
    b.add_const_d(-0.4, low, d1.min(d2));
    b.add_const_d(0.0, mid, d2);
    b.add_const_d(0.4, high, 2.0 * (high - mid));
    b.add_const_d(1.0, peaks, 0.7 * (peaks - high));
    b.build()
}

#[allow(clippy::too_many_arguments)]
fn build_erosion_offset_spline(
    erosion: SamRef,
    ridges: SamRef,
    low_valley: f32,
    hill: f32,
    tall_hill: f32,
    mountain_factor: f32,
    plain: f32,
    swamp: f32,
    include_extreme_hills: bool,
    saddle: bool,
) -> SplineRef {
    // Mth.lerp(mountainFactor, 0.6, 1.5) = 0.6 + mf*(1.5-0.6)
    let very_low = build_mountain_ridge_spline_with_points(
        ridges.clone(),
        0.6 + mountain_factor * (1.5 - 0.6),
        saddle,
    );
    let low_m = build_mountain_ridge_spline_with_points(ridges.clone(), 0.6 + mountain_factor * (1.0 - 0.6), saddle);
    let mountains = build_mountain_ridge_spline_with_points(ridges.clone(), mountain_factor, saddle);
    let wide_plateau = ridge_spline(
        ridges.clone(),
        low_valley - 0.15,
        0.5 * mountain_factor,
        0.5 * mountain_factor,
        0.5 * mountain_factor,
        0.6 * mountain_factor,
        0.5,
    );
    let narrow_plateau = ridge_spline(
        ridges.clone(),
        low_valley,
        plain * mountain_factor,
        hill * mountain_factor,
        0.5 * mountain_factor,
        0.6 * mountain_factor,
        0.5,
    );
    let plains = ridge_spline(ridges.clone(), low_valley, plain, plain, hill, tall_hill, 0.5);
    let plains_far_inland = ridge_spline(ridges.clone(), low_valley, plain, plain, hill, tall_hill, 0.5);
    let extreme_hills = {
        let mut b = SplineBuilder::new(ridges.clone());
        b.add_const(-1.0, low_valley);
        b.add_spline(-0.4, plains.clone());
        b.add_const(0.0, tall_hill + 0.07);
        b.build()
    };
    let swamps = ridge_spline(ridges.clone(), -0.02, swamp, swamp, hill, tall_hill, 0.0);
    let mut builder = SplineBuilder::new(erosion);
    builder.add_spline(-0.85, very_low);
    builder.add_spline(-0.7, low_m);
    builder.add_spline(-0.4, mountains);
    builder.add_spline(-0.35, wide_plateau);
    builder.add_spline(-0.1, narrow_plateau);
    builder.add_spline(0.2, plains);
    if include_extreme_hills {
        builder.add_spline(0.4, plains_far_inland.clone());
        builder.add_spline(0.45, extreme_hills.clone());
        builder.add_spline(0.55, extreme_hills);
        builder.add_spline(0.58, plains_far_inland);
    }
    builder.add_spline(0.7, swamps);
    builder.build()
}

fn get_erosion_factor(
    erosion: SamRef,
    weirdness: SamRef,
    ridges: SamRef,
    base_value: f32,
    shattered_terrain: bool,
) -> SplineRef {
    let base_spline = {
        let mut b = SplineBuilder::new(weirdness.clone());
        b.add_const(-0.2, 6.3);
        b.add_const(0.2, base_value);
        b.build()
    };
    let mut erosion_points = SplineBuilder::new(erosion);
    erosion_points.add_spline(-0.6, base_spline.clone());
    erosion_points.add_spline(
        -0.5,
        {
            let mut b = SplineBuilder::new(weirdness.clone());
            b.add_const(-0.05, 6.3);
            b.add_const(0.05, 2.67);
            b.build()
        },
    );
    erosion_points.add_spline(-0.35, base_spline.clone());
    erosion_points.add_spline(-0.25, base_spline.clone());
    erosion_points.add_spline(
        -0.1,
        {
            let mut b = SplineBuilder::new(weirdness.clone());
            b.add_const(-0.05, 2.67);
            b.add_const(0.05, 6.3);
            b.build()
        },
    );
    erosion_points.add_spline(0.03, base_spline.clone());
    if shattered_terrain {
        let weirdness_shattered = {
            let mut b = SplineBuilder::new(weirdness);
            b.add_const(0.0, base_value);
            b.add_const(0.1, 0.625);
            b.build()
        };
        let ridges_shattered = {
            let mut b = SplineBuilder::new(ridges);
            b.add_const(-0.9, base_value);
            b.add_spline(-0.69, weirdness_shattered);
            b.build()
        };
        erosion_points.add_const(0.35, base_value);
        erosion_points.add_spline(0.45, ridges_shattered.clone());
        erosion_points.add_spline(0.55, ridges_shattered);
        erosion_points.add_const(0.62, base_value);
    } else {
        let extreme_hills = {
            let mut b = SplineBuilder::new(ridges.clone());
            b.add_spline(-0.7, base_spline.clone());
            b.add_const(-0.15, 1.37);
            b.build()
        };
        let extra_3d = {
            let mut b = SplineBuilder::new(ridges);
            b.add_spline(0.45, base_spline.clone());
            b.add_const(0.7, 1.56);
            b.build()
        };
        erosion_points.add_spline(0.05, extra_3d.clone());
        erosion_points.add_spline(0.4, extra_3d);
        erosion_points.add_spline(0.45, extreme_hills.clone());
        erosion_points.add_spline(0.55, extreme_hills);
        erosion_points.add_const(0.58, base_value);
    }
    erosion_points.build()
}

fn build_weirdness_jaggedness_spline(weirdness: SamRef, jaggedness_factor: f32) -> SplineRef {
    let max_neg = 0.63 * jaggedness_factor;
    let max_pos = 0.3 * jaggedness_factor;
    let mut b = SplineBuilder::new(weirdness);
    b.add_const(-0.01, max_neg);
    b.add_const(0.01, max_pos);
    b.build()
}

fn build_ridge_jaggedness_spline(
    weirdness: SamRef,
    ridges: SamRef,
    jf_at_peak: f32,
    jf_at_high: f32,
) -> SplineRef {
    let high_slice_start = peaks_and_valleys(0.4);
    let high_slice_end = peaks_and_valleys(0.56666666);
    let high_slice_middle = (high_slice_start + high_slice_end) / 2.0;
    let mut b = SplineBuilder::new(ridges);
    b.add_const(high_slice_start, 0.0);
    if jf_at_high > 0.0 {
        let w = build_weirdness_jaggedness_spline(weirdness.clone(), jf_at_high);
        b.add_spline(high_slice_middle, w);
    } else {
        b.add_const(high_slice_middle, 0.0);
    }
    if jf_at_peak > 0.0 {
        let w = build_weirdness_jaggedness_spline(weirdness, jf_at_peak);
        b.add_spline(1.0, w);
    } else {
        b.add_const(1.0, 0.0);
    }
    b.build()
}

fn build_erosion_jaggedness_spline(
    erosion: SamRef,
    weirdness: SamRef,
    ridges: SamRef,
    jf_peak_0: f32,
    jf_peak_1: f32,
    jf_high_0: f32,
    jf_high_1: f32,
) -> SplineRef {
    let at_erosion0 = build_ridge_jaggedness_spline(weirdness.clone(), ridges.clone(), jf_peak_0, jf_high_0);
    let at_erosion1 = build_ridge_jaggedness_spline(weirdness, ridges, jf_peak_1, jf_high_1);
    let mut b = SplineBuilder::new(erosion);
    b.add_spline(-1.0, at_erosion0);
    b.add_spline(-0.78, at_erosion1.clone());
    b.add_spline(-0.5775, at_erosion1);
    b.add_const(-0.375, 0.0);
    b.build()
}

// ---------------------------------------------------------------- overworld router

pub struct OverworldRouter263 {
    pub temperature: SamRef,
    pub jaggedness: SamRef,
    pub base_3d: SamRef,
    pub vegetation: SamRef,
    pub continents: SamRef,
    pub erosion: SamRef,
    pub depth: SamRef,
    pub ridges: SamRef,
    pub chunk_surface_level: SamRef,
    pub final_density: SamRef,
    /// "minecraft:overworld/preliminary_surface_level" — the aquifer surface
    /// DF reference binds to this exact compiled node.
    pub prelim: SamRef,
    pub offset: SamRef,
    pub factor: SamRef,
    pub sloped_cheese: SamRef,
    pub noodle: SamRef,
    pub entrances: SamRef,
    pub spaghetti_2d: SamRef,
    pub pillars: SamRef,
    pub spaghetti_roughness: SamRef,
}

pub struct RouterBuilder263 {
    pub factory: NoiseFactory263,
    next_cache: u32,
    /// Compiled router nodes reused by the aquifer config (same node identity
    /// as the DF-registry references).
    erosion_node: Option<SamRef>,
    depth_node: Option<SamRef>,
    prelim_node: Option<SamRef>,
}

fn noise_gradient_density(factor: &SamRef, depth_jagged: SamRef) -> SamRef {
    binary(
        BinOp::Mul,
        unary(
            UnOp::QuarterNegative,
            binary(BinOp::Mul, depth_jagged, factor.clone()),
        ),
        constant(4.0),
    )
}

fn offset_to_depth(offset: &SamRef) -> SamRef {
    binary(BinOp::Add, y_clamped_gradient(-64, 320, 1.5, -1.5), offset.clone())
}

/// slide(caves, -64, 384, 80, 64, -0.078125, 0, 24, 0.1171875)
fn slide_overworld(caves: SamRef) -> SamRef {
    let top_factor = y_clamped_gradient(240, 256, 1.0, 0.0);
    let noise_value = lerp(top_factor, constant(-0.078125), caves);
    let bottom_factor = y_clamped_gradient(-64, -40, 0.0, 1.0);
    lerp(bottom_factor, constant(0.1171875), noise_value)
}

impl RouterBuilder263 {
    pub fn new(seed: i64) -> Self {
        RouterBuilder263 {
            factory: NoiseFactory263::new(seed),
            next_cache: 0,
            erosion_node: None,
            depth_node: None,
            prelim_node: None,
        }
    }

    fn cache(&mut self, a: SamRef) -> SamRef {
        let id = self.next_cache;
        self.next_cache += 1;
        sam(DfSam263::Cache { id, a })
    }

    fn normal_noise(&mut self, name: &str, p: NP) -> Arc<N263> {
        self.factory.normal(name, params(p.0, p.1, p.2, p.3))
    }

    #[allow(clippy::many_single_char_names)]
    pub fn build_overworld(&mut self) -> OverworldRouter263 {
        let y = y_clamped_gradient(-4064, 4062, -4064.0, 4062.0);

        let shift_noise = self.normal_noise("offset", NoiseParams263::OFFSET);
        let shift_x = self.cache(sam(DfSam263::ConstMul {
            a: noise_fn(shift_noise.clone(), 0.25, 0.0),
            c: 4.0,
        }));
        let shift_z = self.cache(sam(DfSam263::ShiftB {
            noise: shift_noise.clone(),
        }));

        let temperature_noise = self.normal_noise("temperature", NoiseParams263::TEMPERATURE);
        let vegetation_noise = self.normal_noise("vegetation", NoiseParams263::VEGETATION);
        let continentalness_noise = self.normal_noise("continentalness", NoiseParams263::CONTINENTALNESS);
        let erosion_noise = self.normal_noise("erosion", NoiseParams263::EROSION);
        let ridge_noise = self.normal_noise("ridge", NoiseParams263::RIDGE);
        let jagged_noise = self.normal_noise("jagged", NoiseParams263::JAGGED);

        let shifted_2d = |noise: Arc<N263>, shift_x: &SamRef, shift_z: &SamRef| {
            sam(DfSam263::ShiftedXz {
                shift_x: shift_x.clone(),
                shift_z: shift_z.clone(),
                noise,
                xz_scale: 0.25,
                y_scale: 0.0,
            })
        };
        let temperature = shifted_2d(temperature_noise, &shift_x, &shift_z);
        let vegetation = shifted_2d(vegetation_noise, &shift_x, &shift_z);
        let continents = self.cache(shifted_2d(continentalness_noise, &shift_x, &shift_z));
        let erosion = self.cache(shifted_2d(erosion_noise, &shift_x, &shift_z));
        self.erosion_node = Some(erosion.clone());
        let ridges = self.cache(shifted_2d(ridge_noise, &shift_x, &shift_z));

        // ridges_folded = peaksAndValleys(ridges)
        let ridges_folded = binary(
            BinOp::Mul,
            binary(
                BinOp::Add,
                unary(
                    UnOp::Abs,
                    binary(
                        BinOp::Add,
                        unary(UnOp::Abs, ridges.clone()),
                        constant(-0.6666667),
                    ),
                ),
                constant(-0.33333334),
            ),
            constant(-3.0),
        );

        // ---- cave registry functions (bootstrap order) ----
        let spa_rough = self.normal_noise("spaghetti_roughness", NoiseParams263::SPAGHETTI_M5);
        let spa_rough_mod = self.normal_noise("spaghetti_roughness_modulator", NoiseParams263::SPAGHETTI_M8);
        let spaghetti_roughness = self.cache(binary(
            BinOp::Mul,
            mapped_noise(spa_rough_mod, 0.0, -0.1),
            binary(
                BinOp::Add,
                unary(UnOp::Abs, noise_fn(spa_rough, 1.0, 1.0)),
                constant(-0.4),
            ),
        ));

        let spa_3d_rarity = self.normal_noise("spaghetti_3d_rarity", NoiseParams263::SPAGHETTI_M11);
        let spa_3d_thickness_noise = self.normal_noise("spaghetti_3d_thickness", NoiseParams263::SPAGHETTI_M8);
        let spa_3d_1 = self.normal_noise("spaghetti_3d_1", NoiseParams263::SPAGHETTI);
        let spa_3d_2 = self.normal_noise("spaghetti_3d_2", NoiseParams263::SPAGHETTI);
        let cave_entrance_noise = self.normal_noise("cave_entrance", NoiseParams263::CAVE_ENTRANCE);
        let rarity_modulator = self.cache(noise_fn(spa_3d_rarity, 2.0, 1.0));
        let wrap_3d = |noise: Arc<N263>, rarity_modulator: &SamRef| -> SamRef {
            let branch = |noise: &Arc<N263>, rarity: f32| -> SamRef {
                binary(
                    BinOp::Mul,
                    noise_fn(noise.clone(), 1.0 / (rarity as f64), 1.0 / (rarity as f64)),
                    constant(rarity),
                )
            };
            unary(
                UnOp::Abs,
                sam(DfSam263::IntervalSelect {
                    a: rarity_modulator.clone(),
                    thresholds: vec![-0.5, 0.0, 0.5],
                    branches: vec![branch(&noise, 0.75), branch(&noise, 1.0), branch(&noise, 1.5), branch(&noise, 2.0)],
                }),
            )
        };
        let spaghetti_3d = clamp_fn(
            binary(
                BinOp::Add,
                binary(
                    BinOp::Max,
                    wrap_3d(spa_3d_1, &rarity_modulator),
                    wrap_3d(spa_3d_2, &rarity_modulator),
                ),
                mapped_noise(spa_3d_thickness_noise, -0.065, -0.088),
            ),
            -1.0,
            1.0,
        );
        let big_entrances = binary(
            BinOp::Add,
            sam(DfSam263::ConstAdd {
                a: noise_fn(cave_entrance_noise, 0.75, 0.5),
                c: 0.37,
            }),
            y_clamped_gradient(-10, 30, 0.3, 0.0),
        );
        let entrances = self.cache(binary(
            BinOp::Min,
            big_entrances,
            binary(BinOp::Add, spaghetti_roughness.clone(), spaghetti_3d),
        ));

        let spa_2d = self.normal_noise("spaghetti_2d", NoiseParams263::SPAGHETTI);
        let spa_2d_modulator = self.normal_noise("spaghetti_2d_modulator", NoiseParams263::SPAGHETTI_M11);
        let spa_2d_thickness_noise = self.normal_noise("spaghetti_2d_thickness", NoiseParams263::SPAGHETTI_M11);
        let spa_2d_elevation = self.normal_noise("spaghetti_2d_elevation", NoiseParams263::SPAGHETTI_M8);
        let thickness_modulator = self.cache(mapped_noise_scaled(spa_2d_thickness_noise, 2.0, 1.0, -0.6, -1.3));
        let wrap_2d = {
            let branch = |rarity: f32| -> SamRef {
                binary(
                    BinOp::Mul,
                    noise_fn(spa_2d.clone(), 1.0 / (rarity as f64), 1.0 / (rarity as f64)),
                    constant(rarity),
                )
            };
            unary(
                UnOp::Abs,
                sam(DfSam263::IntervalSelect {
                    a: noise_fn(spa_2d_modulator, 2.0, 1.0),
                    thresholds: vec![-0.75, -0.5, 0.5, 0.75],
                    branches: vec![branch(0.5), branch(0.75), branch(1.0), branch(2.0), branch(3.0)],
                }),
            )
        };
        // mappedNoise(SPA_2D_ELEVATION, yScale=floorDiv(-64,8)=-8.0, -8.0, 8.0):
        // noise(noise, xz=1.0, y=-8.0) remapped -1..1 -> -8..8.
        // mappedNoise(SPA_2D_ELEVATION, 0.0, -8.0F, 8.0F) = 4-arg overload:
        // xz=1.0, yScale=0.0, remap -1..1 -> -8..8 (factor 8, offset 0).
        let elevation = self.cache(remap(
            noise_fn(spa_2d_elevation, 1.0, 0.0),
            -1.0,
            1.0,
            -8.0,
            8.0,
        ));
        let sloped_spaghetti = unary(
            UnOp::Abs,
            binary(
                BinOp::Add,
                elevation,
                y_clamped_gradient(-64, 320, 8.0, -40.0),
            ),
        );
        let layer_ridged = unary(
            UnOp::Cube,
            binary(
                BinOp::Add,
                sloped_spaghetti,
                thickness_modulator.clone(),
            ),
        );
        let cave_noise = binary(
            BinOp::Add,
            wrap_2d,
            binary(BinOp::Mul, thickness_modulator, constant(0.083)),
        );
        let spaghetti_2d = clamp_fn(binary(BinOp::Max, cave_noise, layer_ridged), -1.0, 1.0);

        let pillar_noise = self.normal_noise("pillar", NoiseParams263::PILLAR);
        let pillar_rareness = self.normal_noise("pillar_rareness", NoiseParams263::SPAGHETTI_M8);
        let pillar_thickness = self.normal_noise("pillar_thickness", NoiseParams263::SPAGHETTI_M8);
        let pillars = self.cache(binary(
            BinOp::Mul,
            binary(
                BinOp::Add,
                binary(BinOp::Mul, noise_fn(pillar_noise, 25.0, 0.3), constant(2.0)),
                mapped_noise(pillar_rareness, 0.0, -2.0),
            ),
            unary(UnOp::Cube, mapped_noise(pillar_thickness, 0.0, 1.1)),
        ));

        let noodle_noise = self.normal_noise("noodle", NoiseParams263::NOODLE);
        let noodle_thickness_noise = self.normal_noise("noodle_thickness", NoiseParams263::NOODLE);
        let noodle_ridge_a = self.normal_noise("noodle_ridge_a", NoiseParams263::NOODLE_RIDGE);
        let noodle_ridge_b = self.normal_noise("noodle_ridge_b", NoiseParams263::NOODLE_RIDGE);
        let noodle = {
            let y_lim = |inner: SamRef, out: f32| -> SamRef {
                interpolated(
                    sam(DfSam263::RangeChoice {
                        a: y.clone(),
                        min: -60.0,
                        max: 321.0,
                        in_range: inner,
                        out_of_range: constant(out),
                    }),
                    4,
                    8,
                )
            };
            let noodle_toggle = y_lim(noise_fn(noodle_noise, 1.0, 1.0), -1.0);
            let noodle_thickness = y_lim(
                mapped_noise(noodle_thickness_noise, -0.05, -0.1),
                0.0,
            );
            let ridged = binary(
                BinOp::Mul,
                binary(
                    BinOp::Max,
                    unary(
                        UnOp::Abs,
                        y_lim(
                            noise_fn(noodle_ridge_a, 2.6666666666666665, 2.6666666666666665),
                            0.0,
                        ),
                    ),
                    unary(
                        UnOp::Abs,
                        y_lim(
                            noise_fn(noodle_ridge_b, 2.6666666666666665, 2.6666666666666665),
                            0.0,
                        ),
                    ),
                ),
                constant(1.5),
            );
            sam(DfSam263::RangeChoice {
                a: noodle_toggle,
                min: -1000000.0,
                max: 0.0,
                in_range: constant(64.0),
                out_of_range: binary(BinOp::Add, noodle_thickness, ridged),
            })
        };

        // ---- terrain noises (registerTerrainNoises, non-amplified) ----
        let offset = self.cache(lerp(
            constant(1.0), // blend_alpha (Blender empty -> 1)
            constant(0.0), // blend_offset
            binary(
                BinOp::Add,
                constant(-0.50375),
                spline_df({
                    let beach = build_erosion_offset_spline(erosion.clone(), ridges_folded.clone(), -0.15, 0.0, 0.0, 0.1, 0.0, -0.03, false, false);
                    let low = build_erosion_offset_spline(erosion.clone(), ridges_folded.clone(), -0.1, 0.03, 0.1, 0.1, 0.01, -0.03, false, false);
                    let mid = build_erosion_offset_spline(erosion.clone(), ridges_folded.clone(), -0.1, 0.03, 0.1, 0.7, 0.01, -0.03, true, true);
                    let high = build_erosion_offset_spline(erosion.clone(), ridges_folded.clone(), -0.05, 0.03, 0.1, 1.0, 0.01, 0.01, true, true);
                    let mut b = SplineBuilder::new(continents.clone());
                    b.add_const(-1.1, 0.044);
                    b.add_const(-1.02, -0.2222);
                    b.add_const(-0.51, -0.2222);
                    b.add_const(-0.44, -0.12);
                    b.add_const(-0.18, -0.12);
                    b.add_spline(-0.16, beach.clone());
                    b.add_spline(-0.15, beach);
                    b.add_spline(-0.1, low);
                    b.add_spline(0.25, mid);
                    b.add_spline(1.0, high);
                    b.build()
                }),
            ),
        ));

        let factor = self.cache(lerp(
            constant(1.0),
            constant(10.0),
            spline_df({
                let mut b = SplineBuilder::new(continents.clone());
                b.add_const(-0.19, 3.95);
                b.add_spline(-0.15, get_erosion_factor(erosion.clone(), ridges.clone(), ridges_folded.clone(), 6.25, true));
                b.add_spline(-0.1, get_erosion_factor(erosion.clone(), ridges.clone(), ridges_folded.clone(), 5.47, true));
                b.add_spline(0.03, get_erosion_factor(erosion.clone(), ridges.clone(), ridges_folded.clone(), 5.08, true));
                b.add_spline(0.06, get_erosion_factor(erosion.clone(), ridges.clone(), ridges_folded.clone(), 4.69, false));
                b.build()
            }),
        ));

        let depth = binary(BinOp::Add, y_clamped_gradient(-64, 320, 1.5, -1.5), offset.clone());
        self.depth_node = Some(depth.clone());

        let jaggedness = self.cache(binary(
            BinOp::Mul,
            spline_df({
                let mut b = SplineBuilder::new(continents.clone());
                b.add_const(-0.11, 0.0);
                b.add_spline(
                    0.03,
                    build_erosion_jaggedness_spline(erosion.clone(), ridges.clone(), ridges_folded.clone(), 1.0, 0.5, 0.0, 0.0),
                );
                b.add_spline(
                    0.65,
                    build_erosion_jaggedness_spline(erosion.clone(), ridges.clone(), ridges_folded.clone(), 1.0, 1.0, 1.0, 0.0),
                );
                b.build()
            }),
            unary(
                UnOp::HalfNegative,
                noise_fn(jagged_noise, 1500.0, 0.0),
            ),
        ));

        let initial_density = noise_gradient_density(
            &factor,
            binary(BinOp::Add, depth.clone(), jaggedness.clone()),
        );

        // base_3d_noise overworld = BlendedNoise(0.25, 0.125, 80, 160, 8)
        let (min_stack, max_stack, main_stack, xzm, ym, mxd, myd) = self.factory.blended(0.25, 0.125, 80.0, 160.0, 8.0);
        let base_3d = sam(DfSam263::Blended {
            min_stack,
            max_stack,
            main_stack,
            xz_multiplier: xzm,
            y_multiplier: ym,
            main_xz_div: mxd,
            main_y_div: myd,
        });

        let sloped_cheese = self.cache(binary(BinOp::Add, initial_density, base_3d.clone()));

        // underground(...)
        let cave_layer_noise = self.normal_noise("cave_layer", NoiseParams263::CAVE_LAYER);
        let cave_cheese_noise = self.normal_noise("cave_cheese", NoiseParams263::CAVE_CHEESE);
        let layerized = binary(
            BinOp::Mul,
            // JSON: xz_scale=1.0, y_scale=8.0 (2-arg noise(Holder, yScale)).
                unary(UnOp::Square, noise_fn(cave_layer_noise, 1.0, 8.0)),
            constant(4.0),
        );
        let solid_cheese = binary(
            BinOp::Add,
            clamp_fn(
                binary(
                    BinOp::Add,
                    // JSON: xz_scale=1.0, y_scale=0.6666666666666666.
                    noise_fn(cave_cheese_noise, 1.0, 0.6666666666666666),
                    constant(0.27),
                ),
                -1.0,
                1.0,
            ),
            clamp_fn(
                binary(
                    BinOp::Add,
                    binary(BinOp::Mul, sloped_cheese.clone(), constant(-0.64)),
                    constant(1.5),
                ),
                0.0,
                0.5,
            ),
        );
        let base_cave_density = binary(BinOp::Add, layerized, solid_cheese);
        let subtractions = binary(
            BinOp::Min,
            binary(BinOp::Min, base_cave_density, entrances.clone()),
            binary(BinOp::Add, spaghetti_2d.clone(), spaghetti_roughness.clone()),
        );
        let pillars_choice = sam(DfSam263::RangeChoice {
            a: pillars.clone(),
            min: -1000000.0,
            max: 0.03,
            in_range: constant(-1000000.0),
            out_of_range: pillars.clone(),
        });
        let underground = binary(BinOp::Max, subtractions, pillars_choice);

        let surface_with_entrances = binary(
            BinOp::Min,
            sloped_cheese.clone(),
            binary(BinOp::Mul, entrances.clone(), constant(5.0)),
        );
        let caves = sam(DfSam263::RangeChoice {
            a: sloped_cheese.clone(),
            min: -1000000.0,
            max: 1.5625,
            in_range: surface_with_entrances,
            out_of_range: underground,
        });

        let post_processed = unary(
            UnOp::Squeeze,
            interpolated(
                binary(
                    BinOp::Mul,
                    sam(DfSam263::BlendDensity { a: slide_overworld(caves) }),
                    constant(0.64),
                ),
                4,
                8,
            ),
        );

        let final_density = binary(
            BinOp::Add,
            binary(BinOp::Min, post_processed, noodle.clone()),
            // ContextBoundSampler(Beardifier.CONTEXT_KEY, fallback 0.0): EMPTY
            // beardifier contributes exactly 0.0 (oracle parity holds).
            sam(DfSam263::Beard),
        );

        // preliminary_surface_level
        let upper_bound = clamp_fn(
            remap(
                binary(
                    BinOp::Sub,
                    binary(BinOp::Div, constant(0.2734375), factor.clone()),
                    offset.clone(),
                ),
                1.5,
                -1.5,
                -64.0,
                320.0,
            ),
            -40.0,
            320.0,
        );
        let prelim_density_inner = clamp_fn(
            binary(
                BinOp::Add,
                noise_gradient_density(&factor, offset_to_depth(&offset)),
                constant(-0.703125),
            ),
            -64.0,
            64.0,
        );
        let preliminary = sam(DfSam263::FindTopSurface {
            density: slide_overworld(prelim_density_inner),
            upper_bound,
            lower_bound: -64,
            cell_height: 8,
        });
        self.prelim_node = Some(preliminary.clone());
        let chunk_surface_level = interpolated(preliminary.clone(), 16, 1);

        OverworldRouter263 {
            temperature,
            jaggedness,
            base_3d,
            noodle,
            entrances,
            spaghetti_2d,
            pillars,
            spaghetti_roughness,
            vegetation,
            continents,
            erosion,
            depth,
            ridges,
            chunk_surface_level,
            final_density,
            prelim: preliminary.clone(),
            offset,
            factor,
            sloped_cheese,
        }
    }

    /// The noise_settings "aquifers" section DFs (26.3 moved them out of the
    /// router): barrier/floodedness/spread/lava are noise nodes, exclusion
    /// references the SAME compiled erosion/depth nodes as the router.
    pub fn build_aquifer_config(&mut self) -> crate::aquifer263::AquiferConfig263 {
        let barrier = noise_fn(
            self.normal_noise("aquifer_barrier", (0.955388882960065, -3, 1, &[])),
            1.0,
            0.5,
        );
        let floodedness = noise_fn(
            self.normal_noise(
                "aquifer_fluid_level_floodedness",
                (0.955388882960065, -7, 1, &[]),
            ),
            1.0,
            0.67,
        );
        let spread = noise_fn(
            self.normal_noise(
                "aquifer_fluid_level_spread",
                (0.955388882960065, -5, 1, &[]),
            ),
            1.0,
            0.7142857142857143,
        );
        let lava = noise_fn(
            self.normal_noise("aquifer_lava", (0.955388882960065, -1, 1, &[])),
            1.0,
            1.0,
        );
        let erosion = self
            .erosion_node
            .clone()
            .expect("build_overworld before build_aquifer_config");
        let depth = self
            .depth_node
            .clone()
            .expect("build_overworld before build_aquifer_config");
        // exclusion = min(sub(-0.225, erosion), max(sub(depth, 0.9), 0.0))
        let exclusion = binary(
            BinOp::Min,
            binary(BinOp::Sub, constant(-0.225), erosion),
            binary(
                BinOp::Max,
                binary(BinOp::Sub, depth, constant(0.9)),
                constant(0.0),
            ),
        );
        let surface_level = self
            .prelim_node
            .clone()
            .expect("build_overworld before build_aquifer_config");
        crate::aquifer263::AquiferConfig263 {
            barrier,
            floodedness: floodedness,
            spread,
            lava,
            exclusion,
            surface_level,
        }
    }

    /// 26.3 material-rule ore-vein DF trees
    /// (worldgen/density_function/overworld/ore_vein/*.json transcribed).
    /// Cache ids live in THIS builder's namespace (a fresh RouterBuilder263
    /// per surface state) — never mix with the fill router's ctx.
    pub fn build_ore_vein_dfs(&mut self) -> OreVeinDfs263 {
        // y = YClampedGradient(-4064, 4062, ...) — the "minecraft:y" DF.
        let y = y_clamped_gradient(-4064, 4062, -4064.0, 4062.0);
        let mk = |name: &str, p: NP, s: &mut Self| -> Arc<N263> {
            s.factory.normal(name, params(p.0, p.1, p.2, p.3))
        };
        let veininess = mk("ore_veininess", (0.955388882960065, -8, 1, &[]), self);
        let vein_a = mk("ore_vein_a", (0.955388882960065, -7, 1, &[]), self);
        let vein_b = mk("ore_vein_b", (0.955388882960065, -7, 1, &[]), self);
        let vein_gap = mk("ore_gap", (0.955388882960065, -5, 1, &[]), self);

        // toggle = cache(interpolated(range_choice(y, -64, 57, noise(veininess,1.5,1.5), 0), 4, 8))
        let toggle = self.cache(interpolated(
            sam(DfSam263::RangeChoice {
                a: y.clone(),
                min: -64.0,
                max: 57.0,
                in_range: noise_fn(veininess, 1.5, 1.5),
                out_of_range: constant(0.0),
            }),
            4,
            8,
        ));

        // veinnoise(n) = abs(interpolated(range_choice(y, -64, 57, noise(n,4,4), 1), 4, 8))
        let veinnoise = |n: Arc<N263>| -> SamRef {
            unary(UnOp::Abs, interpolated(
                sam(DfSam263::RangeChoice {
                    a: y.clone(),
                    min: -64.0,
                    max: 57.0,
                    in_range: noise_fn(n, 4.0, 4.0),
                    out_of_range: constant(1.0),
                }),
                4,
                8,
            ))
        };
        let mask = {
            // when_out_of_range = 0.08 - max(abs_a, abs_b)
            let outer = binary(
                BinOp::Sub,
                constant(0.08),
                binary(BinOp::Max, veinnoise(vein_a), veinnoise(vein_b)),
            );
            self.cache(sam(DfSam263::RangeChoice {
                a: toggle.clone(),
                min: -0.4,
                max: 0.4,
                in_range: constant(-1.0),
                out_of_range: outer,
            }))
        };

        // edge_term = clamp(min(50-y, y-0), 0, 20) * 0.01 + (-0.2)
        let edge_term = |top: f32, bottom: f32, s: &mut Self| -> SamRef {
            let a = sam(DfSam263::Clamp {
                a: binary(
                    BinOp::Min,
                    binary(BinOp::Sub, constant(top), y.clone()),
                    binary(BinOp::Sub, y.clone(), constant(bottom)),
                ),
                min: 0.0,
                max: 20.0,
            });
            binary(
                BinOp::Add,
                binary(BinOp::Mul, a, constant(0.01)),
                constant(-0.2),
            )
        };

        // copper: rc(y, 0, 50, rc(mask, 0, 1e6, rc((toggle-0.4) + edge(50,0), 0, 1e6, 0.7, -1), -1), -1)
        let copper = sam(DfSam263::RangeChoice {
            a: y.clone(),
            min: 0.0,
            max: 50.0,
            in_range: sam(DfSam263::RangeChoice {
                a: mask.clone(),
                min: 0.0,
                max: 1000000.0,
                in_range: sam(DfSam263::RangeChoice {
                    a: binary(
                        BinOp::Add,
                        sam(DfSam263::ConstAdd {
                            a: toggle.clone(),
                            c: -0.4,
                        }),
                        edge_term(50.0, 0.0, self),
                    ),
                    min: 0.0,
                    max: 1000000.0,
                    in_range: constant(0.7),
                    out_of_range: constant(-1.0),
                }),
                out_of_range: constant(-1.0),
            }),
            out_of_range: constant(-1.0),
        });

        // iron: rc(y, -60, -8, rc(mask, 0, 1e6, rc((-toggle-0.4) + edge(-8,-60), 0, 1e6, 0.7, -1), -1), -1)
        let iron = sam(DfSam263::RangeChoice {
            a: y.clone(),
            min: -60.0,
            max: -8.0,
            in_range: sam(DfSam263::RangeChoice {
                a: mask.clone(),
                min: 0.0,
                max: 1000000.0,
                in_range: sam(DfSam263::RangeChoice {
                    a: binary(
                        BinOp::Add,
                        binary(
                            BinOp::Sub,
                            unary(UnOp::Negate, toggle.clone()),
                            constant(0.4),
                        ),
                        edge_term(-8.0, -60.0, self),
                    ),
                    min: 0.0,
                    max: 1000000.0,
                    in_range: constant(0.7),
                    out_of_range: constant(-1.0),
                }),
                out_of_range: constant(-1.0),
            }),
            out_of_range: constant(-1.0),
        });

        // richness = (clamp(abs(toggle), 0.4, 0.6) * 1.0) + (-0.3)
        let richness = binary(
            BinOp::Add,
            binary(
                BinOp::Mul,
                sam(DfSam263::Clamp {
                    a: unary(UnOp::Abs, toggle.clone()),
                    min: 0.4,
                    max: 0.6,
                }),
                constant(1.0),
            ),
            constant(-0.3),
        );

        // gap = -0.3 - noise(ore_gap, 1, 1)
        let gap = sam(DfSam263::ConstSub {
            c: -0.3,
            a: noise_fn(vein_gap, 1.0, 1.0),
        });

        OreVeinDfs263 {
            copper_density: copper,
            iron_density: iron,
            richness,
            gap,
            toggle,
            mask,
        }
    }
}

/// The four 26.3 ore-vein material-rule DFs (registry refs resolved) + the
/// shared cache-wrapped toggle/mask subtrees the trio reads.
pub struct OreVeinDfs263 {
    pub copper_density: SamRef,
    pub iron_density: SamRef,
    pub richness: SamRef,
    pub gap: SamRef,
    pub toggle: SamRef,
    pub mask: SamRef,
}
