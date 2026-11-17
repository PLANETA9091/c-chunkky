// Native carver takeover: vanilla cave + canyon carvers, bit-exact.
//
// Mirrors NoiseBasedChunkGenerator.applyCarvers + WorldCarver /
// CaveWorldCarver / CanyonWorldCarver (paper 1.21.8, CFR-decompiled).
// The plugin flips shouldGenerateCaves() when the ctx carries a CarveState;
// the carve pass runs inside the native fill right after the surface tail
// (vanilla order: NOISE -> SURFACE -> CARVERS).
//
// Parity notes (verified against the decompile):
//   - WorldgenRandom = LegacyRandomSource (java.util.Random LCG);
//     setLargeFeatureSeed(seed + carverIndex, neighborX, neighborZ).
//   - Carvers iterate a hardcoded -8..=8 chunk neighborhood; the NEIGHBOR
//     chunk's biome (quart (minX>>2, 0, minZ>>2)) picks the carver list;
//     carving writes into the CENTER chunk with the CENTER chunk's aquifer.
//   - Mth.sin/cos use the 65536-entry f32 table.
//   - All provider sampling is f32 exactly like vanilla (FloatProvider).
//   - carveBlock: aquifer.computeSubstance(SinglePointContext, 0.0), lava
//     shortcut at y <= lavaLevel, shouldScheduleFluidUpdate persists across
//     calls (stale value reused when the lava shortcut skips the aquifer),
//     and the grass->DIRT topMaterial rewrite below the carved block.

use crate::json::Json;
use crate::rng::LegacyRandom;
use crate::worldgen::Assets;
use std::collections::{HashMap, HashSet};

// ---------------------------------------------------------------------------
// Mth helpers (f32 table parity)
// ---------------------------------------------------------------------------

const SIN_SCALE: f32 = 10430.378f32;
const PI_F32: f32 = 3.1415927f32; // (float)Math.PI
const HALF_PI: f32 = 1.5707964f32; // vanilla literal in createRoom
const TWO_PI_F32: f32 = PI_F32 * 2.0f32; // (float)Math.PI * 2 = 6.2831855f

fn sin_table() -> &'static [f32; 65536] {
    use std::sync::OnceLock;
    static T: OnceLock<Box<[f32; 65536]>> = OnceLock::new();
    T.get_or_init(|| {
        let mut t = Box::new([0f32; 65536]);
        let mut i = 0usize;
        while i < 65536 {
            t[i] = ((i as f64) * std::f64::consts::PI * 2.0 / 65536.0).sin() as f32;
            i += 1;
        }
        t
    })
}

#[inline]
fn mth_sin(v: f32) -> f32 {
    sin_table()[((v * SIN_SCALE) as i32 & 0xFFFF) as usize]
}

#[inline]
fn mth_cos(v: f32) -> f32 {
    sin_table()[((v * SIN_SCALE + 16384.0f32) as i32 & 0xFFFF) as usize]
}

#[inline]
fn mth_floor(d: f64) -> i32 {
    let i = d as i32;
    if d < i as f64 {
        i - 1
    } else {
        i
    }
}

#[inline]
fn mth_abs_f(v: f32) -> f32 {
    if v < 0.0 {
        -v
    } else {
        v
    }
}

/// Mth.nextInt(random, min, max) — inclusive.
#[inline]
fn mth_next_int(rng: &mut LegacyRandom, min: i32, max: i32) -> i32 {
    if min >= max {
        min
    } else {
        rng.next_int_bound(max - min + 1) + min
    }
}

/// Mth.randomBetween(random, min, max) — f32.
#[inline]
fn mth_random_between(rng: &mut LegacyRandom, min: f32, max: f32) -> f32 {
    rng.next_f32() * (max - min) + min
}

/// Mth.randomBetweenInclusive.
#[inline]
fn mth_between_inclusive(rng: &mut LegacyRandom, min: i32, max: i32) -> i32 {
    rng.next_int_bound(max - min + 1) + min
}

/// WorldgenRandom.setLargeFeatureSeed(baseSeed, x, z).
fn set_large_feature_seed(rng: &mut LegacyRandom, base: i64, x: i32, z: i32) {
    rng.set_seed(base);
    let l1 = rng.next_long();
    let l2 = rng.next_long();
    rng.set_seed((x as i64).wrapping_mul(l1) ^ (z as i64).wrapping_mul(l2) ^ base);
}

// ---------------------------------------------------------------------------
// Vertical anchors / float providers / height providers
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
enum VAnchor {
    Absolute(i32),
    AboveBottom(i32),
    BelowTop(i32),
}

impl VAnchor {
    fn parse(j: &Json) -> Option<Self> {
        if !j.is_obj() {
            return None;
        }
        let obj = j;
        if let Some(v) = obj.get("absolute").and_then(|v| v.as_i32()) {
            return Some(VAnchor::Absolute(v));
        }
        if let Some(v) = obj.get("above_bottom").and_then(|v| v.as_i32()) {
            return Some(VAnchor::AboveBottom(v));
        }
        if let Some(v) = obj.get("below_top").and_then(|v| v.as_i32()) {
            return Some(VAnchor::BelowTop(v));
        }
        None
    }
    #[inline]
    fn resolve_y(&self, min_gen_y: i32, gen_depth: i32) -> i32 {
        match self {
            VAnchor::Absolute(v) => *v,
            VAnchor::AboveBottom(v) => min_gen_y + v,
            VAnchor::BelowTop(v) => gen_depth - 1 + min_gen_y - v,
        }
    }
}

#[derive(Debug, Clone)]
enum FProv {
    Uniform(f32, f32),
    Trapezoid { min: f32, max: f32, plateau: f32 },
    ClampedNormal { min: f32, max: f32, mean: f32, dev: f32 },
    Constant(f32),
}

