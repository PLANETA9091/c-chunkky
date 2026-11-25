// NATIVE CARVERS — MC 26.3 (paper-26.3-222, mojmap decompile).
//
// 26.3 moved carvers INTO buildTerrain (noise stage): WorldCarver implementations
// only write a CarvingMask; NoiseBasedChunkGenerator.applyCarvingMask then fills
// carved cells with aquifer.computeSubstance(x, y, z, 0.0) and converts the dirt
// floor under carved air via topMaterial. We capture the whole stage natively:
//
//   generateCarvers (26.3 semantics, CFR WorldCarver/CaveWorldCarver/
//   CanyonWorldCarver/NoiseBasedChunkGenerator):
//     - range = 8 (17x17 source window), LegacyRandomSource seeded per
//       (carver index, source chunk) via WorldgenRandom.setLargeFeatureSeed
//     - carvers list per SOURCE-chunk biome (quart y=0), flat list in 26.3
//     - mask y range [minGenY + 1, minGenY + genDepth - 1 - 7] (fresh chunks)
//
//   CaveWorldCarver / CanyonWorldCarver: direct ports (tunnel recursion,
//   room, thickness bias, width factors, canReach, carveEllipsoid).
//
//   applyCarvingMask: per column top->bottom, skip UNCARVABLE (bedrock),
//   state = aquifer.computeSubstance(x, y, z, 0.0); null -> keep block;
//   fluid updates scheduled (Paper applies post-processing itself).

use crate::density::{Env, Mode, Pt};
use crate::rng::LegacyRandom;

/// Parsed 26.3 carver descriptor (worldgen/carver/*.json).
#[derive(Clone, Debug)]
pub enum CarverKind {
    Cave {
        probability: f32,
        y: HeightProvider,
        count: IntProvider,
        thickness: FloatProvider,
        weird_thickness_bias: bool,
        room_vertical_radius_multiplier: FloatProvider,
        horizontal_radius_multiplier: FloatProvider,
        vertical_radius_multiplier: FloatProvider,
        start_vertical_radius_multiplier: FloatProvider,
        floor_level: FloatProvider,
    },
    Canyon {
        probability: f32,
        y: HeightProvider,
        vertical_rotation: FloatProvider,
        shape: CanyonShape,
    },
}

#[derive(Clone, Copy, Debug)]
pub struct CanyonShape {
    pub distance_factor: FloatProvider,
    pub thickness: FloatProvider,
    pub width_smoothness: i32,
    pub horizontal_radius_factor: FloatProvider,
    pub vertical_radius_default_factor: f32,
    pub vertical_radius_center_factor: f32,
    pub y_scale: FloatProvider,
}

#[derive(Clone, Copy, Debug)]
pub enum HeightProvider {
    Uniform { min: i32, max: i32 },
}

#[derive(Clone, Copy, Debug)]
pub enum IntProvider {
    Constant(i32),
    Uniform { min: i32, max: i32 },
    /// BiasedToBottomInt: min + nextInt(nextInt(range+1) + 1) — double nest.
    BiasedToBottom { min: i32, max: i32 },
    /// VeryBiasedToBottomInt (the vanilla cave "count" provider): min +
    /// nextInt(nextInt(nextInt(range+1) + 1) + 1) — TRIPLE nest, bit-exact
    /// with carver263::IProv263::VeryBiasedToBottom (mask-probe verified).
    VeryBiasedToBottom { min: i32, max: i32 },
}

#[derive(Clone, Copy, Debug)]
pub enum FloatProvider {
    Constant(f32),
    Uniform { min: f32, max: f32 },
    Trapezoid { min: f32, max: f32, plateau: f32 },
    /// ClampedNormalFloat: (float) clamp(mean + nextGaussian * dev, min, max)
    /// — f64 gaussian math, f64 clamp, single f32 cast (same verified formula
    /// as features_placement::FloatProv::ClampedNormal).
    ClampedNormal { mean: f32, dev: f32, min: f32, max: f32 },
}

/// A resolved carver with its biome-list index (order defines RNG seeding).
#[derive(Clone, Debug)]
pub struct CarverEntry {
    pub name: String,
    pub kind: CarverKind,
}

/// World-level carve state: registry + per-biome lists.
pub struct CarverState {
    pub registry: Vec<CarverEntry>,
    /// biome id ("<ns>:<name>") -> list of carver registry indexes (JSON list
    /// order). PRESENT keys are explicit biome lists (possibly EMPTY — the
    /// biome opted out of carving); missing keys fall back to `default`.
    pub per_biome: std::collections::HashMap<String, Vec<usize>>,
    pub min_y: i32,
    pub gen_depth: i32,
    /// carve-in-fill gate (JNI carverSetEnabled flips it; the fill tail also
    /// honors the global CRUS_CARVE_IN_FILL gate).
    pub enabled: bool,
    /// vanilla trio registry indexes — the fallback list for biomes without
    /// an explicit pack JSON (all vanilla biomes list exactly these three).
    pub default: Vec<usize>,
    /// source chunk -> biome id memo (the 17x17 window overlaps heavily
    /// between adjacent center chunks; queries are pure functions).
    pub biome_memo: std::collections::HashMap<(i32, i32), String>,
}

fn getf(j: &crate::json::Json, k: &str) -> Option<f64> {
    j.get(k).and_then(|v| v.as_f64())
}
fn gets<'a>(j: &'a crate::json::Json, k: &str) -> Option<&'a str> {
    j.get(k).and_then(|v| v.as_str())
}

fn parse_float_provider(j: &crate::json::Json) -> Option<FloatProvider> {
    // bare number -> ConstantFloat ("yScale": 13)
    if let Some(v) = j.as_f64() {
        return Some(FloatProvider::Constant(v as f32));
    }
    if let Some(v) = getf(j, "value") {
        return Some(FloatProvider::Constant(v as f32));
    }
    let t = gets(j, "type")?;
    match t.strip_prefix("minecraft:").unwrap_or(t) {
        "constant" => Some(FloatProvider::Constant(getf(j, "value")? as f32)),
        "uniform" => Some(FloatProvider::Uniform {
            min: getf(j, "min_inclusive")? as f32,
            max: getf(j, "max_exclusive").or_else(|| getf(j, "max_inclusive"))? as f32,
        }),
        "trapezoid" => Some(FloatProvider::Trapezoid {
            min: getf(j, "min")? as f32,
            max: getf(j, "max")? as f32,
            plateau: getf(j, "plateau")? as f32,
        }),
        "clamped_normal" => Some(FloatProvider::ClampedNormal {
            mean: getf(j, "mean")? as f32,
            dev: getf(j, "deviation")? as f32,
            min: getf(j, "min")? as f32,
            max: getf(j, "max")? as f32,
        }),
        _ => None,
    }
}

