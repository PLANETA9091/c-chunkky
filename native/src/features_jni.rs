//! Features JNI (stage F1): deco engine exports + JVM upcall bridge for the
//! 3x3 WorldGenRegion queries (states/heights/biomes/canSurvive).
//!
//! Java protocol (com.planeta.crussty.ChunkGenNative):
//!   decoStates(ctx) -> String[]
//!   decoPlan(ctx) -> String[][]      (steps x names, FeatureSorter order)
//!   decoChunkPlan(ctx, String[] regionBiomes, int[] out) -> int runs
//!       out[2*k] = step, out[2*k+1] = index-in-step (step-major, index-ascending)
//!   decoRunFeature(ctx, long decoSeed, long paperSeed, int originX, originY,
//!       originZ, int step, int gid, DecoLevelBridge bridge, int[] out) -> int ops
//!       out[5*k ..+5] = x, y(abs), z, stateIdx, flags
//!   decoDecoSeed(long levelSeed, int originX, int originZ) -> long
//!
//! DecoLevelBridge upcalls (resolved per decoRunFeature call):
//!   int height(int ty, int x, int z)          — vanilla getHeight semantics
//!   boolean stateIs(int x, int y, int z, String[] names)
//!   boolean fluidIs(int x, int y, int z, String[] names)
//!   boolean isReplaceable(int x, int y, int z)
//!   boolean isSolid(int x, int y, int z)
//!   boolean hasSturdyFace(int x, int y, int z, int dir)
//!   boolean wouldSurvive(String state, int x, int y, int z)
//!   boolean biomeHasFeature(int x, int y, int z, String placed)
//!   boolean outsideBuildHeight(int y)
//!
//! NOTE: the bridge methods must NOT call back into ChunkGenNative natives —
//! SESSIONS is held for the duration of decoRunFeature (std Mutex is not
//! reentrant; see surface_assign_ids deadlock history).

use crate::features_deco::{DecoLevel, DecoLevelExt, HeightmapTy};
use crate::features_master::{chunk_plan, decoration_seed, run_feature_master};
use crate::features_placement::BlockPos;
use jni_sys::{jint, jlong, jobject, jobjectArray};

// ---------------------------------------------------------------------------
// JVM upcall bridge (DecoLevel impl over the real WorldGenRegion)
// ---------------------------------------------------------------------------

struct BridgeMids {
    height: jni_sys::jmethodID,
    state_is: jni_sys::jmethodID,
    fluid_is: jni_sys::jmethodID,
    is_replaceable: jni_sys::jmethodID,
    is_solid: jni_sys::jmethodID,
    has_sturdy_face: jni_sys::jmethodID,
    would_survive: jni_sys::jmethodID,
    biome_has_feature: jni_sys::jmethodID,
    outside_build_height: jni_sys::jmethodID,
}

pub struct JniLevel<'a> {
    env: *mut jni_sys::JNIEnv,
    obj: jobject,
    mids: BridgeMids,
    /// uid -> placed name (BiomeFilter upcalls carry names)
    uid_names: &'a [String],
    min_y: i32,
    max_y: i32,
}

