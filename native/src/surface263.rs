// ---------------------------------------------------------------------------
// Native 26.3 MaterialSystem (surface rules) — per-chunk column walk.
//
// Decompiled source of truth (paper-26.3-222, vineflower): MaterialSystem,
// MaterialRuleContext, condition/*, rule/OreVeinRule + RandomState.
//
// Differences vs the legacy 1.21.8 SurfaceSystem engine (surface_fill.rs):
//   1. minSurfaceLevel = Mth.floor(chunk_surface_level(x, 0, z)) + depth - 8
//      (RandomState 26.3 wires MaterialSystem with router.chunkSurfaceLevel;
//      the legacy initial_density top-down walk is GONE).
//   2. NO default-block identity gate: 26.3 calls rule.tryApply for every
//      solid block (bytecode-verified: only the y-bounds check remains).
//   3. Ore veins are material rules (OreVeinRule); density/richness sample
//      through the VOLUME path (prefilled buffers, cell 4x8 interpolation),
//      filler_gap is a point eval.
//   4. noise_threshold carries is_3d (3D sampler at the block position).
//   5. steep = surfaceGradientX <= -4 || surfaceGradientZ >= 4 (precomputed
//      per column from the WORLD_SURFACE_WG heightmap; identical truth
//      table to the legacy hm-pair form).
//   6. eroded_badlands + frozen_ocean column extensions (the legacy engine
//      never had them — this closes that parity gap too).
//   7. surface rule fluid writes append fluid-update marks (vanilla
//      markPosForPostProcessing in the column's setBlock).
// ---------------------------------------------------------------------------

use crate::df263::{DensityVolume, SamplerCtx263};
use crate::df263::N263;
use crate::n263::{NormalNoiseParameters, Normalization};
use crate::rng::{XoroshiroPositionalRandomFactory, XoroshiroRandomSource};
use crate::surface::{SrCond, SrRule, SurfaceConfig, TempNoises};

/// 26.3 surface runtime state (built once per ctx at world load).
pub struct SurfaceState263 {
    pub cfg: SurfaceConfig,
    /// noise handles referenced by the tree, resolved to 26.3 instances
    /// (index == cfg cond/handle ids).
    pub noises: Vec<std::sync::Arc<N263>>,
    /// static SurfaceSystem noise handles (surface / surface_secondary /
    /// clay_bands_offset) in the noises index space.
    pub surface_handle: u32,
    pub secondary_handle: u32,
    pub clay_handle: u32,
    /// material-rule DF trees ("material:<id>" -> index into this Vec).
    pub dfs: Vec<crate::df263::SamRef>,
    /// ore-vein trio slots in dfs (copper, iron, richness) + the gap slot.
    pub ore_copper: u32,
    pub ore_iron: u32,
    pub ore_richness: u32,
    pub ore_gap: u32,
    pub temps: Vec<f32>,
    pub frozen: Vec<bool>,
    pub temps_noises: std::sync::Arc<TempNoises>,
    /// RandomState.random (jitter, bandlands seed, gradient factories).
    pub factory: XoroshiroPositionalRandomFactory,
    /// RandomState.getOrCreateRandomFactory("minecraft:ore").
    pub ore_factory: XoroshiroPositionalRandomFactory,
    /// router.chunkSurfaceLevel (preliminary surface 26.3).
    pub chunk_surface_level: crate::df263::SamRef,
    /// biome name-table indexes of the extension biome targets.
    pub eroded_badlands_biome: Option<u32>,
    pub frozen_ocean_biome: Option<u32>,
    pub deep_frozen_ocean_biome: Option<u32>,
    /// extension noise handles (badlands/iceberg pillars — code-referenced,
    /// not part of the parsed rule tree).
    pub ext_noise_handles: std::collections::HashMap<String, u32>,
    /// cfg.states positions of the iceberg extension states.
    pub snow_block_state: Option<u32>,
    pub packed_ice_state: Option<u32>,
}

fn trace_target() -> Option<(i32, i32, i32)> {
    static T: std::sync::OnceLock<Option<(i32, i32, i32)>> = std::sync::OnceLock::new();
    *T.get_or_init(|| {
        std::env::var("CRUS_S263_TRACE").ok().and_then(|v| {
            let p: Vec<i32> = v.split(',').filter_map(|s| s.parse().ok()).collect();
            if p.len() == 3 { Some((p[0], p[1], p[2])) } else { None }
        })
    })
}

#[inline]
fn mth_floor(d: f64) -> i32 {
    let i = d as i32;
    if d < i as f64 { i - 1 } else { i }
}

#[inline]
fn mth_map(v: f64, lo_in: f64, hi_in: f64, lo_out: f64, hi_out: f64) -> f64 {
    (v - lo_in) / (hi_in - lo_in) * (hi_out - lo_out) + lo_out
}

