// FFI surface: C-ABI exports + JNI natives (Paper plugin bridge).
//
// JNI notes (verified empirically — see worklog):
//   - The native method receives JNIEnv* = POINTER TO the function table
//     pointer: ((*tbl).Fn)(env, ...) via JNIEnv_.functions.
//   - jni-sys 0.4.1: JNIEnv_ { functions: *const JNINativeInterface_ } — flat
//     function table, all JNI fns are direct fields.

use crate::density::Env as DEnv;
use crate::nochunk::{ChunkGen, OUT_LEN};
use std::collections::HashMap;
use std::sync::Mutex;

// Sessions own a leaked Env (seed-bound); ChunkGen reuses it by &'static.
// Single global map guarded by a Mutex: Paper worker threads serialize per
// ctx in the plugin (ReentrantLock), the global lock is uncontended.
pub struct Ctx {
    gen: ChunkGen<'static>,
    /// 26.3 engine (df263 router + aquifer 26.3 + beardifier 26.3). Some =
    /// the ctx runs the 26.3 fill path (overworld-signature ctxs); None =
    /// legacy 1.21.8 engine (nether/end, probe harnesses).
    pub v263: Option<Gen263State>,
    /// Native multi_noise biome source (pack dimension override). None =
    /// keep the vanilla Java biome path (embedded-vanilla ctx has no
    /// data-driven dimension json).
    biomes: Option<BiomeState>,
    /// Native SurfaceRules engine (pack surface_rule). None = vanilla Java
    /// surface stage stays active.
    surface: Option<SurfaceState>,
    /// Native features engine (stage F). None until the pack assets carry a
    /// data-driven dimension (embedded-vanilla ctx keeps vanilla decorations).
    pub deco: Option<crate::features_master::DecoCtx>,
}

/// 26.3 per-ctx mutable state (marks buffer + last beard list + carve ops).
pub struct Gen263State {
    pub g: crate::fill263::Gen263,
    pub marks: Vec<i64>,
    pub beard: Vec<i32>,
    pub ops: Vec<i64>,
    /// Native 26.3 multi_noise biome source (embedded-vanilla overworld
    /// preset, runtime-extracted). Arc: the JNI biome fill clones the Arc
    /// under the sessions lock and fills WITHOUT holding it (the fill must
    /// not serialize against the noise fills on the global mutex).
    pub biomes: Option<std::sync::Arc<crate::biome263::BiomeState263>>,
    /// Native 26.3 MaterialSystem (surface rules). None = the vanilla Java
    /// surface stage stays active (kill-switch / parse failure / no biomes).
    pub surface: Option<crate::surface263::SurfaceState263>,
}

pub type CarverState = crate::carver::CarverState;

/// Native surface engine state (config parsed from the pack noise_settings;
/// material ids + biome temperatures arrive from the JVM at world load).
pub struct SurfaceState {
    pub cfg: crate::surface::SurfaceConfig,
    pub noise_handles: [u32; 3],
    pub temps: Vec<f32>,
    pub frozen: Vec<bool>,
    pub temps_noises: std::sync::Arc<crate::surface::TempNoises>,
    pub compiled: std::sync::Arc<crate::density::Compiled>,
    pub env: &'static DEnv,
    /// (cx, cz) -> biome quart grid filled by fillBiomes (consumed by fill).
    pub grid_cache: Mutex<HashMap<(i32, i32), Box<[i32]>>>,
}

static TEMP_NOISES: std::sync::OnceLock<std::sync::Arc<crate::surface::TempNoises>> =
    std::sync::OnceLock::new();

/// Noise ids referenced by the winning surface_rule (plus the three static
/// SurfaceSystem noises). Deterministic order, deduped.
fn surface_extra_noises(assets: &crate::worldgen::Assets) -> Vec<String> {
    let ns_id = assets.resolve_noise_settings();
    let path = ns_id
        .replacen(':', "/worldgen/noise_settings/", 1)
        + ".json";
    let Some(bytes) = assets.get(&path) else {
        return Vec::new();
    };
    let Ok(text) = std::str::from_utf8(bytes) else {
        return Vec::new();
    };
    let Ok(j) = crate::json::parse(text) else {
        return Vec::new();
    };
    let mut out = vec![
        "minecraft:surface".to_string(),
        "minecraft:surface_secondary".to_string(),
        "minecraft:clay_bands_offset".to_string(),
        "minecraft:badlands_pillar".to_string(),
        "minecraft:badlands_pillar_roof".to_string(),
        "minecraft:badlands_surface".to_string(),
        "minecraft:iceberg_pillar".to_string(),
        "minecraft:iceberg_pillar_roof".to_string(),
        "minecraft:iceberg_surface".to_string(),
    ];
    if let Some(rule) = j.get("surface_rule") {
        crate::surface::collect_noise_refs(rule, &mut out);
    }
    out.sort();
    out.dedup();
    out
}

fn build_surface_state(
    assets: &crate::worldgen::Assets,
    env: &'static DEnv,
    biomes: Option<&BiomeState>,
) -> Option<SurfaceState> {
    // resolve_noise_settings yields the id ("minecraft:overworld"); map it
    // to the asset path exactly like WorldGen::load_with does.
    let path = assets
        .resolve_noise_settings()
        .replacen(':', "/worldgen/noise_settings/", 1)
        + ".json";
    let bytes = match assets.get(&path) {
        Some(b) => b,
        None => {
            log_warn(&format!("surface: noise_settings asset missing: {path} (have {} paths)", assets.paths().count()));
            return None;
        }
    };
    let ns = match std::str::from_utf8(bytes) {
        Ok(v) => v,
        Err(_) => { log_warn("surface: noise_settings not utf8"); return None; }
    };
    let noise_idx = |id: &str| env.noise.list_idx.get(id).copied();
    let biome_idx = |name: &str| -> Option<u32> {
        biomes
            .and_then(|b| b.src.biome_names.iter().position(|n| n == name))
            .map(|i| i as u32)
    };
    // 26.3: resolve material-rule DF refs ("material:<id>") against the
    // compiled arena's router map; assets come from the WorldGen store.
    let compiled_for_dfc = std::sync::Arc::new(env.compile());
    let dfc = |key: &str| -> Option<u32> { compiled_for_dfc.router.get(key).copied() };
    let assets = crate::worldgen::Assets::new();
    let sb = match crate::surface::build(&env.wg, ns, &noise_idx, &biome_idx, Some(&assets), Some(&dfc)) {
        Some(v) => v,
        None => {
            log_warn("surface config build returned None (parse or handle lookup)");
            return None;
        }
    };
    let compiled = match biomes {
        Some(b) => b.compiled.clone(),
        None => std::sync::Arc::new(env.compile()),
    };
    let temps_noises = TEMP_NOISES
        .get_or_init(|| std::sync::Arc::new(crate::surface::temp_noises()))
        .clone();
    Some(SurfaceState {
        cfg: sb.cfg,
        noise_handles: sb.noise_handles,
        temps: Vec::new(),
        frozen: Vec::new(),
        temps_noises,
        compiled,
        env,
        grid_cache: Mutex::new(HashMap::new()),
    })
}