impl<'a> JniLevel<'a> {
    pub unsafe fn new(
        env: *mut jni_sys::JNIEnv,
        obj: jobject,
        uid_names: &'a [String],
        min_y: i32,
        max_y: i32,
    ) -> Option<Self> {
        let tbl = *env;
        {
            let class = ((*tbl).v1_1.GetObjectClass)(env, obj);
            if class.is_null() {
                return None;
            }
            let mid = |name: &str, sig: &str| -> jni_sys::jmethodID {
                let n = std::ffi::CString::new(name).unwrap();
                let s = std::ffi::CString::new(sig).unwrap();
                ((*tbl).v1_1.GetMethodID)(env, class, n.as_ptr(), s.as_ptr())
            };
            let mids = BridgeMids {
                height: mid("height", "(III)I"),
                state_is: mid("stateIs", "(III[Ljava/lang/String;)Z"),
                fluid_is: mid("fluidIs", "(III[Ljava/lang/String;)Z"),
                is_replaceable: mid("isReplaceable", "(III)Z"),
                is_solid: mid("isSolid", "(III)Z"),
                has_sturdy_face: mid("hasSturdyFace", "(IIII)Z"),
                would_survive: mid("wouldSurvive", "(Ljava/lang/String;III)Z"),
                biome_has_feature: mid("biomeHasFeature", "(IIILjava/lang/String;)Z"),
                outside_build_height: mid("outsideBuildHeight", "(I)Z"),
            };
            let ok = [
                mids.height,
                mids.state_is,
                mids.fluid_is,
                mids.is_replaceable,
                mids.is_solid,
                mids.has_sturdy_face,
                mids.would_survive,
                mids.biome_has_feature,
                mids.outside_build_height,
            ]
            .iter()
            .all(|m| !m.is_null());
            if !ok {
                return None;
            }
            Some(JniLevel { env, obj, mids, uid_names, min_y, max_y })
        }
    }

    #[inline]
    fn z_call(&self, mid: jni_sys::jmethodID, args: &[jni_sys::jvalue]) -> bool {
        let tbl = unsafe { *self.env };
        unsafe {
            let r = ((*tbl).v1_1.CallBooleanMethod)(self.env, self.obj, mid, args);
            if ((*tbl).v1_2.ExceptionCheck)(self.env) {
                ((*tbl).v1_1.ExceptionClear)(self.env);
                return false;
            }
            r
        }
    }

    fn new_jstring(&self, s: &str) -> Option<jni_sys::jstring> {
        let tbl = unsafe { *self.env };
        let c = std::ffi::CString::new(s).ok()?;
        unsafe {
            let js = ((*tbl).v1_1.NewStringUTF)(self.env, c.as_ptr());
            if js.is_null() || ((*tbl).v1_2.ExceptionCheck)(self.env) {
                return None;
            }
            Some(js)
        }
    }

    fn new_string_array(&self, items: &[String]) -> Option<jobjectArray> {
        let tbl = unsafe { *self.env };
        unsafe {
            let class = ((*tbl).v1_1.FindClass)(self.env, c"java/lang/String".as_ptr());
            if class.is_null() {
                return None;
            }
            let arr = ((*tbl).v1_1.NewObjectArray)(self.env, items.len() as jni_sys::jsize, class, std::ptr::null_mut());
            if arr.is_null() {
                return None;
            }
            for (i, s) in items.iter().enumerate() {
                let Some(js) = self.new_jstring(s) else { return None };
                ((*tbl).v1_1.SetObjectArrayElement)(self.env, arr, i as jni_sys::jsize, js);
            }
            Some(arr)
        }
    }

    #[inline]
    fn val_i(v: i32) -> jni_sys::jvalue {
        jni_sys::jvalue { i: v }
    }

    #[inline]
    fn val_l(v: jobject) -> jni_sys::jvalue {
        jni_sys::jvalue { l: v }
    }
}

