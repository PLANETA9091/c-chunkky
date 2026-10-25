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

const DF_DIR: &str = "worldgen/density_function/";
const NOISE_DIR: &str = "worldgen/noise/";

// ---------------------------------------------------------------------------
// Assets — pack overrides over the embedded vanilla manifest
// ---------------------------------------------------------------------------

/// Worldgen JSONs provided by world datapacks, keyed by repo-relative path
/// "<namespace>/worldgen/<...>.json" (the plugin strips the leading "data/").
/// Lookups fall back to the embedded vanilla manifest, so a partial pack
/// (e.g. crussty-custom sea-level override) keeps resolving vanilla refs.
#[derive(Debug, Default)]
pub struct Assets {
    files: HashMap<String, Vec<u8>>,
}

impl Assets {
    pub fn new() -> Self {
        Assets { files: HashMap::new() }
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    pub fn insert(&mut self, path: impl Into<String>, bytes: Vec<u8>) {
        self.files.insert(path.into(), bytes);
    }

    pub fn get(&self, path: &str) -> Option<&[u8]> {
        if let Some(b) = self.files.get(path) {
            return Some(b.as_slice());
        }
        crate::asset(path)
    }

    /// Pack-provided paths (deterministic order for scans).
    pub fn paths(&self) -> impl Iterator<Item = &String> {
        self.files.keys()
    }

    /// Number of pack-provided files (0 = embedded-vanilla mode).
    pub fn len(&self) -> usize {
        self.files.len()
    }

    /// Union of embedded manifest paths and pack paths carrying
    /// "<marker><stem>.json" (marker like "worldgen/noise/"), normalized to
    /// "<namespace>:<stem>" ids, sorted (deterministic arena order).
    fn ids_with(&self, marker: &str) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for p in crate::ASSET_PATHS {
            if let Some(id) = id_of_path(p, marker) {
                out.push(id);
            }
        }
        for p in self.files.keys() {
            if let Some(id) = id_of_path(p, marker) {
                out.push(id);
            }
        }
        out.sort();
        out.dedup();
        out
    }

    /// Resolve the winning overworld noise_settings id ("minecraft:overworld")
    /// from a pack-provided dimension override (generator.settings), or the
    /// vanilla default. The plugin applies pack merge order (later wins)
    /// before handing the map over, so at most one dimension file is present;
    /// on the unlikely multi-namespace case the smallest path wins
    /// deterministically.
    pub fn resolve_noise_settings(&self) -> String {
        let mut dim_path: Option<&String> = None;
        for p in self.files.keys() {
            if p.ends_with("/dimension/overworld.json") {
                dim_path = match dim_path {
                    Some(cur) if cur <= p => Some(cur),
                    _ => Some(p),
                };
            }
        }
        let Some(dp) = dim_path else {
            return "minecraft:overworld".to_string();
        };
        let bytes = &self.files[dp];
        let Ok(text) = std::str::from_utf8(bytes) else {
            return "minecraft:overworld".to_string();
        };
        let Ok(json) = parse(text) else {
            return "minecraft:overworld".to_string();
        };
        let settings = json
            .get("generator")
            .and_then(|g| g.get("settings"))
            .and_then(Json::as_str)
            .unwrap_or("minecraft:overworld");
        settings.to_string()
    }
}

/// "<ns>/worldgen/noise/<stem>.json" -> "<ns>:<stem>"; None for other paths.
fn id_of_path(path: &str, marker: &str) -> Option<String> {
    let i = path.find(marker)?;
    if i == 0 || !path.ends_with(".json") {
        return None;
    }
    let ns = &path[..i - 1]; // path[i-1] == '/'
    if ns.is_empty() || ns.contains('/') {
        return None;
    }
    let stem = &path[i + marker.len()..path.len() - ".json".len()];
    Some(format!("{ns}:{stem}"))
}