fn parse_int_provider(j: &crate::json::Json) -> Option<IntProvider> {
    // bare number -> ConstantInt
    if let Some(v) = j.as_f64() {
        return Some(IntProvider::Constant(v as i32));
    }
    if let Some(v) = getf(j, "value") {
        return Some(IntProvider::Constant(v as i32));
    }
    let t = gets(j, "type")?;
    match t.strip_prefix("minecraft:").unwrap_or(t) {
        "constant" => Some(IntProvider::Constant(getf(j, "value")? as i32)),
        "uniform" => Some(IntProvider::Uniform {
            min: getf(j, "min_inclusive")? as i32,
            max: getf(j, "max_inclusive")? as i32,
        }),
        // 26.3 name; 1.21.8 alias kept
        "very_biased_to_bottom" => Some(IntProvider::VeryBiasedToBottom {
            min: getf(j, "min_inclusive")? as i32,
            max: getf(j, "max_inclusive")? as i32,
        }),
        "biased_to_bottom" => Some(IntProvider::BiasedToBottom {
            min: getf(j, "min_inclusive")? as i32,
            max: getf(j, "max_inclusive")? as i32,
        }),
        _ => None,
    }
}

/// HeightProvider "uniform" with absolute/above_bottom/below_top anchors.
fn parse_height_provider(j: &crate::json::Json, min_y: i32, height: i32) -> Option<HeightProvider> {
    let t = gets(j, "type")?;
    let anchor = |v: &crate::json::Json| -> Option<i32> {
        if let Some(y) = v.get("absolute").and_then(|x| x.as_f64()) {
            return Some(y as i32);
        }
        if let Some(y) = v.get("above_bottom").and_then(|x| x.as_f64()) {
            return Some(min_y + y as i32);
        }
        if let Some(y) = v.get("below_top").and_then(|x| x.as_f64()) {
            return Some(min_y + height - 1 - y as i32);
        }
        None
    };
    match t.strip_prefix("minecraft:").unwrap_or(t) {
        "uniform" => Some(HeightProvider::Uniform {
            min: anchor(j.get("min_inclusive")?)?,
            max: anchor(j.get("max_inclusive")?)?,
        }),
        _ => None,
    }
}

fn parse_canyon_shape(j: &crate::json::Json, root: &crate::json::Json) -> Option<CanyonShape> {
    Some(CanyonShape {
        distance_factor: parse_float_provider(j.get("distance_factor")?)?,
        thickness: parse_float_provider(j.get("thickness")?)?,
        width_smoothness: j.get("width_smoothness").and_then(|v| v.as_f64())? as i32,
        horizontal_radius_factor: parse_float_provider(j.get("horizontal_radius_factor")?)?,
        vertical_radius_default_factor: getf(j, "vertical_radius_default_factor")? as f32,
        vertical_radius_center_factor: getf(j, "vertical_radius_center_factor")? as f32,
        // 26.3 field; 1.21.x packs carry the top-level "yScale" (Terralith
        // canyon/deep_ravine ship shape WITHOUT y_scale + a top-level one).
        y_scale: j
            .get("y_scale")
            .and_then(parse_float_provider)
            .or_else(|| root.get("yScale").and_then(parse_float_provider))?,
    })
}

impl CarverState {
    /// Parse the 26.3 carver registry + biome carver lists from assets.
    ///
    /// The registry is SEEDED with the three vanilla overworld carvers
    /// (26.3-verified params — the same values carver263::overworld_carvers
    /// ships, which the CarverMaskProbe/CaveStreamProbe gates proved
    /// bit-identical to the real 26.3 carver classes). Pack assets then
    /// REPLACE entries by full id ("<ns>:<name>") or append new ones
    /// (Terralith's minecraft:canyon override + terralith:deep_ravine).
    /// Vanilla cave/cave_extra_underground have no pack JSONs — the seeded
    /// defaults stand, exactly like the server's registry (packs ship no
    /// vanilla-namespace cave files; Tectonic ships none at all outside its
    /// inactive no_carvers overlay).
    pub fn from_assets(assets: &crate::worldgen::Assets, min_y: i32, height: i32) -> Option<CarverState> {
        let mut registry: Vec<CarverEntry> = vanilla_trio(min_y, height)
            .into_iter()
            .map(|(name, kind)| CarverEntry { name: name.to_string(), kind })
            .collect();
        for path in assets.paths() {
            // configured_carver (the real 26.3 path, all namespaces) + the
            // legacy "carver" alias the old dptt merged set shipped.
            let Some((ns, stem)) = strip_carver_path(path) else { continue };
            let bytes = match assets.get(path) {
                Some(b) => b,
                None => continue,
            };
            let Ok(text) = std::str::from_utf8(bytes) else { continue };
            let Ok(j) = crate::json::parse(text) else { continue };
            let Some(kind) = parse_carver_kind(&j, min_y, height) else { continue };
            let name = format!("{ns}:{stem}");
            if let Some(slot) = registry.iter_mut().find(|e| e.name == name) {
                *slot = CarverEntry { name, kind }; // pack override (Terralith canyon)
            } else {
                registry.push(CarverEntry { name, kind }); // new id (terralith:deep_ravine)
            }
        }
        // default list = the vanilla trio in biome-JSON order (cave,
        // cave_extra_underground, canyon — the registry head order)
        let default: Vec<usize> = ["minecraft:cave", "minecraft:cave_extra_underground", "minecraft:canyon"]
            .iter()
            .filter_map(|n| registry.iter().position(|e| e.name == *n))
            .collect();
        // biome carvers lists: 26.3 map {"air": [...]} AND the legacy flat
        // array ([...] — Terralith ships flat arrays). A biome JSON WITHOUT a
        // carvers field falls through to the default trio.
        let mut per_biome: std::collections::HashMap<String, Vec<usize>> = std::collections::HashMap::new();
        for path in assets.paths() {
            let Some(rest) = path.strip_suffix(".json") else { continue };
            let Some((ns, stem)) = rest
                .split_once("/worldgen/biome/")
                .map(|(ns, f)| (ns.to_string(), f.to_string()))
            else {
                continue;
            };
            let bytes = match assets.get(path) {
                Some(b) => b,
                None => continue,
            };
            let Ok(text) = std::str::from_utf8(bytes) else { continue };
            let Ok(j) = crate::json::parse(text) else { continue };
            let list = match j.get("carvers") {
                Some(crate::json::Json::Arr(list)) => list,
                Some(crate::json::Json::Obj(_)) => match j.get("carvers").and_then(|c| c.get("air")) {
                    Some(crate::json::Json::Arr(list)) => list,
                    _ => continue, // carvers map without "air" — no air carvers
                },
                _ => continue, // no carvers field — default trio stands
            };
            let mut idxs = Vec::new();
            for c in list {
                if let crate::json::Json::Str(name) = c {
                    let name = if name.contains(':') {
                        name.clone()
                    } else {
                        format!("minecraft:{name}")
                    };
                    if let Some(i) = registry.iter().position(|e| e.name == name) {
                        idxs.push(i);
                    }
                }
            }
            per_biome.insert(format!("{ns}:{stem}"), idxs);
        }
        let enabled = std::env::var("CRUS_CARVE_IN_FILL").as_deref() == Ok("1");
        Some(CarverState {
            registry,
            per_biome,
            min_y,
            gen_depth: height,
            enabled,
            default,
            biome_memo: std::collections::HashMap::new(),
        })
    }