impl<'a> DecoLevel for JniLevel<'a> {
    fn min_y(&self) -> i32 {
        self.min_y
    }
    fn max_y(&self) -> i32 {
        self.max_y
    }
    fn height(&self, ty: HeightmapTy, x: i32, z: i32) -> i32 {
        let tbl = unsafe { *self.env };
        unsafe {
            let r = ((*tbl).v1_1.CallIntMethod)(
                self.env,
                self.obj,
                self.mids.height,
                Self::val_i(ty as i32),
                Self::val_i(x),
                Self::val_i(z),
            );
            if ((*tbl).v1_2.ExceptionCheck)(self.env) {
                ((*tbl).v1_1.ExceptionClear)(self.env);
                return self.min_y(); // heights below min disable heightmap moves
            }
            r
        }
    }
    fn state(&self, pos: BlockPos) -> u32 {
        // opaque id: encode the coords so downstream string comparisons go to
        // the bridge anyway; predicates always call state_is/state through
        // JniLevelExt paths that carry the real query.
        let _ = pos;
        0
    }
    fn fluid(&self, pos: BlockPos) -> u32 {
        let _ = pos;
        0
    }
    fn biome_has_feature(&self, pos: BlockPos, feature: u32) -> bool {
        let Some(name) = self.uid_names.get(feature as usize) else { return false };
        let Some(js) = self.new_jstring(name) else { return false };
        self.z_call(
            self.mids.biome_has_feature,
            &[
                Self::val_i(pos.x),
                Self::val_i(pos.y),
                Self::val_i(pos.z),
                Self::val_l(js),
            ],
        )
    }
    fn can_survive(&self, state: &str, pos: BlockPos) -> bool {
        let Some(js) = self.new_jstring(state) else { return false };
        self.z_call(
            self.mids.would_survive,
            &[
                Self::val_l(js),
                Self::val_i(pos.x),
                Self::val_i(pos.y),
                Self::val_i(pos.z),
            ],
        )
    }
    fn is_replaceable(&self, pos: BlockPos) -> bool {
        self.z_call(
            self.mids.is_replaceable,
            &[Self::val_i(pos.x), Self::val_i(pos.y), Self::val_i(pos.z)],
        )
    }
    fn is_solid(&self, pos: BlockPos) -> bool {
        self.z_call(
            self.mids.is_solid,
            &[Self::val_i(pos.x), Self::val_i(pos.y), Self::val_i(pos.z)],
        )
    }
    fn has_sturdy_face(&self, pos: BlockPos, dir: u8) -> bool {
        self.z_call(
            self.mids.has_sturdy_face,
            &[
                Self::val_i(pos.x),
                Self::val_i(pos.y),
                Self::val_i(pos.z),
                Self::val_i(dir as i32),
            ],
        )
    }
    fn is_outside_build_height(&self, y: i32) -> bool {
        self.z_call(self.mids.outside_build_height, &[Self::val_i(y)])
    }
}

impl<'a> DecoLevelExt for JniLevel<'a> {
    fn state_is(&self, pos: BlockPos, ids: &[String]) -> bool {
        let Some(arr) = self.new_string_array(ids) else { return false };
        self.z_call(
            self.mids.state_is,
            &[
                Self::val_i(pos.x),
                Self::val_i(pos.y),
                Self::val_i(pos.z),
                Self::val_l(arr),
            ],
        )
    }
    fn fluid_is(&self, pos: BlockPos, ids: &[String]) -> bool {
        let Some(arr) = self.new_string_array(ids) else { return false };
        self.z_call(
            self.mids.fluid_is,
            &[
                Self::val_i(pos.x),
                Self::val_i(pos.y),
                Self::val_i(pos.z),
                Self::val_l(arr),
            ],
        )
    }
}

// ---------------------------------------------------------------------------
// JNI exports
// ---------------------------------------------------------------------------

use crate::ffi::SESSIONS;

/// decoStates(ctx) -> String[] — distinct block state strings of the decoded
/// feature registry (first-appearance order, ops reference them by index).
/// Null when the ctx has no native deco engine.
#[no_mangle]
pub unsafe extern "system" fn Java_com_planeta_crussty_ChunkGenNative_decoStates(
    env: *mut jni_sys::JNIEnv,
    _class: jni_sys::jclass,
    ctx: jlong,
) -> jobjectArray {
    let tbl = *env;
    let guard = SESSIONS.lock().unwrap();
    let Some(d) = guard.as_ref().and_then(|m| m.get(&(ctx as i64)))
        .and_then(|c| c.deco.as_ref()) else {
        return std::ptr::null_mut();
    };
    let str_class = ((*tbl).v1_1.FindClass)(env, c"java/lang/String".as_ptr());
    if str_class.is_null() {
        return std::ptr::null_mut();
    }
    let arr = ((*tbl).v1_1.NewObjectArray)(env, d.state.states.len() as jni_sys::jsize, str_class, std::ptr::null_mut());
    if arr.is_null() {
        return std::ptr::null_mut();
    }
    for (i, name) in d.state.states.iter().enumerate() {
        let Ok(cstr) = std::ffi::CString::new(name.as_str()) else { continue };
        let js = ((*tbl).v1_1.NewStringUTF)(env, cstr.as_ptr());
        if js.is_null() {
            continue;
        }
        ((*tbl).v1_1.SetObjectArrayElement)(env, arr, i as jni_sys::jsize, js);
    }
    arr
}

