// ---------------------------------------------------------------------------
// Native SurfaceRules engine — config/parse side (see surface_fill.rs for the
// per-chunk column walk). Exact replica of vanilla SurfaceSystem + SurfaceRules
// (Paper 1.21.8, mojmap), driven by the datapack noise_settings surface_rule.
//
// Parity contract (javap-verified against server/versions/1.21.8 jar):
//   * getSurfaceDepth(x,z) = (int)(surfaceNoise.getValue(x,0,z) * 2.75 + 3.0
//     + noiseRandom.at(x,0,z).nextDouble() * 0.25)                    [f64]
//   * getSurfaceSecondary(x,z) = surfaceSecondary.getValue(x,0,z)
//   * getMinSurfaceLevel: 4 preliminarySurfaceLevel samples on the >>4 cell
//     corners, Mth.lerp2(((x&15)/16f) as f64, ((z&15)/16f) as f64, ...),
//     Mth.floor() + surfaceDepth - 8 (per-column, cached vs updateXZ counter)
//   * preliminarySurfaceLevel(x,z): quart-snap x/z (QuartPos.fromBlock/
//     toBlock), per chunk cache; scan y from maxY down by cellHeight while
//     y >= minY; d = initial_density_without_jaggedness at (x, y, z);
//     if d > 0.390625 -> return y; exhausted -> Integer.MAX_VALUE.
//   * temperature (Context$TemperatureHelperCondition, LazyY):
//     biome.coldEnoughToSnow(pos, seaLevel):
//       f = modifier.modifyTemperature(base)  [NONE: identity; FROZEN: see
//       frozen_modifier below]
//       i = seaLevel + 17; if y > i:
//         f - ((f32)(TEMP_NOISE.getValue((f32)x/8f, (f32)z/8f) * 8.0)
//              + (f32)y - (f32)i) * 0.05f / 40.0f          [f32 arithmetic]
//       return f < 0.15f
//     FROZEN modifier: if (FROZEN_NOISE(LegacyRandom 3456, oct[-2,-1,0])
//       .getValue(x*0.05, z*0.05) * 7.0 + BIOME_INFO(2345, oct[0])
//       .getValue(x*0.2, z*0.2)) < 0.3 && BIOME_INFO.getValue(x*0.09,
//       z*0.09) < 0.8 -> 0.2f else base.        [x/z as f64 * const]
//   * conditions (all formulas javap-decoded):
//     stone_depth: i = ceiling ? stoneDepthBelow : stoneDepthAbove;
//       j = addSurfaceDepth ? surfaceDepth : 0;
//       k = secondaryRange ? (int)map(secondary, -1, 1, 0, range) : 0;
//       true when i < 1 + offset + j + k
//     water: waterHeight == MIN -> true; else
//       blockY + (addStoneDepth ? stoneDepthAbove : 0)
//         < waterHeight + offset + surfaceDepth * multiplier
//     y_above: blockY + (addSD ? stoneDepthAbove : 0)
//       >= anchor + surfaceDepth * multiplier
//     noise_threshold (LazyXZ): min <= noise.getValue(x,0,z) <= max
//     biome (LazyY): biome idx in set
//     vertical_gradient (LazyY): y <= trueY -> true; y >= falseY -> false;
//       map(y, trueY, falseY, 1.0, 0.0) [f64] > factory.fromHashOf(name)
//       .at(x,y,z).nextFloat()
//     temperature/hole/steep/above_preliminary: see Context inners —
//       hole (LazyXZ): surfaceDepth <= 0
//       steep (LazyXZ): hm(x, z-1..z+1) neighbor >= other + 4 (z pair then
//         x pair; inner indices clamped 0..15)
//       above_preliminary (LazyY): blockY >= getMinSurfaceLevel()
//   * bandlands: 192 bands from factory.fromHashOf("minecraft:clay_bands"),
//     generateBands/makeBands exactly per javap; getBand(x,y,z) =
//     bands[(y + round(clayBandsOffset.getValue(x,0,z) * 4.0) + 192) % 192]
//     [Math.round on f64]
//   * noiseRandom = RandomState.random = Xoroshiro(seed).forkPositional();
//     surface/secondary/clay_bands_offset noises = NormalNoise via
//     root_factory.from_hash_of(id) (env.noise handles).
// ---------------------------------------------------------------------------

use crate::json::{self, Json};
use crate::noise::NormalNoise;
use crate::rng::{LegacyRandom, XoroshiroPositionalRandomFactory, XoroshiroRandomSource};
use crate::worldgen::{Assets, WorldGen};

/// Parsed surface rule tree (rules reference conditions by arena index).
pub enum SrRule {
    Sequence(Vec<usize>),
    IfTrue { cond: usize, then: usize },
    Block { state: u32 },
    Bandlands,
    /// MC 26.3: ore veins became a material rule (OreVeinRule). DF indices
    /// point into the density Compiled arena via Compiled.router lookups.
    OreVein {
        ore: u32,
        raw: u32,
        filler: u32,
        raw_chance: f32,
        density_df: u32,
        richness_df: u32,
        gap_df: u32,
    },
}