    fn carvers_for_biome(&self, biome_name: &str) -> &[usize] {
        match self.per_biome.get(biome_name) {
            Some(v) => v, // explicit list (possibly EMPTY — biome opted out)
            None => &self.default, // vanilla/unlisted biome — the trio
        }
    }
}

/// "<ns>/worldgen/configured_carver/<name>.json" (or the legacy
/// "<ns>/worldgen/carver/...") -> Some((ns, name)); None otherwise.
fn strip_carver_path(path: &str) -> Option<(String, String)> {
    let rest = path.strip_suffix(".json")?;
    let (ns, stem) = rest
        .split_once("/worldgen/configured_carver/")
        .or_else(|| rest.split_once("/worldgen/carver/"))?;
    Some((ns.to_string(), stem.to_string()))
}

/// One configured_carver JSON -> CarverKind (26.3 record fields; unknown
/// fields like aquifers_enabled/lava_level/replaceable are 1.21.x leftovers
/// that do not participate in the 26.3 mask+apply pipeline and are ignored).
/// Carver fields live under "config" (the configured_carver JSON shape); a
/// flat config-without-wrapper form is tolerated for probe fixtures.
fn parse_carver_kind(j: &crate::json::Json, min_y: i32, height: i32) -> Option<CarverKind> {
    let ty = gets(j, "type")?.strip_prefix("minecraft:")?.to_string();
    let cfg = j.get("config").unwrap_or(j);
    let prob = getf(cfg, "probability")? as f32;
    let yj = cfg.get("y")?;
    let y = parse_height_provider(yj, min_y, height)?;
    match ty.as_str() {
        "cave" => Some(CarverKind::Cave {
            probability: prob,
            y,
            count: parse_int_provider(cfg.get("count")?)?,
            thickness: parse_float_provider(cfg.get("thickness")?)?,
            weird_thickness_bias: cfg
                .get("weird_thickness_bias")
                .map(|v| matches!(v, crate::json::Json::Bool(true)))
                .unwrap_or(false),
            room_vertical_radius_multiplier: parse_float_provider(cfg.get("room_vertical_radius_multiplier")?)?,
            horizontal_radius_multiplier: parse_float_provider(cfg.get("horizontal_radius_multiplier")?)?,
            vertical_radius_multiplier: parse_float_provider(cfg.get("vertical_radius_multiplier")?)?,
            start_vertical_radius_multiplier: cfg
                .get("start_vertical_radius_multiplier")
                .and_then(parse_float_provider)
                .unwrap_or(FloatProvider::Constant(1.0)),
            floor_level: parse_float_provider(cfg.get("floor_level")?)?,
        }),
        "canyon" => Some(CarverKind::Canyon {
            probability: prob,
            y,
            vertical_rotation: parse_float_provider(cfg.get("vertical_rotation")?)?,
            shape: parse_canyon_shape(cfg.get("shape")?, cfg)?,
        }),
        _ => None,
    }
}

/// The three vanilla overworld carvers (biome-JSON order) — 26.3-verified
/// parameters, expressed in the legacy provider types. Mirrors
/// carver263::overworld_carvers() bit-for-bit (same probes verify both).
pub fn vanilla_trio(min_y: i32, _height: i32) -> Vec<(&'static str, CarverKind)> {
    let cave = CarverKind::Cave {
        probability: 0.15,
        y: HeightProvider::Uniform { min: min_y + 8, max: 180 },
        count: IntProvider::VeryBiasedToBottom { min: 0, max: 14 },
        thickness: FloatProvider::Trapezoid { min: 0.0, max: 3.0, plateau: 1.0 },
        weird_thickness_bias: true,
        room_vertical_radius_multiplier: FloatProvider::Uniform { min: 0.1, max: 0.9 },
        horizontal_radius_multiplier: FloatProvider::Uniform { min: 0.7, max: 1.4 },
        vertical_radius_multiplier: FloatProvider::Uniform { min: 0.8, max: 1.3 },
        start_vertical_radius_multiplier: FloatProvider::Constant(1.0),
        floor_level: FloatProvider::Uniform { min: -1.0, max: -0.4 },
    };
    let cave_extra = CarverKind::Cave {
        probability: 0.07,
        y: HeightProvider::Uniform { min: min_y + 8, max: 47 },
        count: IntProvider::VeryBiasedToBottom { min: 0, max: 14 },
        thickness: FloatProvider::Trapezoid { min: 0.0, max: 3.0, plateau: 1.0 },
        weird_thickness_bias: true,
        room_vertical_radius_multiplier: FloatProvider::Uniform { min: 0.1, max: 0.9 },
        horizontal_radius_multiplier: FloatProvider::Uniform { min: 0.7, max: 1.4 },
        vertical_radius_multiplier: FloatProvider::Uniform { min: 0.8, max: 1.3 },
        start_vertical_radius_multiplier: FloatProvider::Constant(1.0),
        floor_level: FloatProvider::Uniform { min: -1.0, max: -0.4 },
    };
    let canyon = CarverKind::Canyon {
        probability: 0.01,
        y: HeightProvider::Uniform { min: 10, max: 67 },
        vertical_rotation: FloatProvider::Uniform { min: -0.125, max: 0.125 },
        shape: CanyonShape {
            distance_factor: FloatProvider::Uniform { min: 0.75, max: 1.0 },
            thickness: FloatProvider::Trapezoid { min: 0.0, max: 6.0, plateau: 2.0 },
            width_smoothness: 3,
            horizontal_radius_factor: FloatProvider::Uniform { min: 0.75, max: 1.0 },
            vertical_radius_default_factor: 1.0,
            vertical_radius_center_factor: 0.0,
            y_scale: FloatProvider::Constant(3.0),
        },
    };
    vec![
        ("minecraft:cave", cave),
        ("minecraft:cave_extra_underground", cave_extra),
        ("minecraft:canyon", canyon),
    ]
}

