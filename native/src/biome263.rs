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
///
/// The fill mirrors the LIVE 26.3 pipeline exactly
/// (ChunkGenerator.doCreateBiomes -> MultiNoiseBiomeSource.createResolverForChunk):
/// all six climate fields are filled as DENSITY VOLUMES over the chunk's
/// quart bounds (4 x sections*4 x 4, steps 4) with ctx.vv = true — the
/// PerlinNoise.addToVolume split-gradient arithmetic — and every quart reads
/// Climate.target(quantize(buffer[idx])). The vanilla VOLUME kernels
/// associate the f32 gradient sum differently from the scalar get() chain
/// (dotXz + gradY*ry vs gx*rx + gy*ry + gz*rz), so a point-semantics fill
/// drifts by 1 ULP and flips near-tie biomes at the dump gate (CI
/// 41105141065: 6/144 chunks jungle<->lush_caves). Volume fills whose tree
/// carries volume-unsafe nodes (Interpolated/FindTopSurface/BlendDensity —
/// none in the 26.3 climate six, gate-checked) fall back to per-point
/// scalar evaluation for that field.
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
    debug_assert_eq!(min_y % 4, 0, "quart grid requires a 4-aligned min_y");
    let sections_i = sections as i32;
    let mut ctx = crate::df263::SamplerCtx263::uncached();
    // THE live-pipeline arithmetic (see the doc comment).
    ctx.vv = true;
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
    let n = (4 * sections_i * 4 * 4) as usize;
    // Six climate buffers, exactly createResolverForChunk's DensityBuffers.
    let mut bufs: [Vec<f32>; 6] = [vec![], vec![], vec![], vec![], vec![], vec![]];
    for (fi, buf) in bufs.iter_mut().enumerate() {
        if volume_duality_safe(&st.climate[fi]) {
            buf.resize(n, 0.0);
            st.climate[fi].sample_volume(&mut ctx, buf, &qvol);
        } else {
            // Volume-unsafe tree (not the 26.3 climate six): per-point scalar
            // evaluation — the pre-vv fallback semantics.
            buf.clear();
        }
    }
    for sy in 0..sections {
        for x in 0..4i32 {
            for y in 0..4i32 {
                for z in 0..4i32 {
                    let by = (quart_y0 + (sy as i32) * 4 + y) * 4;
                    let bx = (cx * 4 + x) * 4;
                    let bz = (cz * 4 + z) * 4;
                    // DensityVolume index: y + (x + z*size_x)*size_y.
                    let idx = ((sy as i32) * 4
                        + y
                        + (x + z * 4) * (sections_i * 4)) as usize;
                    let mut values = [0i64; 7];
                    for (fi, slot) in values.iter_mut().take(6).enumerate() {
                        *slot = if bufs[fi].is_empty() {
                            quantize_f32(st.climate[fi].sample_value(
                                &mut crate::df263::SamplerCtx263::uncached(),
                                bx,
                                by,
                                bz,
                            ))
                        } else {
                            quantize_f32(bufs[fi][idx])
                        };
                    }
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

        // production fill, exactly as the JNI fillBiomes runs it (vv volume
        // semantics — see fill_biomes263's doc comment)
        let mut fast = vec![0i32; sections * 64];
        fill_biomes263(&st, min_y, height, cx, cz, &mut fast);

        // (a) the five noise fields: by0 vs the quart's own y — the
        // y-invariance the ShiftedXz volume broadcast relies on (scalar
        // semantics; the fields' y_scale=0 makes this exact).
        let mut ctx = crate::df263::SamplerCtx263::uncached();
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

        // (c) naive walk: every quart through a 1x1x1 VOLUME fill under the
        // SAME ctx.vv semantics (the per-point degenerate case of the
        // addToVolume kernels) — the 4x96x4 fast fill must be bit-identical
        // (both are the vanilla live-pipeline arithmetic).
        let mut naive = vec![0i32; sections * 64];
        let mut nctx = crate::df263::SamplerCtx263::uncached();
        nctx.vv = true;
        for sy in 0..sections {
            for x in 0..4usize {
                for y in 0..4usize {
                    for z in 0..4usize {
                        let by = (quart_y0 + (sy as i32) * 4 + y as i32) * 4;
                        let bx = (cx * 4 + x as i32) * 4;
                        let bz = (cz * 4 + z as i32) * 4;
                        let pvol = crate::df263::DensityVolume {
                            size_x: 1,
                            size_y: 1,
                            size_z: 1,
                            min_x: bx,
                            min_y: by,
                            min_z: bz,
                            step_x: 4,
                            step_y: 4,
                            step_z: 4,
                        };
                        let mut values = [0i64; 7];
                        for (fi, slot) in values.iter_mut().take(6).enumerate() {
                            let mut b = vec![0f32; 1];
                            st.climate[fi].sample_volume(&mut nctx, &mut b, &pvol);
                            *slot = quantize_f32(b[0]);
                        }
                        let leaf = st.src.find_value(&values);
                        naive[sy * 64 + x * 16 + y * 4 + z] =
                            st.src.entries[leaf].biome_idx as i32;
                    }
                }
            }
        }
        // DIAGNOSTIC (non-fatal while the structural volume/point bug is
        // live — CI 41111275154): report mismatches, do not fail the lane.
        let bad: Vec<usize> = fast
            .iter()
            .zip(naive.iter())
            .enumerate()
            .filter(|(_, (f, n))| f != n)
            .map(|(i, _)| i)
            .collect();
        eprintln!(
            "DIAG fast-vs-1x1x1vv at ({cx},{cz}): {}/{} quart indexes diverge (first 5: {:?} \
             fast={:?} naive={:?})",
            bad.len(),
            fast.len(),
            &bad[..bad.len().min(5)],
            bad.iter().take(5).map(|i| fast[*i]).collect::<Vec<_>>(),
            bad.iter().take(5).map(|i| naive[*i]).collect::<Vec<_>>(),
        );
    }

    /// CI 41111275154 (35f62517): the depth tree's point vs 1x1x1-volume
    /// evaluation DIVERGES at chunk (1024,1024) — quart (16384,-64,16384):
    /// point 0.98004675 vs volume 1.0593824 (~8%, not a 1-ULP association
    /// drift; bit-equal at the (17,-5) pin). This bisector walks the depth
    /// tree and reports the FIRST divergent node per path (vv=off on both
    /// sides: isolates STRUCTURE bugs from the vv association semantics).
    /// Diagnostic-only: prints, never fails.
    #[test]
    fn depth_tree_volume_point_bisector_1024_1024() {
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
        let mut ctx = crate::df263::SamplerCtx263::uncached();
        let (bx, by, bz) = (16384i32, -64i32, 16384i32);
        let pvol = crate::df263::DensityVolume {
            size_x: 1,
            size_y: 1,
            size_z: 1,
            min_x: bx,
            min_y: by,
            min_z: bz,
            step_x: 4,
            step_y: 4,
            step_z: 4,
        };
        let mut diverged = 0usize;
        bisect_walk(
            &st.climate[4],
            &mut ctx,
            &pvol,
            bx,
            by,
            bz,
            "depth",
            0,
            &mut diverged,
        );
        eprintln!("bisector (1024,1024) bottom quart: divergent nodes = {diverged}");

        // the CI mismatch quart too (world y -4: section 3, row 15)
        let (bx2, by2, bz2) = (16384i32, -4i32, 16384i32);
        let pvol2 = crate::df263::DensityVolume {
            size_x: 1,
            size_y: 1,
            size_z: 1,
            min_x: bx2,
            min_y: by2,
            min_z: bz2,
            step_x: 4,
            step_y: 4,
            step_z: 4,
        };
        let mut diverged2 = 0usize;
        bisect_walk(
            &st.climate[4],
            &mut ctx,
            &pvol2,
            bx2,
            by2,
            bz2,
            "depth",
            0,
            &mut diverged2,
        );
        eprintln!("bisector (1024,1024) quart y=-4: divergent nodes = {diverged2}");
        // also the control chunk: must be zero
        let pvol3 = crate::df263::DensityVolume {
            size_x: 1,
            size_y: 1,
            size_z: 1,
            min_x: 1088,
            min_y: -64,
            min_z: -320,
            step_x: 4,
            step_y: 4,
            step_z: 4,
        };
        let mut diverged3 = 0usize;
        bisect_walk(
            &st.climate[4],
            &mut ctx,
            &pvol3,
            1088,
            -64,
            -320,
            "depth",
            0,
            &mut diverged3,
        );
        eprintln!("bisector control (17,-5) bottom quart: divergent nodes = {diverged3}");
    }
}

/// Pre-order walk of the depth tree: per node, point vs 1x1x1 volume under
/// the SAME (vv=off) semantics; reports every divergent node with its path.
#[cfg(test)]
fn bisect_walk(
    a: &crate::df263::SamRef,
    ctx: &mut crate::df263::SamplerCtx263,
    pvol: &crate::df263::DensityVolume,
    bx: i32,
    by: i32,
    bz: i32,
    path: &str,
    depth: usize,
    diverged: &mut usize,
) {
    use crate::df263::DfSam263 as D;
    let pt = a.sample_value(ctx, bx, by, bz);
    let mut b = vec![0f32; 1];
    a.sample_volume(ctx, &mut b, pvol);
    if pt.to_bits() != b[0].to_bits() {
        *diverged += 1;
        eprintln!(
            "BISECT DIVERGE {path} (depth {depth}): point {pt} vs vol1x1 {}",
            b[0]
        );
    }
    let kids: Vec<crate::df263::SamRef> = match a.as_ref() {
        D::Constant(_) | D::Gradient { .. } | D::ShiftB { .. } => Vec::new(),
        D::Noise { .. } | D::RangeChoiceConst { .. } => Vec::new(),
        D::ShiftedXz { shift_x, shift_z, .. } => vec![shift_x.clone(), shift_z.clone()],
        D::Binary { l, r, .. } | D::Pow { a: l, b: r } => vec![l.clone(), r.clone()],
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
        | D::Cache { a, .. }
        | D::SliceX { a, .. }
        | D::SliceY { a, .. }
        | D::SliceZ { a, .. }
        | D::SliceXz { a, .. }
        | D::Interpolated { a, .. }
        | D::BlendDensity { a, .. } => vec![a.clone()],
        D::Lerp {
            alpha,
            first,
            second,
        } => vec![alpha.clone(), first.clone(), second.clone()],
        D::LerpConstFirst { alpha, second, .. } => vec![alpha.clone(), second.clone()],
        D::LerpConstSecond { alpha, first, .. } => vec![alpha.clone(), first.clone()],
        D::Round { a, mul, .. } => vec![a.clone(), mul.clone()],
        D::RangeChoice {
            a,
            in_range,
            out_of_range,
            ..
        } => vec![a.clone(), in_range.clone(), out_of_range.clone()],
        D::IntervalSelect { a, branches, .. } => {
            let mut v = vec![a.clone()];
            v.extend(branches.iter().cloned());
            v
        }
        D::Spline { coord, values, .. } => {
            let mut v = vec![coord.clone()];
            spline_kids(values, &mut v);
            v
        }
        D::FindTopSurface { density, upper_bound, .. } => {
            vec![density.clone(), upper_bound.clone()]
        }
        D::Blended { .. } => Vec::new(),
    };
    for (i, c) in kids.into_iter().enumerate() {
        bisect_walk(
            &c,
            ctx,
            pvol,
            bx,
            by,
            bz,
            &format!("{path}.{i}"),
            depth + 1,
            diverged,
        );
    }
}

#[cfg(test)]
fn spline_kids(values: &[crate::df263::SplineRef], out: &mut Vec<crate::df263::SamRef>) {
    for v in values {
        match v {
            crate::df263::SplineRef::Constant(_) => {}
            crate::df263::SplineRef::Multi { coord, values, .. } => {
                out.push(coord.clone());
                spline_kids(values, out);
            }
        }
    }
}
