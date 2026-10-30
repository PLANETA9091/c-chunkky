// Native 26.3 biome resolution for the embedded-vanilla overworld.
//
// Replaces the Java BIOMES chunk-status work (JFR: 22.6% of gen-worker):
// RandomState.createClimateSampler + moonrise createResolverForChunk volume
// buffering + moonrise$findNearest per quart.
//
// Semantics (decompiled paper-26.3-222):
// - the preset list: MultiNoiseBiomeSourceParameterList.Preset.OVERWORLD —
//   lives in CODE (OverworldBiomeBuilder); extracted at runtime by
//   scripts/ExtractParams263.java into params263_overworld.json (exact
//   quantized i64 ranges);
// - the tree: Climate.RTree with CHILDREN_PER_NODE = 19 and the moonrise
//   warm-start search (lastReturned ThreadLocal) — shared with biomes.rs
//   via BiomeSourceData::from_params263 + find_value;
// - the sampler: the six RAW router functions (temperature, vegetation,
//   continents, erosion, depth, ridges) evaluated at the quart's BLOCK
//   coordinates; Climate.target(f32...) -> quantizeCoord (long)(f * 10000f).
//   None of the six are InterpolatedFunction nodes (shifted noise chain),
//   so vanilla's chunk volume buffering produces the same values as
//   per-point evaluation — bit-equal, cache-independent;
// - the walk: fillBiomesFromNoise — sections ascending, then x, y, z within
//   a section (the first (0,0,0) resolve + x/y/z loop equals a plain triple
//   loop), matching the legacy fill_biomes order so the warm-start state
//   carries across chunks identically.

use crate::df263::SamRef;

pub struct BiomeState263 {
    pub src: crate::biomes::BiomeSourceData,
    /// Compiled router climate nodes, order:
    /// temperature, vegetation, continents, erosion, depth, ridges.
    pub climate: [SamRef; 6],
    /// Per-field y-invariance at QUART sampling. 26.3 overworld: all five
    /// noise fields are ShiftedXz with y_scale = 0.0 (y-invariant); depth
    /// is the y-dependent offset/spline chain.
    pub y_invariant: [bool; 6],
}

#[inline]
pub(crate) fn quantize_f32(v: f32) -> i64 {
    let q = v * 10000.0f32;
    if q.is_nan() {
        0
    } else {
        q as i64
    }
}