pub enum SrCond {
    Biome { set: Vec<u32> },
    NoiseThreshold {
        noise: u32,
        min: f64,
        max: f64,
        /// 26.3 NoiseThresholdCondition.is3d — 3D sampler at the block pos
        /// (false = legacy 2D at y=0).
        is_3d: bool,
    },
    StoneDepth { offset: i32, add_surface_depth: bool, secondary_range: i32, ceiling: bool },
    Water { offset: i32, surface_depth_multiplier: i32, add_stone_depth: bool },
    YAbove { anchor: i32, add_stone_depth: bool, surface_depth_multiplier: i32 },
    VerticalGradient { name: String, true_y: i32, false_y: i32 },
    Temperature,
    Hole,
    Steep,
    AbovePreliminary,
    Not(usize),
}

pub struct SurfaceConfig {
    pub rules: Vec<SrRule>,
    pub conds: Vec<SrCond>,
    pub root: usize,
    /// Distinct block state strings in first-appearance order. Material ids
    /// are assigned by the JVM (setSurfaceIds) into `ids` (same order).
    pub states: Vec<String>,
    pub ids: Vec<u16>,
    /// State indexes of the 7 bandlands terracottas (fixed order — see
    /// TERRACOTTA IDS below).
    pub terracotta: [u32; 7],
    /// 192 band entries as state indexes (filled at ids-set time).
    pub bandlands: Vec<u32>,
    pub sea_level: i32,
    /// native default-block material id (STONE = 1)
    pub default_id: u16,
    pub min_y: i32,
    pub height: i32,
    pub cell_height: i32,
}

pub const MAT_NAMES: [&str; 22] = [
    "minecraft:air",
    "minecraft:stone",
    "minecraft:bedrock",
    "minecraft:water[level=0]",
    "minecraft:grass_block[snowy=false]",
    "minecraft:dirt",
    "minecraft:deepslate[axis=y]",
    "minecraft:sand",
    "minecraft:gravel",
    "minecraft:granite",
    "minecraft:diorite",
    "minecraft:andesite",
    "minecraft:tuff",
    "minecraft:sandstone",
    "minecraft:clay",
    "minecraft:powder_snow",
    "minecraft:ice",
    "minecraft:lava[level=0]",
    "minecraft:copper_ore",
    "minecraft:raw_copper_block",
    "minecraft:deepslate_iron_ore",
    "minecraft:raw_iron_block",
];

/// Fixed terracotta order matching generateBands usage.
pub const TERRACOTTA_IDS: [&str; 7] = [
    "minecraft:terracotta",
    "minecraft:orange_terracotta",
    "minecraft:yellow_terracotta",
    "minecraft:brown_terracotta",
    "minecraft:red_terracotta",
    "minecraft:white_terracotta",
    "minecraft:light_gray_terracotta",
];

fn st_str(name: &str, props: Option<&Json>) -> String {
    match props {
        Some(Json::Obj(entries)) => {
            let mut kv: Vec<(String, String)> = entries
                .iter()
                .map(|(k, v)| (k.to_string(), v.as_str().unwrap_or("").to_string()))
                .collect();
            kv.sort();
            let inner: Vec<String> = kv.iter().map(|(k, v)| format!("{k}={v}")).collect();
            format!("{name}[{}]", inner.join(","))
        }
        _ => name.to_string(),
    }
}

struct Parser<'a> {
    cfg: Partial,
    noise_idx: &'a dyn Fn(&str) -> Option<u32>,
    biome_idx: &'a dyn Fn(&str) -> Option<u32>,
    wg: &'a WorldGen,
    /// embedded asset store — resolves 26.3 material_rule / material_condition
    /// string refs
    assets: Option<&'a Assets>,
    /// resolves a DF string ref ("minecraft:overworld/ore_vein/iron_density")
    /// to a density Compiled arena index (26.3 ore_vein rules)
    dfc: Option<&'a dyn Fn(&str) -> Option<u32>>,
    depth: u32,
}

struct Partial {
    rules: Vec<SrRule>,
    conds: Vec<SrCond>,
    states: Vec<String>,
    state_idx: std::collections::HashMap<String, u32>,
}

impl Partial {
    fn state(&mut self, s: String) -> u32 {
        if let Some(i) = self.state_idx.get(&s) {
            return *i;
        }
        let i = self.states.len() as u32;
        self.states.push(s.clone());
        self.state_idx.insert(s, i);
        i
    }
}

/// "deep_frozen_ocean" -> "minecraft:deep_frozen_ocean" (vanilla
/// ResourceLocation defaults the namespace).
fn canon(name: &str) -> String {
    if name.contains(':') {
        name.to_string()
    } else {
        format!("minecraft:{name}")
    }
}

/// required-field helper (surface JSON)
fn req<'a>(j: &'a Json, key: &str) -> &'a Json {
    static NULL: Json = Json::Null;
    j.get(key).unwrap_or(&NULL)
}

/// {"Name": "minecraft:x", "Properties": {...}} -> "minecraft:x[k=v,...]"
fn state_key(j: &Json) -> String {
    let name = j.get("Name").and_then(Json::as_str).unwrap_or("");
    st_str(name, j.get("Properties"))
}

fn cdesc(j: &Json) -> String {
    j.get("type").and_then(|v| v.as_str()).unwrap_or("?").to_string()
}