/// Surface pass after the noise-stage fill (shared by fill/fillMarked/
/// fillBeard — vanilla applies surface at the SURFACE status, right after
/// NOISE, before carvers).
fn surface_tail(ctx: &mut Ctx, cx: i32, cz: i32, out: &mut [u16; OUT_LEN]) {
    let Some(s) = ctx.surface.as_ref() else { return };
    if s.cfg.ids.len() != s.cfg.states.len() || s.cfg.bandlands.len() != 192 || s.temps.is_empty()
    {
        return;
    }
    let grid = s.grid_cache.lock().unwrap().remove(&(cx, cz));
    let bs = ctx.biomes.as_ref().map(|b| (&b.src, &b.climate));
    let default_names: Vec<String> = Vec::new();
    let names: &[String] = ctx
        .biomes
        .as_ref()
        .map(|b| &b.src.biome_names)
        .unwrap_or(&default_names);
    crate::surface_fill::apply_surface(
        s.env,
        &s.compiled,
        &s.cfg,
        s.noise_handles,
        &s.temps,
        &s.frozen,
        &s.temps_noises,
        bs,
        names,
        cx,
        cz,
        out,
        grid.as_deref(),
    );
}

/// Compiled biome resolution state (source tree + climate DFs + arena).
pub struct BiomeState {
    pub src: crate::biomes::BiomeSourceData,
    pub climate: crate::biomes::ClimateDFs,
    pub compiled: std::sync::Arc<crate::density::Compiled>,
    pub env: &'static DEnv,
}

fn build_biome_state(
    assets: &crate::worldgen::Assets,
    env: &'static DEnv,
) -> Option<BiomeState> {
    let compiled = std::sync::Arc::new(env.compile());
    let src = crate::biomes::BiomeSourceData::from_assets(assets)?;
    let climate = crate::biomes::ClimateDFs::from_router(&compiled.router, env, &compiled)?;
    Some(BiomeState {
        src,
        climate,
        compiled,
        env,
    })
}

pub(crate) static SESSIONS: Mutex<Option<HashMap<i64, Ctx>>> = Mutex::new(None);
static NEXT_ID: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(1);

/// Create a generator context for (seed, minY, height, seaLevel). Returns the
/// ctx id (non-zero) or 0 on failure.
pub fn ctx_new(seed: i64, min_y: i32, height: i32, sea_level: i32) -> i64 {
    let id = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    // 26.3-only directive: the overworld signature (minY -64 / height 384)
    // builds the bit-exact 26.3 engine (df263 + aquifer 26.3). Other
    // dimensions (nether/end signatures) stay on the legacy engine until
    // their 26.3 routers are ported.
    let v263 = if min_y == -64 && height == 384 && std::env::var("CRUS_LEGACY").as_deref() != Ok("1") {
        match std::panic::catch_unwind(|| {
            let g = crate::fill263::Gen263::new(seed, min_y, height, sea_level);
            // Native biome stage (stage B-263): the runtime-extracted 26.3
            // overworld preset + the six climate router nodes. A/B kill-switch
            // restores the vanilla Java BIOMES status for parity testing.
            let biomes = if std::env::var("CRUS_NO_NATIVE_BIOMES_263").as_deref() == Ok("1") {
                None
            } else {
                std::panic::catch_unwind(|| {
                    crate::biomes::BiomeSourceData::from_params263(include_str!(
                        "params263_overworld.json"
                    ))
                    .map(|src| std::sync::Arc::new(crate::biome263::BiomeState263 {
                        src,
                        climate: g.climate.clone(),
                        // 26.3 overworld router shape: the five noise fields
                        // are ShiftedXz(y_scale=0); depth is y-dependent.
                        y_invariant: [true, true, true, true, false, true],
                    }))
                })
                .unwrap_or(None)
            };
            // Native surface stage (26.3 MaterialSystem): parse the embedded
            // 26.3 material_rule tree + resolve 26.3 noises/DFs. Needs the
            // native biome name table (cond indexes); kill-switch restores
            // the vanilla Java SURFACE status for A/B parity testing.
            let surface = if std::env::var("CRUS_NO_NATIVE_SURFACE_263").as_deref() == Ok("1") {
                None
            } else {
                std::panic::catch_unwind(|| {
                    crate::surface263::build_surface263(
                        seed,
                        min_y,
                        height,
                        8,
                        sea_level,
                        biomes.as_deref(),
                    )
                })
                .unwrap_or(None)
            };
            (g, biomes, surface)
        }) {
            Ok((g, biomes263, surface263)) => {
                if surface263.is_some() {
                    crate::ffi::log_warn("surface263: native MaterialSystem built");
                }
                Some(Gen263State { g, marks: Vec::new(), beard: Vec::new(), ops: Vec::new(), biomes: biomes263, surface: surface263 })
            }
            Err(_) => None,
        }
    } else {
        None
    };
    let mut env: Box<DEnv> = Box::new(DEnv::new(seed));
    // The plugin resolves the datapack-aware sea level (NoiseGeneratorSettings
    // from the server registry); override the bundled-vanilla default.
    env.wg.sea_level = sea_level as i64;
    let env: &'static DEnv = Box::leak(env);
    let gen = ChunkGen::new(env);
    let carvers = crate::carver::CarverState::from_assets(&crate::worldgen::Assets::new(), env.wg.min_y as i32, env.wg.height as i32);
    let mut map = SESSIONS.lock().unwrap();
    map.get_or_insert_with(HashMap::new).insert(
        id,
        Ctx { gen, v263, biomes: None, surface: None, deco: None },
    );
    id
}

/// Create a context whose worldgen comes from world-datapack asset overrides.
/// Parse failures are reported as Err (never panic across the FFI boundary);
/// the plugin falls back to ctx_new (embedded vanilla) on Err/0.
pub fn ctx_new_assets(
    seed: i64,
    sea_level: i32,
    assets: crate::worldgen::Assets,
) -> Result<i64, String> {
    let id = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let result = std::panic::catch_unwind(move || {
        // Pre-scan the winning surface_rule for noise ids outside the router
        // (pack surface rules reference their own noise defs) so the env
        // instantiates them with the exact vanilla seeding.
        let extra = surface_extra_noises(&assets);
        let mut env: Box<DEnv> = Box::new(DEnv::new_assets_extra(seed, &assets, &extra));
        env.wg.sea_level = sea_level as i64;
        let env: &'static DEnv = Box::leak(env);
        let biomes = build_biome_state(&assets, env);
        let surface = biomes
            .as_ref()
            .and_then(|b| build_surface_state(&assets, b.env, Some(b)));
        (ChunkGen::new(env), biomes, surface, crate::features_master::build_deco_ctx(&assets))
    });
    let (gen, biomes, surface, deco) = match result {
        Ok(v) => v,
        Err(p) => {
            let msg = if let Some(s) = p.downcast_ref::<String>() {
                s.clone()
            } else if let Some(s) = p.downcast_ref::<&str>() {
                (*s).to_string()
            } else {
                "unknown worldgen parse panic".to_string()
            };
            return Err(msg);
        }
    };
    let mut map = SESSIONS.lock().unwrap();
    map.get_or_insert_with(HashMap::new).insert(
        id,
        Ctx { gen, v263: None, biomes, surface, deco },
    );
    Ok(id)
}