// --- provider sampling (exact vanilla formulas) -----------------------------

impl HeightProvider {
    #[inline]
    fn sample(&self, r: &mut LegacyRandom) -> i32 {
        match *self {
            HeightProvider::Uniform { min, max } => min + r.next_int_bound(max - min + 1),
        }
    }
}

impl IntProvider {
    #[inline]
    fn sample(&self, r: &mut LegacyRandom) -> i32 {
        match *self {
            IntProvider::Constant(v) => v,
            IntProvider::Uniform { min, max } => min + r.next_int_bound(max - min + 1),
            // BiasedToBottomInt: min + nextInt(nextInt(range+1) + 1)
            IntProvider::BiasedToBottom { min, max } => {
                let range = max - min + 1;
                let inner = r.next_int_bound(range);
                min + r.next_int_bound(inner + 1)
            }
            // VeryBiasedToBottomInt: min + nextInt(nextInt(nextInt(range+1)+1)+1)
            // (carver263::IProv263::VeryBiasedToBottom, probe-verified)
            IntProvider::VeryBiasedToBottom { min, max } => {
                let a = r.next_int_bound(max - min + 1);
                let b = r.next_int_bound(a + 1);
                min + r.next_int_bound(b + 1)
            }
        }
    }
}

impl FloatProvider {
    #[inline]
    fn sample(&self, r: &mut LegacyRandom) -> f32 {
        match *self {
            FloatProvider::Constant(v) => v,
            // UniformFloat: min + nextFloat() * (max - min)
            FloatProvider::Uniform { min, max } => min + r.next_f32() * (max - min),
            // TrapezoidFloat: min + nextFloat()*plateauEnd + nextFloat()*plateauStart
            FloatProvider::Trapezoid { min, max, plateau } => {
                let range = max - min;
                let plateau_start = (range - plateau) / 2.0;
                let plateau_end = range - plateau_start;
                min + r.next_f32() * plateau_end + r.next_f32() * plateau_start
            }
            // ClampedNormalFloat: (float) clamp(mean + nextGaussian*dev, min, max)
            // — f64 gaussian + f64 clamp, single f32 cast
            FloatProvider::ClampedNormal { mean, dev, min, max } => {
                let v = (mean as f64) + r.next_gaussian() * (dev as f64);
                let c = if v < min as f64 {
                    min as f64
                } else if v > max as f64 {
                    max as f64
                } else {
                    v
                };
                c as f32
            }
        }
    }
}

// --- RNG: WorldgenRandom.setLargeFeatureSeed over LegacyRandomSource --------

#[inline]
fn set_large_feature_seed(r: &mut LegacyRandom, seed: i64, x: i32, z: i32) {
    r.set_seed(seed);
    let x_scale = r.next_long();
    let z_scale = r.next_long();
    let result = (x as i64).wrapping_mul(x_scale) ^ (z as i64).wrapping_mul(z_scale) ^ seed;
    r.set_seed(result);
}

#[inline]
fn can_reach(chunk_pos_x: f64, chunk_pos_z: f64, x: f64, z: f64, current_step: i32, total_steps: i32, thickness: f32) -> bool {
    let xd = x - chunk_pos_x;
    let zd = z - chunk_pos_z;
    let remaining = (total_steps - current_step) as f64;
    let rr = (thickness + 2.0f32 + 16.0f32) as f64;
    xd * xd + zd * zd - remaining * remaining <= rr * rr
}

// --- the carve pass ----------------------------------------------------------

/// Native 26.3 carve. `biome_at_source` resolves the biome name of a source
/// chunk (quart y=0); `aquifer_substance` maps (x, y, z) -> Some(native id)
/// for the carve fill (aquifer.computeSubstance with density 0.0).
/// out layout: (y - min_y) * 256 + z * 16 + x; ids per the fill table.
pub struct CarveCtx<'a> {
    pub state: &'a CarverState,
    pub seed: i64,
    /// native mat ids
    pub bedrock: Option<u16>,
    pub grass: Option<u16>,
    pub mycelium: Option<u16>,
    pub dirt: u16,
    /// runs after a carved cell is set: return the (optional) top-material id
    /// for the block below (26.3 topMaterial on dirt). None = keep dirt.
    pub top_material: Option<Box<dyn Fn(i32, i32, i32) -> Option<u16> + 'a>>,
}

#[inline]
fn idx_of(x: usize, y: usize, z: usize) -> usize {
    y * 256 + z * 16 + x
}

