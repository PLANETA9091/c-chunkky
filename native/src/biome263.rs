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
fn quantize_f32(v: f32) -> i64 {
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
    for sy in 0..sections {
        for x in 0..4i32 {
            for y in 0..4i32 {
                for z in 0..4i32 {
                    let by = (quart_y0 + (sy as i32) * 4 + y) * 4;
                    let bx = (cx * 4 + x) * 4;
                    let bz = (cz * 4 + z) * 4;
                    let cv = &col_vals[(x * 4 + z) as usize];
                    let d = st.climate[4].sample_value(&mut ctx, bx, by, bz);
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
                    out[sy * 64 + (x as usize) * 16 + (y as usize) * 4 + (z as usize)] =
                        st.src.entries[leaf].biome_idx as i32;
                }
            }
        }
    }
}