pub fn ctx_free(id: i64) {
    if let Some(map) = SESSIONS.lock().unwrap().as_mut() {
        map.remove(&id);
    }
}

pub fn fill(id: i64, cx: i32, cz: i32, out: &mut [u16; OUT_LEN]) {
    // RAW-BORROW INVARIANT (stage B-263 perf fix): the pointer is taken under
    // the sessions lock and the fill runs WITHOUT it. Safety:
    // (a) the Java CtxPool lock serializes JNI calls per ctx (fills, carves,
    //     base_height on their own ctxs) — a ctx is never touched by two
    //     threads at once;
    // (b) the map itself is mutation-frozen during generation: ctxs are
    //     created at world load and freed at world unload, both outside the
    //     chunk pipeline, so no rehash can move the value under the pointer.
    // Holding the lock across the whole fill serialized ALL noise fills AND
    // the biome fills against each other (~13ms exclusive each).
    let ctx_ptr: *mut Ctx = {
        let mut map_guard = SESSIONS.lock().unwrap();
        let map = map_guard.as_mut().expect("sessions map");
        map.get_mut(&id).expect("ctx id") as *mut Ctx
    };
    // SAFETY: see the invariant above — exclusive per-ctx access, stable map.
    let ctx: &mut Ctx = unsafe { &mut *ctx_ptr };
    if let Some(st) = ctx.v263.as_mut() {
        let Gen263State {
            g,
            marks,
            beard,
            surface,
            biomes,
            ..
        } = st;
        let beard_list = std::mem::take(beard);
        g.fill_chunk(cx, cz, &beard_list, out, marks);
        *beard = beard_list;
        // Native surface tail 26.3 — vanilla order: surface at the SURFACE
        // status, right after NOISE, before carvers. The biome grid comes
        // from fillBiomes (BIOMES status runs first); without a grid the
        // tail is skipped (the vanilla surface stage then re-runs on top —
        // same contract as the legacy surface_tail guard). The 26.3 biome
        // read goes through BiomeManager.getBiome (fiddle) which can pick a
        // NEIGHBOR chunk's container — the hood carries all 3x3 grids.
        if let Some(sf) = surface.as_ref() {
            let b263 = biomes.as_ref().map(|b| b as &std::sync::Arc<crate::biome263::BiomeState263>);
            if let Some(hood) = hood263(b263, g.min_y, g.height, cx, cz) {
                let zoom_seed = crate::sha256::obfuscate_seed(g.seed);
                crate::surface263::apply_surface263(
                    sf,
                    g.min_y,
                    g.height,
                    g.sea_level,
                    cx,
                    cz,
                    out,
                    marks,
                    &hood,
                    zoom_seed,
                );
            }
        }
        return;
    }
    ctx.gen.fill(cx, cz, out);
    surface_tail(ctx, cx, cz, out);
}

/// Native vanilla-parity getBaseHeight (iterateNoiseColumn, 1-cell walk).
/// mode: 0 = WORLD_SURFACE_WG/WORLD_SURFACE (stop at first non-air),
///       1 = OCEAN_FLOOR_WG/OCEAN_FLOOR (stop at first blocksMotion state).
/// Returns i32::MIN when the ctx does not exist or the walk panicked —
/// the caller falls back to the NMS delegate (vanilla) path.
pub fn base_height(id: i64, x: i32, z: i32, mode: i32) -> i32 {
    const FAIL: i32 = i32::MIN;
    let mut map_guard = match SESSIONS.lock() {
        Ok(g) => g,
        Err(_) => return FAIL,
    };
    let Some(map) = map_guard.as_mut() else {
        return FAIL;
    };
    let Some(ctx) = map.get_mut(&id) else {
        return FAIL;
    };
    if let Some(st) = ctx.v263.as_ref() {
        let g = &st.g;
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            g.base_height(x, z, mode as u8)
        }));
        return res.unwrap_or(FAIL);
    }
    let gen = &mut ctx.gen;
    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        gen.base_height(x, z, mode as u8)
    }));
    res.unwrap_or(FAIL)
}

/// Native carve entry: decodes the section pack, runs the carver mask+apply,
/// stashes ops (idx<<16|newId) and fluid marks on the ctx state. Returns the
/// op count (or i32::MIN on failure — the caller falls back to vanilla).
pub fn carve(
    id: i64,
    cx: i32,
    cz: i32,
    seed: i64,
    bits: &[i32],
    raw: &[i64],
    raw_off: &[i32],
    pal: &[i16],
    pal_off: &[i32],
    io: &mut [u16; OUT_LEN],
) -> i32 {
    const FAIL: i32 = i32::MIN;
    let mut map_guard = match SESSIONS.lock() {
        Ok(g) => g,
        Err(_) => return FAIL,
    };
    let Some(map) = map_guard.as_mut() else {
        return FAIL;
    };
    let Some(ctx) = map.get_mut(&id) else {
        return FAIL;
    };
    let Some(st) = ctx.v263.as_mut() else {
        return FAIL;
    };
    let pack = crate::fill263::SectionPack { bits, raw, raw_off, pal, pal_off };
    let g = &st.g;
    let (marks_buf, ops_buf) = (&mut st.marks, &mut st.ops);
    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        g.carve_chunk(cx, cz, seed, &pack, io, marks_buf, ops_buf);
    }));
    match res {
        Ok(()) => st.ops.len() as i32,
        Err(_) => FAIL,
    }
}

// ---------------------------------------------------------------------------
// C ABI
// ---------------------------------------------------------------------------

/// Opaque handle = ctx id.
#[no_mangle]
pub extern "C" fn crussty_chunkgen_abi_version() -> u32 {
    crate::abi_version()
}

#[no_mangle]
pub extern "C" fn crussty_chunkgen_ctx_new(seed: i64, min_y: i32, height: i32) -> i64 {
    ctx_new(seed, min_y, height, 63)
}

#[no_mangle]
pub extern "C" fn crussty_chunkgen_ctx_free(ctx: i64) {
    ctx_free(ctx)
}

/// Fills `out` (length OUT_LEN = 98304 u16) with block ids for the chunk.
#[no_mangle]
pub unsafe extern "C" fn crussty_chunkgen_fill(
    ctx: i64,
    cx: i32,
    cz: i32,
    out: *mut u16,
    out_len: usize,
) -> i32 {
    if out_len != OUT_LEN || out.is_null() {
        return -1;
    }
    let slice = std::slice::from_raw_parts_mut(out, OUT_LEN);
    let arr: &mut [u16; OUT_LEN] = slice.try_into().unwrap();
    fill(ctx, cx, cz, arr);
    0
}

// ---------------------------------------------------------------------------
// JNI (plugin side: ChunkGenNative)
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "system" fn Java_com_planeta_crussty_ChunkGenNative_abiVersion(
    _env: *mut jni_sys::JNIEnv,
    _class: jni_sys::jclass,
) -> i32 {
    crate::abi_version() as i32
}

#[no_mangle]
pub unsafe extern "system" fn Java_com_planeta_crussty_ChunkGenNative_newCtx(
    _env: *mut jni_sys::JNIEnv,
    _class: jni_sys::jclass,
    seed: jni_sys::jlong,
    min_y: jni_sys::jint,
    height: jni_sys::jint,
    sea_level: jni_sys::jint,
) -> jni_sys::jlong {
    ctx_new(seed as i64, min_y as i32, height as i32, sea_level as i32)
}