/// Parse one 26.3 worldgen/noise/<name>.json into NormalNoiseParameters.
/// Full form: {base_amplitude, base_octave, octave_count?, amplitude_modifiers?};
/// simplified (no octave_count) == one octave, no modifiers (the convention
/// the verified fill/biomes param table uses for cave_layer et al).
fn noise_params_from_asset(
    assets: &crate::worldgen::Assets,
    name: &str,
) -> Option<NormalNoiseParameters> {
    let short = name.strip_prefix("minecraft:").unwrap_or(name);
    let path = format!("minecraft/worldgen/noise/{short}.json");
    let bytes = assets.get(&path)?;
    let text = std::str::from_utf8(bytes).ok()?;
    let j = crate::json::parse(text).ok()?;
    let base_amplitude = j.get("base_amplitude")?.as_f64()?;
    let base_octave = j.get("base_octave")?.as_i32()?;
    let octave_count = j.get("octave_count").and_then(|v| v.as_i32()).unwrap_or(1);
    let mods: Vec<f64> = match j.get("amplitude_modifiers") {
        Some(crate::json::Json::Arr(a)) => a.iter().filter_map(|v| v.as_f64()).collect(),
        _ => Vec::new(),
    };
    Some(NormalNoiseParameters {
        base_amplitude,
        base_octave,
        octave_count,
        normalize: Normalization::Enabled,
        amplitude_modifiers: mods,
    })
}

fn biome_pos(names: &[String], name: &str) -> Option<u32> {
    names.iter().position(|n| n == name).map(|i| i as u32)
}

fn state_pos(cfg: &SurfaceConfig, prefix: &str) -> Option<u32> {
    cfg.states
        .iter()
        .position(|s| s.starts_with(prefix))
        .map(|i| i as u32)
}