/// Fill the chunk's biome quart indexes (biome name-table indexes) in the
/// vanilla fillBiomesFromNoise order:
/// out[s * 64 + x * 16 + y * 4 + z], sections ascending.
/// Per (x, z) quart column the y-invariant fields evaluate once; only the
/// y-dependent fields (depth) re-evaluate per quart.
pub fn fill_biomes263(
    st: &BiomeState263,
    min_y: i32,
    height: i32,
    cx: i32,
    cz: i32,
    out: &mut [i32],
) {
    let sections = (height as usize) / 16;
    debug_assert!(out.len() >= sections * 64);
    let quart_y0 = min_y / 4;
    let mut ctx = crate::df263::SamplerCtx263::uncached();
    // Pass 1: the y-invariant fields (t, v, c, e, r) per (x, z) quart column
    // — 80 evaluations for the whole chunk (y_scale = 0.0 on all five noise
    // fields; evaluating at the chunk's first y is bit-identical to any y).
    let mut col_vals = [[0f32; 5]; 16];
    let by0 = quart_y0 * 4;
    for x in 0..4i32 {
        for z in 0..4i32 {
            let bx = (cx * 4 + x) * 4;
            let bz = (cz * 4 + z) * 4;
            let mut row = [0f32; 5];
            for (fi, slot) in row.iter_mut().enumerate() {
                let node = &[0usize, 1, 2, 3, 5][fi];
                *slot = st.climate[*node].sample_value(&mut ctx, bx, by0, bz);
            }
            col_vals[(x * 4 + z) as usize] = row;
        }
    }
    // Pass 2: the vanilla search walk (sections ascending, x, y, z within a
    // section — the warm-start sequence). Only depth re-evaluates per quart.
    // DEPTH VOLUME FAST PATH: one sample_volume over the quart grid replaces
    // sections*64 per-point tree walks. The generic volume machinery already
    // hoists per-column noise prologues (stack263_fill_col) and broadcasts
    // y-invariant children (the five ShiftedXz climate fields), so the fill
    // collapses to ~16 depth-noise column kernels. The volume/point duality
    // is structural for the climate six (NO Interpolated/FindTopSurface —
    // module header); volume_duality_safe re-verifies per call and any
    // structural surprise falls back to the scalar per-point walk. The walk
    // order (warm-start search) is untouched — only the depth READ moves.
    let sections_i = sections as i32;
    debug_assert_eq!(min_y % 4, 0, "quart grid requires a 4-aligned min_y");
    let use_depth_vol = volume_duality_safe(&st.climate[4]);
    let depth_buf: Vec<f32> = if use_depth_vol {
        let qvol = crate::df263::DensityVolume {
            size_x: 4,
            size_y: sections_i * 4,
            size_z: 4,
            min_x: cx * 64,
            min_y,
            min_z: cz * 64,
            step_x: 4,
            step_y: 4,
            step_z: 4,
        };
        let mut b = vec![0f32; (4 * sections_i * 4 * 4) as usize];
        st.climate[4].sample_volume(&mut ctx, &mut b, &qvol);
        b
    } else {
        Vec::new()
    };
    for sy in 0..sections {
        for x in 0..4i32 {
            for y in 0..4i32 {
                for z in 0..4i32 {
                    let by = (quart_y0 + (sy as i32) * 4 + y) * 4;
                    let bx = (cx * 4 + x) * 4;
                    let bz = (cz * 4 + z) * 4;
                    let cv = &col_vals[(x * 4 + z) as usize];
                    let d = if use_depth_vol {
                        // DensityVolume index: y + (x + z*size_x)*size_y
                        // with size_x=4, size_y=sections*4; (x, sy*4+y, z).
                        depth_buf[(sy as i32 * 4
                            + y
                            + (x + z * 4) * (sections_i * 4)) as usize]
                    } else {
                        st.climate[4].sample_value(&mut ctx, bx, by, bz)
                    };
                    let values = [
                        quantize_f32(cv[0]),
                        quantize_f32(cv[1]),
                        quantize_f32(cv[2]),
                        quantize_f32(cv[3]),
                        quantize_f32(d),
                        quantize_f32(cv[4]),
                        0, // offset: all vanilla preset entries carry 0
                    ];
                    let leaf = st.src.find_value(&values);
                    out[(sy * 64 + (x as usize) * 16 + (y as usize) * 4 + (z as usize)) as usize] =
                        st.src.entries[leaf].biome_idx as i32;
                }
            }
        }
    }
}

/// Structural volume/point duality gate for the depth quart fast path.
/// A tree evaluates identically per point and per volume fill iff it has NO
/// Interpolated / FindTopSurface / BlendDensity nodes: interpolated nodes
/// change their sampling lattice with the volume (cell buffering), and
/// FindTopSurface / BlendDensity scan or blend in ways the quart volume does
/// not reproduce pointwise. Everything else (noise stacks, splines, binary
/// and constant-combinator chains, caches, slices, gradients) is pointwise
/// or column-hoisted bit-exactly by the volume machinery. The 26.3 climate
/// six satisfy this by construction (module header); this walk re-verifies
/// so a future router edit cannot silently break the fast path.
fn volume_duality_safe(a: &SamRef) -> bool {
    use crate::df263::DfSam263 as D;
    match a.as_ref() {
        D::Interpolated { .. }
        | D::FindTopSurface { .. }
        | D::BlendDensity { .. } => false,
        D::Constant(_) | D::Noise { .. } | D::ShiftB { .. } | D::Blended { .. }
        | D::Gradient { .. } | D::Beard => true,
        D::ShiftedXz { shift_x, shift_z, .. } => {
            volume_duality_safe(shift_x) && volume_duality_safe(shift_z)
        }
        D::Binary { l, r, .. } | D::Pow { a: l, b: r } => {
            volume_duality_safe(l) && volume_duality_safe(r)
        }
        D::ConstAdd { a, .. }
        | D::ConstSub { a, .. }
        | D::ConstMul { a, .. }
        | D::ConstDiv { a, .. }
        | D::ConstMin { a, .. }
        | D::ConstMax { a, .. }
        | D::Unary { a, .. }
        | D::Clamp { a, .. }
        | D::PowConstBase { exp: a, .. }
        | D::PowConstExp { a, .. }
        | D::RoundInt { a, .. }
        | D::Round { a, .. }
        | D::RangeChoiceConst { a, .. }
        | D::Cache { a, .. }
        | D::SliceX { a, .. }
        | D::SliceY { a, .. }
        | D::SliceZ { a, .. }
        | D::SliceXz { a, .. } => volume_duality_safe(a),
        D::Lerp {
            alpha,
            first,
            second,
        } => {
            volume_duality_safe(alpha)
                && volume_duality_safe(first)
                && volume_duality_safe(second)
        }
        D::LerpConstFirst { alpha, second, .. } => {
            volume_duality_safe(alpha) && volume_duality_safe(second)
        }
        D::LerpConstSecond { alpha, first, .. } => {
            volume_duality_safe(alpha) && volume_duality_safe(first)
        }
        D::RangeChoice {
            a,
            in_range,
            out_of_range,
            ..
        } => volume_duality_safe(a) && volume_duality_safe(in_range) && volume_duality_safe(out_of_range),
        D::IntervalSelect { a, branches, .. } => {
            volume_duality_safe(a) && branches.iter().all(|b| volume_duality_safe(b))
        }
        D::Spline { coord, values, .. } => {
            volume_duality_safe(coord)
                && values.iter().all(|v| spline_duality_safe(v))
        }
    }
}