#[no_mangle]
pub unsafe extern "system" fn Java_com_planeta_crussty_ChunkGenNative_freeCtx(
    _env: *mut jni_sys::JNIEnv,
    _class: jni_sys::jclass,
    ctx: jni_sys::jlong,
) {
    ctx_free(ctx as i64)
}

/// newCtxAssets(long seed, int seaLevel, String[] paths, byte[][] blobs) -> long.
/// Builds a worldgen registry from world-datapack asset overrides
/// (paths are repo-relative "<ns>/worldgen/..."). Returns the ctx id, or 0 on
/// parse failure (the caller falls back to newCtx).
#[no_mangle]
pub unsafe extern "system" fn Java_com_planeta_crussty_ChunkGenNative_newCtxAssets(
    env: *mut jni_sys::JNIEnv,
    _class: jni_sys::jclass,
    seed: jni_sys::jlong,
    sea_level: jni_sys::jint,
    paths: jni_sys::jobjectArray,
    blobs: jni_sys::jobjectArray,
) -> jni_sys::jlong {
    let tbl = *env;
    let n = ((*tbl).v1_1.GetArrayLength)(env, paths as jni_sys::jarray) as usize;
    let nb = ((*tbl).v1_1.GetArrayLength)(env, blobs as jni_sys::jarray) as usize;
    if n != nb {
        return 0;
    }
    let mut assets = crate::worldgen::Assets::new();
    for i in 0..n {
        // path string
        let jpath = ((*tbl).v1_1.GetObjectArrayElement)(env, paths, i as jni_sys::jsize);
        if jpath.is_null() {
            return 0;
        }
        let cstr = ((*tbl).v1_1.GetStringUTFChars)(
            env,
            jpath as jni_sys::jstring,
            std::ptr::null_mut(),
        );
        if cstr.is_null() {
            return 0;
        }
        let path = std::ffi::CStr::from_ptr(cstr).to_string_lossy().into_owned();
        ((*tbl).v1_1.ReleaseStringUTFChars)(env, jpath as jni_sys::jstring, cstr);
        // blob bytes
        let jblob = ((*tbl).v1_1.GetObjectArrayElement)(env, blobs, i as jni_sys::jsize)
            as jni_sys::jbyteArray;
        if jblob.is_null() {
            return 0;
        }
        let blen = ((*tbl).v1_1.GetArrayLength)(env, jblob as jni_sys::jarray) as usize;
        if blen == 0 {
            continue;
        }
        let bptr = ((*tbl).v1_1.GetByteArrayElements)(
            env,
            jblob,
            std::ptr::null_mut(),
        );
        if bptr.is_null() {
            return 0;
        }
        let bytes = std::slice::from_raw_parts(bptr as *const u8, blen).to_vec();
        ((*tbl).v1_1.ReleaseByteArrayElements)(
            env,
            jblob,
            bptr,
            0,
        );
        assets.insert(path, bytes);
    }
    match ctx_new_assets(seed as i64, sea_level as i32, assets) {
        Ok(id) => id,
        Err(msg) => {
            // surface the reason in the server log once per failure
            eprintln!("[crussty] newCtxAssets failed, falling back to embedded vanilla: {msg}");
            0
        }
    }
}

/// fill(long ctx, int chunkX, int chunkZ, short[] out) — writes block ids
/// directly into the Java array (zero-copy via GetPrimitiveArrayCritical).
#[no_mangle]
pub unsafe extern "system" fn Java_com_planeta_crussty_ChunkGenNative_fill(
    env: *mut jni_sys::JNIEnv,
    _class: jni_sys::jclass,
    ctx: jni_sys::jlong,
    cx: jni_sys::jint,
    cz: jni_sys::jint,
    out: jni_sys::jshortArray,
) {
    fill_jni(env, ctx, cx, cz, out, std::ptr::null_mut());
}

/// fillMarked(long ctx, int cx, int cz, short[] out, long[] marks) -> int.
/// Same as fill, additionally writes the packed BlockPos list of aquifer
/// fluid-update marks (vanilla shouldScheduleFluidUpdate positions, written
/// block has a fluid state) into `marks`; returns the count.
#[no_mangle]
pub unsafe extern "system" fn Java_com_planeta_crussty_ChunkGenNative_fillMarked(
    env: *mut jni_sys::JNIEnv,
    _class: jni_sys::jclass,
    ctx: jni_sys::jlong,
    cx: jni_sys::jint,
    cz: jni_sys::jint,
    out: jni_sys::jshortArray,
    marks: jni_sys::jlongArray,
) -> jni_sys::jint {
    fill_jni(env, ctx, cx, cz, out, marks)
}

/// fillBeard(long ctx, int cx, int cz, short[] out, long[] marks, int[] beard) -> int.
/// Atomic set_beard + fillMarked: `beard` is the plugin-serialized vanilla
/// Beardifier piece/junction list for THIS chunk
/// ([nPieces, (minX,minY,minZ,maxX,maxY,maxZ,adj,delta)*, nJunctions, (x,y,z)*]).
#[no_mangle]
pub unsafe extern "system" fn Java_com_planeta_crussty_ChunkGenNative_fillBeard(
    env: *mut jni_sys::JNIEnv,
    _class: jni_sys::jclass,
    ctx: jni_sys::jlong,
    cx: jni_sys::jint,
    cz: jni_sys::jint,
    out: jni_sys::jshortArray,
    marks: jni_sys::jlongArray,
    beard: jni_sys::jintArray,
) -> jni_sys::jint {
    if !beard.is_null() {
        let tbl = *env;
        let blen = ((*tbl).v1_1.GetArrayLength)(env, beard as jni_sys::jarray) as usize;
        if blen > 0 {
            let bptr = ((*tbl).v1_1.GetIntArrayElements)(
                env,
                beard as jni_sys::jintArray,
                std::ptr::null_mut(),
            );
            if !bptr.is_null() {
                let data = std::slice::from_raw_parts(bptr, blen).to_vec();
                ((*tbl).v1_1.ReleaseIntArrayElements)(
                    env,
                    beard as jni_sys::jintArray,
                    bptr,
                    0,
                );
                {
                    let mut map = SESSIONS.lock().unwrap();
                    let map = map.as_mut().expect("sessions map");
                    if let Some(c) = map.get_mut(&ctx as &i64) {
                        if let Some(st) = c.v263.as_mut() {
                            st.beard = data;
                        } else {
                            c.gen.set_beard(&data);
                        }
                    }
                }
            }
        }
    }
    fill_jni(env, ctx, cx, cz, out, marks)
}

/// baseHeight(long ctx, int x, int z, int mode) -> int
/// Native replica of vanilla NoiseBasedChunkGenerator.getBaseHeight.
/// mode 0 = WORLD_SURFACE_WG/WORLD_SURFACE, 1 = OCEAN_FLOOR_WG/OCEAN_FLOOR.
/// Returns i32::MIN on any failure (caller falls back to the NMS delegate).
#[no_mangle]
pub unsafe extern "system" fn Java_com_planeta_crussty_ChunkGenNative_baseHeight(
    _env: *mut jni_sys::JNIEnv,
    _class: jni_sys::jclass,
    ctx: jni_sys::jlong,
    x: jni_sys::jint,
    z: jni_sys::jint,
    mode: jni_sys::jint,
) -> jni_sys::jint {
    base_height(ctx as i64, x as i32, z as i32, mode as i32)
}

