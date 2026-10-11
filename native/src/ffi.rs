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
}

static SESSIONS: Mutex<Option<HashMap<i64, Ctx>>> = Mutex::new(None);
static NEXT_ID: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(1);

/// Create a generator context for (seed, minY, height). Returns the ctx id
/// (non-zero) or 0 on failure.
pub fn ctx_new(seed: i64, _min_y: i32, _height: i32) -> i64 {
    let id = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let env: &'static DEnv = Box::leak(Box::new(DEnv::new(seed)));
    let gen = ChunkGen::new(env);
    let mut map = SESSIONS.lock().unwrap();
    map.get_or_insert_with(HashMap::new).insert(id, Ctx { gen });
    id
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
    ctx_new(seed, min_y, height)
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
) -> jni_sys::jlong {
    ctx_new(seed as i64, min_y as i32, height as i32)
}

#[no_mangle]
pub unsafe extern "system" fn Java_com_planeta_crussty_ChunkGenNative_freeCtx(
    _env: *mut jni_sys::JNIEnv,
    _class: jni_sys::jclass,
    ctx: jni_sys::jlong,
) {
    ctx_free(ctx as i64)
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
    if out.is_null() {
        return;
    }
    // jni-sys: JNIEnv = *const JNINativeInterface_ (C "JNIEnv"), and the macro
    // jni_to_union rewrites the table into a union of per-version views
    // (v1_1, v1_2, ...). The JVM hands us JNIEnv* — one deref, then the
    // versioned view, passing the original env through (C ((*env)->Fn)(env,...)).
    let tbl = *env;
    let len = ((*tbl).v1_1.GetArrayLength)(env, out as jni_sys::jarray) as usize;
    if len != OUT_LEN {
        // contract mismatch: refuse (plugin passes short[98304])
        return;
    }
    let ptr = ((*tbl).v1_2.GetPrimitiveArrayCritical)(
        env,
        out as jni_sys::jarray,
        std::ptr::null_mut(),
    ) as *mut u16;
    if ptr.is_null() {
        return;
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
}
