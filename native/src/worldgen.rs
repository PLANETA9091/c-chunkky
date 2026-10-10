// M3: worldgen registry — parse the embedded vanilla worldgen JSONs into an
// arena of density-function nodes, ready for the interpreter (M4).
//
// Design (interpreter-facing):
// - `df_arena: Vec<DfNode>` is flat, index-addressed, plain mutable storage.
//   Children are arena indices; the interpreter can fold constants, attach
//   per-slot caches or precompute gradients by indexing the arena.
// - All "minecraft/worldgen/density_function/**/*.json" files are loaded
//   eagerly into `root_indices` (id -> arena index). String refs inside trees
//   resolve against that file set lazily-but-memoized, with cycle detection
//   that panics at build time with the full ref path.
// - The inline trees of noise_settings/overworld.json `noise_router` are
//   built into the same arena; `router_indices` maps the router field name
//   ("temperature", "final_density", ...) to the arena index. Note "depth"
//   (router field) is a different tree than the id "minecraft:overworld/depth".
// - Unknown "type" strings panic HERE at build time ("unknown density type"),
//   never during evaluation.
//
// Schema notes from the actual 1.21.8 asset set (verified by survey):
// - clamp uses field "input" (not "argument"); we accept "argument" as alias.
// - spline nodes nest the real payload under a "spline" key:
//   {"type":"minecraft:spline","spline":{"coordinate":<df ref|obj>,"points":
//    [{"location":f64,"value":f64|<spline obj>,"derivative":f64}]}}.
// - Types present in the df file set but easy to miss: blend_alpha,
//   end_islands (both argument-less) and weird_scaled_sampler
//   {noise, rarity_value_mapper: "type_1"|"type_2", input}.
// - add/mul/min/max always come as argument1+argument2 here; the "arguments"
//   array form is accepted and normalized to a children list.
// - shift_a/shift_b "argument" is a NOISE params id ("minecraft:offset"),
//   verified against the worldgen/noise file set at build time.
// - cell_scale, weighted_list, beardifier, bandlands, cache_all_in_cell are
//   NOT present anywhere in the 1.21.8 density-function set; they are kept
//   as variants per spec, accepted if encountered, never constructed here.

use std::collections::{BTreeMap, HashMap};

use crate::json::{parse, Json};

const DF_DIR: &str = "minecraft/worldgen/density_function/";
const NOISE_DIR: &str = "minecraft/worldgen/noise/";

// ---------------------------------------------------------------------------
// public model
// ---------------------------------------------------------------------------

/// Noise parameters, e.g. worldgen/noise/offset.json:
/// firstOctave=-3, amplitudes=[1,1,1,0].
#[derive(Debug, Clone, PartialEq)]
pub struct NoiseParams {
    pub first_octave: i32,
    pub amplitudes: Vec<f64>,
}

/// Reference to noise parameters: id into `WorldGen::noise_params`, or inline
/// params embedded directly in a density-function tree.
#[derive(Debug, Clone, PartialEq)]
pub enum NoiseSource {
    Id(String),
    Inline(NoiseParams),
}