/// profStatsGlobal() -> String: the global per-fill phase snapshot.
#[no_mangle]
pub unsafe extern "system" fn Java_com_planeta_crussty_ChunkGenNative_profStatsGlobal(
    env: *mut jni_sys::JNIEnv,
    _class: jni_sys::jclass,
) -> jni_sys::jstring {
    let s = crate::nochunk::jni_prof_snapshot();
    let tbl = *env;
    let c = match std::ffi::CString::new(s) {
        Ok(c) => c,
        Err(_) => return std::ptr::null_mut(),
    };
    ((*tbl).v1_1.NewStringUTF)(env, c.as_ptr())
}

/// Shared fill path. If `marks` is non-null it receives the packed fluid
/// update positions; returns the mark count (0 when marks is null).
unsafe fn fill_jni(
    env: *mut jni_sys::JNIEnv,
    ctx: jni_sys::jlong,
    cx: jni_sys::jint,
    cz: jni_sys::jint,
    out: jni_sys::jshortArray,
    marks: jni_sys::jlongArray,
) -> jni_sys::jint {
    if out.is_null() {
        return 0;
    }
    // jni-sys: JNIEnv = *const JNINativeInterface_ (C "JNIEnv"), and the macro
    // jni_to_union rewrites the table into a union of per-version views
    // (v1_1, v1_2, ...). The JVM hands us JNIEnv* — one deref, then the
    // versioned view, passing the original env through (C ((*env)->Fn)(env,...)).
    let tbl = *env;
    let len = ((*tbl).v1_1.GetArrayLength)(env, out as jni_sys::jarray) as usize;
    if len != OUT_LEN {
        // contract mismatch: refuse (plugin passes short[98304])
        return 0;
    }
    let ptr = ((*tbl).v1_2.GetPrimitiveArrayCritical)(
        env,
        out as jni_sys::jarray,
        std::ptr::null_mut(),
    ) as *mut u16;
    if ptr.is_null() {
        return 0;
    }
    let slice = std::slice::from_raw_parts_mut(ptr, OUT_LEN);
    let arr: &mut [u16; OUT_LEN] = slice.try_into().unwrap();
    fill(ctx as i64, cx as i32, cz as i32, arr);
    ((*tbl).v1_2.ReleasePrimitiveArrayCritical)(
        env,
        out as jni_sys::jarray,
        ptr as *mut core::ffi::c_void,
        0,
    );
    if marks.is_null() {
        return 0;
    }
    let mlen = ((*tbl).v1_1.GetArrayLength)(env, marks as jni_sys::jarray) as usize;
    let count = {
        let map = SESSIONS.lock().unwrap();
        let map = map.as_ref().expect("sessions map");
        let c = map.get(&ctx as &i64).expect("ctx id");
        let m: &[i64] = if let Some(st) = c.v263.as_ref() {
            &st.marks
        } else {
            c.gen.marks()
        };
        let n = m.len().min(mlen);
        let mptr = ((*tbl).v1_1.GetLongArrayElements)(
            env,
            marks as jni_sys::jlongArray,
            std::ptr::null_mut(),
        );
        if !mptr.is_null() {
            std::ptr::copy_nonoverlapping(m.as_ptr() as *const u64, mptr as *mut u64, n);
            ((*tbl).v1_1.ReleaseLongArrayElements)(
                env,
                marks as jni_sys::jlongArray,
                mptr,
                0,
            );
            n as i32
        } else {
            0
        }
    };
    count
}

// ---------------------------------------------------------------------------
// Biome JNI (native multi_noise takeover for pack worlds)
// ---------------------------------------------------------------------------

/// ctxBiomeNames(long ctx) -> String[] — the deduplicated biome names of the
/// native biome source, in the stable index order used by fillBiomes.
/// Returns null when the ctx has no native biome source.
#[no_mangle]
pub unsafe extern "system" fn Java_com_planeta_crussty_ChunkGenNative_ctxBiomeNames(
    env: *mut jni_sys::JNIEnv,
    _class: jni_sys::jclass,
    ctx: jni_sys::jlong,
) -> jni_sys::jobjectArray {
    let tbl = *env;
    let bs_guard = SESSIONS.lock().unwrap();
    // v263 ctxs carry the 26.3 preset biome source inside the Gen263 slot;
    // legacy ctxs use the pack-assets slot.
    let bs = bs_guard
        .as_ref()
        .and_then(|m| m.get(&(ctx as i64)))
        .and_then(|c| {
            c.biomes
                .as_ref()
                .map(|b| &b.src)
                .or_else(|| {
                    c.v263
                        .as_ref()
                        .and_then(|v| v.biomes.as_ref())
                        .map(|b| &b.src)
                })
        });
    let Some(bs) = bs else {
        return std::ptr::null_mut();
    };
    let str_class = ((*tbl).v1_1.FindClass)(env, c"java/lang/String".as_ptr());
    if str_class.is_null() {
        return std::ptr::null_mut();
    }
    let names = &bs.biome_names;
    let arr = ((*tbl).v1_1.NewObjectArray)(
        env,
        names.len() as jni_sys::jsize,
        str_class,
        std::ptr::null_mut(),
    );
    if arr.is_null() {
        return std::ptr::null_mut();
    }
    for (i, name) in names.iter().enumerate() {
        let cstr = match std::ffi::CString::new(name.as_str()) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let js = ((*tbl).v1_1.NewStringUTF)(env, cstr.as_ptr());
        if js.is_null() {
            continue;
        }
        ((*tbl).v1_1.SetObjectArrayElement)(env, arr, i as jni_sys::jsize, js);
    }
    arr as jni_sys::jobjectArray
}