/// Build the 26.3 surface state: parse the material_rule tree from the
/// embedded 26.3 assets, resolve noise handles (26.3 instances), the ore-vein
/// DF trees (df263) and the chunk_surface_level router node.
/// `biomes` supplies the biome name table for cond indexes.
pub fn build_surface263(
    seed: i64,
    min_y: i32,
    height: i32,
    cell_height: i32,
    sea_level: i32,
    biomes: Option<&crate::biome263::BiomeState263>,
) -> Option<SurfaceState263> {
    let assets = crate::worldgen::Assets::new();
    let ns_path = "minecraft/worldgen/noise_settings/overworld.json";
    let ns_bytes = assets.get(ns_path)?;
    let ns = std::str::from_utf8(ns_bytes).ok()?;

    // minimal WorldGen for the parser (anchors + cfg geometry only)
    let wg = crate::worldgen::WorldGen {
        noise_params: std::collections::HashMap::new(),
        df_arena: Vec::new(),
        root_indices: std::collections::HashMap::new(),
        router_indices: std::collections::HashMap::new(),
        sea_level: sea_level as i64,
        aquifers_enabled: true,
        default_block_is_stone: true,
        min_y,
        height,
        legacy_random_source: false,
        ore_veins_enabled: true,
        cell_width: 4,
        cell_height,
        type_counts: std::collections::BTreeMap::new(),
    };

    // noise handles: register names on demand; instances resolved after parse
    let noise_names: std::cell::RefCell<Vec<String>> = std::cell::RefCell::new(Vec::new());
    let noise_idx = |name: &str| -> Option<u32> {
        let mut v = noise_names.borrow_mut();
        if let Some(i) = v.iter().position(|n| n == name) {
            return Some(i as u32);
        }
        v.push(name.to_string());
        Some((v.len() - 1) as u32)
    };
    let biome_idx = |name: &str| -> Option<u32> {
        let b = biomes?;
        b.src.biome_names.iter().position(|n| n == name).map(|i| i as u32)
    };

    // material-rule DF refs: build the ore-vein trees via a dedicated builder
    // (cache-id namespace separate from the fill router's).
    let mut ore_builder = crate::df263_router::RouterBuilder263::new(seed);
    let ore_dfs = ore_builder.build_ore_vein_dfs();
    let mut dfs: Vec<crate::df263::SamRef> = Vec::new();
    dfs.push(ore_dfs.copper_density.clone());
    dfs.push(ore_dfs.iron_density.clone());
    dfs.push(ore_dfs.richness.clone());
    dfs.push(ore_dfs.gap.clone());
    let (copper_slot, iron_slot, richness_slot, gap_slot) = (0u32, 1u32, 2u32, 3u32);
    let dfc = |key: &str| -> Option<u32> {
        match key.strip_prefix("material:")? {
            "minecraft:overworld/ore_vein/copper_density" => Some(copper_slot),
            "minecraft:overworld/ore_vein/iron_density" => Some(iron_slot),
            "minecraft:overworld/ore_vein/richness" => Some(richness_slot),
            "minecraft:overworld/ore_vein/gap" => Some(gap_slot),
            _ => {
                crate::ffi::log_warn(&format!("surface263: unknown material DF ref {key}"));
                None
            }
        }
    };

    let sb = crate::surface::build(&wg, ns, &noise_idx, &biome_idx, Some(&assets), Some(&dfc))?;

    // the column-extension noises are code-referenced (MaterialSystem fields,
    // never appear in the rule tree) — register them now.
    for extra in [
        "minecraft:badlands_pillar",
        "minecraft:badlands_pillar_roof",
        "minecraft:badlands_surface",
        "minecraft:iceberg_pillar",
        "minecraft:iceberg_pillar_roof",
        "minecraft:iceberg_surface",
    ] {
        let _ = noise_idx(extra);
    }

    // resolve the collected noise names to 26.3 noise instances
    let names: Vec<String> = noise_names.borrow().clone();
    let mut ext_noise_handles: std::collections::HashMap<String, u32> =
        std::collections::HashMap::new();
    let mut factory263 = crate::df263::NoiseFactory263::new(seed);
    let mut noises: Vec<std::sync::Arc<N263>> = Vec::with_capacity(names.len());
    for name in &names {
        let params = match noise_params_from_asset(&assets, name) {
            Some(p) => p,
            None => {
                crate::ffi::log_warn(&format!("surface263: noise params missing: {name}"));
                return None;
            }
        };
        let n = factory263.normal(name.strip_prefix("minecraft:").unwrap_or(name), params);
        noises.push(n);
    }

    // the chunk_surface_level router node (own builder — cache ids disjoint
    // from the ore dfs; the node tree is value-identical to Gen263's)
    let csl_node = {
        let mut rb = crate::df263_router::RouterBuilder263::new(seed);
        rb.build_overworld().chunk_surface_level
    };

    let biome_names: &[String] = match biomes {
        Some(b) => &b.src.biome_names,
        None => &[],
    };
    for (i, name) in names.iter().enumerate() {
        if name.starts_with("minecraft:badlands_") || name.starts_with("minecraft:iceberg_") {
            ext_noise_handles.insert(name.clone(), i as u32);
        }
    }
    let temps_noises = std::sync::Arc::new(crate::surface::temp_noises());
    let factory = crate::surface::root_factory(seed);
    let ore_factory = factory.from_hash_of("minecraft:ore").fork_positional();
    let snow_pos = state_pos(&sb.cfg, "minecraft:snow_block");
    let packed_pos = state_pos(&sb.cfg, "minecraft:packed_ice");

    Some(SurfaceState263 {
        surface_handle: sb.noise_handles[0],
        secondary_handle: sb.noise_handles[1],
        clay_handle: sb.noise_handles[2],
        cfg: sb.cfg,
        noises,
        dfs,
        ore_copper: copper_slot,
        ore_iron: iron_slot,
        ore_richness: richness_slot,
        ore_gap: gap_slot,
        temps: Vec::new(),
        frozen: Vec::new(),
        temps_noises,
        factory,
        ore_factory,
        chunk_surface_level: csl_node,
        eroded_badlands_biome: biome_pos(biome_names, "minecraft:eroded_badlands"),
        frozen_ocean_biome: biome_pos(biome_names, "minecraft:frozen_ocean"),
        deep_frozen_ocean_biome: biome_pos(biome_names, "minecraft:deep_frozen_ocean"),
        ext_noise_handles,
        snow_block_state: snow_pos,
        packed_ice_state: packed_pos,
    })
}

/// The 26.3 column walk (MaterialSystem.buildSurface + RuleEvaluator).
struct Walk263<'a> {
    cfg: &'a SurfaceConfig,
    st: &'a SurfaceState263,
    grid: &'a [i32],
    heightmap: &'a [i32; 256],
    // chunk volume + prefilled ore-vein buffers (density/richness)
    volume: DensityVolume,
    copper_buf: Vec<f32>,
    iron_buf: Vec<f32>,
    richness_buf: Vec<f32>,
    ctx_ore: SamplerCtx263,
    ctx_csl: SamplerCtx263,
    // per-column state
    block_x: i32,
    block_z: i32,
    surface_depth: i32,
    surface_secondary: f64,
    min_surface_level: i32,
    gradient_x: i32,
    gradient_z: i32,
    xz_counter: u64,
    y_counter: u64,
    last_sec_update: u64,
    // per-block state
    block_y: i32,
    stone_depth_above: i32,
    stone_depth_below: i32,
    water_height: i32,
    // per-site lazy caches (vanilla LazyXZ/LazyY)
    xz_cache: Vec<(u64, bool)>,
    y_cache: Vec<(u64, bool)>,
    biome_cache: Vec<(u64, bool)>,
    sea_level: i32,
    /// state string is a fluid (water/lava) — for the postproc marks
    fluid_states: Vec<bool>,
}

impl<'a> Walk263<'a> {
    #[inline]
    fn noise(&self, h: u32) -> &N263 {
        &self.st.noises[h as usize]
    }

