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
    BiasedToBottom { min: i32, max: i32 },
}

#[derive(Clone, Copy, Debug)]
pub enum FloatProvider {
    Constant(f32),
    Uniform { min: f32, max: f32 },
    Trapezoid { min: f32, max: f32, plateau: f32 },
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
    /// biome name -> list of carver registry indexes (JSON list order)
    pub per_biome: Vec<(String, Vec<usize>)>,
    pub min_y: i32,
    pub gen_depth: i32,
}

fn getf(j: &crate::json::Json, k: &str) -> Option<f64> {
    j.get(k).and_then(|v| v.as_f64())
}
fn gets<'a>(j: &'a crate::json::Json, k: &str) -> Option<&'a str> {
    j.get(k).and_then(|v| v.as_str())
}

fn parse_float_provider(j: &crate::json::Json) -> Option<FloatProvider> {
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
        _ => None,
    }
}

fn parse_int_provider(j: &crate::json::Json) -> Option<IntProvider> {
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
        "very_biased_to_bottom" | "biased_to_bottom" => Some(IntProvider::BiasedToBottom {
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

fn parse_canyon_shape(j: &crate::json::Json) -> Option<CanyonShape> {
    Some(CanyonShape {
        distance_factor: parse_float_provider(j.get("distance_factor")?)?,
        thickness: parse_float_provider(j.get("thickness")?)?,
        width_smoothness: j.get("width_smoothness").and_then(|v| v.as_f64())? as i32,
        horizontal_radius_factor: parse_float_provider(j.get("horizontal_radius_factor")?)?,
        vertical_radius_default_factor: getf(j, "vertical_radius_default_factor")? as f32,
        vertical_radius_center_factor: getf(j, "vertical_radius_center_factor")? as f32,
        y_scale: parse_float_provider(j.get("y_scale")?)?,
    })
}

impl CarverState {
    /// Parse the 26.3 carver registry + biome carver lists from assets.
    pub fn from_assets(assets: &crate::worldgen::Assets, min_y: i32, height: i32) -> Option<CarverState> {
        let mut registry: Vec<CarverEntry> = Vec::new();
        for path in assets.paths() {
            let Some(rest) = path.strip_prefix("minecraft/worldgen/carver/") else { continue };
            let Some(stem) = rest.strip_suffix(".json") else { continue };
            let bytes = assets.get(path)?;
            let Ok(text) = std::str::from_utf8(bytes) else { continue };
            let Ok(j) = crate::json::parse(text) else { continue };
            let ty = gets(&j, "type")?.strip_prefix("minecraft:")?.to_string();
            let prob = getf(&j, "probability")? as f32;
            let yj = j.get("y")?;
            let y = parse_height_provider(yj, min_y, height)?;
            let kind = match ty.as_str() {
                "cave" => CarverKind::Cave {
                    probability: prob,
                    y,
                    count: parse_int_provider(j.get("count")?)?,
                    thickness: parse_float_provider(j.get("thickness")?)?,
                    weird_thickness_bias: j.get("weird_thickness_bias").map(|v| matches!(v, crate::json::Json::Bool(true))).unwrap_or(false),
                    room_vertical_radius_multiplier: parse_float_provider(j.get("room_vertical_radius_multiplier")?)?,
                    horizontal_radius_multiplier: parse_float_provider(j.get("horizontal_radius_multiplier")?)?,
                    vertical_radius_multiplier: parse_float_provider(j.get("vertical_radius_multiplier")?)?,
                    start_vertical_radius_multiplier: j
                        .get("start_vertical_radius_multiplier")
                        .and_then(parse_float_provider)
                        .unwrap_or(FloatProvider::Constant(1.0)),
                    floor_level: parse_float_provider(j.get("floor_level")?)?,
                },
                "canyon" => CarverKind::Canyon {
                    probability: prob,
                    y,
                    vertical_rotation: parse_float_provider(j.get("vertical_rotation")?)?,
                    shape: parse_canyon_shape(j.get("shape")?)?,
                },
                _ => continue,
            };
            registry.push(CarverEntry { name: format!("minecraft:{stem}"), kind });
        }
        if registry.is_empty() {
            return None;
        }
        // biome carvers lists (26.3: flat "carvers": [...])
        let mut per_biome = Vec::new();
        for path in assets.paths() {
            let Some(rest) = path.strip_prefix("minecraft/worldgen/biome/") else { continue };
            let Some(stem) = rest.strip_suffix(".json") else { continue };
            let bytes = match assets.get(path) {
                Some(b) => b,
                None => continue,
            };
            let Ok(text) = std::str::from_utf8(bytes) else { continue };
            let Ok(j) = crate::json::parse(text) else { continue };
            let Some(crate::json::Json::Arr(list)) = j.get("carvers") else { continue };
            let mut idxs = Vec::new();
            for c in list {
                if let crate::json::Json::Str(name) = c {
                    let name = if name.contains(':') { name.clone() } else { format!("minecraft:{name}") };
                    if let Some(i) = registry.iter().position(|e| e.name == name) {
                        idxs.push(i);
                    }
                }
            }
            per_biome.push((format!("minecraft:{stem}"), idxs));
        }
        Some(CarverState {
            registry,
            per_biome,
            min_y,
            gen_depth: height,
        })
    }

    fn carvers_for_biome(&self, biome_name: &str) -> &[usize] {
        static EMPTY: [usize; 0] = [];
        self.per_biome
            .iter()
            .find(|(n, _)| n == biome_name)
            .map(|(_, v)| v.as_slice())
            .unwrap_or(&EMPTY)
    }
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
    biome_at_source: F,
    mut aquifer_substance: G,
) where
    F: Fn(i32, i32) -> Option<String>,
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