/// One spline point. `value` is a constant or another (nested) spline that
/// was built into the arena.
#[derive(Debug, Clone, PartialEq)]
pub struct SplinePoint {
    pub location: f64,
    pub value: SplineValue,
    pub derivative: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SplineValue {
    Const(f64),
    /// Arena index of the nested spline node.
    Node(usize),
}

/// A density-function node, stored raw (no interpretation here). Numeric
/// field names mirror the vanilla JSON keys; children are arena indices.
#[derive(Debug, Clone, PartialEq)]
pub enum DfNode {
    /// Bare JSON number, or {"type":"minecraft:constant","argument":n}.
    Constant(f64),
    Noise {
        noise: NoiseSource,
        xz_scale: f64,
        y_scale: f64,
    },
    ShiftedNoise {
        noise: NoiseSource,
        xz_scale: f64,
        y_scale: f64,
        shift_x: usize,
        shift_y: usize,
        shift_z: usize,
    },
    /// "argument" is a noise-params id (not a density function).
    ShiftA { noise: NoiseSource },
    /// "argument" is a noise-params id (not a density function).
    ShiftB { noise: NoiseSource },
    BlendDensity { argument: usize },
    BlendOffset,
    /// Argument-less: appears in overworld{,_amplified,_large_biomes}
    /// offset/factor/jaggedness; evaluates to 1.0 far from chunk borders.
    BlendAlpha,
    /// Argument-less; used by end/sloped_cheese.
    EndIslands,
    /// Vanilla 1.21.8 field is "input"; we accept the "argument" alias.
    Clamp { input: usize, min: f64, max: f64 },
    Abs { argument: usize },
    Square { argument: usize },
    Cube { argument: usize },
    HalfNegative { argument: usize },
    QuarterNegative { argument: usize },
    Squeeze { argument: usize },
    /// argument1+argument2, or "arguments":[...], normalized to a list.
    Add { children: Vec<usize> },
    Mul { children: Vec<usize> },
    Min { children: Vec<usize> },
    Max { children: Vec<usize> },
    RangeChoice {
        input: usize,
        min_inclusive: f64,
        max_exclusive: f64,
        when_in_range: usize,
        when_out_of_range: usize,
    },
    YClampedGradient {
        from_y: i32,
        to_y: i32,
        from_value: f64,
        to_value: f64,
    },
    Interpolated { argument: usize },
    FlatCache { argument: usize },
    Cache2d { argument: usize },
    CacheOnce { argument: usize },
    CacheAllInCell { argument: usize },
    /// Not present in the 1.21.8 asset set (kept per spec; "scale" is the
    /// speculative field name — no file to verify against).
    CellScale { argument: usize, scale: f64 },
    OldBlendedNoise {
        xz_scale: f64,
        y_scale: f64,
        xz_factor: f64,
        y_factor: f64,
        smear_scale_multiplier: f64,
    },
    /// Outer JSON: {"type":"minecraft:spline","spline":{"coordinate":...}}.
    Spline {
        coordinate: usize,
        points: Vec<SplinePoint>,
    },
    Beardifier,
    /// Only appears in surface_rule JSON in this asset set (different
    /// schema); kept as a node per spec, never constructed by the loader.
    Bandlands,
    WeightedList { distribution: Vec<(usize, f64)> },
    /// {"noise": id|obj, "rarity_value_mapper": "type_1"|"type_2", "input"}.
    WeirdScaledSampler {
        noise: NoiseSource,
        rarity_value_mapper: String,
        input: usize,
    },
}

impl DfNode {
    pub fn kind(&self) -> &'static str {
        match self {
            DfNode::Constant(_) => "constant",
            DfNode::Noise { .. } => "noise",
            DfNode::ShiftedNoise { .. } => "shifted_noise",
            DfNode::ShiftA { .. } => "shift_a",
            DfNode::ShiftB { .. } => "shift_b",
            DfNode::BlendDensity { .. } => "blend_density",
            DfNode::BlendOffset => "blend_offset",
            DfNode::BlendAlpha => "blend_alpha",
            DfNode::EndIslands => "end_islands",
            DfNode::Clamp { .. } => "clamp",
            DfNode::Abs { .. } => "abs",
            DfNode::Square { .. } => "square",
            DfNode::Cube { .. } => "cube",
            DfNode::HalfNegative { .. } => "half_negative",
            DfNode::QuarterNegative { .. } => "quarter_negative",
            DfNode::Squeeze { .. } => "squeeze",
            DfNode::Add { .. } => "add",
            DfNode::Mul { .. } => "mul",
            DfNode::Min { .. } => "min",
            DfNode::Max { .. } => "max",
            DfNode::RangeChoice { .. } => "range_choice",
            DfNode::YClampedGradient { .. } => "y_clamped_gradient",
            DfNode::Interpolated { .. } => "interpolated",
            DfNode::FlatCache { .. } => "flat_cache",
            DfNode::Cache2d { .. } => "cache_2d",
            DfNode::CacheOnce { .. } => "cache_once",
            DfNode::CacheAllInCell { .. } => "cache_all_in_cell",
            DfNode::CellScale { .. } => "cell_scale",
            DfNode::OldBlendedNoise { .. } => "old_blended_noise",
            DfNode::Spline { .. } => "spline",
            DfNode::Beardifier => "beardifier",
            DfNode::Bandlands => "bandlands",
            DfNode::WeightedList { .. } => "weighted_list",
            DfNode::WeirdScaledSampler { .. } => "weird_scaled_sampler",
        }
    }

