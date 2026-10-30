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
    /// cache-wrapped toggle/mask subtree slots (the fused sweep's inputs).
    pub ore_toggle: u32,
    pub ore_mask: u32,
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
    dfs.push(ore_dfs.toggle.clone());
    dfs.push(ore_dfs.mask.clone());
    let (copper_slot, iron_slot, richness_slot, gap_slot) = (0u32, 1u32, 2u32, 3u32);
    let (toggle_slot, mask_slot) = (4u32, 5u32);
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
        ore_toggle: toggle_slot,
        ore_mask: mask_slot,
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

/// The 3x3 chunk neighborhood of biome grids (fillBiomes263 layout:
/// sections*64, index = sy*64 + qx*16 + qy*4 + qz). The 26.3 surface pass
/// resolves biomes through BiomeManager.getBiome — the fiddle can pick a
/// parent quart OUTSIDE the center chunk (parentX = (x-2)>>2 spans chunk
/// quart -1..4), and vanilla then reads the NEIGHBOR chunk's container
/// (LevelReader.getNoiseBiome default -> chunk.getNoiseBiome).
pub struct Neighborhood263 {
    pub cx: i32,
    pub cz: i32,
    pub min_y: i32,
    pub sections: usize,
    /// [(dx+1)*3 + (dz+1)]; None = grid unavailable (degraded: biome
    /// conditions resolve false — vanilla parity requires all 9).
    pub grids: [Option<Box<[i32]>>; 9],
}

impl Neighborhood263 {
    /// ChunkAccess.getNoiseBiome 26.3 (fiddle-resolved world quart coords):
    /// sectionY = (qy>>2) - minSection; clamps: sectionY<0 -> (0, rel 0),
    /// top overflow -> (last section, rel 3); x/z = local (qx&3, qz&3).
    #[inline]
    pub fn quart(&self, qx: i32, qy: i32, qz: i32) -> usize {
        let dx = (qx >> 2) - self.cx;
        let dz = (qz >> 2) - self.cz;
        if !(dx >= -1 && dx <= 1 && dz >= -1 && dz <= 1) {
            return usize::MAX;
        }
        let Some(g) = self.grids[((dx + 1) * 3 + (dz + 1)) as usize].as_deref() else {
            return usize::MAX;
        };
        let mut section_y = (qy >> 2) - (self.min_y >> 4);
        let mut rel = qy & 3;
        if section_y < 0 {
            section_y = 0;
            rel = 0;
        } else if section_y as usize >= self.sections {
            section_y = self.sections as i32 - 1;
            rel = 3;
        }
        let i = section_y as usize * 64
            + ((qx & 3) as usize) * 16
            + (rel as usize) * 4
            + (qz & 3) as usize;
        g.get(i).copied().unwrap_or(0).max(0) as usize
    }
}

/// The 8-corner fiddle pick (BiomeManager.getBiome geometry) — returns the
/// picked parent quart coords. Shared by the grid path (container read) and
/// the point path (climate resampler, topMaterial biomeGetter).
fn fiddle_pick_quart(seed: i64, bx: i32, y: i32, bz: i32) -> (i32, i32, i32) {
    let abs_x = bx - 2;
    let abs_y = y - 2;
    let abs_z = bz - 2;
    let parent_x = abs_x >> 2;
    let parent_y = abs_y >> 2;
    let parent_z = abs_z >> 2;
    let fract_x = (abs_x & 3) as f64 / 4.0;
    let fract_y = (abs_y & 3) as f64 / 4.0;
    let fract_z = (abs_z & 3) as f64 / 4.0;
    let mut min_i = 0usize;
    let mut min_d = f64::INFINITY;
    for i in 0..8usize {
        let x_even = (i & 4) == 0;
        let y_even = (i & 2) == 0;
        let z_even = (i & 1) == 0;
        let corner_x = if x_even { parent_x } else { parent_x + 1 };
        let corner_y = if y_even { parent_y } else { parent_y + 1 };
        let corner_z = if z_even { parent_z } else { parent_z + 1 };
        let dist_x = if x_even { fract_x } else { fract_x - 1.0 };
        let dist_y = if y_even { fract_y } else { fract_y - 1.0 };
        let dist_z = if z_even { fract_z } else { fract_z - 1.0 };
        let next = fiddled_distance(seed, corner_x, corner_y, corner_z, dist_x, dist_y, dist_z);
        if min_d > next {
            min_i = i;
            min_d = next;
        }
    }
    let biome_x = if (min_i & 4) == 0 { parent_x } else { parent_x + 1 };
    let biome_y = if (min_i & 2) == 0 { parent_y } else { parent_y + 1 };
    let biome_z = if (min_i & 1) == 0 { parent_z } else { parent_z + 1 };
    (biome_x, biome_y, biome_z)
}