impl FProv {
    fn parse(j: &Json) -> Option<Self> {
        match j {
            // bare number => ConstantFloat
            Json::Num(n) => Some(FProv::Constant(*n as f32)),
            Json::Obj(_) => {
                let t = j.get("type").and_then(|v| v.as_str()).unwrap_or("");
                match t {
                    "minecraft:uniform" => Some(FProv::Uniform(
                        j.get("min_inclusive")?.as_f64()? as f32,
                        j.get("max_exclusive")?.as_f64()? as f32,
                    )),
                    "minecraft:trapezoid" => Some(FProv::Trapezoid {
                        min: j.get("min")?.as_f64()? as f32,
                        max: j.get("max")?.as_f64()? as f32,
                        plateau: j.get("plateau")?.as_f64()? as f32,
                    }),
                    "minecraft:clamped_normal" => Some(FProv::ClampedNormal {
                        min: j.get("min")?.as_f64()? as f32,
                        max: j.get("max")?.as_f64()? as f32,
                        mean: j.get("mean")?.as_f64()? as f32,
                        dev: j.get("deviation")?.as_f64()? as f32,
                    }),
                    "minecraft:constant" => {
                        Some(FProv::Constant(j.get("value")?.as_f64()? as f32))
                    }
                    _ => {
                        // typeless constant object {"value": n}
                        j.get("value")
                            .and_then(|v| v.as_f64())
                            .map(|v| FProv::Constant(v as f32))
                    }
                }
            }
            _ => None,
        }
    }