/// fillBiomes(long ctx, int cx, int cz, int[] out) -> int.
/// Writes the chunk's biome quart indexes (biome name-table indexes, NOT
/// registry ids) in the vanilla fillBiomesFromNoise order:
/// out[s * 64 + x * 16 + y * 4 + z], sections ascending (24 sections x
/// 4x4x4 quarts = 1536 for height 384). Returns the count written, or -1
/// when the ctx has no native biome source (caller keeps vanilla biomes).
#[no_mangle]
pub unsafe extern "system" fn Java_com_planeta_crussty_ChunkGenNative_fillBiomes(
    env: *mut jni_sys::JNIEnv,
    _class: jni_sys::jclass,
    ctx: jni_sys::jlong,
    cx: jni_sys::jint,
    cz: jni_sys::jint,
    out: jni_sys::jintArray,
) -> jni_sys::jint {
    let tbl = *env;
    let state_guard = SESSIONS.lock().unwrap();
    let Some(c) = state_guard.as_ref().and_then(|m| m.get(&(ctx as i64))) else {
        return -1;
    };
    let len = ((*tbl).v1_1.GetArrayLength)(env, out as jni_sys::jarray) as usize;
    if len < 64 {
        return -1;
    }
    let ptr = ((*tbl).v1_1.GetIntArrayElements)(env, out, std::ptr::null_mut());
    if ptr.is_null() {
        return -1;
    }
    let slice = std::slice::from_raw_parts_mut(ptr as *mut i32, len);
    // v263 path: clone the Arc + copy (min_y, height) under the sessions
    // lock, then fill WITHOUT the lock (no serialization vs noise fills;
    // the warm-start cell is a thread_local, the climate nodes immutable).
    let b263 = c
        .v263
        .as_ref()
        .and_then(|v| v.biomes.clone().map(|b| (b, v.g.min_y, v.g.height)));
    if let Some((b, min_y, height)) = b263 {
        let n = (height as usize) / 16 * 64;
        if len < n {
            ((*tbl).v1_1.ReleaseIntArrayElements)(env, out, ptr, 0);
            return -1;
        }
        drop(state_guard);
        crate::biome263::fill_biomes263(&b, min_y, height, cx as i32, cz as i32, &mut slice[..n]);
        ((*tbl).v1_1.ReleaseIntArrayElements)(env, out, ptr, 0);
        // the grid handoff is global (see V263_GRIDS) — the consuming fill
        // may run on ANY pool ctx.
        store_biome_grid263(cx as i32, cz as i32, &slice[..n]);
        return n as jni_sys::jint;
    }
    let Some(state) = c.biomes.as_ref() else {
        ((*tbl).v1_1.ReleaseIntArrayElements)(env, out, ptr, 0);
        return -1;
    };
    let height = state.env.wg.height as usize;
    let sections = height / 16;
    let n = sections * 64;
    if len < n {
        ((*tbl).v1_1.ReleaseIntArrayElements)(env, out, ptr, 0);
        return -1;
    }
    crate::biomes::fill_biomes(
        &state.src,
        &state.climate,
        state.env,
        &state.compiled,
        cx as i32,
        cz as i32,
        &mut slice[..n],
    );
    store_biome_grid(&c, cx as i32, cz as i32, &slice[..n]);
    ((*tbl).v1_1.ReleaseIntArrayElements)(env, out, ptr, 0);
    n as jni_sys::jint
}

/// Store a computed biome grid for (cx, cz) so the surface tail consumes it
/// instead of recomputing climates (fillBiomes JNI path). Public so the
/// isolated bench can replicate the plugin's BIOMES -> NOISE order.
pub fn store_biome_grid(c: &Ctx, cx: i32, cz: i32, grid: &[i32]) {
    if let Some(s) = c.surface.as_ref() {
        let mut g = s.grid_cache.lock().unwrap();
        if g.len() > 48 {
            g.clear();
        }
        g.insert((cx, cz), Box::from(grid));
    }
}

/// 26.3 biome-grid handoff (BIOMES status -> NOISE fill tail). GLOBAL by
/// (cx, cz): fillBiomes runs on pool.firstCtx() while fills acquire ANY pool
/// ctx — a per-ctx cache would skip the surface tail on every other ctx
/// (raw-stone chunks). The grid is per-chunk data, not per-ctx state.
static V263_GRIDS: std::sync::Mutex<Option<std::collections::HashMap<(i32, i32), Box<[i32]>>>> =
    std::sync::Mutex::new(None);

pub fn store_biome_grid263(cx: i32, cz: i32, grid: &[i32]) {
    let mut guard = V263_GRIDS.lock().unwrap();
    let map = guard.get_or_insert_with(std::collections::HashMap::new);
    // generous cap: the chunk system may run many BIOMES steps before the
    // matching NOISE fills (per-chunk status ordering, gen single-threaded);
    // a tight cap + clear() would drop pending grids -> raw-stone chunks.
    // 6 KiB per grid, consumed (removed) by the fill tail immediately after.
    if map.len() > 8192 {
        map.clear();
    }
    map.insert((cx, cz), Box::from(grid));
}

/// Take the cached grid for (cx, cz) (the fill tail consumes it once).
fn take_biome_grid263(cx: i32, cz: i32) -> Option<Box<[i32]>> {
    let mut guard = V263_GRIDS.lock().unwrap();
    guard.as_mut().and_then(|m| m.remove(&(cx, cz)))
}

/// Clone a grid WITHOUT consuming it (neighbors keep theirs for their own
/// fill tail). Computes on the fly when the map entry is gone (eviction /
/// concurrent consumption): fill_biomes263 is deterministic, so the result
/// is identical either way.
fn clone_or_compute_grid263(
    b263: &std::sync::Arc<crate::biome263::BiomeState263>,
    min_y: i32,
    height: i32,
    cx: i32,
    cz: i32,
) -> Option<Box<[i32]>> {
    {
        let guard = V263_GRIDS.lock().unwrap();
        if let Some(m) = guard.as_ref() {
            if let Some(g) = m.get(&(cx, cz)) {
                return Some(g.clone());
            }
        }
    }
    let n = (height as usize) / 16 * 64;
    let mut buf = vec![0i32; n].into_boxed_slice();
    crate::biome263::fill_biomes263(b263, min_y, height, cx, cz, &mut buf);
    Some(buf)
}

/// Build the 3x3 biome-grid neighborhood for the 26.3 surface pass (fiddle
/// pick can land in a neighbor chunk's container). The center grid is
/// consumed from V263_GRIDS (same contract as before); neighbors are cloned
/// or recomputed. Returns None when the center grid is missing — the caller
/// skips the native tail (vanilla Java surface re-runs on top).
fn hood263(
    b263: Option<&std::sync::Arc<crate::biome263::BiomeState263>>,
    min_y: i32,
    height: i32,
    cx: i32,
    cz: i32,
) -> Option<crate::surface263::Neighborhood263> {
    let center = take_biome_grid263(cx, cz)?;
    let sections = (height as usize) / 16;
    let mut grids: [Option<Box<[i32]>>; 9] = std::array::from_fn(|_| None);
    grids[4] = Some(center);
    for dz in -1i32..=1 {
        for dx in -1i32..=1 {
            if dx == 0 && dz == 0 {
                continue;
            }
            grids[((dx + 1) * 3 + (dz + 1)) as usize] = b263
                .and_then(|b| clone_or_compute_grid263(b, min_y, height, cx + dx, cz + dz));
        }
    }
    Some(crate::surface263::Neighborhood263 {
        cx,
        cz,
        min_y,
        sections,
        grids,
    })
}

/// Bench helper: the v263 biome grid + global store (no JNI).
pub fn bench_fill_biomes263(id: i64, cx: i32, cz: i32, out: &mut [i32]) -> i32 {
    let (b, min_y, height) = {
        let guard = SESSIONS.lock().unwrap();
        let Some((b, mn, h)) = guard
            .as_ref()
            .and_then(|m| m.get(&id))
            .and_then(|c| c.v263.as_ref())
            .and_then(|v| v.biomes.clone().map(|b| (b, v.g.min_y, v.g.height)))
        else {
            return -1;
        };
        (b, mn, h)
    };
    let n = (height as usize) / 16 * 64;
    if out.len() < n {
        return -1;
    }
    crate::biome263::fill_biomes263(&b, min_y, height, cx, cz, &mut out[..n]);
    store_biome_grid263(cx, cz, &out[..n]);
    n as i32
}