/// BiomeManager.getBiome 26.3, GRID path (WorldGenRegion wiring): the fiddle
/// over the 8 parent quart corners, then the picked chunk's biome container
/// (LevelReader default). Returns the picked world quart + biome index.
pub fn fiddle_biome_pick(
    hood: &Neighborhood263,
    seed: i64,
    bx: i32,
    y: i32,
    bz: i32,
) -> (i32, i32, i32, usize) {
    let (biome_x, biome_y, biome_z) = fiddle_pick_quart(seed, bx, y, bz);
    let bi = hood.quart(biome_x, biome_y, biome_z);
    (biome_x, biome_y, biome_z, bi)
}

/// MultiNoiseBiomeSource POINT path at a parent quart corner
/// (createUncachedResolver -> Climate.Sampler.sample 26.3): the six raw
/// climate router functions evaluated at the corner's BLOCK coords
/// (QuartPos.toBlock), Climate.target quantization ((long)(f*10000f)),
/// then the preset RTree find_value. Returns the biome name-table index.
pub fn point_biome263(
    b: &crate::biome263::BiomeState263,
    ctx: &mut crate::df263::SamplerCtx263,
    qx: i32,
    qy: i32,
    qz: i32,
) -> usize {
    let (px, py, pz) = (qx * 4, qy * 4, qz * 4);
    let mut v = [0f32; 6];
    for (i, slot) in v.iter_mut().enumerate() {
        *slot = b.climate[i].sample_value(ctx, px, py, pz);
    }
    let values = [
        crate::biome263::quantize_f32(v[0]),
        crate::biome263::quantize_f32(v[1]),
        crate::biome263::quantize_f32(v[2]),
        crate::biome263::quantize_f32(v[3]),
        crate::biome263::quantize_f32(v[4]),
        crate::biome263::quantize_f32(v[5]),
        0, // offset: every vanilla preset entry carries 0
    ];
    let leaf = b.src.find_value(&values);
    b.src.entries[leaf].biome_idx as usize
}

/// LinearCongruentialGenerator.next 26.3: rval *= rval*MULT + INCR; return
/// rval + c (i64 wrap).
#[inline]
fn lcg_next(rval: i64, c: i64) -> i64 {
    rval.wrapping_mul(rval.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407))
        .wrapping_add(c)
}

/// BiomeManager.getFiddle: ((rval >> 24 & 1023) - 512) * 8.7890625e-4.
#[inline]
fn get_fiddle(rval: i64) -> f64 {
    (((rval >> 24) & 1023) - 512) as f64 * 8.7890625e-4
}

/// BiomeManager.getFiddledDistance 26.3 (f64 path, exact add order Z+Y+X).
fn fiddled_distance(seed: i64, xr: i32, yr: i32, zr: i32, dx: f64, dy: f64, dz: f64) -> f64 {
    let mut rval = lcg_next(seed, xr as i64);
    rval = lcg_next(rval, yr as i64);
    rval = lcg_next(rval, zr as i64);
    rval = lcg_next(rval, xr as i64);
    rval = lcg_next(rval, yr as i64);
    rval = lcg_next(rval, zr as i64);
    let fx = get_fiddle(rval);
    rval = lcg_next(rval, seed);
    let fy = get_fiddle(rval);
    rval = lcg_next(rval, seed);
    let fz = get_fiddle(rval);
    let z2 = (dz + fz) * (dz + fz);
    let y2 = (dy + fy) * (dy + fy);
    let x2 = (dx + fx) * (dx + fx);
    z2 + y2 + x2
}