/// decoPlan(ctx) -> String[][] — featuresPerStep: steps x placed feature
/// names in FeatureSorter order (index within a step = the vanilla i3 seed
/// index). Null when the ctx has no native deco engine.
#[no_mangle]
pub unsafe extern "system" fn Java_com_planeta_crussty_ChunkGenNative_decoPlan(
    env: *mut jni_sys::JNIEnv,
    _class: jni_sys::jclass,
    ctx: jlong,
) -> jobjectArray {
    let tbl = *env;
    let guard = SESSIONS.lock().unwrap();
    let Some(d) = guard.as_ref().and_then(|m| m.get(&(ctx as i64)))
        .and_then(|c| c.deco.as_ref()) else {
        return std::ptr::null_mut();
    };
    let str_class = ((*tbl).v1_1.FindClass)(env, c"java/lang/String".as_ptr());
    if str_class.is_null() {
        return std::ptr::null_mut();
    }
    let outer = ((*tbl).v1_1.NewObjectArray)(env, d.plan.steps.len() as jni_sys::jsize, str_class, std::ptr::null_mut());
    if outer.is_null() {
        return std::ptr::null_mut();
    }
    for (s, names) in d.plan.steps.iter().enumerate() {
        let inner = ((*tbl).v1_1.NewObjectArray)(env, names.len() as jni_sys::jsize, str_class, std::ptr::null_mut());
        if inner.is_null() {
            return std::ptr::null_mut();
        }
        for (i, n) in names.iter().enumerate() {
            let Ok(cstr) = std::ffi::CString::new(n.as_str()) else { continue };
            let js = ((*tbl).v1_1.NewStringUTF)(env, cstr.as_ptr());
            if js.is_null() {
                continue;
            }
            ((*tbl).v1_1.SetObjectArrayElement)(env, inner, i as jni_sys::jsize, js);
        }
        ((*tbl).v1_1.SetObjectArrayElement)(env, outer, s as jni_sys::jsize, inner);
    }
    outer
}

/// decoChunkPlan(ctx, String[] regionBiomes, int[] out) -> int.
/// Union of the region biomes' features mapped to (step, index) runs, sorted
/// per step (vanilla IntArraySet + sort). out[2k] = step, out[2k+1] = index.
/// Returns the run count, -1 when out is too small.
#[no_mangle]
pub unsafe extern "system" fn Java_com_planeta_crussty_ChunkGenNative_decoChunkPlan(
    env: *mut jni_sys::JNIEnv,
    _class: jni_sys::jclass,
    ctx: jlong,
    region_biomes: jobjectArray,
    out: jni_sys::jintArray,
) -> jint {
    let tbl = *env;
    let guard = SESSIONS.lock().unwrap();
    let Some(d) = guard.as_ref().and_then(|m| m.get(&(ctx as i64)))
        .and_then(|c| c.deco.as_ref()) else {
        return -1;
    };
    let n = ((*tbl).v1_1.GetArrayLength)(env, region_biomes as jni_sys::jarray) as usize;
    let mut biomes: Vec<String> = Vec::with_capacity(n);
    for i in 0..n {
        let js = ((*tbl).v1_1.GetObjectArrayElement)(env, region_biomes, i as jni_sys::jsize);
        if js.is_null() {
            continue;
        }
        let cstr = ((*tbl).v1_1.GetStringUTFChars)(env, js as jni_sys::jstring, std::ptr::null_mut());
        if cstr.is_null() {
            continue;
        }
        biomes.push(std::ffi::CStr::from_ptr(cstr).to_string_lossy().into_owned());
        ((*tbl).v1_1.ReleaseStringUTFChars)(env, js as jni_sys::jstring, cstr);
    }
    let runs = chunk_plan(&d.plan, &d.biome_features, &biomes);
    let olen = ((*tbl).v1_1.GetArrayLength)(env, out as jni_sys::jarray) as usize;
    if olen < runs.len() * 2 {
        return -1;
    }
    let ptr = ((*tbl).v1_1.GetIntArrayElements)(env, out, std::ptr::null_mut());
    if ptr.is_null() {
        return -1;
    }
    let slice = std::slice::from_raw_parts_mut(ptr as *mut i32, olen);
    for (k, (step, idx)) in runs.iter().enumerate() {
        slice[k * 2] = *step as i32;
        slice[k * 2 + 1] = *idx as i32;
    }
    ((*tbl).v1_1.ReleaseIntArrayElements)(env, out, ptr, 0);
    runs.len() as jint
}