    #[inline]
    fn update_xz(&mut self, bx: i32, bz: i32) {
        self.xz_counter += 1;
        self.y_counter += 1; // vanilla updateXZ bumps BOTH counters
        self.block_x = bx;
        self.block_z = bz;
        let sh = self.st.surface_handle;
        let d = self.noise(sh).get(bx as f64, 0.0, bz as f64);
        let jit = self.st.factory.at(bx, 0, bz).next_f64();
        self.surface_depth = (d as f64 * 2.75 + 3.0 + jit * 0.25) as i32;
        let lx = bx & 15;
        let lz = bz & 15;
        let h = |x: i32, z: i32| self.heightmap[(z.clamp(0, 15) * 16 + x.clamp(0, 15)) as usize];
        self.gradient_x = h(lx + 1, lz) - h(lx - 1, lz);
        self.gradient_z = h(lx, lz + 1) - h(lx, lz - 1);
        // minSurfaceLevel 26.3: floor(chunk_surface_level(x, 0, z)) + depth - 8
        let csl = self.st.chunk_surface_level.sample_value(&mut self.ctx_csl, bx, 0, bz);
        self.min_surface_level = mth_floor(csl as f64) + self.surface_depth - 8;
    }

    #[inline]
    fn update_y(&mut self, sda: i32, sdb: i32, wh: i32, y: i32) {
        self.y_counter += 1;
        self.stone_depth_above = sda;
        self.stone_depth_below = sdb;
        self.water_height = wh;
        self.block_y = y;
    }

    #[inline]
    fn surface_secondary(&mut self) -> f64 {
        if self.last_sec_update != self.xz_counter {
            self.last_sec_update = self.xz_counter;
            let h = self.st.secondary_handle;
            self.surface_secondary = self.noise(h).get(self.block_x as f64, 0.0, self.block_z as f64) as f64;
        }
        self.surface_secondary
    }

    #[inline]
    fn biome_idx_at(&self, bx: i32, y: i32, bz: i32) -> usize {
        let sy = ((y - self.cfg.min_y) >> 4).max(0) as usize;
        let qy = ((y - self.cfg.min_y) >> 2) & 3;
        let qx = (bx & 15) >> 2;
        let qz = (bz & 15) >> 2;
        let i = sy * 64 + (qx as usize) * 16 + (qy as usize) * 4 + qz as usize;
        let v = self.grid.get(i).copied().unwrap_or(0);
        v.max(0) as usize
    }

    /// Biome.getTemperature 26.3 (f32 path; legacy-verified formulas).
    fn get_temperature(&self, bi: usize, y: i32) -> f32 {
        let mut f = self.st.temps.get(bi).copied().unwrap_or(0.0);
        if self.st.frozen.get(bi).copied().unwrap_or(false) {
            let fx = self.block_x as f64 * 0.05;
            let fz = self.block_z as f64 * 0.05;
            let d1 = self.st.temps_noises.frozen.value(fx, fz, false) * 7.0;
            let d2 = self
                .st
                .temps_noises
                .biome_info
                .value(self.block_x as f64 * 0.2, self.block_z as f64 * 0.2, false);
            if d1 + d2 < 0.3
                && self
                    .st
                    .temps_noises
                    .biome_info
                    .value(self.block_x as f64 * 0.09, self.block_z as f64 * 0.09, false)
                    < 0.8
            {
                f = 0.2f32;
            }
        }
        let sea = self.sea_level;
        let i = sea + 17;
        if y > i {
            let tx = (self.block_x as f32) / 8.0f32;
            let tz = (self.block_z as f32) / 8.0f32;
            let n = self.st.temps_noises.temperature.value(tx as f64, tz as f64, false);
            let n8 = (n * 8.0f64) as f32;
            let adj = ((n8 + y as f32) - i as f32) * 0.05f32 / 40.0f32;
            f -= adj;
        }
        f
    }

    /// OreVeinRule density/richness getters (volume-path prefilled buffers).
    #[inline]
    fn ore_buffer(&self, which: u32, x: i32, y: i32, z: i32) -> Option<f32> {
        let buf = match which {
            0 => &self.copper_buf,
            1 => &self.iron_buf,
            _ => &self.richness_buf,
        };
        let i = self.volume.index_of_block(x, y, z);
        if i < 0 {
            return None;
        }
        buf.get(i as usize).copied()
    }

    // -- condition evaluation ------------------------------------------------