fn parse_anchor(j: &Json, wg: &WorldGen) -> Option<i32> {
    // shorthand form: {"absolute": y} / {"above_bottom": y} / {"below_top": y}
    if let Some(y) = j.get("absolute").and_then(|v| v.as_i32()) {
        return Some(y);
    }
    if let Some(y) = j.get("above_bottom").and_then(|v| v.as_i32()) {
        return Some(wg.min_y + y);
    }
    if let Some(y) = j.get("below_top").and_then(|v| v.as_i32()) {
        return Some(wg.min_y + wg.height - 1 - y);
    }
    // long form: {"type": "...", "y": n}
    let t = j.get("type")?.as_str()?;
    let y = j.get("y")?.as_i32()?;
    Some(match t {
        "minecraft:absolute" => y,
        "minecraft:above_bottom" => wg.min_y + y,
        "minecraft:below_top" => wg.min_y + wg.height - 1 - y,
        _ => return None,
    })
}

impl<'a> Parser<'a> {
    fn rule(&mut self, j: &Json) -> Option<usize> {
        // 26.3: a bare string is a material_rule ref ("minecraft:overworld/biome_surface/frozen_peaks")
        if let Json::Str(s) = j {
            return self.rule_ref(s);
        }
        let t = match j.get("type").and_then(|v| v.as_str()) {
            Some(t) => t,
            None => { eprintln!("surface parse: rule without type: {j:?}"); return None; }
        };
        let idx = match t {
            "minecraft:sequence" => {
                // vanilla codec field is "to_be_shorter"; Terralith 2.5.x
                // ships "sequence" (both appear in the wild — accept both)
                let arr = match j
                    .get("to_be_shorter")
                    .or_else(|| j.get("sequence"))
                    .and_then(|v| v.as_arr())
                {
                    Some(a) => a,
                    None => { eprintln!("surface parse: sequence without children list"); return None; }
                };
                let mut kids = Vec::new();
                for k in arr {
                    kids.push(self.rule(k)?);
                }
                if kids.is_empty() {
                    return None;
                }
                self.cfg.rules.push(SrRule::Sequence(kids));
                self.cfg.rules.len() - 1
            }
            "minecraft:condition" => {
                let c = match j.get("if_true") {
                    Some(v) => match self.cond(v) { Some(c) => c, None => { eprintln!("surface parse: condition if_true failed ({})", cdesc(v)); return None; } },
                    None => { eprintln!("surface parse: condition without if_true"); return None; }
                };
                let th = match j.get("then_run") {
                    Some(v) => match self.rule(v) { Some(r) => r, None => { eprintln!("surface parse: condition then_run failed"); return None; } },
                    None => { eprintln!("surface parse: condition without then_run"); return None; }
                };
                self.cfg.rules.push(SrRule::IfTrue { cond: c, then: th });
                self.cfg.rules.len() - 1
            }
            "minecraft:block" => {
                // "result" (pre-1.21 naming) / "result_state" (Terralith
                // 2.5.x); 26.3 material rules carry a plain "minecraft:id"
                // STRING (default state) instead of a {Name, Properties} obj.
                let res = match j.get("result").or_else(|| j.get("result_state")) {
                    Some(r) => r,
                    None => { eprintln!("surface parse: block without result"); return None; }
                };
                let (name, props) = match res {
                    Json::Str(s) => (s.clone(), None),
                    _ => match res.get("Name").and_then(|v| v.as_str()) {
                        Some(n) => (n.to_string(), res.get("Properties")),
                        None => { eprintln!("surface parse: block without Name"); return None; }
                    },
                };
                let s = st_str(&name, props);
                let st = self.cfg.state(s);
                self.cfg.rules.push(SrRule::Block { state: st });
                self.cfg.rules.len() - 1
            }
            "minecraft:bandlands" => {
                self.cfg.rules.push(SrRule::Bandlands);
                self.cfg.rules.len() - 1
            }
            "minecraft:ore_vein" => {
                // 26.3 OreVeinRule: ore/raw/filler are block states;
                // density/richness/filler_gap are DF refs pre-registered in
                // Compiled.router under "material:<id>".
                let block_at = |v: &Json| -> String {
                    match v {
                        Json::Str(s) => canon(s),
                        Json::Obj(_) => state_key(v),
                        _ => String::new(),
                    }
                };
                let ore = self.cfg.state(block_at(req(j, "ore_block")));
                let raw = self.cfg.state(block_at(req(j, "raw_ore_block")));
                let filler = self.cfg.state(block_at(req(j, "filler_block")));
                let df_at = |p: &Parser<'a>, v: &Json| -> Option<u32> {
                    let id = match v {
                        Json::Str(s) => s.clone(),
                        _ => { eprintln!("surface parse: ore_vein DF not a string ref"); return None; }
                    };
                    let key = format!("material:{id}");
                    p.dfc.and_then(|f| f(&key))
                };
                let density_df = match df_at(self, req(j, "density")) { Some(v) => v, None => { eprintln!("surface parse: ore_vein density ref failed"); return None; } };
                let richness_df = match df_at(self, req(j, "richness")) { Some(v) => v, None => { eprintln!("surface parse: ore_vein richness ref failed"); return None; } };
                let gap_df = match df_at(self, req(j, "filler_gap")) { Some(v) => v, None => { eprintln!("surface parse: ore_vein filler_gap ref failed"); return None; } };
                let raw_chance = req(j, "raw_ore_chance").as_f64().unwrap_or(0.02) as f32;
                self.cfg.rules.push(SrRule::OreVein { ore, raw, filler, raw_chance, density_df, richness_df, gap_df });
                self.cfg.rules.len() - 1
            }
            _ => {
                eprintln!("surface parse: unknown rule type {t}");
                return None;
            }
        };
        Some(idx)
    }

    /// 26.3: resolve "minecraft:overworld/biome_surface/frozen_peaks" ->
    /// minecraft/worldgen/material_rule/overworld/biome_surface/frozen_peaks.json
    fn rule_ref(&mut self, s: &str) -> Option<usize> {
        let assets = match self.assets {
            Some(a) => a,
            None => { eprintln!("surface parse: rule ref {s} but no assets"); return None; }
        };
        self.depth += 1;
        if self.depth > 64 {
            eprintln!("surface parse: ref depth cap at {s}");
            return None;
        }
        let path = format!("minecraft/worldgen/material_rule/{}.json", s.strip_prefix("minecraft:").unwrap_or(s));
        let bytes = match assets.get(&path) {
            Some(b) => b,
            None => { eprintln!("surface parse: rule ref not found: {path}"); return None; }
        };
        let text = match std::str::from_utf8(bytes) { Ok(t) => t, Err(_) => { eprintln!("surface parse: {path} not utf-8"); return None; } };
        let j = match json::parse(text) {
            Ok(v) => v,
            Err(e) => { eprintln!("surface parse: {path}: {e}"); return None; }
        };
        let r = self.rule(&j);
        self.depth -= 1;
        r
    }

    /// 26.3: resolve "minecraft:on_floor" ->
    /// minecraft/worldgen/material_condition/on_floor.json
    fn cond_ref(&mut self, s: &str) -> Option<usize> {
        let assets = match self.assets {
            Some(a) => a,
            None => { eprintln!("surface parse: cond ref {s} but no assets"); return None; }
        };
        self.depth += 1;
        if self.depth > 64 {
            eprintln!("surface parse: cond ref depth cap at {s}");
            return None;
        }
        let path = format!("minecraft/worldgen/material_condition/{}.json", s.strip_prefix("minecraft:").unwrap_or(s));
        let bytes = match assets.get(&path) {
            Some(b) => b,
            None => { eprintln!("surface parse: cond ref not found: {path}"); return None; }
        };
        let text = match std::str::from_utf8(bytes) { Ok(t) => t, Err(_) => { eprintln!("surface parse: {path} not utf-8"); return None; } };
        let j = match json::parse(text) {
            Ok(v) => v,
            Err(e) => { eprintln!("surface parse: {path}: {e}"); return None; }
        };
        let r = self.cond(&j);
        self.depth -= 1;
        r
    }

    fn cond(&mut self, j: &Json) -> Option<usize> {
        // 26.3: a bare string is a material_condition ref ("minecraft:on_floor")
        if let Json::Str(s) = j {
            return self.cond_ref(s);
        }
        let t = match j.get("type").and_then(|v| v.as_str()) {
            Some(t) => t,
            None => { eprintln!("surface parse: cond without type: {j:?}"); return None; }
        };
        let idx = match t {
            "minecraft:biome" => {
                let mut set = Vec::new();
                // "biomes" (vanilla; list or single string) / "biome_is"
                // (Terralith 2.5.x list form)
                let node = match j.get("biomes").or_else(|| j.get("biome_is")) {
                    Some(v) => v,
                    None => { eprintln!("surface parse: biome cond without biomes"); return None; }
                };
                if let Some(arr) = node.as_arr() {
                    for b in arr {
                        // biome entries may be "terralith:foo" or "#tag" —
                        // tags unsupported in this asset set (verified: none).
                        let raw = match b.as_str() { Some(n) => n, None => { eprintln!("surface parse: biome entry not a string"); return None; } };
                        let name: &str = &canon(raw);
                        match (self.biome_idx)(name) {
                            Some(bi) => set.push(bi),
                            // Terralith ships marker "biomes" (e.g.
                            // terralith:comment_dont_generate_strips_here)
                            // that never spawn — treat as never-matching.
                            None => {
                                eprintln!("surface parse: biome not in native list (treated as never-match): {name}");
                            }
                        }
                    }
                } else if let Some(name) = node.as_str() {
                    // 26.3 single-string form ("biome_is": "minecraft:desert")
                    let bi = (self.biome_idx)(&canon(name))?;
                    set.push(bi);
                } else if let Some(name) = j.get("biomes").and_then(|b| b.as_str()) {
                    let bi = (self.biome_idx)(name)?;
                    set.push(bi);
                } else {
                    return None;
                }
                self.cfg.conds.push(SrCond::Biome { set });
                self.cfg.conds.len() - 1
            }
            "minecraft:noise_threshold" => {
                let noise = match j.get("noise") {
                    Some(Json::Str(id)) => match (self.noise_idx)(&canon(id)) {
                        Some(n) => n,
                        None => { eprintln!("surface parse: noise_threshold noise not in env: {id}"); return None; }
                    },
                    Some(obj) => {
                        // inline params — allocate into the env noise list at
                        // runtime; rare, not in this asset set.
                        let fo = obj.get("firstOctave")?.as_i32()?;
                        let amps = obj.get("amplitudes")?.as_arr()?;
                        let _ = (fo, amps);
                        return None;
                    }
                    None => return None,
                };
                let min = match j.get("min_threshold").and_then(|v| v.as_f64()) { Some(v) => v, None => { eprintln!("surface parse: noise_threshold min missing"); return None; } };
                let max = match j.get("max_threshold").and_then(|v| v.as_f64()) { Some(v) => v, None => { eprintln!("surface parse: noise_threshold max missing"); return None; } };
                let is_3d = j.get("is_3d").and_then(crate::json::Json::as_bool).unwrap_or(false);
                self.cfg.conds.push(SrCond::NoiseThreshold { noise, min, max, is_3d });
                self.cfg.conds.len() - 1
            }
            "minecraft:stone_depth" => {
                let offset = j.get("offset").and_then(|v| v.as_i32()).unwrap_or(0);
                if j.get("surface_type").and_then(|v| v.as_str()).is_none() {
                    eprintln!("surface parse: stone_depth without surface_type");
                    return None;
                }
                let asd = j.get("add_surface_depth").and_then(crate::json::Json::as_bool).unwrap_or(false);
                let sdr = j
                    .get("secondary_depth_range")
                    .and_then(|v| v.as_i32())
                    .unwrap_or(0);
                let ty = j.get("surface_type")?.as_str()?;
                // vanilla StringRepresentable enum: unnamespaced "floor"/"ceiling"
                let ceiling = ty == "ceiling" || ty == "minecraft:ceiling";
                self.cfg.conds.push(SrCond::StoneDepth {
                    offset,
                    add_surface_depth: asd,
                    secondary_range: sdr,
                    ceiling,
                });
                self.cfg.conds.len() - 1
            }
            "minecraft:water" => {
                let offset = j.get("offset").and_then(|v| v.as_i32()).unwrap_or(-1);
                let sdm = j
                    .get("surface_depth_multiplier")
                    .and_then(|v| v.as_i32())
                    .unwrap_or(0);
                let asd = j.get("add_stone_depth").and_then(crate::json::Json::as_bool).unwrap_or(false);
                self.cfg.conds.push(SrCond::Water { offset, surface_depth_multiplier: sdm, add_stone_depth: asd });
                self.cfg.conds.len() - 1
            }
            "minecraft:y_above" => {
                let anchor = match j.get("anchor").and_then(|v| parse_anchor(v, self.wg)) {
                    Some(a) => a,
                    None => { eprintln!("surface parse: y_above anchor bad: {j:?}"); return None; }
                };
                let sdm = j
                    .get("surface_depth_multiplier")
                    .and_then(|v| v.as_i32())
                    .unwrap_or(0);
                let asd = j.get("add_stone_depth").and_then(crate::json::Json::as_bool).unwrap_or(false);
                self.cfg.conds.push(SrCond::YAbove { anchor, add_stone_depth: asd, surface_depth_multiplier: sdm });
                self.cfg.conds.len() - 1
            }
            // Paper bedrock rule patch — 1.21.8 name (condition_source) and
            // 26.3 name (material_condition): same vertical_gradient shape;
            // flat bedrock off in bench worlds (generate-flat-bedrock=false).
            "paper:optionally_flat_bedrock_condition_source"
            | "paper:optionally_flat_bedrock_material_condition" => {
                let name = match j.get("random_name").and_then(|v| v.as_str()) {
                    Some(n) => canon(n),
                    None => { eprintln!("surface parse: paper bedrock without random_name"); return None; }
                };
                let ty = match j.get("true_at_and_below").and_then(|v| parse_anchor(v, self.wg)) {
                    Some(a) => a,
                    None => { eprintln!("surface parse: paper bedrock true anchor bad"); return None; }
                };
                let fy = match j.get("false_at_and_above").and_then(|v| parse_anchor(v, self.wg)) {
                    Some(a) => a,
                    None => { eprintln!("surface parse: paper bedrock false anchor bad"); return None; }
                };
                self.cfg.conds.push(SrCond::VerticalGradient { name, true_y: ty, false_y: fy });
                self.cfg.conds.len() - 1
            }
            "minecraft:vertical_gradient" => {
                let name = match j.get("random_name").and_then(|v| v.as_str()) {
                    Some(n) => canon(n),
                    None => { eprintln!("surface parse: vertical_gradient without random_name"); return None; }
                };
                let ty = match j.get("true_at_and_below").and_then(|v| parse_anchor(v, self.wg)) {
                    Some(a) => a,
                    None => { eprintln!("surface parse: vertical_gradient true anchor bad"); return None; }
                };
                let fy = match j.get("false_at_and_above").and_then(|v| parse_anchor(v, self.wg)) {
                    Some(a) => a,
                    None => { eprintln!("surface parse: vertical_gradient false anchor bad"); return None; }
                };
                self.cfg.conds.push(SrCond::VerticalGradient { name, true_y: ty, false_y: fy });
                self.cfg.conds.len() - 1
            }
            "minecraft:temperature" => {
                self.cfg.conds.push(SrCond::Temperature);
                self.cfg.conds.len() - 1
            }
            "minecraft:hole" => {
                self.cfg.conds.push(SrCond::Hole);
                self.cfg.conds.len() - 1
            }
            "minecraft:steep" => {
                self.cfg.conds.push(SrCond::Steep);
                self.cfg.conds.len() - 1
            }
            "minecraft:above_preliminary_surface" => {
                self.cfg.conds.push(SrCond::AbovePreliminary);
                self.cfg.conds.len() - 1
            }
            "minecraft:not" => {
                let inner = match j.get("invert") {
                    Some(v) => match self.cond(v) { Some(c) => c, None => { eprintln!("surface parse: not invert failed"); return None; } },
                    None => { eprintln!("surface parse: not without invert"); return None; }
                };
                self.cfg.conds.push(SrCond::Not(inner));
                self.cfg.conds.len() - 1
            }
            _ => {
                eprintln!("surface parse: unknown cond type {t}");
                return None;
            }
        };
        Some(idx)
    }
}