    /// FloatProvider.sample(RandomSource) -> float (f32 semantics).
    #[inline]
    fn sample(&self, rng: &mut LegacyRandom) -> f32 {
        match self {
            FProv::Uniform(a, b) => mth_random_between(rng, *a, *b),
            FProv::Constant(v) => *v,
            FProv::Trapezoid { min, max, plateau } => {
                let f = max - min;
                let f1 = (f - plateau) / 2.0f32;
                let f2 = f - f1;
                min + rng.next_f32() * f2 + rng.next_f32() * f1
            }
            FProv::ClampedNormal { min, max, mean, dev } => {
                // Mth.normal: mean + (float)random.nextGaussian() * deviation
                let g = rng.next_gaussian();
                let v = mean + (g as f32) * dev;
                if v < *min {
                    *min
                } else if v > *max {
                    *max
                } else {
                    v
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
enum HProv {
    Uniform(VAnchor, VAnchor),
    Trapezoid(VAnchor, VAnchor, i32),
    BiasedToBottom(VAnchor, VAnchor, i32),
    VeryBiasedToBottom(VAnchor, VAnchor, i32),
    Constant(VAnchor),
}

impl HProv {
    fn parse(j: &Json) -> Option<Self> {
        if !j.is_obj() {
            return None;
        }
        let obj = j;
        let t = obj.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match t {
            "minecraft:uniform" => Some(HProv::Uniform(
                VAnchor::parse(obj.get("min_inclusive")?)?,
                VAnchor::parse(obj.get("max_inclusive")?)?,
            )),
            "minecraft:trapezoid" => Some(HProv::Trapezoid(
                VAnchor::parse(obj.get("min_inclusive")?)?,
                VAnchor::parse(obj.get("max_inclusive")?)?,
                obj.get("plateau").and_then(|v| v.as_i32()).unwrap_or(1),
            )),
            "minecraft:biased_to_bottom" => Some(HProv::BiasedToBottom(
                VAnchor::parse(obj.get("min_inclusive")?)?,
                VAnchor::parse(obj.get("max_inclusive")?)?,
                obj.get("inner").and_then(|v| v.as_i32()).unwrap_or(1),
            )),
            "minecraft:very_biased_to_bottom" => Some(HProv::VeryBiasedToBottom(
                VAnchor::parse(obj.get("min_inclusive")?)?,
                VAnchor::parse(obj.get("max_inclusive")?)?,
                obj.get("inner").and_then(|v| v.as_i32()).unwrap_or(1),
            )),
            "minecraft:constant" => Some(HProv::Constant(VAnchor::parse(obj.get("value")?)?)),
            _ => {
                // typeless: {"value": anchor} => ConstantHeight
                obj.get("value").and_then(VAnchor::parse).map(HProv::Constant)
            }
        }
    }

    /// HeightProvider.sample(RandomSource, WorldGenerationContext) -> int.
    #[inline]
    fn sample(&self, rng: &mut LegacyRandom, min_gen_y: i32, gen_depth: i32) -> i32 {
        let lo_hi = |a: &VAnchor, b: &VAnchor| {
            (a.resolve_y(min_gen_y, gen_depth), b.resolve_y(min_gen_y, gen_depth))
        };
        match self {
            HProv::Constant(a) => a.resolve_y(min_gen_y, gen_depth),
            HProv::Uniform(a, b) => {
                let (i, i1) = lo_hi(a, b);
                if i > i1 {
                    return i;
                }
                mth_between_inclusive(rng, i, i1)
            }
            HProv::Trapezoid(a, b, plateau) => {
                let (i, i1) = lo_hi(a, b);
                if i > i1 {
                    return i;
                }
                let i2 = i1 - i;
                if *plateau >= i2 {
                    return mth_between_inclusive(rng, i, i1);
                }
                let i3 = (i2 - plateau) / 2;
                let i4 = i2 - i3;
                i + mth_between_inclusive(rng, 0, i4) + mth_between_inclusive(rng, 0, i3)
            }
            HProv::BiasedToBottom(a, b, inner) => {
                let (i, i1) = lo_hi(a, b);
                if i1 - i - inner + 1 <= 0 {
                    return i;
                }
                let ri = rng.next_int_bound(i1 - i - inner + 1);
                rng.next_int_bound(ri + inner) + i
            }
            HProv::VeryBiasedToBottom(a, b, inner) => {
                let (i, i1) = lo_hi(a, b);
                if i1 - i - inner + 1 <= 0 {
                    return i;
                }
                let ri = mth_next_int(rng, i + inner, i1);
                let ri1 = mth_next_int(rng, i, ri - 1);
                mth_next_int(rng, i, ri1 - 1 + inner)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Carver configuration (configured_carver JSON)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct CanyonShape {
    distance_factor: FProv,
    thickness: FProv,
    width_smoothness: i32,
    horizontal_radius_factor: FProv,
    vertical_radius_default_factor: f32,
    vertical_radius_center_factor: f32,
}

#[derive(Debug, Clone)]
enum Kind {
    Cave {
        horizontal_radius_multiplier: FProv,
        vertical_radius_multiplier: FProv,
        floor_level: FProv,
    },
    Canyon {
        vertical_rotation: FProv,
        shape: CanyonShape,
    },
}

#[derive(Debug, Clone)]
struct CarverCfg {
    probability: f32,
    y: HProv,
    y_scale: FProv,
    lava_level: VAnchor,
    kind: Kind,
}

fn fp(j: &Json, key: &str) -> Option<FProv> {
    FProv::parse(j.get(key)?)
}

impl CarverCfg {
    fn parse(j: &Json) -> Option<Self> {
        let cfg = j.get("config")?;
        let typ = j.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let probability = cfg.get("probability").and_then(|v| v.as_f64())? as f32;
        let y = HProv::parse(cfg.get("y")?)?;
        let y_scale = fp(cfg, "yScale")?;
        let lava_level = VAnchor::parse(cfg.get("lava_level")?)?;
        let kind = match typ {
            "minecraft:cave" => Kind::Cave {
                horizontal_radius_multiplier: fp(cfg, "horizontal_radius_multiplier")?,
                vertical_radius_multiplier: fp(cfg, "vertical_radius_multiplier")?,
                floor_level: fp(cfg, "floor_level")?,
            },
            "minecraft:canyon" => {
                let shape = cfg.get("shape")?;
                Kind::Canyon {
                    vertical_rotation: fp(cfg, "vertical_rotation")?,
                    shape: CanyonShape {
                        distance_factor: fp(shape, "distance_factor")?,
                        thickness: fp(shape, "thickness")?,
                        width_smoothness: shape
                            .get("width_smoothness")
                            .and_then(|v| v.as_i32())
                            .unwrap_or(2),
                        horizontal_radius_factor: fp(shape, "horizontal_radius_factor")?,
                        vertical_radius_default_factor: shape
                            .get("vertical_radius_default_factor")
                            .and_then(|v| v.as_f64())
                            .unwrap_or(1.0) as f32,
                        vertical_radius_center_factor: shape
                            .get("vertical_radius_center_factor")
                            .and_then(|v| v.as_f64())
                            .unwrap_or(0.0) as f32,
                    },
                }
            }
            _ => return None, // nether_cave etc — overworld engine skips
        };
        Some(CarverCfg { probability, y, y_scale, lava_level, kind })
    }
}

// ---------------------------------------------------------------------------
// CarveState — per-ctx assets + scratch
// ---------------------------------------------------------------------------

/// Per-block classification cache entry (name-matched against the tag set).
#[derive(Debug, Clone, Copy)]
struct Blk {
    replaceable: bool,
    grass: bool,
    mycelium: bool,
    dirt: bool,
    fluid: bool,
}

pub struct CarveState {
    configs: Vec<CarverCfg>,
    /// Distinct carver lists (slot 0 = the overworld default list).
    lists: Vec<Vec<u32>>,
    /// biome name -> slot index into lists.
    biome_lists: HashMap<String, usize>,
    /// No biome in the source deviates from the default list (fast path:
    /// skip all neighbor biome lookups).
    all_default: bool,
    /// replaceable base block names (resolved overworld_carver_replaceables).
    replaceable: HashSet<String>,
    /// id -> classification cache (ids are stable per ctx).
    block_cache: HashMap<u16, Blk>,
    /// (qx, qz) -> slot index (neighbor biome list cache).
    biome_cache: HashMap<(i32, i32), usize>,
    /// scratch: carving mask bits + canyon width factors
    mask: Vec<u64>,
    widths: Vec<f32>,
}

fn id_of_path(p: &str, marker: &str) -> Option<String> {
    // "<ns>/worldgen/configured_carver/<name>.json" -> "<ns>:<name>"
    let i = p.find(marker)?;
    let ns = &p[..i - 1]; // strip trailing '/'
    let stem = &p[i + marker.len()..];
    let stem = stem.strip_suffix(".json")?;
    Some(format!("{ns}:{stem}"))
}

fn read_json(assets: &Assets, path: &str) -> Option<Json> {
    let b = assets.get(path)?;
    let s = std::str::from_utf8(b).ok()?;
    crate::json::parse(s).ok()
}

/// Vanilla tag resolution: values array; "#ns:tag" refs resolved recursively
/// (embedded manifest provides the referenced vanilla tags); "replace": true
/// starts a fresh set (pack override semantics).
fn resolve_tag(assets: &Assets, name: &str, set: &mut HashSet<String>, depth: u32) {
    if depth > 4 {
        return;
    }
    // name is a tag id "ns:path" (e.g. "minecraft:overworld_carver_replaceables")
    let Some((ns, stem)) = name.split_once(':') else {
        return;
    };
    let path = format!("{ns}/tags/block/{stem}.json");
    let Some(j) = read_json(assets, &path) else { return };
    if j.get("replace")
        .and_then(|v| v.as_f64())
        .map(|v| v > 0.0)
        .unwrap_or(false)
    {
        set.clear();
    }
    let Some(vals) = j.get("values").and_then(|v| v.as_arr()) else { return };
    for v in vals {
        match v.as_str() {
            Some(s) if s.starts_with('#') => {
                resolve_tag(assets, &s[1..], set, depth + 1);
            }
            Some(s) => {
                set.insert(s.to_string());
            }
            None => {}
        }
    }
}

impl CarveState {
    /// Build the carve engine from assets (pack overrides over the embedded
    /// vanilla manifest). `biome_names` is the native biome source's name
    /// table when available (None => assume the default overworld list).
    pub fn from_assets(assets: &Assets, biome_names: Option<&[String]>) -> Option<CarveState> {
        // 1. configured carvers
        let mut paths: Vec<&str> = Vec::new();
        for p in crate::ASSET_PATHS {
            if p.contains("/worldgen/configured_carver/") {
                paths.push(p);
            }
        }
        for p in assets.paths() {
            if p.contains("/worldgen/configured_carver/") {
                paths.push(p);
            }
        }
        paths.sort();
        paths.dedup();
        let mut configs: Vec<CarverCfg> = Vec::new();
        let mut by_id: HashMap<String, u32> = HashMap::new();
        for p in &paths {
            let Some(id) = id_of_path(p, "worldgen/configured_carver/") else { continue };
            let Some(j) = read_json(assets, p) else { continue };
            let Some(cfg) = CarverCfg::parse(&j) else { continue };
            let idx = configs.len() as u32;
            configs.push(cfg);
            by_id.insert(id, idx);
        }
        if configs.is_empty() {
            if std::env::var("CRUS_DIAG").is_ok() {
                eprintln!("[carve] no configured_carver parsed (paths={:?})", &paths[..paths.len().min(8)]);
            }
            return None;
        }
        let default_ids = [
            "minecraft:cave",
            "minecraft:cave_extra_underground",
            "minecraft:canyon",
        ];
        let mut default_list: Vec<u32> = Vec::new();
        for id in default_ids {
            let Some(v) = by_id.get(id) else {
                if std::env::var("CRUS_DIAG").is_ok() {
                    eprintln!("[carve] default carver id missing: {id} (have {:?})", by_id.keys().take(12).collect::<Vec<_>>());
                }
                return None;
            };
            default_list.push(*v);
        }
        let mut lists = vec![default_list.clone()];

        // 2. per-biome carver lists (biome JSONs; pack overrides embedded by
        //    the sorted-iteration overwrite below)
        let mut biome_lists: HashMap<String, usize> = HashMap::new();
        let mut bpaths: Vec<&str> = Vec::new();
        for p in crate::ASSET_PATHS {
            if p.contains("/worldgen/biome/") && p.ends_with(".json") {
                bpaths.push(p);
            }
        }
        for p in assets.paths() {
            if p.contains("/worldgen/biome/") && p.ends_with(".json") {
                bpaths.push(p);
            }
        }
        bpaths.sort();
        for p in &bpaths {
            let Some(id) = id_of_path(p, "worldgen/biome/") else { continue };
            let Some(j) = read_json(assets, p) else { continue };
            let Some(carvers) = j.get("carvers") else { continue };
            let Some(arr) = carvers.as_arr() else { continue };
            let mut l: Vec<u32> = Vec::new();
            let mut ok = true;
            for c in arr {
                match c.as_str() {
                    Some(s) => match by_id.get(s) {
                        Some(v) => l.push(*v),
                        None => {
                            ok = false; // unknown carver id — default stays
                            break;
                        }
                    },
                    None => {
                        ok = false; // inline carver object — unsupported
                        break;
                    }
                }
            }
            if !ok {
                continue;
            }
            let slot = match lists.iter().position(|e| e == &l) {
                Some(s) => s,
                None => {
                    lists.push(l);
                    lists.len() - 1
                }
            };
            biome_lists.insert(id, slot);
        }

        // 3. fast-path check over the active biome source names
        let mut all_default = true;
        if let Some(names) = biome_names {
            for n in names {
                let slot = biome_lists.get(n).copied().unwrap_or(0);
                if lists[slot] != lists[0] {
                    all_default = false;
                    break;
                }
            }
        }

        // 4. replaceable tag (vanilla overworld_carver_replaceables)
        let mut replaceable = HashSet::new();
        resolve_tag(assets, "minecraft:overworld_carver_replaceables", &mut replaceable, 0);
        if replaceable.is_empty() {
            if std::env::var("CRUS_DIAG").is_ok() {
                eprintln!("[carve] replaceable tag empty");
            }
            return None;
        }

        Some(CarveState {
            configs,
            lists,
            biome_lists,
            all_default,
            replaceable,
            block_cache: HashMap::new(),
            biome_cache: HashMap::new(),
            mask: vec![0u64; 384 * 256 / 64],
            widths: vec![0f32; 384],
        })
    }

    #[inline]
    fn blk(&mut self, id: u16, surface_states: &[String], surface_ids: &[u16]) -> Blk {
        if let Some(b) = self.block_cache.get(&id) {
            return *b;
        }
        let name: String = if (id as usize) < crate::surface::MAT_NAMES.len() {
            crate::surface::MAT_NAMES[id as usize].to_string()
        } else {
            // extended palette: surface state string for this JVM id
            let mut s = String::new();
            for (i, mid) in surface_ids.iter().enumerate() {
                if *mid == id {
                    s = surface_states.get(i).cloned().unwrap_or_default();
                    break;
                }
            }
            s
        };
        let base = name.split('[').next().unwrap_or("").to_string();
        let b = Blk {
            replaceable: !base.is_empty() && self.replaceable.contains(&base),
            grass: base == "minecraft:grass_block",
            mycelium: base == "minecraft:mycelium",
            dirt: base == "minecraft:dirt",
            fluid: base == "minecraft:water" || base == "minecraft:lava",
        };
        self.block_cache.insert(id, b);
        b
    }
}

// ---------------------------------------------------------------------------
// CarveKit — per-call resources (borrowed from the session ctx)
// ---------------------------------------------------------------------------

/// Neighbor biome resolution (present only when the native biome source is
/// active AND some biome deviates from the default carver list).
pub struct BiomeLookup<'a> {
    pub src: &'a crate::biomes::BiomeSourceData,
    pub climate: &'a crate::biomes::ClimateDFs,
    pub env: &'a crate::density::Env,
    pub compiled: &'a crate::density::Compiled,
}

/// Center-chunk surface context for the carveBlock DIRT topMaterial rewrite.
pub struct TopMat<'a> {
    pub env: &'a crate::density::Env,
    pub compiled: &'a crate::density::Compiled,
    pub cfg: &'a crate::surface::SurfaceConfig,
    pub noise_handles: [u32; 3],
    pub temps: &'a [f32],
    pub frozen: &'a [bool],
    pub temps_noises: &'a crate::surface::TempNoises,
    pub names: &'a [String],
    pub biome_grid: &'a [i32],
    pub heightmap: &'a [i32; 256],
}

pub struct CarveKit<'a, 'g, 'c> {
    pub seed: i64,
    pub min_gen_y: i32,
    pub gen_depth: i32,
    pub biome_lookup: Option<BiomeLookup<'a>>,
    pub top_mat: Option<TopMat<'a>>,
    pub gen: &'g mut crate::nochunk::ChunkGen<'c>,
    pub surface_states: &'a [String],
    pub surface_ids: &'a [u16],
}

/// Mutable half of CarveKit (the run borrows it mutably while the asset
/// half stays immutably borrowed).
struct CarveKitMut<'a, 'g, 'k> {
    gen: &'g mut crate::nochunk::ChunkGen<'a>,
    surface_states: &'k [String],
    surface_ids: &'k [u16],
    top_mat: &'k Option<TopMat<'k>>,
}

// ---------------------------------------------------------------------------
// Chunk-local state
// ---------------------------------------------------------------------------

struct Frame {
    min_x: i32,
    min_z: i32,
    min_gen_y: i32,
    gen_depth: i32,
    lava_y: i32,
}

#[inline]
fn out_idx(min_y: i32, y: i32, lz: i32, lx: i32) -> usize {
    ((y - min_y) * 256 + lz * 16 + lx) as usize
}

// ---------------------------------------------------------------------------
// The carve pass (applyCarvers equivalent)
// ---------------------------------------------------------------------------

/// skip checker — cave: relY <= floor || rel^2 >= 1; canyon: width factors.
enum Skip {
    Cave { floor: f64 },
    Canyon,
}

#[inline]
fn should_skip(sk: &Skip, widths: &[f32], rel_x: f64, rel_y: f64, rel_z: f64, y: i32, min_gen_y: i32) -> bool {
    match sk {
        Skip::Cave { floor } => {
            rel_y <= *floor || rel_x * rel_x + rel_y * rel_y + rel_z * rel_z >= 1.0
        }
        Skip::Canyon => {
            // (relX^2 + relZ^2) * widths[y - minGenY - 1] + relY^2 / 6.0 >= 1.0
            let idx = (y - min_gen_y - 1) as usize;
            let wf = if idx < widths.len() { widths[idx] as f64 } else { return false };
            (rel_x * rel_x + rel_z * rel_z) * wf + rel_y * rel_y / 6.0 >= 1.0
        }
    }
}

/// WorldCarver.canReach (center chunk).
#[inline]
fn can_reach(min_x: i32, min_z: i32, x: f64, z: f64, branch_index: i32, branch_count: i32, width: f32) -> bool {
    let d = (min_x + 8) as f64;
    let d1 = (min_z + 8) as f64;
    let d2 = x - d;
    let d3 = z - d1;
    let d4 = (branch_count - branch_index) as f64;
    let d5 = (width + 2.0f32 + 16.0f32) as f64;
    d2 * d2 + d3 * d3 - d4 * d4 <= d5 * d5
}

struct CarveRun<'s, 'a, 'g, 'k> {
    state: &'s mut CarveState,
    kit: CarveKitMut<'a, 'g, 'k>,
    fr: Frame,
    /// neighbor chunk pos currently driving the RNG stream
    nbr_x: i32,
    nbr_z: i32,
}

impl<'s, 'a, 'g, 'k> CarveRun<'s, 'a, 'g, 'k> {
    /// carveEllipsoid: returns true when a block was carved.
    fn carve_ellipsoid(
        &mut self,
        x: f64,
        y: f64,
        z: f64,
        hr: f64,
        vr: f64,
        sk: &Skip,
        widths: &[f32],
        cx: i32,
        cz: i32,
        out: &mut [u16; crate::nochunk::OUT_LEN],
    ) -> bool {
        let min_x = self.fr.min_x;
        let min_z = self.fr.min_z;
        let min_gen_y = self.fr.min_gen_y;
        let gen_depth = self.fr.gen_depth;
        let d = (cx * 16 + 8) as f64; // ChunkPos.getMiddleBlockX
        let d1 = (cz * 16 + 8) as f64;
        let d2 = 16.0 + hr * 2.0;
        if (x - d).abs() > d2 || (z - d1).abs() > d2 {
            return false;
        }
        let x0 = (mth_floor(x - hr) - min_x - 1).max(0);
        let x1 = (mth_floor(x + hr) - min_x).min(15);
        let max1 = (mth_floor(y - vr) - 1).max(min_gen_y + 1);
        let min1 = (mth_floor(y + vr) + 1).min(min_gen_y + gen_depth - 1 - 7);
        let z0 = (mth_floor(z - hr) - min_z - 1).max(0);
        let z1 = (mth_floor(z + hr) - min_z).min(15);
        let mut flag = false;
        for i1 in x0..=x1 {
            let block_x = min_x + i1;
            let d3 = ((block_x as f64) + 0.5 - x) / hr;
            for i2 in z0..=z1 {
                let block_z = min_z + i2;
                let d4 = ((block_z as f64) + 0.5 - z) / hr;
                if d3 * d3 + d4 * d4 >= 1.0 {
                    continue;
                }
                let mut reached = false;
                let mut yy = min1;
                while yy > max1 {
                    let d5 = ((yy as f64) - 0.5 - y) / vr;
                    let bit = ((yy - min_gen_y) * 16 + i2) * 16 + i1;
                    let bit = bit as usize;
                    let word = self.state.mask[bit / 64];
                    if should_skip(sk, widths, d3, d5, d4, yy, min_gen_y)
                        || (word >> (bit % 64)) & 1 == 1
                    {
                        yy -= 1;
                        continue;
                    }
                    self.state.mask[bit / 64] |= 1u64 << (bit % 64);
                    if self.carve_block(block_x, yy, block_z, out, &mut reached) {
                        flag = true;
                    }
                    yy -= 1;
                }
            }
        }
        flag
    }

    /// carveBlock (WorldCarver).
    fn carve_block(
        &mut self,
        x: i32,
        y: i32,
        z: i32,
        out: &mut [u16; crate::nochunk::OUT_LEN],
        reached: &mut bool,
    ) -> bool {
        let min_y = self.fr.min_gen_y;
        let lx = x & 15;
        let lz = z & 15;
        let idx = out_idx(min_y, y, lz, lx);
        let cur = out[idx];
        let b = self.state.blk(cur, self.kit.surface_states, self.kit.surface_ids);
        if b.grass || b.mycelium {
            *reached = true;
        }
        if !b.replaceable {
            return false;
        }
        // getCarveState
        let carve_state: Option<u16> = if y <= self.fr.lava_y {
            Some(crate::nochunk::LAVA)
        } else {
            self.kit.gen.carve_substance(x, y, z)
        };
        let Some(state_id) = carve_state else { return false };
        let sched = self.kit.gen.sched_flag();
        out[idx] = state_id;
        let st = self.state.blk(state_id, self.kit.surface_states, self.kit.surface_ids);
        if sched && st.fluid {
            self.kit.gen.push_mark(x, y, z);
        }
        if *reached && y - 1 >= min_y {
            let bidx = out_idx(min_y, y - 1, lz, lx);
            let below = self.state.blk(out[bidx], self.kit.surface_states, self.kit.surface_ids);
            if below.dirt {
                let has_fluid = st.fluid;
                if let Some(tm) = self.top_material(x, y - 1, z, has_fluid) {
                    out[bidx] = tm;
                    let tmb = self.state.blk(tm, self.kit.surface_states, self.kit.surface_ids);
                    if tmb.fluid {
                        self.kit.gen.push_mark(x, y - 1, z);
                    }
                }
            }
        }
        true
    }

    /// SurfaceSystem.topMaterial: fresh context per call.
    fn top_material(&mut self, x: i32, y: i32, z: i32, has_fluid: bool) -> Option<u16> {
        let tm = (self.kit.top_mat).as_ref()?;
        crate::surface_fill::top_material_at(
            tm.env,
            tm.compiled,
            tm.cfg,
            tm.noise_handles,
            tm.temps,
            tm.frozen,
            tm.temps_noises,
            tm.names,
            tm.biome_grid,
            tm.heightmap,
            x,
            y,
            z,
            has_fluid,
        )
    }

    // ---------------- cave carver ----------------

    fn cave_carve(
        &mut self,
        ci: u32,
        rng: &mut LegacyRandom,
        cx: i32,
        cz: i32,
        out: &mut [u16; crate::nochunk::OUT_LEN],
    ) {
        // clone the small config (borrow checker: self is mutated below)
        let cfg = self.state.configs[ci as usize].clone();
        let Kind::Cave { horizontal_radius_multiplier: hrm_p, vertical_radius_multiplier: vrm_p, floor_level: floor_p } = &cfg.kind else {
            return;
        };
        let mgy = self.fr.min_gen_y;
        let gd = self.fr.gen_depth;
        let branch_total = 112i32; // SectionPos.sectionToBlockCoord(getRange()*2-1)
        let g1 = rng.next_int_bound(15);
        let g2 = rng.next_int_bound(g1 + 1);
        let groups = rng.next_int_bound(g2 + 1);
        for _ in 0..groups {
            let sx = ((self.nbr_x * 16 + rng.next_int_bound(16)) as i64) as f64;
            let sy = cfg.y.sample(rng, mgy, gd) as f64;
            let sz = ((self.nbr_z * 16 + rng.next_int_bound(16)) as i64) as f64;
            let hrm = hrm_p.sample(rng) as f64;
            let vrm = vrm_p.sample(rng) as f64;
            let floor = floor_p.sample(rng) as f64;
            let sk = Skip::Cave { floor };
            let mut tunnels = 1u32;
            if rng.next_int_bound(4) == 0 {
                let hvr = cfg.y_scale.sample(rng) as f64;
                let radius = 1.0f32 + rng.next_f32() * 6.0f32;
                // createRoom: d = 1.5 + (double)(Mth.sin(1.5707964f) * radius)
                let d = 1.5f64 + ((mth_sin(HALF_PI) * radius) as f64);
                let d1 = d * hvr;
                self.carve_ellipsoid(sx + 1.0, sy, sz, d, d1, &sk, &[], cx, cz, out);
                tunnels += rng.next_int_bound(4) as u32;
            }
            for _ in 0..tunnels {
                let yaw0 = rng.next_f32() * TWO_PI_F32;
                let pitch0 = (rng.next_f32() - 0.5f32) / 4.0f32;
                let thickness = cave_thickness(rng);
                let branch_count = branch_total - rng.next_int_bound(branch_total / 4);
                let seed = rng.next_long();
                self.cave_tunnel(
                    seed, sx, sy, sz, hrm, vrm, thickness, yaw0, pitch0, 0, branch_count, 1.0,
                    &sk, cx, cz, out,
                );
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn cave_tunnel(
        &mut self,
        seed: i64,
        mut x: f64,
        mut y: f64,
        mut z: f64,
        hrm: f64,
        vrm: f64,
        thickness: f32,
        mut yaw: f32,
        mut pitch: f32,
        branch_index: i32,
        branch_count: i32,
        hvr: f64,
        sk: &Skip,
        cx: i32,
        cz: i32,
        out: &mut [u16; crate::nochunk::OUT_LEN],
    ) {
        let mut r2 = LegacyRandom::new(seed);
        let split_at = r2.next_int_bound(branch_count / 2) + branch_count / 4;
        let twist = r2.next_int_bound(6) == 0;
        let mut wy = 0f32;
        let mut wx = 0f32;
        let mut i = branch_index;
        while i < branch_count {
            let d = 1.5f64 + ((mth_sin((i as f32) * PI_F32 / (branch_count as f32)) * thickness) as f64);
            let d1 = d * hvr;
            let cosp = mth_cos(pitch);
            x += (mth_cos(yaw) * cosp) as f64;
            y += mth_sin(pitch) as f64;
            z += (mth_sin(yaw) * cosp) as f64;
            pitch *= if twist { 0.92f32 } else { 0.7f32 };
            pitch += wy * 0.1f32;
            yaw += wx * 0.1f32;
            wy *= 0.9f32;
            wx *= 0.75f32;
            wy += (r2.next_f32() - r2.next_f32()) * r2.next_f32() * 2.0f32;
            wx += (r2.next_f32() - r2.next_f32()) * r2.next_f32() * 4.0f32;
            if i == split_at && thickness > 1.0f32 {
                // vanilla evaluates the two createTunnel arg lists eagerly,
                // left to right: seedA, thicknessA, seedB, thicknessB.
                let seed_a = r2.next_long();
                let th_a = r2.next_f32() * 0.5f32 + 0.5f32;
                let seed_b = r2.next_long();
                let th_b = r2.next_f32() * 0.5f32 + 0.5f32;
                self.cave_tunnel(
                    seed_a, x, y, z, hrm, vrm, th_a, yaw - HALF_PI, pitch / 3.0f32, i,
                    branch_count, 1.0, sk, cx, cz, out,
                );
                self.cave_tunnel(
                    seed_b, x, y, z, hrm, vrm, th_b, yaw + HALF_PI, pitch / 3.0f32, i,
                    branch_count, 1.0, sk, cx, cz, out,
                );
                return;
            }
            if r2.next_int_bound(4) == 0 {
                i += 1;
                continue;
            }
            if !can_reach(self.fr.min_x, self.fr.min_z, x, z, i, branch_count, thickness) {
                return;
            }
            self.carve_ellipsoid(x, y, z, d * hrm, d1 * vrm, sk, &[], cx, cz, out);
            i += 1;
        }
    }

    // ---------------- canyon carver ----------------

    fn canyon_carve(
        &mut self,
        ci: u32,
        rng: &mut LegacyRandom,
        cx: i32,
        cz: i32,
        out: &mut [u16; crate::nochunk::OUT_LEN],
    ) {
        // clone the small config (borrow checker: self is mutated below)
        let cfg = self.state.configs[ci as usize].clone();
        let Kind::Canyon { vertical_rotation, shape } = &cfg.kind else { return };
        let shape = shape.clone();
        let vrot = vertical_rotation.clone();
        let mgy = self.fr.min_gen_y;
        let gd = self.fr.gen_depth;
        let i_total = 112f32;
        let x = ((self.nbr_x * 16 + rng.next_int_bound(16)) as i64) as f64;
        let y = cfg.y.sample(rng, mgy, gd) as f64;
        let z = ((self.nbr_z * 16 + rng.next_int_bound(16)) as i64) as f64;
        let yaw = rng.next_f32() * TWO_PI_F32;
        let pitch = vrot.sample(rng);
        let yscale = cfg.y_scale.sample(rng) as f64;
        let thickness = shape.thickness.sample(rng);
        let branch_count = (i_total * shape.distance_factor.sample(rng)) as i32;
        let seed = rng.next_long();
        self.canyon_tunnel(
            seed, x, y, z, thickness, yaw, pitch, 0, branch_count, yscale, &shape, cx, cz, out,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn canyon_tunnel(
        &mut self,
        seed: i64,
        mut x: f64,
        mut y: f64,
        mut z: f64,
        thickness: f32,
        mut yaw: f32,
        mut pitch: f32,
        branch_index: i32,
        branch_count: i32,
        hvr: f64,
        shape: &CanyonShape,
        cx: i32,
        cz: i32,
        out: &mut [u16; crate::nochunk::OUT_LEN],
    ) {
        let mut r2 = LegacyRandom::new(seed);
        let gd = self.fr.gen_depth;
        // initWidthFactors
        let mut f = 1.0f32;
        for gi in 0..gd.min(self.state.widths.len() as i32) {
            let gi = gi as usize;
            if gi == 0 || r2.next_int_bound(shape.width_smoothness) == 0 {
                f = 1.0f32 + r2.next_f32() * r2.next_f32();
            }
            self.state.widths[gi] = f * f;
        }
        let widths: Vec<f32> = self.state.widths[..gd.min(self.state.widths.len() as i32) as usize].to_vec();
        let mut wy = 0f32;
        let mut wx = 0f32;
        let mut i = branch_index;
        while i < branch_count {
            let mut d = 1.5f64
                + ((mth_sin((i as f32) * PI_F32 / (branch_count as f32)) * thickness) as f64);
            let d1_0 = d * hvr;
            d *= shape.horizontal_radius_factor.sample(&mut r2) as f64;
            let d1 = update_vertical_radius(shape, &mut r2, d1_0, branch_count as f32, i as f32);
            let cosp = mth_cos(pitch);
            let sinp = mth_sin(pitch);
            x += (mth_cos(yaw) * cosp) as f64;
            y += sinp as f64;
            z += (mth_sin(yaw) * cosp) as f64;
            pitch *= 0.7f32;
            pitch += wy * 0.05f32;
            yaw += wx * 0.05f32;
            wy *= 0.8f32;
            wx *= 0.5f32;
            wy += (r2.next_f32() - r2.next_f32()) * r2.next_f32() * 2.0f32;
            wx += (r2.next_f32() - r2.next_f32()) * r2.next_f32() * 4.0f32;
            if r2.next_int_bound(4) == 0 {
                i += 1;
                continue;
            }
            if !can_reach(self.fr.min_x, self.fr.min_z, x, z, i, branch_count, thickness) {
                return;
            }
            let sk = Skip::Canyon;
            self.carve_ellipsoid(x, y, z, d, d1, &sk, &widths, cx, cz, out);
            i += 1;
        }
    }
}

/// CanyonWorldCarver.updateVerticalRadius.
#[inline]
fn update_vertical_radius(shape: &CanyonShape, rng: &mut LegacyRandom, vr: f64, branch_count: f32, current: f32) -> f64 {
    let f = 1.0f32 - mth_abs_f(0.5f32 - current / branch_count) * 2.0f32;
    let f1 = shape.vertical_radius_default_factor + shape.vertical_radius_center_factor * f;
    f1 as f64 * vr * (mth_random_between(rng, 0.75f32, 1.0f32) as f64)
}

/// CaveWorldCarver.getThickness.
#[inline]
fn cave_thickness(rng: &mut LegacyRandom) -> f32 {
    let mut f = rng.next_f32() * 2.0f32 + rng.next_f32();
    if rng.next_int_bound(10) == 0 {
        f *= rng.next_f32() * rng.next_f32() * 3.0f32 + 1.0f32;
    }
    f
}

/// Resolve the carver list for a neighbor chunk (biome at quart (minX>>2,
/// 0, minZ>>2)). all_default fast path avoids the lookups entirely.
fn resolve_list(
    state: &mut CarveState,
    bl: Option<&BiomeLookup>,
    nbr_x: i32,
    nbr_z: i32,
    scratch: &mut Option<(crate::density::InterpState, crate::density::MemoCtx)>,
) -> usize {
    if state.all_default {
        return 0;
    }
    let qx = nbr_x << 2; // QuartPos.fromBlock(minBlockX) == minBlockX / 4
    let qz = nbr_z << 2;
    let key = (qx, qz);
    if let Some(slot) = state.biome_cache.get(&key) {
        return *slot;
    }
    // biome eval at block (qx*4, 0, qz*4) — ClimateDFs + RTree, same as
    // fill_biomes' per-quart path.
    let bl = bl.expect("biome lookup when !all_default");
    let (src, climate, env, compiled) = (bl.src, bl.climate, bl.env, bl.compiled);
    if scratch.is_none() {
        *scratch = Some((
            crate::density::InterpState::new(compiled.interp_slots, 4, env.wg.height as usize / 8),
            crate::density::MemoCtx::new(compiled.arena.len())
                .with_cols(compiled.col_count as usize),
        ));
    }
    let (interp, memo) = scratch.as_mut().unwrap();
    let pt = crate::density::Pt { x: qx * 4, y: 0, z: qz * 4 };
    let mut values = [0i64; 7];
    for fi in 0..6usize {
        memo.next_serial();
        let v = env.eval(
            compiled,
            climate.idx[fi],
            pt,
            crate::density::Mode::Direct,
            interp,
            Some(memo),
        );
        values[fi] = crate::biomes::quantize_pub(v);
    }
    let leaf = src.find_value(&values);
    let name = &src.biome_names[src.entries[leaf].biome_idx as usize];
    let slot = state.biome_lists.get(name).copied().unwrap_or(0);
    state.biome_cache.insert(key, slot);
    slot
}

/// The applyCarvers equivalent: 17x17 neighbor chunks, per-carver seeds,
/// carve into the CENTER chunk.
pub fn carve_chunk(
    state: &mut CarveState,
    kit: &mut CarveKit<'_, '_, '_>,
    cx: i32,
    cz: i32,
    out: &mut [u16; crate::nochunk::OUT_LEN],
) {
    // Borrow-split the kit: immutable asset half (biome_lookup, top_mat,
    // names) vs the mutable ChunkGen. Disjoint struct fields, so the
    // borrow checker accepts field-by-field access.
    let seed = kit.seed;
    let mgy = kit.min_gen_y;
    let gd = kit.gen_depth;
    let biome_lookup = kit.biome_lookup.as_ref();
    let fr = Frame {
        min_x: cx * 16,
        min_z: cz * 16,
        min_gen_y: mgy,
        gen_depth: gd,
        lava_y: 0,
    };
    let mut run = CarveRun {
        state,
        kit: CarveKitMut {
            gen: &mut *kit.gen,
            surface_states: kit.surface_states,
            surface_ids: kit.surface_ids,
            top_mat: &kit.top_mat,
        },
        fr,
        nbr_x: 0,
        nbr_z: 0,
    };
    let mut scratch: Option<(crate::density::InterpState, crate::density::MemoCtx)> = None;
    let mut rng = LegacyRandom::new(0);
    for dx in -8i32..=8 {
        for dz in -8i32..=8 {
            let nx = cx + dx;
            let nz = cz + dz;
            let slot = resolve_list(run.state, biome_lookup, nx, nz, &mut scratch);
            let slot2 = run.state.lists[slot].clone();
            for (i3, &ci) in slot2.iter().enumerate() {
                set_large_feature_seed(&mut rng, seed + i3 as i64, nx, nz);
                if rng.next_f32() > run.state.configs[ci as usize].probability {
                    continue;
                }
                run.nbr_x = nx;
                run.nbr_z = nz;
                // per-carver lava level (VerticalAnchor resolve)
                run.fr.lava_y = run.state.configs[ci as usize].lava_level.resolve_y(mgy, gd);
                let kind_is_cave = matches!(run.state.configs[ci as usize].kind, Kind::Cave { .. });
                if kind_is_cave {
                    run.cave_carve(ci, &mut rng, cx, cz, out);
                } else {
                    run.canyon_carve(ci, &mut rng, cx, cz, out);
                }
            }
        }
    }
    // clear the chunk-scoped mask for the next fill (cache stays)
    run.state.mask.iter_mut().for_each(|w| *w = 0);
    if run.state.biome_cache.len() > 1_500_000 {
        run.state.biome_cache.clear();
    }
}