    fn test(&mut self, ci: usize) -> bool {
        match &self.cfg.conds[ci] {
            SrCond::Not(inner) => {
                let i = *inner;
                !self.test(i)
            }
            _ => {
                let is_y_lazy = matches!(
                    &self.cfg.conds[ci],
                    SrCond::Biome { .. }
                        | SrCond::StoneDepth { .. }
                        | SrCond::Water { .. }
                        | SrCond::YAbove { .. }
                        | SrCond::VerticalGradient { .. }
                        | SrCond::Temperature
                        | SrCond::AbovePreliminary
                );
                let counter = if is_y_lazy { self.y_counter } else { self.xz_counter };
                let v = {
                    let cache: &Vec<(u64, bool)> = if is_y_lazy { &self.y_cache } else { &self.xz_cache };
                    let (last, cached) = cache[ci];
                    if last == counter {
                        Some(cached)
                    } else {
                        None
                    }
                };
                if let Some(v) = v {
                    return v;
                }
                let v = self.compute(ci);
                {
                    let cache: &mut Vec<(u64, bool)> = if is_y_lazy {
                        &mut self.y_cache
                    } else {
                        &mut self.xz_cache
                    };
                    cache[ci] = (counter, v);
                }
                v
            }
        }
    }

    fn compute(&mut self, ci: usize) -> bool {
        match &self.cfg.conds[ci] {
            SrCond::Not(_) => unreachable!(),
            SrCond::Biome { set } => {
                let qy = ((self.block_y - self.cfg.min_y) >> 2).max(0) as u64;
                let key = ((self.xz_counter as u64) << 12) | (qy & 0xFFF);
                let cached = self.biome_cache[ci];
                if cached.0 == key {
                    return cached.1;
                }
                let bi = self.biome_idx_at(self.block_x, self.block_y, self.block_z);
                let v = set.contains(&(bi as u32));
                self.biome_cache[ci] = (key, v);
                if let Some((tx, ty, tz)) = trace_target() {
                    if tx == self.block_x && tz == self.block_z {
                        eprintln!(
                            "TRACE y={} biome cond[{ci}]: grid_bi={bi} set={set:?} => {v}",
                            self.block_y
                        );
                    }
                }
                v
            }
            SrCond::Temperature => {
                let bi = self.biome_idx_at(self.block_x, self.block_y, self.block_z);
                // coldEnoughToSnow = !(getTemperature >= 0.15)
                !(self.get_temperature(bi, self.block_y) >= 0.15f32)
            }
            SrCond::Hole => self.surface_depth <= 0,
            // 26.3: gradientX <= -4 || gradientZ >= 4 (column-precomputed)
            SrCond::Steep => self.gradient_x <= -4 || self.gradient_z >= 4,
            SrCond::NoiseThreshold {
                noise,
                min,
                max,
                is_3d,
            } => {
                let n = self.noise(*noise);
                let v = if *is_3d {
                    n.get(self.block_x as f64, self.block_y as f64, self.block_z as f64)
                } else {
                    n.get(self.block_x as f64, 0.0, self.block_z as f64)
                };
                if let Some((tx, ty, tz)) = trace_target() {
                    if tx == self.block_x && tz == self.block_z {
                        eprintln!(
                            "TRACE y={} noise cond[{ci}]: v={v} [{min},{max}] 3d={is_3d} => {}",
                            self.block_y,
                            v as f64 >= *min && v as f64 <= *max
                        );
                    }
                }
                v as f64 >= *min && v as f64 <= *max
            }
            SrCond::StoneDepth {
                offset,
                add_surface_depth,
                secondary_range,
                ceiling,
            } => {
                let i = if *ceiling {
                    self.stone_depth_below
                } else {
                    self.stone_depth_above
                };
                let j = if *add_surface_depth { self.surface_depth } else { 0 };
                let k = if *secondary_range != 0 {
                    mth_map(self.surface_secondary(), -1.0, 1.0, 0.0, *secondary_range as f64) as i32
                } else {
                    0
                };
                i <= 1 + *offset + j + k
            }
            SrCond::Water {
                offset,
                surface_depth_multiplier,
                add_stone_depth,
            } => {
                if self.water_height == i32::MIN {
                    return true;
                }
                let y2 = self.block_y + if *add_stone_depth { self.stone_depth_above } else { 0 };
                y2 >= self.water_height + *offset + self.surface_depth * *surface_depth_multiplier
            }
            SrCond::YAbove {
                anchor,
                add_stone_depth,
                surface_depth_multiplier,
            } => {
                let y2 = self.block_y + if *add_stone_depth { self.stone_depth_above } else { 0 };
                y2 >= *anchor + self.surface_depth * *surface_depth_multiplier
            }
            SrCond::VerticalGradient {
                name,
                true_y,
                false_y,
            } => {
                let y = self.block_y;
                if y <= *true_y {
                    return true;
                }
                if y >= *false_y {
                    return false;
                }
                let d = mth_map(y as f64, *true_y as f64, *false_y as f64, 1.0, 0.0);
                let mut gf = self.st.factory.from_hash_of(name).fork_positional();
                let mut r: XoroshiroRandomSource = gf.at(self.block_x, self.block_y, self.block_z);
                (r.next_f32() as f64) < d
            }
            SrCond::AbovePreliminary => self.block_y >= self.min_surface_level,
        }
    }

    // -- rule application ------------------------------------------------------