fn spline_duality_safe(a: &crate::df263::SplineRef) -> bool {
    match a {
        crate::df263::SplineRef::Constant(_) => true,
        crate::df263::SplineRef::Multi { coord, values, .. } => {
            volume_duality_safe(coord) && values.iter().all(|v| spline_duality_safe(v))
        }
    }
}

#[cfg(test)]
mod depth_volume_tests {
    use super::*;

    /// Pins the quart-volume duality for the REAL 26.3 depth tree: the
    /// volume fill must be bit-identical to the per-point walk (the fast
    /// path in fill_biomes263 relies on exactly this — module header
    /// invariant "none of the climate six is Interpolated").
    #[test]
    fn depth_quart_volume_bit_exact_vs_points() {
        let g = crate::fill263::Gen263::new(3257840388504953787, -64, 384, 63);
        let depth = g.climate[4].clone();
        assert!(
            volume_duality_safe(&depth),
            "the real depth tree must pass the structural duality gate"
        );

        let (min_y, height) = (-64i32, 384i32);
        let sections = (height / 16) as i32;
        let (cx, cz) = (17i32, -5i32); // arbitrary non-trivial chunk
        let mut ctx = crate::df263::SamplerCtx263::uncached();

        // (a) the volume fill exactly as the fill_biomes263 fast path does it
        let qvol = crate::df263::DensityVolume {
            size_x: 4,
            size_y: sections * 4,
            size_z: 4,
            min_x: cx * 64,
            min_y,
            min_z: cz * 64,
            step_x: 4,
            step_y: 4,
            step_z: 4,
        };
        let mut vol_buf = vec![0f32; (4 * sections * 4 * 4) as usize];
        depth.sample_volume(&mut ctx, &mut vol_buf, &qvol);

        // (b) the scalar per-point walk (the fallback semantics)
        let quart_y0 = min_y / 4;
        for sy in 0..sections {
            for x in 0..4i32 {
                for y in 0..4i32 {
                    for z in 0..4i32 {
                        let by = (quart_y0 + sy as i32 * 4 + y) * 4;
                        let bx = (cx * 4 + x) * 4;
                        let bz = (cz * 4 + z) * 4;
                        let point = depth.sample_value(&mut ctx, bx, by, bz);
                        let fast = vol_buf[(sy as i32 * 4
                            + y
                            + (x + z * 4) * (sections * 4))
                            as usize];
                        assert_eq!(
                            point.to_bits(),
                            fast.to_bits(),
                            "quart ({x},{},{z}) section {sy}: point {point} vs volume {fast}",
                            sy * 4 + y
                        );
                    }
                }
            }
        }
    }