/// decoRunFeature(ctx, decoSeed, paperSeed, originX, originY, originZ, maxY,
/// step, gid, bridge, out) -> int ops. One master-loop feature run with the exact
/// vanilla seeding (Paper featureSeeds override when paperSeed != -1);
/// level-dependent queries go through the bridge upcalls. out: 5 i32 per op.
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "system" fn Java_com_planeta_crussty_ChunkGenNative_decoRunFeature(
    env: *mut jni_sys::JNIEnv,
    _class: jni_sys::jclass,
    ctx: jlong,
    deco_seed: jlong,
    paper_seed: jlong,
    origin_x: jint,
    origin_y: jint,
    origin_z: jint,
    max_y: jint,
    step: jint,
    gid: jint,
    bridge: jobject,
    out: jni_sys::jintArray,
) -> jint {
    if bridge.is_null() || out.is_null() {
        return -1;
    }
    let tbl = *env;
    let guard = SESSIONS.lock().unwrap();
    let Some(d) = guard.as_ref().and_then(|m| m.get(&(ctx as i64)))
        .and_then(|c| c.deco.as_ref()) else {
        return -1;
    };
    let Some(name) = d.plan.steps.get(step as usize).and_then(|v| v.get(gid as usize)) else {
        return -1;
    };
    let name = name.clone();
    // NOTE: env may be attached to a worker thread; direct upcalls only.
    let Some(level) = JniLevel::new(env, bridge, &d.uid_names, origin_y, max_y) else {
        return -1;
    };
    let olen = ((*tbl).v1_1.GetArrayLength)(env, out as jni_sys::jarray) as usize;
    if olen < 5 {
        return -1;
    }
    let ptr = ((*tbl).v1_1.GetIntArrayElements)(env, out, std::ptr::null_mut());
    if ptr.is_null() {
        return -1;
    }
    let slice = std::slice::from_raw_parts_mut(ptr as *mut i32, olen);
    let n = run_feature_master(
        &d.state,
        &level,
        deco_seed as i64,
        if paper_seed == -1 { None } else { Some(paper_seed as i64) },
        BlockPos::new(origin_x as i32, origin_y as i32, origin_z as i32),
        &name,
        step as usize,
        gid as usize,
        slice,
    );
    ((*tbl).v1_1.ReleaseIntArrayElements)(env, out, ptr, 0);
    n as jint
}

/// decoDecoSeed(levelSeed, originX, originZ) -> long — the vanilla chunk
/// decoration seed `l` (computed once per chunk; every feature re-seeds from it).
#[no_mangle]
pub unsafe extern "system" fn Java_com_planeta_crussty_ChunkGenNative_decoDecoSeed(
    _env: *mut jni_sys::JNIEnv,
    _class: jni_sys::jclass,
    level_seed: jlong,
    origin_x: jint,
    origin_z: jint,
) -> jlong {
    decoration_seed(level_seed as i64, origin_x as i32, origin_z as i32) as jlong
}