    fn try_apply(&mut self, ri: usize) -> Option<u16> {
        match &self.cfg.rules[ri] {
            SrRule::Sequence(kids) => {
                for k in kids {
                    if let Some(s) = self.try_apply(*k) {
                        return Some(s);
                    }
                }
                None
            }
            SrRule::IfTrue { cond, then } => {
                let c = *cond;
                if self.test(c) {
                    self.try_apply(*then)
                } else {
                    None
                }
            }
            SrRule::Block { state } => Some(self.cfg.ids[*state as usize]),
            SrRule::Bandlands => {
                let h = self.st.clay_handle;
                let v = self.noise(h).get(self.block_x as f64, 0.0, self.block_z as f64) * 4.0;
                // Math.round(f64): floor(v + 0.5) semantics
                let r = (v + 0.5).floor() as i64;
                let y = self.block_y as i64;
                let idx = ((y + r + 192) % 192) as usize;
                let st_idx = self
                    .cfg
                    .bandlands
                    .get(idx)
                    .copied()
                    .unwrap_or(self.cfg.terracotta[0]);
                Some(self.cfg.ids[st_idx as usize])
            }
            SrRule::OreVein {
                ore,
                raw,
                filler,
                raw_chance,
                density_df,
                richness_df,
                gap_df,
            } => {
                // density/richness: volume-path prefilled buffers (26.3
                // getDensitiesInChunk(prefill=true)); gap: point eval.
                let st = self.st;
                let density = self
                    .ore_buffer(*density_df, self.block_x, self.block_y, self.block_z)
                    .unwrap_or_else(|| {
                        st.dfs[*density_df as usize].sample_value(
                            &mut self.ctx_ore,
                            self.block_x,
                            self.block_y,
                            self.block_z,
                        )
                    });
                if density <= 0.0 {
                    return None;
                }
                let mut rnd = self.st.ore_factory.at(self.block_x, self.block_y, self.block_z);
                if rnd.next_f32() > density {
                    return None;
                }
                let richness = self
                    .ore_buffer(*richness_df, self.block_x, self.block_y, self.block_z)
                    .unwrap_or_else(|| {
                        st.dfs[*richness_df as usize].sample_value(
                            &mut self.ctx_ore,
                            self.block_x,
                            self.block_y,
                            self.block_z,
                        )
                    });
                let gap = st.dfs[*gap_df as usize].sample_value(
                    &mut self.ctx_ore,
                    self.block_x,
                    self.block_y,
                    self.block_z,
                );
                if (rnd.next_f32() as f64) < richness as f64 && gap < 0.0 {
                    if rnd.next_f32() < *raw_chance {
                        Some(self.cfg.ids[*raw as usize])
                    } else {
                        Some(self.cfg.ids[*ore as usize])
                    }
                } else {
                    Some(self.cfg.ids[*filler as usize])
                }
            }
        }
    }
}