/// The 26.3 column walk (MaterialSystem.buildSurface + RuleEvaluator).
/// Biome resolution: `point_biomes` = Some -> the topMaterial POINT path
/// (fiddle corners resolved via the climate resampler + preset RTree, the
/// createUncachedResolver semantics); None -> the chunk-grid fiddle
/// (Neighborhood263, the buildSurface container semantics).
struct Walk263<'a> {
    cfg: &'a SurfaceConfig,
    st: &'a SurfaceState263,
    hood: Option<&'a Neighborhood263>,
    point_biomes: Option<(
        &'a crate::biome263::BiomeState263,
        std::cell::RefCell<crate::df263::SamplerCtx263>,
    )>,
    zoom_seed: i64,
    heightmap: &'a [i32; 256],
    // chunk volume + prefilled ore-vein buffers (density/richness). The
    // buffers are shared read-only across strip walks (fill_chunk-style
    // z-strip parallelism) — owned fresh ctx per strip.
    volume: DensityVolume,
    csl_slab_volume: DensityVolume,
    csl_slab: &'a [f32],
    copper_buf: &'a [f32],
    iron_buf: &'a [f32],
    richness_buf: &'a [f32],
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
        // — the csl value comes from the prefilled 16x1x16 slab at world y=0
        // (MaterialRuleContext.getMinSurfaceLevel: VOLUME-path DensityBuffer
        // read, not the point path; the two paths differ on interpolated /
        // FindTopSurface nodes).
        let si = self.csl_slab_volume.index_of_block(bx, 0, bz);
        let csl = if si >= 0 {
            self.csl_slab
                .get(si as usize)
                .copied()
                .unwrap_or_else(|| self.st.chunk_surface_level.sample_value(&mut self.ctx_csl, bx, 0, bz))
        } else {
            // out-of-volume fallback (vanilla sampleValue branch; never hit
            // for in-chunk columns)
            self.st.chunk_surface_level.sample_value(&mut self.ctx_csl, bx, 0, bz)
        };
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
        if let Some((b, rc)) = &self.point_biomes {
            // topMaterial biomeGetter: correctBiomeManager::getBiome =
            // withDifferentSource(createUncachedResolver) — the fiddle
            // geometry over POINT-path corner biomes (no container).
            let mut ctx = rc.borrow_mut();
            let (qx, qy, qz) = fiddle_pick_quart(self.zoom_seed, bx, y, bz);
            return point_biome263(b, &mut ctx, qx, qy, qz);
        }
        let hood = self.hood.expect("Walk263: hood or point_biomes required");
        let (qx, qy, qz, bi) = fiddle_biome_pick(hood, self.zoom_seed, bx, y, bz);
        if let Some((tx, ty, tz)) = trace_target() {
            if tx == bx && tz == bz {
                eprintln!(
                    "FIDDLE block=({bx},{y},{bz}) pick_quart=({qx},{qy},{qz}) bi={bi}"
                );
            }
        }
        bi
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
            0 => self.copper_buf,
            1 => self.iron_buf,
            _ => self.richness_buf,
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
                        // 26.3 is_3d noise thresholds re-sample per block y
                        // (vanilla MaterialRuleContext.createNoiseSampler3d
                        // caches on lastUpdateY, NOT on the column) — the
                        // xz-cache painted whole columns with the value from
                        // the top block (sulfur/cinnabar band swaps).
                        | SrCond::NoiseThreshold { is_3d: true, .. }
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
                // vanilla MaterialRuleContext.getBiome: `biome` resets on EVERY
                // updateY (per solid block) — the fiddle pick varies WITHIN a
                // quart row (parentY vs parentY+1 by (y-2)&3), so caching by
                // (column, quart) painted whole quart rows with the biome of
                // their first block (sulfur band Y-boundary divergences).
                // The y_cache wrapper (y_counter) already dedupes per block-y.
                let bi = self.biome_idx_at(self.block_x, self.block_y, self.block_z);
                let v = set.contains(&(bi as u32));
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
// ---------------------------------------------------------------------------
// Fused ore-vein aux sweep: one pass over the fill volume computes copper,
// iron and richness from the shared cache-wrapped toggle/mask buffers,
// replacing the three tree walks (each re-read the shared caches through
// full-volume copies and re-evaluated the y gradient per point).
//
// Bit-exact by construction against the walks:
// - y: YClampedGradient(-4064, 4062, -4064.0, 4062.0) via the walk's own
//   compute_gradient (ClampToEdge): from_value + (clamped - from)*factor,
//   factor = 8126.0/8126.0 = 1.0 exactly in f32;
// - RangeChoice select: !(v >= min && v < max) -> out branch (f32 compares);
// - edge_term(top, bottom) = (clamp(min(top - y, y - bottom), 0, 20) * 0.01)
//   + (-0.2) — same op order as the router-built tree;
// - ConstAdd t - 0.4 / Negate-Sub -t - 0.4 in walk order, no re-association;
// - mth_clamp if-chain semantics for the richness clamp.
// Pinned by the A/B unit test ore_fused_bit_exact_vs_walks (real builder,
// real CI seed) — a router tree change fails CI instead of drifting.
// Kill-switch: CRUS_ORE_FUSE=0.
// ---------------------------------------------------------------------------
fn ore_fuse_enabled() -> bool {
    static ORE_FUSE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ORE_FUSE.get_or_init(|| std::env::var("CRUS_ORE_FUSE").ok().as_deref() != Some("0"))
}

#[inline]
fn ore_y_gradient(by: i32) -> f32 {
    let factor = (4062.0f32 - (-4064.0f32)) / (4062 - (-4064)) as f32;
    let clamped = by.clamp(-4064, 4062);
    -4064.0f32 + (clamped - (-4064)) as f32 * factor
}

#[inline]
fn ore_edge_term(top: f32, bottom: f32, yv: f32) -> f32 {
    let a = top - yv;
    let b = yv - bottom;
    let v = if a < b { a } else { b };
    // mth_clamp(v, 0, 20)
    let v = if v < 0.0 {
        0.0
    } else if v > 20.0 {
        20.0
    } else {
        v
    };
    (v * 0.01) + (-0.2)
}