    /// The live CI parity gate (eb0afaec run 41105141065) found the grid
    /// diverging from vanilla point resolution at chunk (1024,1024): whole
    /// quart rows swap jungle <-> lush_caves. This test re-runs the EXACT
    /// failing chunk and bisects the fast path:
    /// (a) the five pass-1 fields at by0 vs the quart's own y (the
    ///     y-invariant assumption),
    /// (b) depth quart-volume vs per-point (ULP-level localization),
    /// (c) the full fill_biomes263 output vs a naive scalar walk.
    #[test]
    fn fill263_grid_naive_parity_at_1024_1024() {
        let src = crate::biomes::BiomeSourceData::from_params263(include_str!(
            "params263_overworld.json"
        ))
        .expect("preset parse");
        let g = crate::fill263::Gen263::new(3257840388504953787, -64, 384, 63);
        let st = BiomeState263 {
            src,
            climate: g.climate.clone(),
            y_invariant: [true, true, true, true, false, true],
        };

        let (min_y, height) = (-64i32, 384i32);
        let (cx, cz) = (1024i32, 1024i32);
        let sections = (height / 16) as usize;
        let sections_i = sections as i32;
        let quart_y0 = min_y / 4;

        // production fill, exactly as the JNI fillBiomes runs it
        let mut fast = vec![0i32; sections * 64];
        fill_biomes263(&st, min_y, height, cx, cz, &mut fast);

        // (b) depth: volume fill (as the fast path does) vs per-point
        let mut ctx = crate::df263::SamplerCtx263::uncached();
        let qvol = crate::df263::DensityVolume {
            size_x: 4,
            size_y: sections_i * 4,
            size_z: 4,
            min_x: cx * 64,
            min_y,
            min_z: cz * 64,
            step_x: 4,
            step_y: 4,
            step_z: 4,
        };
        let mut dvol = vec![0f32; (4 * sections_i * 4 * 4) as usize];
        st.climate[4].sample_volume(&mut ctx, &mut dvol, &qvol);

        // (a) the five pass-1 fields: by0 (what the fill evaluates) vs the
        // quart's own y — the y-invariant assumption at this exact chunk.
        let by0 = quart_y0 * 4;
        for x in 0..4i32 {
            for z in 0..4i32 {
                let bx = (cx * 4 + x) * 4;
                let bz = (cz * 4 + z) * 4;
                for y in 0..sections_i * 4 {
                    let by = (quart_y0 + y) * 4;
                    for (fi, node) in [0usize, 1, 2, 3, 5].into_iter().enumerate() {
                        let at_by0 = st.climate[node].sample_value(&mut ctx, bx, by0, bz);
                        let at_by = st.climate[node].sample_value(&mut ctx, bx, by, bz);
                        assert_eq!(
                            at_by0.to_bits(),
                            at_by.to_bits(),
                            "y-invariance field {fi} chunk ({cx},{cz}) column ({x},{z}) \
                             quart row {y}: by0 {at_by0} vs own-y {at_by}"
                        );
                    }
                }
            }
        }

        // (c) naive scalar walk (all six fields per point) vs the fast fill
        let mut naive = vec![0i32; sections * 64];
        for sy in 0..sections {
            for x in 0..4usize {
                for y in 0..4usize {
                    for z in 0..4usize {
                        let by = (quart_y0 + (sy as i32) * 4 + y as i32) * 4;
                        let bx = (cx * 4 + x as i32) * 4;
                        let bz = (cz * 4 + z as i32) * 4;
                        let d_pt = st.climate[4].sample_value(&mut ctx, bx, by, bz);
                        let d_vol = dvol[(sy as i32 * 4
                            + y as i32
                            + (x as i32 + z as i32 * 4) * (sections_i * 4))
                            as usize];
                        assert_eq!(
                            d_pt.to_bits(),
                            d_vol.to_bits(),
                            "depth point-vs-volume chunk ({cx},{cz}) section {sy} \
                             quart ({x},{},{z}): point {d_pt} vs volume {d_vol}",
                            sy * 4 + y
                        );
                        let mut row = [0f32; 5];
                        for (fi, node) in [0usize, 1, 2, 3, 5].into_iter().enumerate() {
                            row[fi] = st.climate[node].sample_value(&mut ctx, bx, by, bz);
                        }
                        let values = [
                            quantize_f32(row[0]),
                            quantize_f32(row[1]),
                            quantize_f32(row[2]),
                            quantize_f32(row[3]),
                            quantize_f32(d_pt),
                            quantize_f32(row[4]),
                            0,
                        ];
                        let leaf = st.src.find_value(&values);
                        naive[sy * 64 + x * 16 + y * 4 + z] =
                            st.src.entries[leaf].biome_idx as i32;
                    }
                }
            }
        }
        assert_eq!(
            fast, naive,
            "fill_biomes263 fast path vs naive scalar walk at chunk ({cx},{cz})"
        );
    }
}