/// Apply the 26.3 surface rules to a filled chunk buffer (in place).
/// `grid` = the fillBiomes263 quart indexes for this chunk (sections*64).
pub fn apply_surface263(
    st: &SurfaceState263,
    min_y: i32,
    height: i32,
    sea_level: i32,
    cx: i32,
    cz: i32,
    out: &mut [u16; crate::nochunk::OUT_LEN],
    marks: &mut Vec<i64>,
    grid: &[i32],
) {
    // guards mirroring the legacy surface_tail (ids/temps must be live)
    if st.cfg.ids.len() != st.cfg.states.len()
        || st.cfg.bandlands.len() != 192
        || st.temps.is_empty()
        || grid.len() < (height as usize) / 16 * 64
    {
        return;
    }
    let sections = (height as usize) / 16;
    let fluid_states: Vec<bool> = st
        .cfg
        .states
        .iter()
        .map(|s| s.starts_with("minecraft:water") || s.starts_with("minecraft:lava"))
        .collect();
    // heightmap WORLD_SURFACE_WG: topmost non-air per column
    let mut hm = [0i32; 256];
    for lz in 0..16i32 {
        for lx in 0..16i32 {
            let mut top = min_y - 1;
            for sy in (0..sections).rev() {
                let base = sy * 256 + (lz * 16 + lx) as usize;
                let y0 = min_y + (sy as i32) * 16;
                let mut found = false;
                for yy in (0..16).rev() {
                    if out[base + yy * 256] != 0 {
                        top = y0 + yy as i32;
                        found = true;
                        break;
                    }
                }
                if found {
                    break;
                }
            }
            hm[(lz * 16 + lx) as usize] = top;
        }
    }
    let volume = DensityVolume::block_step((16, height, 16), (cx * 16, min_y, cz * 16));
    let mut ctx_ore = SamplerCtx263::cached();
    let mut ctx_csl = SamplerCtx263::cached();
    let mut copper_buf = ctx_ore.scratch.acquire(volume.size());
    let mut iron_buf = ctx_ore.scratch.acquire(volume.size());
    let mut richness_buf = ctx_ore.scratch.acquire(volume.size());
    st.dfs[st.ore_copper as usize].sample_volume(&mut ctx_ore, &mut copper_buf, &volume);
    st.dfs[st.ore_iron as usize].sample_volume(&mut ctx_ore, &mut iron_buf, &volume);
    st.dfs[st.ore_richness as usize].sample_volume(&mut ctx_ore, &mut richness_buf, &volume);

    let cond_count = st.cfg.conds.len();
    let mut walk = Walk263 {
        cfg: &st.cfg,
        st,
        grid,
        heightmap: &hm,
        volume,
        copper_buf,
        iron_buf,
        richness_buf,
        ctx_ore,
        ctx_csl,
        block_x: 0,
        block_z: 0,
        surface_depth: 0,
        surface_secondary: 0.0,
        min_surface_level: 0,
        gradient_x: 0,
        gradient_z: 0,
        xz_counter: 0,
        y_counter: 0,
        last_sec_update: 0,
        block_y: 0,
        stone_depth_above: 0,
        stone_depth_below: 0,
        water_height: i32::MIN,
        xz_cache: vec![(0, false); cond_count],
        y_cache: vec![(0, false); cond_count],
        biome_cache: vec![(0, false); cond_count],
        sea_level,
        fluid_states,
    };

    for lz in 0..16i32 {
        for lx in 0..16i32 {
            let bx = cx * 16 + lx;
            let bz = cz * 16 + lz;
            let col_base = lz * 16 + lx;
            let top = hm[col_base as usize]; // topmost non-air y
            walk.update_xz(bx, bz);
            let surface_biome = walk.biome_idx_at(bx, top + 1, bz) as u32;
            if Some(surface_biome) == st.eroded_badlands_biome {
                eroded_badlands_extension(&mut walk, out, min_y, top);
            }
            let mut depth = 0i32;
            let mut water_height = i32::MIN;
            let mut stone_bottom = i32::MAX;
            let mut y = top;
            while y >= min_y {
                let bi = ((y - min_y) * 256 + col_base) as usize;
                let b = out[bi];
                if b == 0 {
                    // air
                    depth = 0;
                    water_height = i32::MIN;
                } else if b == 3 || b == 17 {
                    // water / lava (fluid non-empty)
                    if water_height == i32::MIN {
                        water_height = y + 1;
                    }
                } else {
                    if stone_bottom >= y {
                        stone_bottom = -32512;
                        let mut y2 = y - 1;
                        while y2 >= min_y - 1 {
                            if y2 < min_y {
                                break;
                            }
                            let i2 = ((y2 - min_y) * 256 + col_base) as usize;
                            let b2 = out[i2];
                            let is_stone = b2 != 0 && b2 != 3 && b2 != 17;
                            if !is_stone {
                                stone_bottom = y2 + 1;
                                break;
                            }
                            y2 -= 1;
                        }
                    }
                    depth += 1;
                    let sdb = y - stone_bottom + 1;
                    walk.update_y(depth, sdb, water_height, y);
                    // 26.3: NO default-block gate — tryApply at every solid.
                    if let Some(new_st) = walk.try_apply(st.cfg.root) {
                        if out[bi] != new_st {
                            out[bi] = new_st;
                            if walk.fluid_states.get(new_st as usize).copied().unwrap_or(false) {
                                marks.push(crate::aquifer263::pos_as_long(bx, y, bz));
                            }
                        }
                    }
                }
                y -= 1;
            }
            if Some(surface_biome) == st.frozen_ocean_biome
                || Some(surface_biome) == st.deep_frozen_ocean_biome
            {
                let min_surface = walk.min_surface_level;
                frozen_ocean_extension(&mut walk, out, min_y, top, min_surface);
            }
        }
    }
}

/// MaterialSystem.erodedBadlandsExtension (26.3 decompile, f64 noise path).
fn eroded_badlands_extension(
    walk: &mut Walk263,
    out: &mut [u16; crate::nochunk::OUT_LEN],
    min_y: i32,
    top: i32,
) {
    let st = walk.st;
    let bx = walk.block_x;
    let bz = walk.block_z;
    // pillarBuffer = min(|surface*8.25|, (double)(pillar(x*0.2) * 15.0F))
    let s = st.noises[badlands_surface_handle(st) as usize].get(bx as f64, 0.0, bz as f64) as f64;
    let p = st.noises[badlands_pillar_handle(st) as usize].get(bx as f64 * 0.2, 0.0, bz as f64 * 0.2) as f64;
    let pillar_buffer = (s * 8.25).abs().min((p * 15.0).abs());
    if pillar_buffer <= 0.0 {
        return;
    }
    let roof = st.noises[badlands_roof_handle(st) as usize]
        .get(bx as f64 * 0.75, 0.0, bz as f64 * 0.75) as f64;
    let pillar_floor = (roof * 1.5).abs();
    let extension_top = 64.0 + (pillar_buffer * pillar_buffer * 2.5)
        .min((pillar_floor * 50.0).ceil() + 24.0);
    let start_y = mth_floor(extension_top);
    if top + 1 > start_y {
        return;
    }
    // scan down for the default block (stone identity) or water (abort)
    let mut y = start_y;
    while y >= min_y {
        let b = out[((y - min_y) * 256 + (bz & 15) * 16 + (bx & 15)) as usize];
        if b == 1 {
            break; // default block (stone)
        }
        if b == 3 {
            return; // water
        }
        y -= 1;
    }
    // fill the air run below startY with the default block
    let mut y = start_y;
    while y >= min_y {
        let idx = ((y - min_y) * 256 + (bz & 15) * 16 + (bx & 15)) as usize;
        if out[idx] != 0 {
            break;
        }
        out[idx] = 1; // defaultBlock (stone)
        y -= 1;
    }
}