    /// Child arena indices (debug/inspection use; splines expose their
    /// coordinate, points are reached via the Spline variant itself).
    pub fn children(&self) -> Vec<usize> {
        match self {
            DfNode::ShiftedNoise { shift_x, shift_y, shift_z, .. } => {
                vec![*shift_x, *shift_y, *shift_z]
            }
            DfNode::BlendDensity { argument }
            | DfNode::Clamp { input: argument, .. }
            | DfNode::Abs { argument }
            | DfNode::Square { argument }
            | DfNode::Cube { argument }
            | DfNode::HalfNegative { argument }
            | DfNode::QuarterNegative { argument }
            | DfNode::Squeeze { argument }
            | DfNode::Interpolated { argument }
            | DfNode::FlatCache { argument }
            | DfNode::Cache2d { argument }
            | DfNode::CacheOnce { argument }
            | DfNode::CacheAllInCell { argument }
            | DfNode::CellScale { argument, .. } => vec![*argument],
            DfNode::Add { children }
            | DfNode::Mul { children }
            | DfNode::Min { children }
            | DfNode::Max { children } => children.clone(),
            DfNode::RangeChoice { input, when_in_range, when_out_of_range, .. } => {
                vec![*input, *when_in_range, *when_out_of_range]
            }
            DfNode::Spline { coordinate, .. } => vec![*coordinate],
            DfNode::WeightedList { distribution } => {
                distribution.iter().map(|(i, _)| *i).collect()
            }
            DfNode::WeirdScaledSampler { input, .. } => vec![*input],
            _ => Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// WorldGen
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct WorldGen {
    /// Noise parameters by id, e.g. "minecraft:offset".
    pub noise_params: HashMap<String, NoiseParams>,
    /// Flat arena of density-function nodes.
    pub df_arena: Vec<DfNode>,
    /// Density-function id ("minecraft:overworld/depth") -> arena index.
    pub root_indices: HashMap<String, usize>,
    /// Inline noise_router trees of the loaded noise_settings, keyed by the
    /// router field name ("temperature", "final_density", ...).
    pub router_indices: HashMap<String, usize>,
    pub sea_level: i64,
    pub aquifers_enabled: bool,
    pub default_block_is_stone: bool,
    // extras recorded for later milestones:
    pub min_y: i32,
    pub height: i32,
    pub legacy_random_source: bool,
    pub ore_veins_enabled: bool,
    /// Constructed node kinds -> counts (unique nodes; shared refs counted
    /// once). Built at load time, useful as a sanity surface for the
    /// interpreter milestone.
    pub type_counts: BTreeMap<String, usize>,
}

impl WorldGen {
    /// Load the default (overworld) worldgen registry from the embedded
    /// assets. `seed_ctx_notes` is reserved for later milestones
    /// (legacy-random-source selection etc.); parsing is seed-independent.
    pub fn load_default(seed_ctx_notes: &str) -> WorldGen {
        let _ = seed_ctx_notes;
        Self::load("minecraft/worldgen/noise_settings/overworld.json")
    }

    /// Build the registry for one noise_settings asset.
    pub fn load(noise_settings_asset: &str) -> WorldGen {
        // 1) noise parameters: worldgen/noise/*.json
        let mut noise_params = HashMap::new();
        for path in crate::ASSET_PATHS {
            let Some(stem) = path.strip_prefix(NOISE_DIR).and_then(|r| r.strip_suffix(".json"))
            else {
                continue;
            };
            let bytes = crate::asset(path).expect("manifest path must exist");
            let text = std::str::from_utf8(bytes).unwrap_or_else(|_| panic!("{path}: not utf-8"));
            let json = parse(text).unwrap_or_else(|e| panic!("{path}: json error: {e}"));
            let id = format!("minecraft:{stem}");
            noise_params.insert(id, parse_noise_params(&json, path));
        }

        // 2) every density-function file, eagerly resolved. ASSET_PATHS is
        //    sorted, so arena order is deterministic across builds.
        let mut loader = Loader {
            noise_params: &noise_params,
            arena: Vec::new(),
            roots: HashMap::new(),
            building: Vec::new(),
            type_counts: BTreeMap::new(),
        };
        for path in crate::ASSET_PATHS {
            if let Some(stem) = path.strip_prefix(DF_DIR).and_then(|r| r.strip_suffix(".json")) {
                loader.resolve(&format!("minecraft:{stem}"));
            }
        }

        // 3) noise_settings header fields
        let bytes = crate::asset(noise_settings_asset)
            .unwrap_or_else(|| panic!("unknown noise settings asset: {noise_settings_asset}"));
        let text = std::str::from_utf8(bytes).unwrap_or_else(|_| panic!("{noise_settings_asset}: not utf-8"));
        let ns = parse(text).unwrap_or_else(|e| panic!("{noise_settings_asset}: json error: {e}"));
        let sea_level = ns
            .get("sea_level")
            .and_then(Json::as_i64)
            .unwrap_or_else(|| panic!("{noise_settings_asset}: missing sea_level"));
        let aquifers_enabled = matches!(ns.get("aquifers_enabled"), Some(Json::Bool(true)));
        let default_block_is_stone = ns
            .get("default_block")
            .and_then(|b| b.get("Name"))
            .and_then(Json::as_str)
            == Some("minecraft:stone");
        let noise_obj = ns
            .get("noise")
            .unwrap_or_else(|| panic!("{noise_settings_asset}: missing noise object"));
        let min_y = noise_obj
            .get("min_y")
            .and_then(Json::as_i32)
            .unwrap_or_else(|| panic!("{noise_settings_asset}: missing noise.min_y"));
        let height = noise_obj
            .get("height")
            .and_then(Json::as_i32)
            .unwrap_or_else(|| panic!("{noise_settings_asset}: missing noise.height"));
        let legacy_random_source =
            matches!(ns.get("legacy_random_source"), Some(Json::Bool(true)));
        let ore_veins_enabled = matches!(ns.get("ore_veins_enabled"), Some(Json::Bool(true)));

        // 4) inline noise_router trees (string refs hit the memoized roots)
        let router = ns
            .get("noise_router")
            .unwrap_or_else(|| panic!("{noise_settings_asset}: missing noise_router"));
        let Json::Obj(entries) = router else {
            panic!("{noise_settings_asset}: noise_router is not an object");
        };
        let mut router_indices = HashMap::new();
        for (k, v) in entries {
            router_indices.insert(k.clone(), loader.build_value(v));
        }

        // destructure the loader: ends its borrow of `noise_params` so the
        // map can be moved into the result below.
        let Loader { arena, roots, type_counts, .. } = loader;

        WorldGen {
            noise_params,
            df_arena: arena,
            root_indices: roots,
            router_indices,
            sea_level,
            aquifers_enabled,
            default_block_is_stone,
            min_y,
            height,
            legacy_random_source,
            ore_veins_enabled,
            type_counts,
        }
    }

    /// Depth-limited textual tree shape, for debugging/tests.
    pub fn tree_shape(&self, root: usize, max_depth: u32) -> String {
        let mut out = String::new();
        self.shape_rec(root, max_depth, 0, &mut out);
        out
    }

    fn shape_rec(&self, idx: usize, max_depth: u32, d: u32, out: &mut String) {
        let pad = "  ".repeat(d as usize);
        if d > max_depth {
            out.push_str(&pad);
            out.push_str("...\n");
            return;
        }
        let node = &self.df_arena[idx];
        match node {
            DfNode::Constant(c) => {
                out.push_str(&format!("{pad}const {c}\n"));
            }
            DfNode::Spline { coordinate, points } => {
                out.push_str(&format!("{pad}spline ({} points)\n", points.len()));
                self.shape_rec(*coordinate, max_depth, d + 1, out);
                for (i, p) in points.iter().enumerate() {
                    let ppad = "  ".repeat(d as usize + 1);
                    match &p.value {
                        SplineValue::Const(v) => out.push_str(&format!(
                            "{ppad}pt[{i}] loc={} -> const {v} (deriv {})\n",
                            p.location, p.derivative
                        )),
                        SplineValue::Node(ni) => {
                            out.push_str(&format!(
                                "{ppad}pt[{i}] loc={} -> node {ni} (deriv {}):\n",
                                p.location, p.derivative
                            ));
                            self.shape_rec(*ni, max_depth, d + 2, out);
                        }
                    }
                }
            }
            other => {
                out.push_str(&pad);
                out.push_str(&describe(other));
                out.push('\n');
                for c in other.children() {
                    self.shape_rec(c, max_depth, d + 1, out);
                }
            }
        }
    }
}

fn describe(n: &DfNode) -> String {
    match n {
        DfNode::Noise { noise, xz_scale, y_scale } => format!(
            "noise {} xz_scale={xz_scale} y_scale={y_scale}",
            noise_label(noise)
        ),
        DfNode::ShiftedNoise { noise, xz_scale, y_scale, shift_x, shift_y, shift_z } => format!(
            "shifted_noise {} xz_scale={xz_scale} y_scale={y_scale} shifts=[{shift_x},{shift_y},{shift_z}]",
            noise_label(noise)
        ),
        DfNode::ShiftA { noise } => format!("shift_a {}", noise_label(noise)),
        DfNode::ShiftB { noise } => format!("shift_b {}", noise_label(noise)),
        DfNode::YClampedGradient { from_y, to_y, from_value, to_value } => format!(
            "y_clamped_gradient y=[{from_y}..{to_y}] v=[{from_value}..{to_value}]"
        ),
        DfNode::Clamp { min, max, .. } => format!("clamp min={min} max={max}"),
        DfNode::OldBlendedNoise { xz_scale, y_scale, xz_factor, y_factor, smear_scale_multiplier } => {
            format!(
                "old_blended_noise xz_scale={xz_scale} y_scale={y_scale} xz_factor={xz_factor} y_factor={y_factor} smear={smear_scale_multiplier}"
            )
        }
        DfNode::WeirdScaledSampler { noise, rarity_value_mapper, .. } => format!(
            "weird_scaled_sampler {} mapper={rarity_value_mapper}",
            noise_label(noise)
        ),
        DfNode::Add { children }
        | DfNode::Mul { children }
        | DfNode::Min { children }
        | DfNode::Max { children } => format!("{} ({} children)", n.kind(), children.len()),
        other => other.kind().to_string(),
    }
}

fn noise_label(n: &NoiseSource) -> String {
    match n {
        NoiseSource::Id(s) => s.clone(),
        NoiseSource::Inline(p) => format!(
            "<inline first_octave={} amplitudes={}>",
            p.first_octave,
            p.amplitudes.len()
        ),
    }
}

// ---------------------------------------------------------------------------
// loader internals
// ---------------------------------------------------------------------------

fn parse_noise_params(v: &Json, ctx: &str) -> NoiseParams {
    let first_octave = v
        .get("firstOctave")
        .and_then(Json::as_i32)
        .unwrap_or_else(|| panic!("{ctx}: missing/invalid firstOctave"));
    let amps = v
        .get("amplitudes")
        .and_then(Json::as_arr)
        .unwrap_or_else(|| panic!("{ctx}: missing amplitudes"));
    let amplitudes = amps
        .iter()
        .map(|a| a.as_f64().unwrap_or_else(|| panic!("{ctx}: non-numeric amplitude {a:?}")))
        .collect();
    NoiseParams { first_octave, amplitudes }
}

fn req_field<'j>(v: &'j Json, key: &str, ty: &str) -> &'j Json {
    v.get(key)
        .unwrap_or_else(|| panic!("{ty}: missing field '{key}'"))
}

fn req_num(v: &Json, key: &str, ty: &str) -> f64 {
    req_field(v, key, ty)
        .as_f64()
        .unwrap_or_else(|| panic!("{ty}: field '{key}' is not a number"))
}

fn req_int(v: &Json, key: &str, ty: &str) -> i32 {
    req_field(v, key, ty)
        .as_i32()
        .unwrap_or_else(|| panic!("{ty}: field '{key}' is not an integer"))
}

struct Loader<'a> {
    noise_params: &'a HashMap<String, NoiseParams>,
    arena: Vec<DfNode>,
    roots: HashMap<String, usize>,
    /// ids currently being expanded — a repeat means a ref cycle.
    building: Vec<String>,
    type_counts: BTreeMap<String, usize>,
}