fn ore_fused_sweep(
    vol: &DensityVolume,
    t: &[f32],
    m: &[f32],
    copper: &mut [f32],
    iron: &mut [f32],
    richness: &mut [f32],
) {
    // index = y + (x + z*size_x)*size_y — y is the fastest axis, so each
    // (x,z) column is a contiguous size_y run.
    let sx = vol.size_x as usize;
    let sy = vol.size_y as usize;
    let sz = vol.size_z as usize;
    for cz in 0..sz {
        for cx in 0..sx {
            let base = (cx + cz * sx) * sy;
            for yy in 0..sy {
                let i = base + yy;
                let yv = ore_y_gradient(vol.block_y(yy as i32));
                let tv = t[i];
                let mv = m[i];
                let ec = ore_edge_term(50.0, 0.0, yv);
                let ei = ore_edge_term(-8.0, -60.0, yv);
                // copper: rc(y, 0, 50, rc(m, 0, 1e6, rc((t-0.4)+ec, 0, 1e6, 0.7, -1), -1), -1)
                copper[i] = if yv >= 0.0 && yv < 50.0 {
                    if mv >= 0.0 && mv < 1_000_000.0 {
                        let v = (tv - 0.4) + ec;
                        if v >= 0.0 && v < 1_000_000.0 {
                            0.7
                        } else {
                            -1.0
                        }
                    } else {
                        -1.0
                    }
                } else {
                    -1.0
                };
                // iron: rc(y, -60, -8, rc(m, 0, 1e6, rc((-t-0.4)+ei, 0, 1e6, 0.7, -1), -1), -1)
                iron[i] = if yv >= -60.0 && yv < -8.0 {
                    if mv >= 0.0 && mv < 1_000_000.0 {
                        let v = (-tv - 0.4) + ei;
                        if v >= 0.0 && v < 1_000_000.0 {
                            0.7
                        } else {
                            -1.0
                        }
                    } else {
                        -1.0
                    }
                } else {
                    -1.0
                };
                // richness = clamp(abs(t), 0.4, 0.6) * 1.0 + (-0.3)
                let a = tv.abs();
                let a = if a < 0.4 {
                    0.4
                } else if a > 0.6 {
                    0.6
                } else {
                    a
                };
                richness[i] = (a * 1.0) + (-0.3);
            }
        }
    }
}

