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
struct Ctx {
    gen: ChunkGen<'static>,
    /// Native multi_noise biome source (pack dimension override). None =
    /// keep the vanilla Java biome path (embedded-vanilla ctx has no
    /// data-driven dimension json).
    biomes: Option<BiomeState>,
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

static SESSIONS: Mutex<Option<HashMap<i64, Ctx>>> = Mutex::new(None);
static NEXT_ID: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(1);

/// Create a generator context for (seed, minY, height, seaLevel). Returns the
/// ctx id (non-zero) or 0 on failure.
pub fn ctx_new(seed: i64, _min_y: i32, _height: i32, sea_level: i32) -> i64 {
    let id = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut env: Box<DEnv> = Box::new(DEnv::new(seed));
    // The plugin resolves the datapack-aware sea level (NoiseGeneratorSettings
    // from the server registry); override the bundled-vanilla default.
    env.wg.sea_level = sea_level as i64;
    let env: &'static DEnv = Box::leak(env);
    let gen = ChunkGen::new(env);
    let mut map = SESSIONS.lock().unwrap();
    map.get_or_insert_with(HashMap::new).insert(id, Ctx { gen, biomes: None });
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
        let mut env: Box<DEnv> = Box::new(DEnv::new_assets(seed, &assets));
        env.wg.sea_level = sea_level as i64;
        let env: &'static DEnv = Box::leak(env);
        let biomes = build_biome_state(&assets, env);
        (ChunkGen::new(env), biomes)
    });
    let (gen, biomes) = match result {
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
    map.get_or_insert_with(HashMap::new).insert(id, Ctx { gen, biomes });
    Ok(id)
}

pub fn ctx_free(id: i64) {
    if let Some(map) = SESSIONS.lock().unwrap().as_mut() {
        map.remove(&id);
    }
}

pub fn fill(id: i64, cx: i32, cz: i32, out: &mut [u16; OUT_LEN]) {
    let mut map_guard = SESSIONS.lock().unwrap();
    let map = map_guard.as_mut().expect("sessions map");
    let ctx = map.get_mut(&id).expect("ctx id");
    ctx.gen.fill(cx, cz, out);
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
                        c.gen.set_beard(&data);
                    }
                }
            }
        }
    }
    fill_jni(env, ctx, cx, cz, out, marks)
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
        let m = c.gen.marks();
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
    let Some(bs) = bs_guard.as_ref().and_then(|m| m.get(&(ctx as i64)))
        .and_then(|c| c.biomes.as_ref()) else {
        return std::ptr::null_mut();
    };
    let str_class = ((*tbl).v1_1.FindClass)(env, c"java/lang/String".as_ptr());
    if str_class.is_null() {
        return std::ptr::null_mut();
    }
    let names = &bs.src.biome_names;
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
    let Some(state) = state_guard.as_ref().and_then(|m| m.get(&(ctx as i64)))
        .and_then(|c| c.biomes.as_ref()) else {
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
    ((*tbl).v1_1.ReleaseIntArrayElements)(env, out, ptr, 0);
    n as jni_sys::jint
}