/// Bench/plugin-test helper: compute + store the biome grid for one chunk
/// (the native half of the plugin's fillBiomes, without JNI).
pub fn bench_fill_biomes(id: i64, cx: i32, cz: i32, out: &mut [i32]) -> i32 {
    let guard = SESSIONS.lock().unwrap();
    let map = guard.as_ref().expect("sessions map");
    let c = map.get(&id).expect("ctx id");
    let Some(state) = &c.biomes else { return -1 };
    let sections = state.env.wg.height as usize / 16;
    let n = sections * 64;
    if out.len() < n {
        return -1;
    }
    crate::biomes::fill_biomes(
        &state.src,
        &state.climate,
        state.env,
        &state.compiled,
        cx,
        cz,
        &mut out[..n],
    );
    store_biome_grid(c, cx, cz, &out[..n]);
    n as i32
}

// ---------------------------------------------------------------------------
// JNI: native surface engine handshake
// ---------------------------------------------------------------------------

/// surfaceStates(long ctx) -> String[] — distinct block state strings of the
/// parsed surface rule tree (first-appearance order). Null when the ctx has
/// no native surface engine.
#[no_mangle]
pub unsafe extern "system" fn Java_com_planeta_crussty_ChunkGenNative_surfaceStates(
    env: *mut jni_sys::JNIEnv,
    _class: jni_sys::jclass,
    ctx: jni_sys::jlong,
) -> jni_sys::jobjectArray {
    let tbl = *env;
    let guard = SESSIONS.lock().unwrap();
    let states: &[String] = match guard
        .as_ref()
        .and_then(|m| m.get(&(ctx as i64)))
        .and_then(|c| {
            c.surface
                .as_ref()
                .map(|s| &s.cfg.states)
                .or_else(|| c.v263.as_ref().and_then(|v| v.surface.as_ref()).map(|s| &s.cfg.states))
        }) {
        Some(s) => s,
        None => return std::ptr::null_mut(),
    };
    let str_class = ((*tbl).v1_1.FindClass)(env, c"java/lang/String".as_ptr());
    if str_class.is_null() {
        return std::ptr::null_mut();
    }
    let arr = ((*tbl).v1_1.NewObjectArray)(
        env,
        states.len() as jni_sys::jsize,
        str_class,
        std::ptr::null_mut(),
    );
    if arr.is_null() {
        return std::ptr::null_mut();
    }
    for (i, name) in states.iter().enumerate() {
        let cstr = match std::ffi::CString::new(name.as_str()) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let js = ((*tbl).v1_1.NewStringUTF)(env, cstr.as_ptr());
        if js.is_null() {
            continue;
        }
        ((*tbl).v1_1.SetObjectArrayElement)(env, arr, i as jni_sys::jsize, js);
    }
    arr as jni_sys::jobjectArray
}

/// surfaceSetIds(long ctx, short[] ids) -> int. Assigns the JVM material id
/// for every state string (same order as surfaceStates) and builds the
/// 192 bandlands bands (needs the world-seed factory). 0 on success.
#[no_mangle]
pub unsafe extern "system" fn Java_com_planeta_crussty_ChunkGenNative_surfaceSetIds(
    env: *mut jni_sys::JNIEnv,
    _class: jni_sys::jclass,
    ctx: jni_sys::jlong,
    ids: jni_sys::jshortArray,
) -> jni_sys::jint {
    if ids.is_null() {
        return -1;
    }
    let tbl = *env;
    let len = ((*tbl).v1_1.GetArrayLength)(env, ids as jni_sys::jarray) as usize;
    let ptr = ((*tbl).v1_1.GetShortArrayElements)(env, ids, std::ptr::null_mut());
    if ptr.is_null() {
        return -1;
    }
    let v: Vec<u16> = std::slice::from_raw_parts(ptr as *const u16, len).to_vec();
    ((*tbl).v1_1.ReleaseShortArrayElements)(env, ids, ptr, 0);
    // NOTE: do NOT pre-lock SESSIONS here — surface_assign_ids takes it
    // internally (std Mutex is not reentrant; pre-locking deadlocked boot).
    surface_assign_ids_inner(ctx as i64, v)
}

/// Rust-side helper shared by JNI + bins: assign material ids + bands.
pub fn surface_assign_ids(id: i64, ids: &[u16]) -> Result<(), String> {
    let mut guard = SESSIONS.lock().unwrap();
    let c = guard
        .as_mut()
        .and_then(|m| m.get_mut(&id))
        .ok_or_else(|| "no ctx".to_string())?;
    if let Some(s) = c.surface.as_mut() {
        if ids.len() != s.cfg.states.len() {
            return Err(format!("ids {} != states {}", ids.len(), s.cfg.states.len()));
        }
        s.cfg.ids = ids.to_vec();
        let factory = crate::surface::root_factory(s.env.seed);
        s.cfg.bandlands = crate::surface::generate_bands(&s.cfg, &factory);
        return Ok(());
    }
    let st = c.v263.as_mut().ok_or_else(|| "no surface".to_string())?;
    let Some(s) = st.surface.as_mut() else {
        return Err("no surface".into());
    };
    if ids.len() != s.cfg.states.len() {
        return Err(format!("ids {} != states {}", ids.len(), s.cfg.states.len()));
    }
    s.cfg.ids = ids.to_vec();
    let factory = crate::surface::root_factory(st.g.seed);
    s.cfg.bandlands = crate::surface::generate_bands(&s.cfg, &factory);
    Ok(())
}

fn surface_assign_ids_inner(id: i64, v: Vec<u16>) -> i32 {
    match surface_assign_ids(id, &v) {
        Ok(()) => 0,
        Err(e) => {
            log_warn(&e);
            -3
        }
    }
}

/// surfaceSetTemps(long ctx, float[] temps, boolean[] frozen) -> int.
/// Per native biome index (ctxBiomeNames order): base temperature and the
/// FROZEN temperature-modifier flag.
#[no_mangle]
pub unsafe extern "system" fn Java_com_planeta_crussty_ChunkGenNative_surfaceSetTemps(
    env: *mut jni_sys::JNIEnv,
    _class: jni_sys::jclass,
    ctx: jni_sys::jlong,
    temps: jni_sys::jfloatArray,
    frozen: jni_sys::jbooleanArray,
) -> jni_sys::jint {
    if temps.is_null() || frozen.is_null() {
        return -1;
    }
    let tbl = *env;
    let tlen = ((*tbl).v1_1.GetArrayLength)(env, temps as jni_sys::jarray) as usize;
    let flen = ((*tbl).v1_1.GetArrayLength)(env, frozen as jni_sys::jarray) as usize;
    if tlen != flen {
        return -2;
    }
    let tptr = ((*tbl).v1_1.GetFloatArrayElements)(env, temps, std::ptr::null_mut());
    if tptr.is_null() {
        return -1;
    }
    let tv: Vec<f32> = std::slice::from_raw_parts(tptr as *const f32, tlen).to_vec();
    ((*tbl).v1_1.ReleaseFloatArrayElements)(env, temps, tptr, 0);
    let fptr = ((*tbl).v1_1.GetBooleanArrayElements)(env, frozen, std::ptr::null_mut());
    if fptr.is_null() {
        return -1;
    }
    let fv: Vec<bool> = {
        let raw = std::slice::from_raw_parts(fptr as *const u8, flen);
        raw.iter().map(|b| *b != 0).collect()
    };
    ((*tbl).v1_1.ReleaseBooleanArrayElements)(env, frozen, fptr, 0);
    match surface_assign_temps(ctx as i64, &tv, &fv) {
        Ok(()) => 0,
        Err(_) => -1,
    }
}