pub fn carve_chunk<F, G>(
    ctx: &mut CarveCtx<'_>,
    cx: i32,
    cz: i32,
    out: &mut [u16],
    mut biome_at_source: F,
    mut aquifer_substance: G,
) where
    // FnMut: the ffi tail memoizes source-chunk biomes across carves
    F: FnMut(i32, i32) -> Option<String>,
    G: FnMut(i32, i32, i32) -> Option<u16>,
{
    let min_y = ctx.state.min_y;
    let height = ctx.state.gen_depth;
    let max_y = min_y + height - 1 - 7; // protectedBlocksOnTop = 7 (fresh chunk)
    let mask_min = min_y + 1;
    let height_span = (max_y - mask_min + 1) as usize;
    if height_span == 0 || height_span > 1024 {
        return;
    }
    // column-major y-run mask: mask[(z + (x << 4)) * height_span + (y - mask_min)]
    let mut mask = vec![false; 256 * height_span];
    let mut random = LegacyRandom::new(0);

    // 26.3: range = 8 -> dx, dz in -8..=8
    for dx in -8i32..=8 {
        for dz in -8i32..=8 {
            let sx = cx + dx;
            let sz = cz + dz;
            let Some(biome) = biome_at_source(sx, sz) else { continue };
            let carvers = ctx.state.carvers_for_biome(&biome);
            for (index, ci) in carvers.iter().enumerate() {
                let entry = &ctx.state.registry[*ci];
                set_large_feature_seed(&mut random, ctx.seed + index as i64, sx, sz);
                // isStartChunk
                match &entry.kind {
                    CarverKind::Cave { probability, .. } => {
                        if random.next_f32() > *probability {
                            continue;
                        }
                    }
                    CarverKind::Canyon { probability, .. } => {
                        if random.next_f32() > *probability {
                            continue;
                        }
                    }
                }
                match &entry.kind {
                    CarverKind::Cave {
                        probability: _,
                        count,
                        y,
                        horizontal_radius_multiplier,
                        vertical_radius_multiplier,
                        start_vertical_radius_multiplier,
                        floor_level,
                        thickness,
                        weird_thickness_bias,
                        room_vertical_radius_multiplier,
                    } => {
                        let max_distance = (4 * 2 - 1) * 16; // getRange()*2-1 in blocks
                        let cave_count = count.sample(&mut random);
                        for _cave in 0..cave_count {
                            let x = (sx * 16 + random.next_int_bound(16)) as f64;
                            let y = y.sample(&mut random) as f64;
                            let z = (sz * 16 + random.next_int_bound(16)) as f64;
                            let hrm = horizontal_radius_multiplier.sample(&mut random) as f64;
                            let vrm = vertical_radius_multiplier.sample(&mut random) as f64;
                            let svrm = start_vertical_radius_multiplier.sample(&mut random) as f64;
                            let fl = floor_level.sample(&mut random) as f64;
                            let should_skip = |_xd: f64, yd: f64, _zd: f64| yd <= fl;
                            let mut tunnels = 1usize;
                            if random.next_int_bound(4) == 0 {
                                let y_scale = room_vertical_radius_multiplier.sample(&mut random) as f64;
                                let th = 1.0f32 + random.next_f32() * 6.0f32;
                                // createRoom: hRadius = 1.5 + sin(pi/2)*th; v = h*yScale
                                let hr = 1.5 + (1.5707963705062866f64.sin() * th as f64);
                                carve_ellipsoid(
                                    cx, cz, x + 1.0, y, z, hr, hr * y_scale,
                                    mask_min, height_span, &mut mask, &should_skip,
                                );
                                tunnels += random.next_int_bound(4) as usize;
                            }
                            for _t in 0..tunnels {
                                let horizontal_rotation = random.next_f32() * std::f32::consts::PI * 2.0;
                                let mut vertical_rotation = (random.next_f32() - 0.5f32) / 4.0f32;
                                let mut th = thickness.sample(&mut random);
                                if *weird_thickness_bias && random.next_int_bound(10) == 0 {
                                    th *= random.next_f32() * random.next_f32() * 3.0f32 + 1.0f32;
                                }
                                let distance = max_distance - random.next_int_bound(max_distance / 4);
                                let tunnel_seed = random.next_long();
                                cave_tunnel(
                                    cx, cz, tunnel_seed, x, y, z, hrm, vrm, th,
                                    horizontal_rotation, vertical_rotation, 0, distance,
                                    svrm, mask_min, height_span, &mut mask, &should_skip,
                                );
                            }
                        }
                    }
                    CarverKind::Canyon { probability: _, y, vertical_rotation, shape } => {
                        let max_distance = (4 * 2 - 1) * 16;
                        let x = (sx * 16 + random.next_int_bound(16)) as f64;
                        let yv = y.sample(&mut random) as f64;
                        let z = (sz * 16 + random.next_int_bound(16)) as f64;
                        let horizontal_rotation = random.next_f32() * std::f32::consts::PI * 2.0;
                        let mut vrot = vertical_rotation.sample(&mut random);
                        let y_scale = shape.y_scale.sample(&mut random) as f64;
                        let thickness = shape.thickness.sample(&mut random);
                        let distance = (max_distance as f32 * shape.distance_factor.sample(&mut random)) as i32;
                        let tunnel_seed = random.next_long();
                        canyon_tunnel(
                            cx, cz, tunnel_seed, x, yv, z, thickness,
                            horizontal_rotation, vrot, 0, distance, y_scale,
                            shape, min_y, mask_min, height_span, &mut mask,
                        );
                        let _ = &mut vrot;
                    }
                }
            }
        }
    }

    // --- applyCarvingMask ----------------------------------------------------
    // per column top->bottom: skip bedrock; state = aquifer.computeSubstance(d=0)
    for lx in 0..16usize {
        for lz in 0..16usize {
            let wx = cx * 16 + lx as i32;
            let wz = cz * 16 + lz as i32;
            let base = (lz + (lx << 4)) * height_span;
            let mut has_grass = false;
            let mut y = max_y;
            while y >= mask_min {
                let set = mask[base + (y - mask_min) as usize];
                if !set {
                    y -= 1;
                    continue;
                }
                let yi = (y - min_y) as usize;
                let cur = out[idx_of(lx, yi, lz)];
                if ctx.bedrock == Some(cur) {
                    y -= 1;
                    continue;
                }
                if ctx.grass == Some(cur) || ctx.mycelium == Some(cur) {
                    has_grass = true;
                }
                if let Some(st) = aquifer_substance(wx, y, wz) {
                    out[idx_of(lx, yi, lz)] = st;
                    if has_grass {
                        // vanilla: if the block below is dirt, re-material it
                        if yi >= 1 && out[idx_of(lx, yi - 1, lz)] == ctx.dirt {
                            if let Some(tm) = ctx.top_material.as_ref() {
                                if let Some(id) = tm(wx, y - 1, wz) {
                                    out[idx_of(lx, yi - 1, lz)] = id;
                                }
                            }
                        }
                    }
                }
                y -= 1;
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn carve_ellipsoid<S>(
    cx: i32, cz: i32,
    x: f64, y: f64, z: f64,
    h_radius: f64, v_radius: f64,
    mask_min: i32, height_span: usize,
    mask: &mut [bool],
    should_skip: &S,
) where
    S: Fn(f64, f64, f64) -> bool,
{
    let center_x = (cx * 16) as f64 + 8.0;
    let center_z = (cz * 16) as f64 + 8.0;
    let max_delta = 16.0 + h_radius * 2.0;
    if !((x - center_x).abs() > max_delta || (z - center_z).abs() > max_delta) {
        let min_x_i = cx * 16;
        let min_z_i = cz * 16;
        let min_x_idx = (((x - h_radius).floor() as i32) - min_x_i - 1).max(0).min(15) as usize;
        let max_x_idx = (((x + h_radius).floor() as i32) - min_x_i).max(0).min(15) as usize;
        let y_min = (((y - v_radius).floor() as i32) - 1).max(mask_min).min(max_y_for(mask_min, height_span));
        let y_max = (((y + v_radius).floor() as i32) + 1).max(mask_min).min(max_y_for(mask_min, height_span));
        let min_z_idx = (((z - h_radius).floor() as i32) - min_z_i - 1).max(0).min(15) as usize;
        let max_z_idx = (((z + h_radius).floor() as i32) - min_z_i).max(0).min(15) as usize;
        for x_idx in min_x_idx..=max_x_idx {
            let world_x = (cx * 16 + x_idx as i32) as f64;
            let xd = (world_x + 0.5 - x) / h_radius;
            for z_idx in min_z_idx..=max_z_idx {
                let world_z = (cz * 16 + z_idx as i32) as f64;
                let zd = (world_z + 0.5 - z) / h_radius;
                if xd * xd + zd * zd >= 1.0 {
                    continue;
                }
                let mut wy = y_max;
                while wy > y_min {
                    let yd = (wy as f64 - 0.5 - y) / v_radius;
                    if !should_skip(xd, yd, zd) {
                        let i = (wy - mask_min) as usize;
                        mask[(z_idx + (x_idx << 4)) * height_span + i] = true;
                    }
                    wy -= 1;
                }
            }
        }
    }
}

#[inline]
fn max_y_for(mask_min: i32, height_span: usize) -> i32 {
    mask_min + height_span as i32 - 1
}

/// CaveWorldCarver.createTunnel (26.3): the recursive spaghetti walk.
#[allow(clippy::too_many_arguments)]
fn cave_tunnel<S>(
    cx: i32, cz: i32,
    tunnel_seed: i64,
    mut x: f64, mut y: f64, mut z: f64,
    hrm: f64, vrm: f64,
    thickness: f32,
    mut horizontal_rotation: f32,
    mut vertical_rotation: f32,
    step: i32, dist: i32,
    y_scale: f64,
    mask_min: i32, height_span: usize,
    mask: &mut [bool],
    should_skip: &S,
) where
    S: Fn(f64, f64, f64) -> bool,
{
    let mut random = LegacyRandom::new(tunnel_seed);
    let split_point = random.next_int_bound(dist / 2) + dist / 4;
    let steep = random.next_int_bound(6) == 0;
    let mut y_rota = 0.0f32;
    let mut x_rota = 0.0f32;
    let center_x = (cx * 16) as f64 + 8.0;
    let center_z = (cz * 16) as f64 + 8.0;
    let mut current_step = step;
    while current_step < dist {
        let h_radius = 1.5 + (((current_step as f32) * std::f32::consts::PI / dist as f32).sin() * thickness) as f64;
        let v_radius = h_radius * y_scale;
        let cos_x = vertical_rotation.cos();
        x += (horizontal_rotation.cos() * cos_x) as f64;
        y += vertical_rotation.sin() as f64;
        z += (horizontal_rotation.sin() * cos_x) as f64;
        vertical_rotation *= if steep { 0.92 } else { 0.7 };
        vertical_rotation += x_rota * 0.1;
        horizontal_rotation += y_rota * 0.1;
        x_rota *= 0.9;
        y_rota *= 0.75;
        x_rota += (random.next_f32() - random.next_f32()) * random.next_f32() * 2.0;
        y_rota += (random.next_f32() - random.next_f32()) * random.next_f32() * 4.0;
        if current_step == split_point && thickness > 1.0 {
            let th1 = random.next_f32() * 0.5 + 0.5;
            let seed1 = random.next_long();
            let seed2 = random.next_long();
            cave_tunnel(cx, cz, seed1, x, y, z, hrm, vrm, th1,
                horizontal_rotation - 1.5707964, vertical_rotation / 3.0, current_step, dist,
                1.0, mask_min, height_span, mask, should_skip);
            cave_tunnel(cx, cz, seed2, x, y, z, hrm, vrm, th1,
                horizontal_rotation + 1.5707964, vertical_rotation / 3.0, current_step, dist,
                1.0, mask_min, height_span, mask, should_skip);
            return;
        }
        if random.next_int_bound(4) != 0 {
            if !can_reach(center_x, center_z, x, z, current_step, dist, thickness) {
                return;
            }
            carve_ellipsoid(cx, cz, x, y, z, h_radius * hrm, v_radius * vrm,
                mask_min, height_span, mask, should_skip);
        }
        current_step += 1;
    }
}

/// CanyonWorldCarver.doCarve (26.3).
#[allow(clippy::too_many_arguments)]
fn canyon_tunnel(
    cx: i32, cz: i32,
    tunnel_seed: i64,
    mut x: f64, mut y: f64, mut z: f64,
    thickness: f32,
    mut horizontal_rotation: f32,
    mut vertical_rotation: f32,
    step: i32, distance: i32,
    y_scale: f64,
    shape: &CanyonShape,
    min_y: i32,
    mask_min: i32, height_span: usize,
    mask: &mut [bool],
) {
    let mut random = LegacyRandom::new(tunnel_seed);
    // initWidthFactors over the generation depth; the width factors only
    // scale the canyon silhouette, and vanilla indexes them by (y - minGenY)
    // clamped to our mask span — size by the mask span + margin.
    let gen_depth = (height_span + 8) as usize;
    let mut width_factor_per_height = vec![1.0f32; gen_depth];
    let mut width_factor = 1.0f32;
    for yi in 0..gen_depth {
        if yi == 0 || random.next_int_bound(shape.width_smoothness) == 0 {
            width_factor = 1.0 + random.next_f32() * random.next_f32();
        }
        width_factor_per_height[yi] = width_factor * width_factor;
    }
    let mut y_rota = 0.0f32;
    let mut x_rota = 0.0f32;
    let center_x = (cx * 16) as f64 + 8.0;
    let center_z = (cz * 16) as f64 + 8.0;
    let mut current_step = step;
    while current_step < distance {
        let mut h_radius = 1.5 + (((current_step as f32) * std::f32::consts::PI / distance as f32).sin() * thickness) as f64;
        let mut v_radius = h_radius * y_scale;
        h_radius *= shape.horizontal_radius_factor.sample(&mut random) as f64;
        // updateVerticalRadius
        let vertical_multiplier = 1.0 - ((0.5 - current_step as f32 / distance as f32).abs()) * 2.0;
        let factor = shape.vertical_radius_default_factor + shape.vertical_radius_center_factor * vertical_multiplier;
        // Mth.randomBetween(random, 0.75, 1.0)
        let between = 0.75f32 + random.next_f32() * (1.0f32 - 0.75f32);
        v_radius = (factor * v_radius as f32 * between) as f64;
        let xc = vertical_rotation.cos();
        let xs = vertical_rotation.sin();
        x += (horizontal_rotation.cos() * xc) as f64;
        y += xs as f64;
        z += (horizontal_rotation.sin() * xc) as f64;
        vertical_rotation *= 0.7;
        vertical_rotation += x_rota * 0.05;
        horizontal_rotation += y_rota * 0.05;
        x_rota *= 0.8;
        y_rota *= 0.5;
        x_rota += (random.next_f32() - random.next_f32()) * random.next_f32() * 2.0;
        y_rota += (random.next_f32() - random.next_f32()) * random.next_f32() * 4.0;
        if random.next_int_bound(4) != 0 {
            if !can_reach(center_x, center_z, x, z, current_step, distance, thickness) {
                return;
            }
            let should_skip = |xd: f64, _yd: f64, zd: f64, wy: i32| -> bool {
                let y_index = (wy - min_y).max(1) as usize;
                let wf = width_factor_per_height.get(y_index - 1).copied().unwrap_or(1.0) as f64;
                (xd * xd + zd * zd) * wf + _yd * _yd / 6.0 >= 1.0
            };
            // carve with canyon skip checker (yd shadows: pass raw yd)
            carve_ellipsoid_canyon(cx, cz, x, y, z, h_radius, v_radius,
                mask_min, height_span, mask, &should_skip);
        }
        current_step += 1;
    }
}

#[allow(clippy::too_many_arguments)]
fn carve_ellipsoid_canyon<S>(
    cx: i32, cz: i32,
    x: f64, y: f64, z: f64,
    h_radius: f64, v_radius: f64,
    mask_min: i32, height_span: usize,
    mask: &mut [bool],
    should_skip: &S,
) where
    S: Fn(f64, f64, f64, i32) -> bool,
{
    let center_x = (cx * 16) as f64 + 8.0;
    let center_z = (cz * 16) as f64 + 8.0;
    let max_delta = 16.0 + h_radius * 2.0;
    if (x - center_x).abs() > max_delta || (z - center_z).abs() > max_delta {
        return;
    }
    let min_x_i = cx * 16;
    let min_z_i = cz * 16;
    let min_x_idx = (((x - h_radius).floor() as i32) - min_x_i - 1).max(0).min(15) as usize;
    let max_x_idx = (((x + h_radius).floor() as i32) - min_x_i).max(0).min(15) as usize;
    let y_min = (((y - v_radius).floor() as i32) - 1).max(mask_min).min(max_y_for(mask_min, height_span));
    let y_max = (((y + v_radius).floor() as i32) + 1).max(mask_min).min(max_y_for(mask_min, height_span));
    let min_z_idx = (((z - h_radius).floor() as i32) - min_z_i - 1).max(0).min(15) as usize;
    let max_z_idx = (((z + h_radius).floor() as i32) - min_z_i).max(0).min(15) as usize;
    for x_idx in min_x_idx..=max_x_idx {
        let world_x = (cx * 16 + x_idx as i32) as f64;
        let xd = (world_x + 0.5 - x) / h_radius;
        for z_idx in min_z_idx..=max_z_idx {
            let world_z = (cz * 16 + z_idx as i32) as f64;
            let zd = (world_z + 0.5 - z) / h_radius;
            if xd * xd + zd * zd >= 1.0 {
                continue;
            }
            let mut wy = y_max;
            while wy > y_min {
                let yd = (wy as f64 - 0.5 - y) / v_radius;
                if !should_skip(xd, yd, zd, wy) {
                    let i = (wy - mask_min) as usize;
                    mask[(z_idx + (x_idx << 4)) * height_span + i] = true;
                }
                wy -= 1;
            }
        }
    }
}

// silence unused import warning for Env (reserved for topMaterial DF wiring)
#[allow(dead_code)]
fn _unused(_e: &Env) {
    let _ = Mode::Direct;
    let _ = Pt { x: 0, y: 0, z: 0 };
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::parse;

    /// Terralith's minecraft:canyon override (real pack bytes, format-aware
    /// fields: top-level yScale, 1.21.x lava_level/replaceable leftovers).
    const TERRALITH_CANYON: &str = r##"{
        "type": "minecraft:canyon",
        "config": {
            "probability": 0.003,
            "y": {"type": "minecraft:uniform", "min_inclusive": {"absolute": -20}, "max_inclusive": {"absolute": 70}},
            "yScale": {"type": "minecraft:clamped_normal", "min": 2.5, "max": 13.5, "mean": 3.5, "deviation": 2.5},
            "lava_level": {"above_bottom": 8},
            "replaceable": "#minecraft:overworld_carver_replaceables",
            "vertical_rotation": {"type": "minecraft:uniform", "min_inclusive": -0.2, "max_exclusive": 0.2},
            "shape": {
                "distance_factor": {"type": "minecraft:uniform", "min_inclusive": 0.75, "max_exclusive": 1.25},
                "thickness": {"type": "minecraft:trapezoid", "min": 0, "max": 8, "plateau": 2},
                "width_smoothness": 2,
                "horizontal_radius_factor": {"type": "minecraft:uniform", "min_inclusive": 0.75, "max_exclusive": 1},
                "vertical_radius_default_factor": 1,
                "vertical_radius_center_factor": 0
            }
        }
    }"##;

    /// Terralith deep_ravine — bare-number yScale (legacy ConstantFloat).
    const DEEP_RAVINE: &str = r##"{
        "type": "minecraft:canyon",
        "config": {
            "aquifers_enabled": true,
            "lava_level": {"above_bottom": 8},
            "probability": 0,
            "replaceable": "#minecraft:overworld_carver_replaceables",
            "shape": {
                "distance_factor": {"type": "minecraft:uniform", "min_inclusive": 0.75, "max_exclusive": 1.25},
                "thickness": {"type": "minecraft:trapezoid", "max": 12, "min": 5, "plateau": 2},
                "width_smoothness": 3,
                "horizontal_radius_factor": {"type": "minecraft:uniform", "min_inclusive": 0.75, "max_exclusive": 1},
                "vertical_radius_center_factor": 0,
                "vertical_radius_default_factor": 1
            },
            "vertical_rotation": {"type": "minecraft:uniform", "min_inclusive": -1, "max_exclusive": 1},
            "y": {"type": "minecraft:uniform", "min_inclusive": {"absolute": -30}, "max_inclusive": {"absolute": 70}},
            "yScale": 13
        }
    }"##;

    fn assets_with() -> crate::worldgen::Assets {
        let mut a = crate::worldgen::Assets::new();
        a.insert(
            // the real pack path: Terralith overrides the VANILLA-namespace
            // carver (data/minecraft/worldgen/configured_carver/canyon.json)
            "minecraft/worldgen/configured_carver/canyon.json",
            TERRALITH_CANYON.as_bytes().to_vec(),
        );
        a.insert(
            "terralith/worldgen/configured_carver/deep_ravine.json",
            DEEP_RAVINE.as_bytes().to_vec(),
        );
        a.insert(
            "terralith/worldgen/biome/alpha_islands.json",
            br#"{"carvers": ["minecraft:cave", "minecraft:cave_extra_underground"]}"#.to_vec(),
        );
        a.insert(
            "terralith/worldgen/biome/vanilla_style.json",
            br#"{"carvers": {"air": ["minecraft:cave", "minecraft:cave_extra_underground", "minecraft:canyon"]}}"#.to_vec(),
        );
        a
    }

    #[test]
    fn vanilla_registry_seeded_without_assets() {
        let st = CarverState::from_assets(&crate::worldgen::Assets::new(), -64, 384)
            .expect("trio registry always present");
        assert_eq!(st.registry.len(), 3);
        assert_eq!(st.registry[0].name, "minecraft:cave");
        assert_eq!(st.registry[1].name, "minecraft:cave_extra_underground");
        assert_eq!(st.registry[2].name, "minecraft:canyon");
        assert_eq!(st.default, vec![0, 1, 2]);
        assert!(st.per_biome.is_empty());
        // unlisted biome falls back to the trio
        assert_eq!(st.carvers_for_biome("minecraft:plains"), &[0, 1, 2]);
    }

    #[test]
    fn pack_override_and_new_entries() {
        let st = CarverState::from_assets(&assets_with(), -64, 384).expect("registry");
        assert_eq!(st.registry.len(), 4, "canyon replaced, deep_ravine appended");
        let can = match &st.registry[2].kind {
            CarverKind::Canyon {
                probability,
                y,
                vertical_rotation,
                shape,
            } => (probability, y, vertical_rotation, shape),
            other => panic!("expected canyon, got {other:?}"),
        };
        assert!((can.0 - 0.003).abs() < 1e-6);
        match can.1 {
            HeightProvider::Uniform { min, max } => {
                assert_eq!((*min, *max), (-20, 70));
            }
        }
        match can.3.thickness {
            FloatProvider::Trapezoid { min, max, plateau } => {
                assert_eq!((min, max, plateau), (0.0, 8.0, 2.0));
            }
            other => panic!("expected trapezoid thickness, got {other:?}"),
        }
        assert_eq!(can.3.width_smoothness, 2);
        match can.3.y_scale {
            FloatProvider::ClampedNormal { mean, dev, min, max } => {
                assert_eq!((mean, dev, min, max), (3.5, 2.5, 2.5, 13.5));
            }
            other => panic!("expected clamped_normal yScale, got {other:?}"),
        }
        match can.2 {
            FloatProvider::Uniform { min, max } => assert_eq!((*min, *max), (-0.2, 0.2)),
            other => panic!("expected uniform vertical_rotation, got {other:?}"),
        }
        // deep_ravine: bare-number yScale -> Constant(13)
        let dr = match &st.registry[3].kind {
            CarverKind::Canyon { probability, shape, .. } => (probability, shape),
            other => panic!("expected canyon, got {other:?}"),
        };
        assert!(*dr.0 == 0.0);
        match dr.1.y_scale {
            FloatProvider::Constant(v) => assert_eq!(v, 13.0),
            other => panic!("expected constant yScale, got {other:?}"),
        }
    }

    #[test]
    fn biome_lists_flat_and_map() {
        let st = CarverState::from_assets(&assets_with(), -64, 384).expect("registry");
        // alpha_islands opts out of the canyon (flat legacy array)
        assert_eq!(st.carvers_for_biome("terralith:alpha_islands"), &[0, 1]);
        // map {"air": [...]} format resolves all three
        assert_eq!(st.carvers_for_biome("terralith:vanilla_style"), &[0, 1, 2]);
        // biome JSON without a carvers field -> default trio
        assert_eq!(st.carvers_for_biome("terralith:unlisted"), &[0, 1, 2]);
    }

    #[test]
    fn very_biased_to_bottom_matches_carver263() {
        // triple-nest parity with carver263::IProv263::VeryBiasedToBottom
        let mut a = LegacyRandom::new(42);
        let mut b = LegacyRandom::new(42);
        for _ in 0..64 {
            let lo = 0i32;
            let hi = 14i32;
            let want = {
                let x = b.next_int_bound(hi - lo + 1);
                let y = b.next_int_bound(x + 1);
                lo + b.next_int_bound(y + 1)
            };
            let got = IntProvider::VeryBiasedToBottom { min: lo, max: hi }.sample(&mut a);
            assert_eq!(got, want);
            assert!((0..=14).contains(&got));
        }
    }

    #[test]
    fn clamped_normal_formula() {
        // (float) clamp(mean + nextGaussian * dev, min, max) — f64 path
        let mut r = LegacyRandom::new(7);
        let mut r2 = LegacyRandom::new(7);
        for _ in 0..32 {
            let g = r2.next_gaussian();
            let want = {
                let v = 3.5f64 + g * 2.5f64;
                let c = v.clamp(2.5f64, 13.5f64);
                c as f32
            };
            let got = FloatProvider::ClampedNormal { mean: 3.5, dev: 2.5, min: 2.5, max: 13.5 }
                .sample(&mut r);
            assert_eq!(got, want);
        }
    }

    #[test]
    fn parse_rejects_unknown_types() {
        let j = parse(r#"{"type": "minecraft:emergent", "config": {"probability": 1.0}}"#).unwrap();
        assert!(parse_carver_kind(&j, -64, 384).is_none());
    }
}