// The extension noise handles are resolved positionally from the noises vec
// by NAME — the handles registry keeps a name lookup built at handoff time.
fn ext_handle(st: &SurfaceState263, name: &str) -> u32 {
    st.ext_noise_handles.get(name).copied().unwrap_or(u32::MAX)
}
fn badlands_surface_handle(st: &SurfaceState263) -> u32 {
    ext_handle(st, "minecraft:badlands_surface")
}
fn badlands_pillar_handle(st: &SurfaceState263) -> u32 {
    ext_handle(st, "minecraft:badlands_pillar")
}
fn badlands_roof_handle(st: &SurfaceState263) -> u32 {
    ext_handle(st, "minecraft:badlands_pillar_roof")
}
fn iceberg_surface_handle(st: &SurfaceState263) -> u32 {
    ext_handle(st, "minecraft:iceberg_surface")
}
fn iceberg_pillar_handle(st: &SurfaceState263) -> u32 {
    ext_handle(st, "minecraft:iceberg_pillar")
}
fn iceberg_roof_handle(st: &SurfaceState263) -> u32 {
    ext_handle(st, "minecraft:iceberg_pillar_roof")
}

/// MaterialSystem.frozenOceanExtension (26.3 decompile).
fn frozen_ocean_extension(
    walk: &mut Walk263,
    out: &mut [u16; crate::nochunk::OUT_LEN],
    min_y: i32,
    top: i32,
    min_surface: i32,
) {
    let st = walk.st;
    let bx = walk.block_x;
    let bz = walk.block_z;
    let sea = walk.sea_level;
    let s = st.noises[iceberg_surface_handle(st) as usize].get(bx as f64, 0.0, bz as f64) as f64;
    let p = st.noises[iceberg_pillar_handle(st) as usize]
        .get(bx as f64 * 1.28, 0.0, bz as f64 * 1.28) as f64;
    let iceberg = (s * 8.25).abs().min((p * 15.0).abs());
    if iceberg <= 1.8 {
        return;
    }
    let roof = st.noises[iceberg_roof_handle(st) as usize]
        .get(bx as f64 * 1.17, 0.0, bz as f64 * 1.17) as f64;
    let mut top_d = (iceberg * iceberg * 1.2).min((roof * 1.5 * 40.0).ceil() + 14.0);
    // shouldMeltFrozenOceanIcebergSlightly = getTemperature > 0.1
    let bi = walk.biome_idx_at(bx, top + 1, bz);
    let melt = walk.get_temperature(bi, sea) > 0.1f32;
    if melt {
        top_d -= 2.0;
    }
    if top_d <= 2.0 {
        return;
    }
    let extension_bottom = sea as f64 - top_d - 7.0;
    top_d += sea as f64;
    let extension_top = top_d;
    let mut random = st.factory.at(bx, 0, bz);
    let max_snow_depth = 2 + random.next_int_bound(4);
    let min_snow_height = sea + 18 + random.next_int_bound(10);
    let mut snow_depth = 0i32;
    let snow_id = st
        .snow_block_state
        .and_then(|p| st.cfg.ids.get(p as usize).copied())
        .unwrap_or(0);
    let packed_id = st
        .packed_ice_state
        .and_then(|p| st.cfg.ids.get(p as usize).copied())
        .unwrap_or(0);
    let start_y = (top + 1).max(extension_top as i32 + 1);
    let mut y = start_y;
    while y >= min_surface {
        if y >= min_y {
            let idx = ((y - min_y) * 256 + (bz & 15) * 16 + (bx & 15)) as usize;
            let b = out[idx];
            let is_air = b == 0;
            let is_water = b == 3;
            let hit = (is_air && y < extension_top as i32 && random.next_f64() > 0.01)
                || (is_water
                    && (y as f64) > extension_bottom
                    && y < sea
                    && random.next_f64() > 0.15);
            if hit {
                if snow_depth <= max_snow_depth && y > min_snow_height {
                    out[idx] = snow_id;
                    snow_depth += 1;
                } else {
                    out[idx] = packed_id;
                }
            }
        }
        y -= 1;
    }
}