impl Loader<'_> {
    fn push(&mut self, node: DfNode) -> usize {
        let kind = node.kind();
        let idx = self.arena.len();
        self.arena.push(node);
        *self.type_counts.entry(kind.to_string()).or_insert(0) += 1;
        idx
    }

    /// Resolve a density-function id ("minecraft:overworld/offset") to its
    /// root arena index. Memoized; panics on ref cycles with the full path.
    fn resolve(&mut self, id: &str) -> usize {
        if let Some(&i) = self.roots.get(id) {
            return i;
        }
        if self.building.iter().any(|b| b == id) {
            let mut path = self.building.join(" -> ");
            path.push_str(" -> ");
            path.push_str(id);
            panic!("density function cycle: {path}");
        }
        let rel = id.strip_prefix("minecraft:").unwrap_or_else(|| {
            panic!("density function ref without namespace: {id}");
        });
        let path = format!("{DF_DIR}{rel}.json");
        let bytes = crate::asset(&path)
            .unwrap_or_else(|| panic!("unknown density function ref: {id}"));
        let text = std::str::from_utf8(bytes).unwrap_or_else(|_| panic!("{path}: not utf-8"));
        let json = parse(text).unwrap_or_else(|e| panic!("{path}: json error: {e}"));
        self.building.push(id.to_string());
        let idx = self.build_value(&json);
        self.building.pop();
        self.roots.insert(id.to_string(), idx);
        idx
    }

    /// A density-function value: bare number | string ref | node object.
    fn build_value(&mut self, v: &Json) -> usize {
        match v {
            Json::Num(n) => self.push(DfNode::Constant(*n)),
            Json::Str(s) => self.resolve(s),
            Json::Obj(_) => {
                let ty = v
                    .get("type")
                    .and_then(Json::as_str)
                    .unwrap_or_else(|| panic!("density function object without type: {v:?}"));
                self.build_node(v, ty)
            }
            other => panic!("bad density function value: {other:?}"),
        }
    }

    fn build_node(&mut self, v: &Json, ty: &str) -> usize {
        match ty {
            "minecraft:constant" => {
                let n = req_num(v, "argument", ty);
                self.push(DfNode::Constant(n))
            }
            "minecraft:noise" => {
                let noise = self.noise_source(req_field(v, "noise", ty), ty);
                let xz_scale = req_num(v, "xz_scale", ty);
                let y_scale = req_num(v, "y_scale", ty);
                self.push(DfNode::Noise { noise, xz_scale, y_scale })
            }
            "minecraft:shifted_noise" => {
                let noise = self.noise_source(req_field(v, "noise", ty), ty);
                let xz_scale = req_num(v, "xz_scale", ty);
                let y_scale = req_num(v, "y_scale", ty);
                let shift_x = self.build_value(req_field(v, "shift_x", ty));
                let shift_y = self.build_value(req_field(v, "shift_y", ty));
                let shift_z = self.build_value(req_field(v, "shift_z", ty));
                self.push(DfNode::ShiftedNoise { noise, xz_scale, y_scale, shift_x, shift_y, shift_z })
            }
            "minecraft:shift_a" | "minecraft:shift_b" => {
                // "argument" is a noise-params id, not a density function.
                let noise = self.noise_source(req_field(v, "argument", ty), ty);
                if ty == "minecraft:shift_a" {
                    self.push(DfNode::ShiftA { noise })
                } else {
                    self.push(DfNode::ShiftB { noise })
                }
            }
            "minecraft:blend_density" => {
                let argument = self.build_value(req_field(v, "argument", ty));
                self.push(DfNode::BlendDensity { argument })
            }
            "minecraft:blend_offset" => self.push(DfNode::BlendOffset),
            "minecraft:blend_alpha" => self.push(DfNode::BlendAlpha),
            "minecraft:end_islands" => self.push(DfNode::EndIslands),
            "minecraft:beardifier" => self.push(DfNode::Beardifier),
            "minecraft:bandlands" => self.push(DfNode::Bandlands),
            "minecraft:clamp" => {
                // 1.21.8 uses "input"; accept "argument" as an alias.
                let input_json = v
                    .get("input")
                    .or_else(|| v.get("argument"))
                    .unwrap_or_else(|| panic!("{ty}: missing field 'input'"));
                let input = self.build_value(input_json);
                let min = req_num(v, "min", ty);
                let max = req_num(v, "max", ty);
                self.push(DfNode::Clamp { input, min, max })
            }
            "minecraft:abs"
            | "minecraft:square"
            | "minecraft:cube"
            | "minecraft:half_negative"
            | "minecraft:quarter_negative"
            | "minecraft:squeeze"
            | "minecraft:interpolated"
            | "minecraft:flat_cache"
            | "minecraft:cache_2d"
            | "minecraft:cache_once"
            | "minecraft:cache_all_in_cell" => {
                let argument = self.build_value(req_field(v, "argument", ty));
                let node = match ty {
                    "minecraft:abs" => DfNode::Abs { argument },
                    "minecraft:square" => DfNode::Square { argument },
                    "minecraft:cube" => DfNode::Cube { argument },
                    "minecraft:half_negative" => DfNode::HalfNegative { argument },
                    "minecraft:quarter_negative" => DfNode::QuarterNegative { argument },
                    "minecraft:squeeze" => DfNode::Squeeze { argument },
                    "minecraft:interpolated" => DfNode::Interpolated { argument },
                    "minecraft:flat_cache" => DfNode::FlatCache { argument },
                    "minecraft:cache_2d" => DfNode::Cache2d { argument },
                    "minecraft:cache_once" => DfNode::CacheOnce { argument },
                    _ => DfNode::CacheAllInCell { argument },
                };
                self.push(node)
            }
            "minecraft:add" | "minecraft:mul" | "minecraft:min" | "minecraft:max" => {
                let children = self.build_children(v, ty);
                let node = match ty {
                    "minecraft:add" => DfNode::Add { children },
                    "minecraft:mul" => DfNode::Mul { children },
                    "minecraft:min" => DfNode::Min { children },
                    _ => DfNode::Max { children },
                };
                self.push(node)
            }
            "minecraft:range_choice" => {
                let input = self.build_value(req_field(v, "input", ty));
                let min_inclusive = req_num(v, "min_inclusive", ty);
                let max_exclusive = req_num(v, "max_exclusive", ty);
                let when_in_range = self.build_value(req_field(v, "when_in_range", ty));
                let when_out_of_range = self.build_value(req_field(v, "when_out_of_range", ty));
                self.push(DfNode::RangeChoice {
                    input,
                    min_inclusive,
                    max_exclusive,
                    when_in_range,
                    when_out_of_range,
                })
            }
            "minecraft:y_clamped_gradient" => {
                let from_y = req_int(v, "from_y", ty);
                let to_y = req_int(v, "to_y", ty);
                let from_value = req_num(v, "from_value", ty);
                let to_value = req_num(v, "to_value", ty);
                self.push(DfNode::YClampedGradient { from_y, to_y, from_value, to_value })
            }
            "minecraft:cell_scale" => {
                let argument = self.build_value(req_field(v, "argument", ty));
                let scale = req_num(v, "scale", ty);
                self.push(DfNode::CellScale { argument, scale })
            }
            "minecraft:old_blended_noise" => {
                let xz_scale = req_num(v, "xz_scale", ty);
                let y_scale = req_num(v, "y_scale", ty);
                let xz_factor = req_num(v, "xz_factor", ty);
                let y_factor = req_num(v, "y_factor", ty);
                let smear_scale_multiplier = req_num(v, "smear_scale_multiplier", ty);
                self.push(DfNode::OldBlendedNoise {
                    xz_scale,
                    y_scale,
                    xz_factor,
                    y_factor,
                    smear_scale_multiplier,
                })
            }
            "minecraft:spline" => {
                // Outer node nests the payload under "spline"; accept the
                // flat {"coordinate","points"} form defensively.
                let inner = v.get("spline").unwrap_or(v);
                self.build_spline_payload(inner, ty)
            }
            "minecraft:weird_scaled_sampler" => {
                let noise = self.noise_source(req_field(v, "noise", ty), ty);
                let rarity_value_mapper = req_field(v, "rarity_value_mapper", ty)
                    .as_str()
                    .unwrap_or_else(|| panic!("{ty}: rarity_value_mapper is not a string"))
                    .to_string();
                let input = self.build_value(req_field(v, "input", ty));
                self.push(DfNode::WeirdScaledSampler { noise, rarity_value_mapper, input })
            }
            "minecraft:weighted_list" => {
                let dist = req_field(v, "distribution", ty)
                    .as_arr()
                    .unwrap_or_else(|| panic!("{ty}: distribution is not an array"));
                let mut distribution = Vec::with_capacity(dist.len());
                for e in dist {
                    let input = self.build_value(req_field(e, "input", ty));
                    let weight = req_num(e, "weight", ty);
                    distribution.push((input, weight));
                }
                self.push(DfNode::WeightedList { distribution })
            }
            other => panic!("unknown density type: {other}"),
        }
    }

    /// Spline payload: {"coordinate": <df value>, "points": [...]}. Used
    /// both for typed spline nodes and for the typeless nested splines that
    /// appear as point values (schema quirk of 1.21.8 — see worklog).
    fn build_spline_payload(&mut self, inner: &Json, ty: &str) -> usize {
        let coordinate = self.build_value(req_field(inner, "coordinate", ty));
        let pts = req_field(inner, "points", ty)
            .as_arr()
            .unwrap_or_else(|| panic!("{ty}: points is not an array"));
        if pts.is_empty() {
            panic!("{ty}: spline with no points");
        }
        let mut points = Vec::with_capacity(pts.len());
        for p in pts {
            let location = req_num(p, "location", ty);
            let derivative = req_num(p, "derivative", ty);
            let value = match p.get("value") {
                Some(Json::Num(n)) => SplineValue::Const(*n),
                Some(other) => SplineValue::Node(self.build_spline_value(other, ty)),
                None => panic!("{ty}: spline point missing value"),
            };
            points.push(SplinePoint { location, value, derivative });
        }
        self.push(DfNode::Spline { coordinate, points })
    }

    /// A spline point value: constant | df ref/obj | typeless nested spline
    /// {"coordinate","points"} (no "type" key in the vanilla data!).
    fn build_spline_value(&mut self, v: &Json, ty: &str) -> usize {
        match v {
            Json::Str(s) => self.resolve(s),
            // Json::Obj is a tuple-vec; use Json::get on the value itself.
            Json::Obj(_) if v.get("type").is_none() => self.build_spline_payload(v, ty),
            Json::Obj(_) => self.build_value(v),
            other => panic!("{ty}: spline point value is not a number/object: {other:?}"),
        }
    }

    /// add/mul/min/max: argument1+argument2, or the "arguments" array form
    /// (normalized to a children list).
    fn build_children(&mut self, v: &Json, ty: &str) -> Vec<usize> {
        if let Some(arr) = v.get("arguments").and_then(Json::as_arr) {
            return arr.iter().map(|a| self.build_value(a)).collect();
        }
        let a = self.build_value(req_field(v, "argument1", ty));
        let b = self.build_value(req_field(v, "argument2", ty));
        vec![a, b]
    }

    /// Noise id string (verified against the noise param set) or inline
    /// {"firstOctave","amplitudes"} object.
    fn noise_source(&self, v: &Json, ty: &str) -> NoiseSource {
        match v {
            Json::Str(s) => {
                if !self.noise_params.contains_key(s) {
                    panic!("{ty}: unknown noise params id: {s}");
                }
                NoiseSource::Id(s.clone())
            }
            Json::Obj(_) => NoiseSource::Inline(parse_noise_params(v, ty)),
            other => panic!("{ty}: noise field is neither id nor object: {other:?}"),
        }
    }
}

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// The 15 noise_router fields of noise_settings/overworld.json.
    const ROUTER_KEYS: &[&str] = &[
        "barrier",
        "continents",
        "depth",
        "erosion",
        "final_density",
        "fluid_level_floodedness",
        "fluid_level_spread",
        "initial_density_without_jaggedness",
        "lava",
        "ridges",
        "temperature",
        "vegetation",
        "vein_gap",
        "vein_ridged",
        "vein_toggle",
    ];

    #[test]
    fn overworld_registry_loads() {
        let wg = WorldGen::load_default("unit-test");
        assert_eq!(wg.sea_level, 63);
        assert!(wg.aquifers_enabled);
        assert!(wg.default_block_is_stone);
        assert_eq!(wg.min_y, -64);
        assert_eq!(wg.height, 384);
        assert!(!wg.legacy_random_source);

        // every router entry resolves to a live arena node
        for k in ROUTER_KEYS {
            let idx = wg
                .router_indices
                .get(*k)
                .unwrap_or_else(|| panic!("router entry '{k}' missing"));
            assert!(*idx < wg.df_arena.len(), "router entry {k}: bad index");
        }
        assert_eq!(wg.router_indices.len(), ROUTER_KEYS.len());

        // every root index is live
        assert_eq!(wg.root_indices.len(), 35, "density_function file count");
        for (id, idx) in &wg.root_indices {
            assert!(*idx < wg.df_arena.len(), "root {id}: bad index");
        }

        // noise params sanity: worldgen/noise/offset.json
        let off = &wg.noise_params["minecraft:offset"];
        assert_eq!(off.first_octave, -3);
        assert_eq!(off.amplitudes, vec![1.0, 1.0, 1.0, 0.0]);

        // distinct types seen while building
        for t in ["add", "mul", "spline", "blend_alpha", "weird_scaled_sampler", "old_blended_noise"] {
            assert!(wg.type_counts.contains_key(t), "expected type '{t}' in arena");
        }

        let total: usize = wg.type_counts.values().sum();
        println!("== worldgen registry (overworld) ==");
        println!("   df files loaded : {}", wg.root_indices.len());
        println!("   arena nodes     : {} (constructed {})", wg.df_arena.len(), total);
        println!("   noise params    : {}", wg.noise_params.len());
        println!("   router entries  : {}", wg.router_indices.len());
        for (t, c) in &wg.type_counts {
            println!("   {c:5}  {t}");
        }
    }

    #[test]
    fn depth_is_add_with_two_children() {
        let wg = WorldGen::load_default("unit-test");
        let root = wg.root_indices["minecraft:overworld/depth"];
        let DfNode::Add { children } = &wg.df_arena[root] else {
            panic!("overworld/depth root is not an add node");
        };
        assert_eq!(children.len(), 2, "depth add must have exactly 2 children");
        assert!(
            matches!(wg.df_arena[children[0]], DfNode::YClampedGradient { .. }),
            "depth child 0 should be the y_clamped_gradient"
        );
        assert!(
            matches!(wg.df_arena[children[1]], DfNode::FlatCache { .. }),
            "depth child 1 should be overworld/offset (a flat_cache)"
        );
        // memoized ref: depth's argument2 shares offset's own root node
        assert_eq!(
            children[1],
            wg.root_indices["minecraft:overworld/offset"],
            "offset subtree should be shared, not duplicated"
        );
        println!(
            "== minecraft:overworld/depth (root {root}) ==\n{}",
            wg.tree_shape(root, 4)
        );
    }

    #[test]
    fn offset_resolves_to_spline_chain() {
        let wg = WorldGen::load_default("unit-test");
        let off = wg.root_indices["minecraft:overworld/offset"];
        assert!(matches!(wg.df_arena[off], DfNode::FlatCache { .. }));

        // find spline nodes below the offset root
        let mut splines: Vec<usize> = Vec::new();
        let mut seen: Vec<usize> = Vec::new();
        collect(&wg, off, &mut splines, &mut seen);
        assert!(!splines.is_empty(), "no spline below overworld/offset");

        // the first spline's coordinate must resolve to the shared root node
        // for minecraft:overworld/continents (a flat_cache of shifted_noise
        // over continentalness in 1.21.8 — NOT itself a spline; the spline
        // evaluates the coordinate density function for its input).
        let DfNode::Spline { coordinate, points } = &wg.df_arena[splines[0]] else {
            unreachable!()
        };
        assert!(!points.is_empty());
        assert_eq!(
            *coordinate,
            wg.root_indices["minecraft:overworld/continents"],
            "offset spline coordinate should be the memoized continents root"
        );
        // the spline chain lives in the POINT values: at least one point
        // nests a further spline (typeless {coordinate,points} objects).
        assert!(
            points.iter().any(|p| matches!(p.value, SplineValue::Node(_))),
            "offset spline points should nest further splines"
        );
        println!(
            "== minecraft:overworld/offset (root {off}, splines: {}) ==\n{}",
            splines.len(),
            wg.tree_shape(off, 5)
        );
    }

    fn collect(wg: &WorldGen, idx: usize, out: &mut Vec<usize>, seen: &mut Vec<usize>) {
        if seen.contains(&idx) {
            return;
        }
        seen.push(idx);
        if matches!(wg.df_arena[idx], DfNode::Spline { .. }) {
            out.push(idx);
        }
        for c in wg.df_arena[idx].children() {
            collect(wg, c, out, seen);
        }
    }
}