/// Rust-side helper shared by JNI + bins.
pub fn surface_assign_temps(id: i64, temps: &[f32], frozen: &[bool]) -> Result<(), String> {
    let mut guard = SESSIONS.lock().unwrap();
    let c = guard
        .as_mut()
        .and_then(|m| m.get_mut(&id))
        .ok_or_else(|| "no ctx".to_string())?;
    if let Some(s) = c.surface.as_mut() {
        s.temps = temps.to_vec();
        s.frozen = frozen.to_vec();
        return Ok(());
    }
    let st = c.v263.as_mut().ok_or_else(|| "no surface".to_string())?;
    let Some(s) = st.surface.as_mut() else {
        return Err("no surface".into());
    };
    s.temps = temps.to_vec();
    s.frozen = frozen.to_vec();
    Ok(())
}

/// Rust-side helper: native biome name table of a ctx.
pub fn ctx_biome_names_of(id: i64) -> Option<Vec<String>> {
    let guard = SESSIONS.lock().unwrap();
    guard
        .as_ref()
        .and_then(|m| m.get(&id))
        .and_then(|c| c.biomes.as_ref())
        .map(|b| b.src.biome_names.clone())
}

/// Rust-side helper: surface state strings of a ctx.
pub fn surface_states_of(id: i64) -> Option<Vec<String>> {
    let guard = SESSIONS.lock().unwrap();
    guard
        .as_ref()
        .and_then(|m| m.get(&id))
        .and_then(|c| {
            c.surface
                .as_ref()
                .map(|s| s.cfg.states.clone())
                .or_else(|| {
                    c.v263
                        .as_ref()
                        .and_then(|v| v.surface.as_ref())
                        .map(|s| s.cfg.states.clone())
                })
        })
}

pub(crate) fn log_warn(msg: &str) {
    eprintln!("[crussty] {msg}");
}

/// carve(long ctx, int cx, int cz, long seed, int[] meta(72), long[] raw,
/// int[] offs(48), short[] pal, long[] marks) -> int opCount.
/// meta layout: bits[24] then rawOff[24] then palOff[24]; offs unused
/// (compat pad). io is rebuilt internally; ops (idx<<16|newId) are stashed
/// on the ctx and read back via carveOps JNI.
#[no_mangle]
pub unsafe extern "system" fn Java_com_planeta_crussty_ChunkGenNative_carve(
    env: *mut jni_sys::JNIEnv,
    _class: jni_sys::jclass,
    ctx: jni_sys::jlong,
    cx: jni_sys::jint,
    cz: jni_sys::jint,
    seed: jni_sys::jlong,
    meta: jni_sys::jintArray,
    raw: jni_sys::jlongArray,
    pal: jni_sys::jshortArray,
) -> jni_sys::jint {
    let tbl = *env;
    let mlen = ((*tbl).v1_1.GetArrayLength)(env, meta as jni_sys::jarray) as usize;
    let rlen = ((*tbl).v1_1.GetArrayLength)(env, raw as jni_sys::jarray) as usize;
    let plen = ((*tbl).v1_1.GetArrayLength)(env, pal as jni_sys::jarray) as usize;
    if mlen < 72 {
        return i32::MIN;
    }
    let mp = ((*tbl).v1_1.GetIntArrayElements)(env, meta, std::ptr::null_mut());
    let rp = ((*tbl).v1_1.GetLongArrayElements)(env, raw, std::ptr::null_mut());
    let pp = ((*tbl).v1_1.GetShortArrayElements)(env, pal, std::ptr::null_mut());
    if mp.is_null() || rp.is_null() || pp.is_null() {
        return i32::MIN;
    }
    let bits = std::slice::from_raw_parts(mp, 24);
    let raw_off = std::slice::from_raw_parts(mp.add(24), 24);
    let pal_off = std::slice::from_raw_parts(mp.add(48), 24);
    let raw_slice = std::slice::from_raw_parts(rp, rlen);
    let pal_slice = std::slice::from_raw_parts(pp as *const i16, plen);
    let mut io = [0u16; OUT_LEN];
    let n = carve(ctx as i64, cx as i32, cz as i32, seed as i64, bits, raw_slice, raw_off, pal_slice, pal_off, &mut io);
    ((*tbl).v1_1.ReleaseIntArrayElements)(env, meta, mp, 0);
    ((*tbl).v1_1.ReleaseLongArrayElements)(env, raw, rp, 0);
    ((*tbl).v1_1.ReleaseShortArrayElements)(env, pal, pp, 0);
    n
}

/// carveOps(long ctx, long[] ops) -> int — copies the stashed carve ops.
#[no_mangle]
pub unsafe extern "system" fn Java_com_planeta_crussty_ChunkGenNative_carveOps(
    env: *mut jni_sys::JNIEnv,
    _class: jni_sys::jclass,
    ctx: jni_sys::jlong,
    ops: jni_sys::jlongArray,
) -> jni_sys::jint {
    let tbl = *env;
    let len = ((*tbl).v1_1.GetArrayLength)(env, ops as jni_sys::jarray) as usize;
    let map = SESSIONS.lock().unwrap();
    let Some(n) = map.as_ref().and_then(|m| m.get(&(ctx as i64))).and_then(|c| c.v263.as_ref()) else {
        return 0;
    };
    let count = n.ops.len().min(len);
    let ptr = ((*tbl).v1_1.GetLongArrayElements)(env, ops, std::ptr::null_mut());
    if ptr.is_null() {
        return 0;
    }
    std::ptr::copy_nonoverlapping(n.ops.as_ptr() as *const u64, ptr as *mut u64, count);
    ((*tbl).v1_1.ReleaseLongArrayElements)(env, ops, ptr, 0);
    count as i32
}

/// carveMarks(long ctx, long[] marks) -> int — copies the stashed carve marks.
#[no_mangle]
pub unsafe extern "system" fn Java_com_planeta_crussty_ChunkGenNative_carveMarks(
    env: *mut jni_sys::JNIEnv,
    _class: jni_sys::jclass,
    ctx: jni_sys::jlong,
    marks: jni_sys::jlongArray,
) -> jni_sys::jint {
    let tbl = *env;
    let len = ((*tbl).v1_1.GetArrayLength)(env, marks as jni_sys::jarray) as usize;
    let map = SESSIONS.lock().unwrap();
    let Some(n) = map.as_ref().and_then(|m| m.get(&(ctx as i64))).and_then(|c| c.v263.as_ref()) else {
        return 0;
    };
    let count = n.marks.len().min(len);
    let ptr = ((*tbl).v1_1.GetLongArrayElements)(env, marks, std::ptr::null_mut());
    if ptr.is_null() {
        return 0;
    }
    std::ptr::copy_nonoverlapping(n.marks.as_ptr() as *const u64, ptr as *mut u64, count);
    ((*tbl).v1_1.ReleaseLongArrayElements)(env, marks, ptr, 0);
    count as i32
}