pub fn apply_surface263(
    st: &SurfaceState263,
    min_y: i32,
    height: i32,
    sea_level: i32,
    cx: i32,
    cz: i32,
    out: &mut [u16; crate::nochunk::OUT_LEN],
    marks: &mut Vec<i64>,
    hood: &Neighborhood263,
    zoom_seed: i64,
) {
    // guards mirroring the legacy surface_tail (ids/temps must be live)
    if st.cfg.ids.len() != st.cfg.states.len()
        || st.cfg.bandlands.len() != 192
        || st.temps.is_empty()
        || hood.grids[4].as_deref().map(|g| g.len()).unwrap_or(0)
            < (height as usize) / 16 * 64
    {
        return;
    }
    let sections = (height as usize) / 16;
    // heightmap WORLD_SURFACE_WG: topmost non-air per column
    let mut hm = [0i32; 256];
    for lz in 0..16i32 {
        for lx in 0..16i32 {
            let mut top = min_y - 1;
            for sy in (0..sections).rev() {
                let base = sy * 4096 + (lz * 16 + lx) as usize;
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
    let prof_on = crate::df263::PROF_ON_263.load(std::sync::atomic::Ordering::Relaxed);
    let t_aux = if prof_on {
        Some(std::time::Instant::now())
    } else {
        None
    };
    let mut copper_buf = ctx_ore.scratch.acquire(volume.size());
    let mut iron_buf = ctx_ore.scratch.acquire(volume.size());
    let mut richness_buf = ctx_ore.scratch.acquire(volume.size());
    // Fused path (CRUS_ORE_FUSE=0 kills it): warm the shared toggle/mask
    // cache cells once, then sweep all three outputs from them in one pass.
    // Falls back to the three tree walks on any structural drift.
    let mut fused = ore_fuse_enabled();
    if fused {
        if let crate::df263::DfSam263::Cache { id, a } = &*st.dfs[st.ore_toggle as usize] {
            ctx_ore.warm_volume_cache(*id, a, &volume);
        } else {
            fused = false;
        }
    }
    if fused {
        if let crate::df263::DfSam263::Cache { id, a } = &*st.dfs[st.ore_mask as usize] {
            ctx_ore.warm_volume_cache(*id, a, &volume);
        } else {
            fused = false;
        }
    }
    if fused {
        let (tid, mid) = match (
            &*st.dfs[st.ore_toggle as usize],
            &*st.dfs[st.ore_mask as usize],
        ) {
            (
                crate::df263::DfSam263::Cache { id, .. },
                crate::df263::DfSam263::Cache { id: id2, .. },
            ) => (*id, *id2),
            _ => {
                fused = false;
                (0, 0)
            }
        };
        if fused {
            match (
                ctx_ore.cache_volume_buffer(tid),
                ctx_ore.cache_volume_buffer(mid),
            ) {
                (Some(tbuf), Some(mbuf))
                    if tbuf.len() >= volume.size() && mbuf.len() >= volume.size() =>
                {
                    ore_fused_sweep(
                        &volume,
                        tbuf,
                        mbuf,
                        &mut copper_buf,
                        &mut iron_buf,
                        &mut richness_buf,
                    );
                }
                _ => {
                    fused = false;
                }
            }
        }
    }
    if !fused {
        st.dfs[st.ore_copper as usize].sample_volume(&mut ctx_ore, &mut copper_buf, &volume);
        st.dfs[st.ore_iron as usize].sample_volume(&mut ctx_ore, &mut iron_buf, &volume);
        st.dfs[st.ore_richness as usize].sample_volume(&mut ctx_ore, &mut richness_buf, &volume);
    }
    // vanilla getMinSurfaceLevel: chunk_surface_level prefilled over the
    // 16x1x16 slab at world y=0 (expectedVolume xz extent, y=0), VOLUME path.
    let csl_slab_volume = DensityVolume::block_step((16, 1, 16), (cx * 16, 0, cz * 16));
    let mut csl_slab = ctx_csl.scratch.acquire(256);
    st.chunk_surface_level
        .sample_volume(&mut ctx_csl, &mut csl_slab, &csl_slab_volume);
    if let Some(t) = t_aux {
        crate::df263::PROF_STAGE_SURF_AUX.fetch_add(
            t.elapsed().as_nanos() as u64,
            std::sync::atomic::Ordering::Relaxed,
        );
    }
    let t_walk = if prof_on {
        Some(std::time::Instant::now())
    } else {
        None
    };

    // ---- per-column walk, strip-parallel over z (the fill_chunk pattern) --
    // Every column is independent (own surface-depth/water/stone state, own
    // ore-buf reads, disjoint out indices), so each strip walks its z-range
    // over a PRIVATE copy of the filled buffer and the merge diffs it back —
    // the result is bit-identical to the sequential walk; marks concatenate
    // in strip (z) order, the same order the sequential walk produced.
    let out_shared: &[u16; crate::nochunk::OUT_LEN] = out;
    let threads = crate::fill263::fill_threads().min(4);
    let strips = crate::fill263::split_strips(16, threads);
    let mut results: Vec<(Vec<i64>, Box<[u16; crate::nochunk::OUT_LEN]>)> =
        std::thread::scope(|scope| {
            let mut handles = Vec::with_capacity(strips.len().saturating_sub(1));
            for &(z0, z1) in strips.iter().skip(1) {
                handles.push(scope.spawn(move || {
                    let mut priv_out = Box::new(*out_shared);
                    let mk = walk_columns(
                        st, hood, zoom_seed, &hm, &volume, &csl_slab_volume, &csl_slab,
                        &copper_buf, &iron_buf, &richness_buf, sea_level, cx, cz, min_y, z0, z1,
                        &mut priv_out,
                    );
                    (mk, priv_out)
                }));
            }
            let mut results = Vec::with_capacity(strips.len());
            if let Some(&(z0, z1)) = strips.first() {
                let mut priv_out = Box::new(*out_shared);
                let mk = walk_columns(
                    st, hood, zoom_seed, &hm, &volume, &csl_slab_volume, &csl_slab,
                    &copper_buf, &iron_buf, &richness_buf, sea_level, cx, cz, min_y, z0, z1,
                    &mut priv_out,
                );
                results.push((mk, priv_out));
            }
            // A panicked strip worker is a real bug elsewhere; recompute its
            // range inline so the chunk never ships without surface rules.
            let mut failed: Vec<(i32, i32)> = Vec::new();
            for (h, &(z0, z1)) in handles.iter().zip(strips.iter().skip(1)) {
                match h.join() {
                    Ok(r) => results.push(r),
                    Err(_) => failed.push((z0, z1)),
                }
            }
            for &(z0, z1) in &failed {
                let mut priv_out = Box::new(*out_shared);
                let mk = walk_columns(
                    st, hood, zoom_seed, &hm, &volume, &csl_slab_volume, &csl_slab,
                    &copper_buf, &iron_buf, &richness_buf, sea_level, cx, cz, min_y, z0, z1,
                    &mut priv_out,
                );
                results.push((mk, priv_out));
            }
            results
        });
    for (mk, buf) in results {
        marks.extend(mk);
        for i in 0..crate::nochunk::OUT_LEN {
            if buf[i] != out[i] {
                out[i] = buf[i];
            }
        }
    }
    if let Some(t) = t_walk {
        crate::df263::PROF_STAGE_SURF_WALK.fetch_add(
            t.elapsed().as_nanos() as u64,
            std::sync::atomic::Ordering::Relaxed,
        );
    }
}

/// The per-column surface-rules walk over the z-strip [z0, z1) of a chunk,
/// mutating a PRIVATE copy of the filled buffer; returns the strip's fluid
/// marks. Column-local by construction (own depth/water/stone state per
/// column, disjoint out indices) — strip results merge order-free except
/// for the marks concat, which the caller keeps in strip (z) order.
#[allow(clippy::too_many_arguments)]
fn walk_columns(
    st: &SurfaceState263,
    hood: &Neighborhood263,
    zoom_seed: i64,
    hm: &[i32; 256],
    volume: &DensityVolume,
    csl_slab_volume: &DensityVolume,
    csl_slab: &[f32],
    copper_buf: &[f32],
    iron_buf: &[f32],
    richness_buf: &[f32],
    sea_level: i32,
    cx: i32,
    cz: i32,
    min_y: i32,
    z0: i32,
    z1: i32,
    out: &mut [u16; crate::nochunk::OUT_LEN],
) -> Vec<i64> {
    let cond_count = st.cfg.conds.len();
    let fluid_states: Vec<bool> = st
        .cfg
        .states
        .iter()
        .map(|s| s.starts_with("minecraft:water") || s.starts_with("minecraft:lava"))
        .collect();
    let mut walk = Walk263 {
        cfg: &st.cfg,
        st,
        hood: Some(hood),
        point_biomes: None,
        zoom_seed,
        heightmap: hm,
        volume: volume.clone(),
        csl_slab_volume: csl_slab_volume.clone(),
        csl_slab,
        copper_buf,
        iron_buf,
        richness_buf,
        ctx_ore: SamplerCtx263::cached(),
        ctx_csl: SamplerCtx263::cached(),
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
        sea_level,
        fluid_states,
    };
    let mut marks: Vec<i64> = Vec::new();
    for lz in z0..z1 {
        for lx in 0..16i32 {
            let bx = cx * 16 + lx;
            let bz = cz * 16 + lz;
            let col_base = lz * 16 + lx;
            let top = hm[col_base as usize]; // topmost non-air y
            walk.update_xz(bx, bz);
            // vanilla buildSurface: surfaceBiome at startingHeight =
            // getHeight(WORLD_SURFACE_WG) + 1 = topmost non-air + 2
            let surface_biome = walk.biome_idx_at(bx, top + 2, bz) as u32;
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
    marks
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
    // vanilla gate: proceed iff startingHeight (topmost + 2) <= startY
    if top + 2 > start_y {
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

// ---------------------------------------------------------------------------
// topMaterial 26.3 (the carver-mask grass→surface fix)
// ---------------------------------------------------------------------------

/// Block-id lookup tables for the carve-stage topMaterial hook (fill263).
/// The io ids are NOT the base 0..21 materials when the surface rule writes
/// the same block: extendPalette appends EVERY state string (22, 23, ...),
/// and the section pack maps NMS states back by IDENTITY (last index wins),
/// so a surface-written grass_block/dirt carries the EXTENDED id. The sets
/// therefore unite the base id with every surface-state id of the block.
pub struct TopMatIds {
    /// ids whose NMS state is grass_block or mycelium (vanilla hasGrass).
    pub grass: Vec<u16>,
    /// ids whose NMS state is dirt (the vanilla helperPos check).
    pub dirt: Vec<u16>,
    /// ids whose state carries a non-empty fluid state (water/lava) — for
    /// the vanilla postproc marks on the substance and topMaterial writes.
    pub fluid: Vec<u16>,
}

impl TopMatIds {
    pub fn build(sf: Option<&SurfaceState263>) -> TopMatIds {
        let mut t = TopMatIds {
            grass: vec![4],
            dirt: vec![5],
            fluid: vec![3, 17],
        };
        if let Some(s) = sf {
            for (i, name) in s.cfg.states.iter().enumerate() {
                let Some(&id) = s.cfg.ids.get(i) else {
                    continue;
                };
                if id < 22 {
                    continue; // base material — already in the set
                }
                let base = name.split('[').next().unwrap_or(name);
                match base {
                    "minecraft:grass_block" | "minecraft:mycelium" => {
                        if !t.grass.contains(&id) {
                            t.grass.push(id);
                        }
                    }
                    "minecraft:dirt" => {
                        if !t.dirt.contains(&id) {
                            t.dirt.push(id);
                        }
                    }
                    "minecraft:water" | "minecraft:lava" => {
                        if !t.fluid.contains(&id) {
                            t.fluid.push(id);
                        }
                    }
                    _ => {}
                }
            }
        }
        t
    }

    #[inline]
    pub fn is_grass(&self, id: u16) -> bool {
        self.grass.contains(&id)
    }

    #[inline]
    pub fn is_dirt(&self, id: u16) -> bool {
        self.dirt.contains(&id)
    }

    #[inline]
    pub fn is_fluid(&self, id: u16) -> bool {
        self.fluid.contains(&id)
    }
}

/// MaterialSystem.topMaterial 26.3 (decompile-frozen semantics):
/// a FRESH 1x1x1 MaterialRuleContext at the helper pos, biomeGetter =
/// correctBiomeManager::getBiome (fiddle geometry over the POINT-path
/// climate resampler), updateXZ with the chunk's frozen post-fill
/// WORLD_SURFACE_WG gradients (the _WG heightmap stops updating at the
/// TERRAIN status — heightmapsAfter — so vanilla reads the post-fill map
/// during the whole carve), updateY(1, 1, underFluid ? y+1 : MIN, y), then
/// one rule.tryApply. Density getters inside the rule: ore density/richness
/// sample the VOLUME path over the 1x1x1 expectedVolume (getDensitiesInChunk
/// prefill=true), filler_gap is a point eval (prefill=false), and the
/// min-surface csl reads the 1x1x1 preliminarySurfaceVolume at (x, 0, z)
/// (VOLUME path — indexOfBlock(x, 0, z) == 0 in this context, never -1).
/// Empty rule result -> None (vanilla keeps the dirt).
#[allow(clippy::too_many_arguments)]
pub fn top_material263(
    st: &SurfaceState263,
    b263: &crate::biome263::BiomeState263,
    zoom_seed: i64,
    wg_hm: &[i32; 256],
    min_y: i32,
    height: i32,
    sea_level: i32,
    px: i32,
    py: i32,
    pz: i32,
    under_fluid: bool,
) -> Option<u16> {
    // guards mirroring apply_surface263 (ids/temps must be live)
    if st.cfg.ids.len() != st.cfg.states.len()
        || st.cfg.bandlands.len() != 192
        || st.temps.is_empty()
    {
        return None;
    }
    let fluid_states: Vec<bool> = st
        .cfg
        .states
        .iter()
        .map(|s| s.starts_with("minecraft:water") || s.starts_with("minecraft:lava"))
        .collect();
    // the 1x1x1 expectedVolume at the helper pos (vanilla topMaterial) +
    // the 1x1x1 preliminarySurfaceVolume at (x, 0, z)
    let volume = DensityVolume::block_step((1, 1, 1), (px, py, pz));
    let csl_slab_volume = DensityVolume::block_step((1, 1, 1), (px, 0, pz));
    // fresh, uncached contexts — vanilla constructs a brand-new
    // MaterialRuleContext per topMaterial call (cold LazyXZ/LazyY caches;
    // the sampler values themselves are coordinate-deterministic).
    let mut ctx_ore = SamplerCtx263::uncached();
    let mut ctx_csl = SamplerCtx263::uncached();
    let mut copper_buf = ctx_ore.scratch.acquire(1);
    let mut iron_buf = ctx_ore.scratch.acquire(1);
    let mut richness_buf = ctx_ore.scratch.acquire(1);
    st.dfs[st.ore_copper as usize].sample_volume(&mut ctx_ore, &mut copper_buf, &volume);
    st.dfs[st.ore_iron as usize].sample_volume(&mut ctx_ore, &mut iron_buf, &volume);
    st.dfs[st.ore_richness as usize].sample_volume(&mut ctx_ore, &mut richness_buf, &volume);
    let mut csl_slab = ctx_csl.scratch.acquire(1);
    st.chunk_surface_level
        .sample_volume(&mut ctx_csl, &mut csl_slab, &csl_slab_volume);

    let cond_count = st.cfg.conds.len();
    let mut walk = Walk263 {
        cfg: &st.cfg,
        st,
        hood: None,
        point_biomes: Some((b263, std::cell::RefCell::new(SamplerCtx263::uncached()))),
        zoom_seed,
        heightmap: wg_hm,
        volume,
        csl_slab_volume,
        csl_slab: &csl_slab,
        copper_buf: &copper_buf,
        iron_buf: &iron_buf,
        richness_buf: &richness_buf,
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
        sea_level,
        fluid_states,
    };
    walk.update_xz(px, pz);
    let water_height = if under_fluid { py + 1 } else { i32::MIN };
    walk.update_y(1, 1, water_height, py);
    walk.try_apply(st.cfg.root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ore_fused_bit_exact_vs_walks() {
        // Real builder + the CI bench seed: the fused sweep must be
        // bit-identical to the three tree walks on every point of real
        // volumes (covering the copper [0,50) / iron [-60,-8) bands and
        // the y=0/y=50 boundaries).
        let seed = 3257840388504953787i64;
        let ore = crate::df263_router::RouterBuilder263::new(seed).build_ore_vein_dfs();
        let dfs = [
            ore.copper_density.clone(),
            ore.iron_density.clone(),
            ore.richness.clone(),
        ];
        let (t_node, m_node) = (ore.toggle, ore.mask);
        let (tid, t_in) = match &*t_node {
            crate::df263::DfSam263::Cache { id, a } => (*id, a.clone()),
            _ => panic!("toggle subtree not cache-wrapped"),
        };
        let (mid, m_in) = match &*m_node {
            crate::df263::DfSam263::Cache { id, a } => (*id, a.clone()),
            _ => panic!("mask subtree not cache-wrapped"),
        };
        for &(cx, cz, height) in &[(3i32, -2i32, 64i32), (-11i32, 7i32, 128i32), (0i32, 0i32, 384i32)]
        {
            let vol = DensityVolume::block_step((16, height, 16), (cx * 16, -64, cz * 16));
            // fused path (scratch bufs first — acquire needs &mut, and the
            // cache views below borrow the ctx immutably)
            let mut ca = SamplerCtx263::cached();
            let mut fc = ca.scratch.acquire(vol.size());
            let mut fi = ca.scratch.acquire(vol.size());
            let mut fr = ca.scratch.acquire(vol.size());
            ca.warm_volume_cache(tid, &t_in, &vol);
            ca.warm_volume_cache(mid, &m_in, &vol);
            let (tb, mb) = match (
                ca.cache_volume_buffer(tid),
                ca.cache_volume_buffer(mid),
            ) {
                (Some(tb), Some(mb)) => (tb, mb),
                _ => panic!("caches not warm"),
            };
            assert_eq!(tb.len(), vol.size());
            assert_eq!(mb.len(), vol.size());
            ore_fused_sweep(&vol, tb, mb, &mut fc, &mut fi, &mut fr);
            drop(tb);
            drop(mb);
            // tree-walk path (fresh ctx — caches are pure memoization)
            let mut cb = SamplerCtx263::cached();
            let mut wc = cb.scratch.acquire(vol.size());
            let mut wi = cb.scratch.acquire(vol.size());
            let mut wr = cb.scratch.acquire(vol.size());
            dfs[0].sample_volume(&mut cb, &mut wc, &vol);
            dfs[1].sample_volume(&mut cb, &mut wi, &vol);
            dfs[2].sample_volume(&mut cb, &mut wr, &vol);
            for i in 0..vol.size() {
                assert_eq!(
                    fc[i].to_bits(),
                    wc[i].to_bits(),
                    "copper drift cx={cx} cz={cz} h={height} i={i}"
                );
                assert_eq!(
                    fi[i].to_bits(),
                    wi[i].to_bits(),
                    "iron drift cx={cx} cz={cz} h={height} i={i}"
                );
                assert_eq!(
                    fr[i].to_bits(),
                    wr[i].to_bits(),
                    "richness drift cx={cx} cz={cz} h={height} i={i}"
                );
            }
        }
    }

    #[test]
    fn fiddle_distance_java_reference() {
        // Python reference (i64 wrap LCG, exact f64 add order):
        // seed = obfuscateSeed(3257840388504953787) = 3050481926029917891
        let seed = 3050481926029917891i64;
        let d = fiddled_distance(seed, 7, -3, 9, 0.25, -0.5, 0.75);
        assert!((d - 1.1595440959930419).abs() < 1e-12, "d={d}");
        let d = fiddled_distance(seed, -1, -16, -1, 0.0, 0.0, 0.0);
        assert!((d - 0.17146654129028324).abs() < 1e-12, "d={d}");
    }

    #[test]
    fn lcg_next_java_reference() {
        // Python reference of the exact wrap semantics (rval*MULT+INCR, +c)
        let v = lcg_next(5, 7);
        assert_eq!(v, 296184137228677623);
        // negative input chain
        let v = lcg_next(3050481926029917891, -3);
        assert_eq!(v, 4229891484463208511);
    }

    #[test]
    fn neighborhood_quart_picks_neighbor_chunk() {
        // 4 sections (64 y), center (5, 7): cell value = 1000 + (dx+1)*100 +
        // (dz+1)*10 + (qx*16 + qy*4 + qz) so every path is observable.
        let mk = |cx: i32, cz: i32, base: i32| -> Box<[i32]> {
            (0..4 * 64)
                .map(|i| {
                    let qx = (i % 64) / 16;
                    let qy = (i % 16) / 4;
                    let qz = i % 4;
                    base + qx * 100 + qy * 10 + qz
                })
                .collect()
        };
        let mut grids: [Option<Box<[i32]>>; 9] = std::array::from_fn(|_| None);
        for dz in -1i32..=1 {
            for dx in -1i32..=1 {
                grids[((dx + 1) * 3 + (dz + 1)) as usize] =
                    Some(mk(dx, dz, 100_000 + (dx + 1) * 1000 + (dz + 1) * 100));
            }
        }
        let hood = Neighborhood263 { cx: 5, cz: 7, min_y: -64, sections: 4, grids };
        // bases: center (0,0) = 101100; east (1,0) = 102100. Cell value =
        // base + qx*100 + qy*10 + qz over the INDEX (sy*64+qx*16+qy*4+qz).
        // center cell (qx&3=2, rel=1, qz&3=3) -> index 39 -> (2,1,3)
        let qx = 5 * 4 + 2;
        let qy = (-64 >> 2) + 1;
        let qz = 7 * 4 + 3;
        assert_eq!(hood.quart(qx, qy, qz), 101_100 + 200 + 10 + 3);
        // neighbor east (dx=1): qx = 6*4+0 -> local 0, rel 0, qz&3=3
        let qx = 6 * 4;
        let qy = -64 >> 2;
        assert_eq!(hood.quart(qx, qy, qz), 102_100 + 0 + 0 + 3);
        // below the container: sectionY<0 clamps to (0, rel 0) -> same cell
        let qy = (-64 >> 2) - 5;
        assert_eq!(hood.quart(qx, qy, qz), 102_100 + 0 + 0 + 3);
        // above the top: last section, rel 3 -> index 207 -> (0,3,3)
        let qy = (-64 >> 2) + 4 * 4 + 9;
        assert_eq!(hood.quart(qx, qy, qz), 102_100 + 0 + 30 + 3);
        // out of the 3x3 range -> MAX
        assert_eq!(hood.quart(5 * 4, -64 >> 2, 10 * 4), usize::MAX);
    }
}