/// "minecraft:overworld" -> "minecraft/worldgen/noise_settings/overworld.json".
fn noise_settings_path(id: &str) -> String {
    let (ns, rel) = id
        .split_once(':')
        .unwrap_or_else(|| panic!("noise_settings id without namespace: {id}"));
    format!("{ns}/worldgen/noise_settings/{rel}.json")
}

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
    // --- 26.3 new nodes ---
    /// axis 0=x/1=y/2=z; tiling 0=clamp_to_edge (default)/1=repeat/2=mirrored.
    Gradient {
        axis: u8,
        tiling: u8,
        from_coord: i32,
        to_coord: i32,
        from_value: f64,
        to_value: f64,
    },
    Lerp {
        alpha: usize,
        first: usize,
        second: usize,
    },
    IntervalSelect {
        input: usize,
        thresholds: Vec<f32>,
        functions: Vec<usize>,
    },
    FindTopSurface {
        density: usize,
        upper_bound: usize,
        lower_bound: i32,
        cell_height: i32,
    },
    Pow {
        base: usize,
        exponent: usize,
    },
    Div {
        left: usize,
        right: usize,
    },
    Sub {
        left: usize,
        right: usize,
    },
    Negate {
        argument: usize,
    },
    Reciprocal {
        argument: usize,
    },
    Log {
        argument: usize,
    },
    Sign {
        argument: usize,
    },
    Round {
        argument: usize,
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
            DfNode::Gradient { .. } => "gradient",
            DfNode::Lerp { .. } => "lerp",
            DfNode::IntervalSelect { .. } => "interval_select",
            DfNode::FindTopSurface { .. } => "find_top_surface",
            DfNode::Pow { .. } => "pow",
            DfNode::Div { .. } => "div",
            DfNode::Sub { .. } => "sub",
            DfNode::Negate { .. } => "negate",
            DfNode::Reciprocal { .. } => "reciprocal",
            DfNode::Log { .. } => "log",
            DfNode::Sign { .. } => "sign",
            DfNode::Round { .. } => "round",
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
            // 26.3 nodes
            DfNode::Gradient { .. } => Vec::new(),
            DfNode::Lerp { alpha, first, second } => vec![*alpha, *first, *second],
            DfNode::IntervalSelect { input, functions, .. } => {
                let mut v = vec![*input];
                v.extend_from_slice(functions);
                v
            }
            DfNode::FindTopSurface { density, upper_bound, .. } => {
                vec![*density, *upper_bound]
            }
            DfNode::Pow { base, exponent } => vec![*base, *exponent],
            DfNode::Div { left, right } | DfNode::Sub { left, right } => {
                vec![*left, *right]
            }
            DfNode::Negate { argument }
            | DfNode::Reciprocal { argument }
            | DfNode::Log { argument }
            | DfNode::Sign { argument }
            | DfNode::Round { argument } => vec![*argument],
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
    /// NoiseChunk cell dims from noise.size_horizontal/size_vertical
    /// (vanilla NoiseSettings.getCellWidth/getCellHeight = QuartPos.toBlock
    /// = 4 * size). Vanilla overworld: 1/2 -> 4x8; Tectonic/Terralith:
    /// 1/1 -> 4x4; end/floating_islands: 2/1 -> 8x4.
    pub cell_width: i32,
    pub cell_height: i32,
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

    /// Build the registry for one noise_settings asset (embedded vanilla
    /// assets only).
    pub fn load(noise_settings_asset: &str) -> WorldGen {
        Self::load_with_assets(&Assets::new(), noise_settings_asset)
    }

    /// Build the registry with world-datapack asset overrides. The
    /// noise_settings to load is resolved from the pack's dimension override
    /// when present (generator.settings), else vanilla overworld.
    pub fn load_with(assets: &Assets) -> WorldGen {
        let ns_id = assets.resolve_noise_settings();
        Self::load_with_assets(assets, &noise_settings_path(&ns_id))
    }

    /// Build the registry for one noise_settings path, with pack overrides
    /// over the embedded vanilla manifest.
    pub fn load_with_assets(assets: &Assets, noise_settings_asset: &str) -> WorldGen {
        // 1) noise parameters: worldgen/noise/*.json (union of embedded and
        //    pack-provided files, ids "<namespace>:<stem>")
        let mut noise_params = HashMap::new();
        for id in assets.ids_with("worldgen/noise/") {
            let (ns, stem) = id.split_once(':').expect("ids_with yields ns:stem");
            let path = format!("{ns}/worldgen/noise/{stem}.json");
            let bytes = assets
                .get(&path)
                .unwrap_or_else(|| panic!("noise params path vanished: {path}"));
            let text = std::str::from_utf8(bytes).unwrap_or_else(|_| panic!("{path}: not utf-8"));
            let json = parse(text).unwrap_or_else(|e| panic!("{path}: json error: {e}"));
            noise_params.insert(id, parse_noise_params(&json, &path));
        }

        // 2) density functions are resolved LAZILY from the noise_router
        //    trees below (vanilla codec semantics: a ref resolves on access,
        //    so dead files with dangling refs — e.g. Tectonic's unused trees
        //    that Terralith's overworld.json replaces — never parse). Memoized
        //    by id in `roots`; the eager union scan is intentionally gone.
        let mut loader = Loader {
            noise_params: &noise_params,
            arena: Vec::new(),
            roots: HashMap::new(),
            building: Vec::new(),
            type_counts: BTreeMap::new(),
            assets,
        };

        // 3) noise_settings header fields (pack override or embedded)
        let bytes = assets
            .get(noise_settings_asset)
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
        // 26.3: noise settings dropped size_horizontal/size_vertical — cell
        // dims come from the Interpolated nodes' cell_size_xz/cell_size_y
        // (overworld 4x8). Default to the overworld shape when absent.
        let size_horizontal = noise_obj
            .get("size_horizontal")
            .and_then(Json::as_i32)
            .unwrap_or(1);
        let size_vertical = noise_obj
            .get("size_vertical")
            .and_then(Json::as_i32)
            .unwrap_or(2);
        if !(1..=4).contains(&size_horizontal) || !(1..=4).contains(&size_vertical) {
            panic!(
                "{noise_settings_asset}: size_horizontal/vertical out of range 1..=4: {size_horizontal}/{size_vertical}"
            );
        }
        let cell_width = size_horizontal * 4;
        let cell_height = size_vertical * 4;

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

        // 4b) MC 26.3: aquifer DFs moved out of noise_router into a dedicated
        // "aquifers" object {barrier, exclusion, fluid_level_floodedness,
        // fluid_level_spread, lava, surface_level}. Merge them into the same
        // lookup so the fill/aquifer wiring resolves keys uniformly.
        if let Some(Json::Obj(aq)) = ns.get("aquifers") {
            for (k, v) in aq {
                router_indices.insert(k.clone(), loader.build_value(v));
            }
        }
        // 1.21.8-only router fields absent in 26.3 — neutral stubs keep the
        // nochunk wiring compiling; the consumers gate on their presence.
        for stub in [
            "initial_density_without_jaggedness",
            "vein_toggle",
            "vein_ridged",
            "vein_gap",
        ] {
            if !router_indices.contains_key(stub) {
                let idx = loader.build_value(&Json::Obj(vec![
                    ("type".to_string(), Json::Str("minecraft:constant".to_string())),
                    ("argument".to_string(), Json::Num(0.0)),
                ]));
                router_indices.insert(stub.to_string(), idx);
            }
        }

        // 4c) MC 26.3 material rules: the material_rule tree may carry
        // ore_vein rules whose density/richness/filler_gap fields reference
        // density functions by string id. Pre-resolve every such DF into the
        // arena and register it under "material:<id>" so the surface parser
        // can reach it through Compiled.router.
        if let Some(Json::Str(mr)) = ns.get("material_rule") {
            let mut queue = vec![mr.clone()];
            let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
            while let Some(id) = queue.pop() {
                if !seen.insert(id.clone()) {
                    continue;
                }
                let path = format!("minecraft/worldgen/material_rule/{}.json", id.strip_prefix("minecraft:").unwrap_or(&id));
                let Some(bytes) = assets.get(&path) else { continue };
                let Ok(text) = std::str::from_utf8(bytes) else { continue };
                let Ok(j) = parse(text) else { continue };
                collect_material_dfs(&j, &id, &mut loader, &mut router_indices, &mut queue);
            }
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
            cell_width,
            cell_height,
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
    // 1.21.8 shape: {"firstOctave": n, "amplitudes": [...]}
    if let Some(fo) = v.get("firstOctave").and_then(Json::as_i32) {
        let amps = v
            .get("amplitudes")
            .and_then(Json::as_arr)
            .unwrap_or_else(|| panic!("{ctx}: missing amplitudes"));
        let amplitudes = amps
            .iter()
            .map(|a| a.as_f64().unwrap_or_else(|| panic!("{ctx}: non-numeric amplitude {a:?}")))
            .collect();
        return NoiseParams { first_octave: fo, amplitudes };
    }
    // MC 26.3 shape: {"base_amplitude": b, "base_octave": n,
    //                  "octave_count": k?, "amplitude_modifiers": [...]?}
    // (NormalNoise$Parameters codec). Approximate the legacy amplitudes list
    // as base_amplitude * modifier_i (default modifier 1.0). NOTE: 26.3
    // switched the noise synthesis to NoiseStack with its own normalization —
    // a full bit-exact re-port of the synth stack is tracked separately.
    let base_octave = v
        .get("base_octave")
        .and_then(Json::as_i32)
        .unwrap_or_else(|| panic!("{ctx}: missing firstOctave/base_octave"));
    let base_amplitude = v.get("base_amplitude").and_then(Json::as_f64).unwrap_or(1.0);
    let octave_count = v.get("octave_count").and_then(Json::as_i32).unwrap_or(1).max(1) as usize;
    let modifiers: Vec<f64> = v
        .get("amplitude_modifiers")
        .and_then(Json::as_arr)
        .map(|arr| arr.iter().map(|a| a.as_f64().unwrap_or(1.0)).collect())
        .unwrap_or_default();
    let amplitudes: Vec<f64> = (0..octave_count)
        .map(|i| base_amplitude * modifiers.get(i).copied().unwrap_or(1.0))
        .collect();
    NoiseParams { first_octave: base_octave, amplitudes }
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
    /// pack overrides; lookup falls back to the embedded manifest
    assets: &'a Assets,
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
        // vanilla codec default: a ref without namespace is minecraft:
        let id = if id.contains(':') {
            id.to_string()
        } else {
            format!("minecraft:{id}")
        };
        let (ns, rel) = id
            .split_once(':')
            .unwrap_or_else(|| panic!("density function ref without namespace: {id}"));
        let path = format!("{ns}/worldgen/density_function/{rel}.json");
        let bytes = self
            .assets
            .get(&path)
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
        // vanilla codec default: an unnamespaced type is minecraft:*
        let ty_owned;
        let ty: &str = if ty.contains(':') {
            ty
        } else {
            ty_owned = format!("minecraft:{ty}");
            &ty_owned
        };
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
                // 1.21.8 "argument"; 26.3 "input"
                let argument = self.build_value(
                    v.get("argument")
                        .or_else(|| v.get("input"))
                        .unwrap_or_else(|| panic!("{ty}: missing argument/input")),
                );
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
                // 1.21.8 uses "argument"; 26.3 uses "input" (interpolated also
                // carries cell_size_xz/cell_size_y which we take from the
                // noise settings defaults)
                let argument = self.build_value(
                    v.get("argument")
                        .or_else(|| v.get("input"))
                        .unwrap_or_else(|| panic!("{ty}: missing field 'argument'/'input'")),
                );
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
            "minecraft:sub" => {
                let children = self.build_children(v, ty);
                if children.len() != 2 {
                    panic!("{ty}: expected exactly 2 children");
                }
                self.push(DfNode::Sub { left: children[0], right: children[1] })
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
            // --- 26.3 new nodes ---
            "minecraft:cache" => {
                // 26.3 renamed cache_once -> cache (CacheFunction)
                let input = self.build_value(req_field(v, "input", ty));
                self.push(DfNode::CacheOnce { argument: input })
            }
            "minecraft:gradient" => {
                let axis = match req_field(v, "axis", ty).as_str() {
                    Some("x") | Some("minecraft:x") => 0u8,
                    Some("y") | Some("minecraft:y") => 1u8,
                    Some("z") | Some("minecraft:z") => 2u8,
                    other => panic!("{ty}: unknown axis {other:?}"),
                };
                let tiling = match v.get("tiling").and_then(|t| t.as_str()) {
                    None => 0u8,
                    Some("minecraft:clamp_to_edge") | Some("clamp_to_edge") => 0u8,
                    Some("minecraft:repeat") | Some("repeat") => 1u8,
                    Some("minecraft:mirrored_repeat") | Some("mirrored_repeat") => 2u8,
                    other => panic!("{ty}: unknown tiling {other:?}"),
                };
                let from_coord = req_field(v, "from_coordinate", ty).as_i64().unwrap_or_else(|| panic!("{ty}: from_coordinate")) as i32;
                let to_coord = req_field(v, "to_coordinate", ty).as_i64().unwrap_or_else(|| panic!("{ty}: to_coordinate")) as i32;
                let from_value = req_num(v, "from_value", ty);
                let to_value = req_num(v, "to_value", ty);
                self.push(DfNode::Gradient { axis, tiling, from_coord, to_coord, from_value, to_value })
            }
            "minecraft:lerp" => {
                let alpha = self.build_value(req_field(v, "alpha", ty));
                let first = self.build_value(req_field(v, "first", ty));
                let second = self.build_value(req_field(v, "second", ty));
                self.push(DfNode::Lerp { alpha, first, second })
            }
            "minecraft:interval_select" => {
                let input = self.build_value(req_field(v, "input", ty));
                let thresholds = req_field(v, "thresholds", ty)
                    .as_arr()
                    .unwrap_or_else(|| panic!("{ty}: thresholds"))
                    .iter()
                    .map(|t| t.as_f64().unwrap_or_else(|| panic!("{ty}: threshold value")) as f32)
                    .collect();
                let functions = req_field(v, "functions", ty)
                    .as_arr()
                    .unwrap_or_else(|| panic!("{ty}: functions"))
                    .iter()
                    .map(|f| self.build_value(f))
                    .collect();
                self.push(DfNode::IntervalSelect { input, thresholds, functions })
            }
            "minecraft:find_top_surface" => {
                let density = self.build_value(req_field(v, "density", ty));
                let upper_bound = self.build_value(req_field(v, "upper_bound", ty));
                let lower_bound = req_field(v, "lower_bound", ty).as_i64().unwrap_or_else(|| panic!("{ty}: lower_bound")) as i32;
                let cell_height = req_field(v, "cell_height", ty).as_i64().unwrap_or_else(|| panic!("{ty}: cell_height")) as i32;
                self.push(DfNode::FindTopSurface { density, upper_bound, lower_bound, cell_height })
            }
            "minecraft:pow" => {
                let base = self.build_value(req_field(v, "base", ty));
                let exponent = self.build_value(req_field(v, "exponent", ty));
                self.push(DfNode::Pow { base, exponent })
            }
            "minecraft:div" => {
                let left = self.build_value(req_field(v, "left", ty));
                let right = self.build_value(req_field(v, "right", ty));
                self.push(DfNode::Div { left, right })
            }
            "minecraft:negate" => {
                let argument = self.build_value(req_field(v, "input", ty));
                self.push(DfNode::Negate { argument })
            }
            "minecraft:reciprocal" => {
                let argument = self.build_value(req_field(v, "input", ty));
                self.push(DfNode::Reciprocal { argument })
            }
            "minecraft:log" => {
                let argument = self.build_value(req_field(v, "input", ty));
                self.push(DfNode::Log { argument })
            }
            "minecraft:sign" => {
                let argument = self.build_value(req_field(v, "input", ty));
                self.push(DfNode::Sign { argument })
            }
            "minecraft:round" => {
                let argument = self.build_value(req_field(v, "input", ty));
                self.push(DfNode::Round { argument })
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
        // 1.21.8: "argument1"/"argument2"; 26.3: "left"/"right"
        let f1 = v.get("argument1").or_else(|| v.get("left"));
        let f2 = v.get("argument2").or_else(|| v.get("right"));
        match (f1, f2) {
            (Some(a), Some(b)) => {
                let ia = self.build_value(a);
                let ib = self.build_value(b);
                vec![ia, ib]
            }
            _ => panic!("{ty}: missing argument1/argument2 (left/right)"),
        }
    }

    /// Noise id string (verified against the noise param set) or inline
    /// {"firstOctave","amplitudes"} object. A bare id without namespace is
    /// minecraft: (vanilla codec default).
    fn noise_source(&mut self, v: &Json, ty: &str) -> NoiseSource {
        match v {
            Json::Str(s) => {
                let id = if s.contains(':') {
                    s.clone()
                } else {
                    format!("minecraft:{s}")
                };
                if !self.noise_params.contains_key(&id) {
                    panic!("{ty}: unknown noise params id: {id}");
                }
                NoiseSource::Id(id)
            }
            Json::Obj(_) => NoiseSource::Inline(parse_noise_params(v, ty)),
            other => panic!("{ty}: noise field is neither id nor object: {other:?}"),
        }
    }
}

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------

/// Walk a material_rule JSON collecting ore_vein DF references (26.3).
/// Registers each referenced DF in the arena under "material:<id>" and queues
/// rule/condition string refs for transitive traversal.
fn collect_material_dfs(
    j: &Json,
    cur_id: &str,
    loader: &mut Loader<'_>,
    router_indices: &mut HashMap<String, usize>,
    queue: &mut Vec<String>,
) {
    match j {
        Json::Obj(entries) => {
            let ty = entries.iter().find(|(k, _)| k == "type").map(|(_, v)| v.clone());
            if let Some(Json::Str(t)) = &ty {
                if t == "minecraft:ore_vein" {
                    for field in ["density", "richness", "filler_gap"] {
                        if let Some(v) = entries.iter().find(|(k, _)| k == field).map(|(_, v)| v) {
                            if let Json::Str(dfid) = v {
                                let key = format!("material:{dfid}");
                                if !router_indices.contains_key(&key) {
                                    let idx = loader.build_value(v);
                                    router_indices.insert(key, idx);
                                }
                            }
                        }
                    }
                }
            }
            for (k, v) in entries {
                match v {
                    Json::Str(s) if k == "then_run" || k == "if_true" || k == "invert" => {
                        queue.push(s.clone());
                    }
                    Json::Arr(_) | Json::Obj(_) => collect_material_dfs(v, cur_id, loader, router_indices, queue),
                    _ => {}
                }
            }
        }
        Json::Arr(items) => {
            for v in items {
                match v {
                    Json::Str(s) => queue.push(s.clone()),
                    Json::Obj(_) | Json::Arr(_) => collect_material_dfs(v, cur_id, loader, router_indices, queue),
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The noise_router fields of the 26.3 overworld noise settings:
    /// the 1.21.8-style 15 fields plus the 26.3 aquifer-object DFs
    /// (barrier/exclusion/fluid_*/lava already overlap; surface_level,
    /// chunk_surface_level are new) and the material_rule ore-vein DFs
    /// registered under "material:" prefixes.
    const ROUTER_KEYS: &[&str] = &[
        "barrier",
        "continents",
        "depth",
        "erosion",
        "exclusion",
        "final_density",
        "fluid_level_floodedness",
        "fluid_level_spread",
        "initial_density_without_jaggedness",
        "lava",
        "ridges",
        "surface_level",
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
        // 26.3: the aquifer bool config is gone — the aquifer DFs live in
        // the noise_router object (barrier/exclusion/fluid_*/lava entries),
        // so the legacy flag stays false while the router carries them.
        assert!(!wg.aquifers_enabled);
        assert!(wg.router_indices.contains_key("barrier"));
        assert!(wg.router_indices.contains_key("exclusion"));
        assert!(wg.router_indices.contains_key("fluid_level_floodedness"));
        // 26.3: blocks resolve through material rules (ore veins, bedrock
        // floor), so the legacy "default block is stone" shortcut is off.
        assert!(!wg.default_block_is_stone);
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
        // 26.3 adds material-rule DF entries ("material:...") on top of the
        // classic router fields — every classic key must be present.
        assert!(wg.router_indices.len() >= ROUTER_KEYS.len());

        // router-reachable density-function roots are memoized (lazy
        // resolution: only files referenced by this noise_settings load)
        assert!(!wg.root_indices.is_empty(), "no reachable df roots");
        for (id, idx) in &wg.root_indices {
            assert!(*idx < wg.df_arena.len(), "root {id}: bad index");
        }

        // noise params sanity: worldgen/noise/offset.json
        let off = &wg.noise_params["minecraft:offset"];
        assert_eq!(off.first_octave, -3);
        // 26.3 noise params carry base_amplitude + modifiers; the loader
        // folds them into the per-octave amplitudes (expected-deviation
        // approximation — documented in port263_design.md).
        assert_eq!(
            off.amplitudes,
            vec![0.9381732587751005, 0.9381732587751005, 0.9381732587751005, 0.0]
        );

        // distinct types seen while building
        // 26.3: weird_scaled_sampler disappeared from the overworld graph
        // (interval_select took over its role); gradient/lerp are new.
        for t in [
            "add",
            "mul",
            "spline",
            "blend_alpha",
            "interval_select",
            "gradient",
            "lerp",
            "old_blended_noise",
        ] {
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
        // 26.3: the jaggedness-free depth offset became a gradient node and
        // the offset term is a cache_once (26.3 renamed flat caches).
        assert!(
            matches!(wg.df_arena[children[0]], DfNode::Gradient { .. }),
            "depth child 0 should be the 26.3 gradient"
        );
        assert!(
            matches!(wg.df_arena[children[1]], DfNode::CacheOnce { .. }),
            "depth child 1 should be overworld/offset (a cache_once)"
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
        assert!(matches!(wg.df_arena[off], DfNode::CacheOnce { .. }));

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

/// Stable key for inline noise params (registry has no id for them).
pub fn format_inline_key(p: &NoiseParams) -> String {
    let mut s = format!("o{}", p.first_octave);
    for a in &p.amplitudes {
        s.push(':');
        // exact f64 bits so the key round-trips losslessly
        s.push_str(&format!("{:016x}", a.to_bits()));
    }
    s
}