/// Collect every noise id referenced by the surface_rule tree
/// (noise_threshold nodes may point at pack-specific noises outside the
/// router). Returns them in deterministic order.
pub fn collect_noise_refs(root: &Json, out: &mut Vec<String>) {
    match root {
        Json::Obj(entries) => {
            let mut has_threshold = false;
            for (k, v) in entries {
                if k == "noise" {
                    if let Json::Str(id) = v {
                        out.push(id.clone());
                        has_threshold = true;
                    }
                }
                collect_noise_refs(v, out);
            }
            let _ = has_threshold;
        }
        Json::Arr(items) => {
            for v in items {
                collect_noise_refs(v, out);
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// PerlinSimplexNoise (vanilla synth) — used by the temperature conditions.
// ---------------------------------------------------------------------------

pub struct SimplexNoise {
    xo: f64,
    yo: f64,
    zo: f64,
    p: [i32; 256],
}

const SIMPLEX_GRADIENT: [[i32; 3]; 16] = [
    [1, 1, 0], [-1, 1, 0], [1, -1, 0], [-1, -1, 0],
    [1, 0, 1], [-1, 0, 1], [1, 0, -1], [-1, 0, -1],
    [0, 1, 1], [0, -1, 1], [0, 1, -1], [0, -1, -1],
    [1, 1, 0], [0, -1, 1], [-1, 1, 0], [0, -1, 0],
];

const SQRT_3: f64 = 1.7320508075688772;
const F2: f64 = 0.5 * (SQRT_3 - 1.0);
const G2: f64 = (3.0 - SQRT_3) / 6.0;

#[inline]
fn mth_floor(d: f64) -> i32 {
    let i = d as i32;
    if d < i as f64 { i - 1 } else { i }
}

impl SimplexNoise {
    pub fn new(rng: &mut LegacyRandom) -> Self {
        let mut s = SimplexNoise { xo: 0.0, yo: 0.0, zo: 0.0, p: [0; 256] };
        s.xo = rng.next_f64() * 256.0;
        s.yo = rng.next_f64() * 256.0;
        s.zo = rng.next_f64() * 256.0;
        for i in 0..256usize {
            s.p[i] = i as i32;
        }
        for i in 0..256usize {
            let j = rng.next_int_bound(256 - i as i32) as usize;
            let tmp = s.p[i];
            s.p[i] = s.p[i + j];
            s.p[i + j] = tmp;
        }
        s
    }

    #[inline]
    fn pi(&self, i: i32) -> i32 {
        self.p[(i & 255) as usize]
    }

    fn dot2(g: &[i32; 3], x: f64, y: f64) -> f64 {
        g[0] as f64 * x + g[1] as f64 * y
    }

    /// vanilla SimplexNoise.getValue(double, double)
    pub fn value2(&self, x: f64, y: f64) -> f64 {
        let d3 = (x + y) * F2;
        let n = mth_floor(x + d3);
        let n2 = mth_floor(y + d3);
        let d4 = (n + n2) as f64 * G2;
        let dx = x - ((n as f64) - d4);
        let dy = y - ((n2 as f64) - d4);
        let (i1, j1) = if dx > dy { (1, 0) } else { (0, 1) };
        let x1 = dx - i1 as f64 + G2;
        let y1 = dy - j1 as f64 + G2;
        let x2 = dx - 1.0 + 2.0 * G2;
        let y2 = dy - 1.0 + 2.0 * G2;
        let ii = n & 255;
        let jj = n2 & 255;
        let mut out = 0.0f64;
        let t0 = 0.5 - dx * dx - dy * dy;
        if t0 > 0.0 {
            let g = self.pi(ii + self.pi(jj));
            out += t0 * t0 * t0 * t0 * Self::dot2(&SIMPLEX_GRADIENT[(g % 12) as usize], dx, dy);
        }
        let t1 = 0.5 - x1 * x1 - y1 * y1;
        if t1 > 0.0 {
            let g = self.pi(ii + i1 + self.pi(jj + j1));
            out += t1 * t1 * t1 * t1 * Self::dot2(&SIMPLEX_GRADIENT[(g % 12) as usize], x1, y1);
        }
        let t2 = 0.5 - x2 * x2 - y2 * y2;
        if t2 > 0.0 {
            let g = self.pi(ii + 1 + self.pi(jj + 1));
            out += t2 * t2 * t2 * t2 * Self::dot2(&SIMPLEX_GRADIENT[(g % 12) as usize], x2, y2);
        }
        70.0 * out
    }
}

/// vanilla PerlinSimplexNoise over an amplitudes list (octave indices).
pub struct PerlinSimplexNoise {
    levels: Vec<Option<SimplexNoise>>,
    highest_freq_input_factor: f64,
    highest_freq_value_factor: f64,
}

impl PerlinSimplexNoise {
    pub fn new(rng: &mut LegacyRandom, amplitudes: &[i32]) -> Self {
        // IntRBTreeSet = sorted unique
        let mut set: Vec<i32> = amplitudes.to_vec();
        set.sort_unstable();
        set.dedup();
        let first = set[0];
        let last = set[set.len() - 1];
        let level_count = (-first + last + 1) as usize;
        let base = SimplexNoise::new(rng);
        let mut levels: Vec<Option<SimplexNoise>> = (0..level_count).map(|_| None).collect();
        if last >= 0 && (last as usize) < level_count && set.contains(&last) {
            levels[last as usize] = Some(base);
        }
        let mut i8 = last + 1;
        while i8 < level_count as i32 {
            if i8 >= 0 && set.contains(&(last - i8)) {
                levels[i8 as usize] = Some(SimplexNoise::new(rng));
            } else {
                // RandomSource.consumeCount(262) — LegacyRandom consumes
                // 262 next() calls.
                for _ in 0..262 {
                    rng.next_i32();
                }
            }
            i8 += 1;
        }
        let highest_freq_input_factor = 2f64.powi(last);
        let highest_freq_value_factor = 1.0 / (2f64.powi(level_count as i32) - 1.0);
        PerlinSimplexNoise { levels, highest_freq_input_factor, highest_freq_value_factor }
    }

    /// getValue(x, z, useSimplexY)
    pub fn value(&self, x: f64, z: f64, use_offsets: bool) -> f64 {
        let mut input = self.highest_freq_input_factor;
        let mut value = self.highest_freq_value_factor;
        let mut d = 0.0f64;
        for lv in &self.levels {
            if let Some(s) = lv {
                let xo = if use_offsets { s.xo } else { 0.0 };
                let yo = if use_offsets { s.yo } else { 0.0 };
                d += s.value2(x * input + xo, z * input + yo) * value;
            }
            input /= 2.0;
            value *= 2.0;
        }
        d
    }
}

// ---------------------------------------------------------------------------
// Shared static noises for the temperature conditions (seed-independent).
// ---------------------------------------------------------------------------

pub struct TempNoises {
    pub temperature: PerlinSimplexNoise, // LegacyRandomSource(1234), octaves [0]
    pub frozen: PerlinSimplexNoise,      // 3456, [-2,-1,0]
    pub biome_info: PerlinSimplexNoise,  // 2345, [0]
}

pub fn temp_noises() -> TempNoises {
    let mut r = LegacyRandom::new(1234);
    let temperature = PerlinSimplexNoise::new(&mut r, &[0]);
    let mut r = LegacyRandom::new(3456);
    let frozen = PerlinSimplexNoise::new(&mut r, &[-2, -1, 0]);
    let mut r = LegacyRandom::new(2345);
    let biome_info = PerlinSimplexNoise::new(&mut r, &[0]);
    TempNoises { temperature, frozen, biome_info }
}

// ---------------------------------------------------------------------------
// Config build: parse the surface_rule JSON of the resolved noise_settings.
// ---------------------------------------------------------------------------

pub struct SurfaceBuild {
    pub cfg: SurfaceConfig,
    /// noise handles ("minecraft:surface", surface_secondary,
    /// clay_bands_offset) as env noise-list indexes.
    pub noise_handles: [u32; 3],
}

pub fn build(
    wg: &WorldGen,
    ns_json: &str,
    noise_idx: &dyn Fn(&str) -> Option<u32>,
    biome_idx: &dyn Fn(&str) -> Option<u32>,
    assets: Option<&Assets>,
    dfc: Option<&dyn Fn(&str) -> Option<u32>>,
) -> Option<SurfaceBuild> {
    let j = match json::parse(ns_json) {
        Ok(v) => v,
        Err(e) => { eprintln!("surface build: ns json parse failed: {e}"); return None; }
    };
    // 1.21.8: inline "surface_rule"; 26.3: "material_rule" is a string ref
    // into material_rule/<path>.json (resolved via the embedded assets)
    let rule_json = match j.get("surface_rule") {
        Some(r) => r.clone(),
        None => match j.get("material_rule") {
            Some(Json::Str(id)) => {
                let a = match assets {
                    Some(a) => a,
                    None => { eprintln!("surface build: material_rule ref {id} but no assets"); return None; }
                };
                let path = format!("minecraft/worldgen/material_rule/{}.json", id.strip_prefix("minecraft:").unwrap_or(id));
                let bytes = match a.get(&path) {
                    Some(b) => b,
                    None => { eprintln!("surface build: material_rule not found: {path}"); return None; }
                };
                let text = match std::str::from_utf8(bytes) { Ok(t) => t, Err(_) => { eprintln!("surface build: {path} not utf-8"); return None; } };
                match json::parse(text) {
                    Ok(v) => v,
                    Err(e) => { eprintln!("surface build: {path}: {e}"); return None; }
                }
            }
            _ => { eprintln!("surface build: no surface_rule/material_rule in ns"); return None; }
        },
    };
    let default_id: u16 = 1; // stone (native STONE const)
    let mut p = Parser {
        cfg: Partial {
            rules: Vec::new(),
            conds: Vec::new(),
            states: Vec::new(),
            state_idx: std::collections::HashMap::new(),
        },
        noise_idx,
        biome_idx,
        wg,
        assets,
        dfc,
        depth: 0,
    };
    let root = p.rule(&rule_json)?;
    // bandlands terracottas are referenced only via the Bandlands rule —
    // append their states explicitly (stable order).
    let mut terracotta = [0u32; 7];
    for (i, name) in TERRACOTTA_IDS.iter().enumerate() {
        terracotta[i] = p.cfg.state(name.to_string());
    }
    let cfg = SurfaceConfig {
        rules: p.cfg.rules,
        conds: p.cfg.conds,
        root,
        states: p.cfg.states,
        ids: Vec::new(),
        terracotta,
        bandlands: Vec::new(),
        sea_level: wg.sea_level as i32,
        default_id,
        min_y: wg.min_y,
        height: wg.height,
        cell_height: wg.cell_height,
    };
    let h0 = noise_idx("minecraft:surface");
    let h1 = noise_idx("minecraft:surface_secondary");
    let h2 = noise_idx("minecraft:clay_bands_offset");
    if h0.is_none() || h1.is_none() || h2.is_none() {
        eprintln!("surface build: noise handle missing ({h0:?},{h1:?},{h2:?})");
        return None;
    }
    Some(SurfaceBuild {
        cfg,
        noise_handles: [h0.unwrap(), h1.unwrap(), h2.unwrap()],
    })
}

/// Build the 192 bandlands bands once ids are known (vanilla generateBands —
/// see contract). `factory` = RandomState.random (world-seed xoroshiro).
pub fn generate_bands(
    cfg: &SurfaceConfig,
    factory: &XoroshiroPositionalRandomFactory,
) -> Vec<u32> {
    let terr = |i: usize| -> u32 { cfg.terracotta[i] };
    // noiseRandom.fromHashOf("minecraft:clay_bands")
    let mut rng: XoroshiroRandomSource = factory.from_hash_of("minecraft:clay_bands");
    let mut bands = vec![terr(0); 192];
    let mut i: i32 = 0;
    while (i as usize) < 192 {
        i += rng.next_int_bound(5) + 1;
        if (i as usize) < 192 {
            bands[i as usize] = terr(1); // ORANGE
        }
        i += 1;
    }
    make_bands(&mut rng, &mut bands, 1, terr(3)); // YELLOW (size 1)
    make_bands(&mut rng, &mut bands, 2, terr(2)); // BROWN (size 2)
    make_bands(&mut rng, &mut bands, 1, terr(4)); // RED (size 1)
    let n = {
        // nextIntBetweenInclusive(9, 15)
        let span = 15 - 9 + 1;
        9 + rng.next_int_bound(span)
    };
    let mut placed = 0;
    let mut pos: i32 = 0;
    while placed < n && (pos as usize) < 192 {
        bands[pos as usize] = terr(5); // WHITE
        if pos - 1 >= 0 && rng.next_bool() {
            bands[(pos - 1) as usize] = terr(6); // LIGHT_GRAY
        }
        if pos + 1 < 192 && rng.next_bool() {
            bands[(pos + 1) as usize] = terr(6);
        }
        placed += 1;
        pos += rng.next_int_bound(16) + 4;
    }
    bands
}

fn make_bands(rng: &mut XoroshiroRandomSource, bands: &mut [u32], size: i32, state: u32) {
    let count = {
        let span = 15 - 6 + 1;
        6 + rng.next_int_bound(span)
    };
    let mut band = 0;
    while band < count {
        let run_len = size + rng.next_int_bound(3);
        let start = rng.next_int_bound(bands.len() as i32);
        let mut off = 0;
        while (start + off) < bands.len() as i32 && off < run_len {
            bands[(start + off) as usize] = state;
            off += 1;
        }
        band += 1;
    }
}

/// Xoroshiro factory from the world seed (RandomState.random).
pub fn root_factory(seed: i64) -> XoroshiroPositionalRandomFactory {
    let mut src = XoroshiroRandomSource::new(seed);
    src.fork_positional()
}

/// NormalNoise from env handles — the surface engine reads noises through
/// the shared env noise list (identical seeding: from_hash_of(id)).
pub fn noise_at(env: &crate::density::Env, handle: u32) -> &NormalNoise {
    &env.noise.list[handle as usize]
}
