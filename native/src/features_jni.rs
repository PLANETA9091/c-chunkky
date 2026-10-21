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
//!       out[6*k ..+6] = x, y(abs), z, stateIdx, flags, aux
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
//!   String stateFull(int x, int y, int z)      — full "block[props]" read
//!   boolean isSolidRender(int x, int y, int z) — PlaceOnGround decorator
//!
//! NOTE: the bridge methods must NOT call back into ChunkGenNative natives —
//! SESSIONS is held for the duration of decoRunFeature (std Mutex is not
//! reentrant; see surface_assign_ids deadlock history).

use crate::features_deco::{DecoLevel, DecoLevelExt, HeightmapTy};
use crate::features_master::{chunk_plan, decoration_seed, run_feature_master};
use crate::features_placement::BlockPos;
use jni_sys::{jint, jintArray, jlong, jobject, jobjectArray};

// ---------------------------------------------------------------------------
// JVM upcall bridge (DecoLevel impl over the real WorldGenRegion)
// ---------------------------------------------------------------------------

struct BridgeMids {
    is_air: jni_sys::jmethodID,
    tag_is: jni_sys::jmethodID,
    state_name: jni_sys::jmethodID,
    name_is_solid: jni_sys::jmethodID,
    height: jni_sys::jmethodID,
    state_is: jni_sys::jmethodID,
    fluid_is: jni_sys::jmethodID,
    is_replaceable: jni_sys::jmethodID,
    is_solid: jni_sys::jmethodID,
    has_sturdy_face: jni_sys::jmethodID,
    would_survive: jni_sys::jmethodID,
    biome_has_feature: jni_sys::jmethodID,
    outside_build_height: jni_sys::jmethodID,
    state_full: jni_sys::jmethodID,
    is_solid_render: jni_sys::jmethodID,
    deco_snap_section: jni_sys::jmethodID,
    deco_heightmaps: jni_sys::jmethodID,
}

pub struct JniLevel<'a> {
    env: *mut jni_sys::JNIEnv,
    obj: jobject,
    mids: BridgeMids,
    /// uid -> placed name (BiomeFilter upcalls carry names)
    uid_names: &'a [String],
    min_y: i32,
    max_y: i32,
    trace: std::cell::Cell<u32>,
    /// CRUS_DECO_NOUPCALL=1: dummy every upcall (height->min_y, bools->false)
    /// — measures the native-only floor of decoRunFeature. Output garbage,
    /// timings honest.
    no_upcall: bool,
}

impl<'a> JniLevel<'a> {
    fn trace_tick(&self) -> bool {
        let t = self.trace.get();
        self.trace.set(t + 1);
        t < 5
    }
    #[inline]
    fn noup(&self) -> bool {
        self.no_upcall
    }
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
                is_air: mid("isAir", "(III)Z"),
                tag_is: mid("tagIs", "(IIILjava/lang/String;)Z"),
                state_name: mid("stateName", "(III)Ljava/lang/String;"),
                name_is_solid: mid("nameIsSolid", "(Ljava/lang/String;)Z"),
                height: mid("height", "(III)I"),
                state_is: mid("stateIs", "(III[Ljava/lang/String;)Z"),
                fluid_is: mid("fluidIs", "(III[Ljava/lang/String;)Z"),
                is_replaceable: mid("isReplaceable", "(III)Z"),
                is_solid: mid("isSolid", "(III)Z"),
                has_sturdy_face: mid("hasSturdyFace", "(IIII)Z"),
                would_survive: mid("wouldSurvive", "(Ljava/lang/String;III)Z"),
                biome_has_feature: mid("biomeHasFeature", "(IIILjava/lang/String;)Z"),
                outside_build_height: mid("outsideBuildHeight", "(I)Z"),
                state_full: mid("stateFull", "(III)Ljava/lang/String;"),
                is_solid_render: mid("isSolidRender", "(III)Z"),
                deco_snap_section: mid("decoSnapSection", "(III)[B"),
                deco_heightmaps: mid("decoHeightmaps", "(II)[I"),
            };
            let ok = [
                mids.is_air,
                mids.tag_is,
                mids.state_name,
                mids.name_is_solid,
                mids.height,
                mids.state_is,
                mids.fluid_is,
                mids.is_replaceable,
                mids.is_solid,
                mids.has_sturdy_face,
                mids.would_survive,
                mids.biome_has_feature,
                mids.outside_build_height,
                mids.state_full,
                mids.is_solid_render,
                mids.deco_snap_section,
                mids.deco_heightmaps,
            ]
            .iter()
            .all(|m| !m.is_null());
            if !ok {
                return None;
            }
            Some(JniLevel {
                env,
                obj,
                mids,
                uid_names,
                min_y,
                max_y,
                trace: std::cell::Cell::new(0),
                no_upcall: std::env::var("CRUS_DECO_NOUPCALL").ok().as_deref() == Some("1"),
            })
        }
    }

    #[inline]
    fn z_call(&self, mid: jni_sys::jmethodID, args: &[jni_sys::jvalue]) -> bool {
        if self.noup() {
            return false;
        }
        let tbl = unsafe { *self.env };
        unsafe {
            // A-variant takes a jvalue* — the plain Call*Method is C-varargs
            // and a Rust slice would be read as a single jvalue (JVM crash).
            let r = ((*tbl).v1_1.CallBooleanMethodA)(self.env, self.obj, mid, args.as_ptr());
            if ((*tbl).v1_2.ExceptionCheck)(self.env) {
                ((*tbl).v1_1.ExceptionClear)(self.env);
                return false;
            }
            r
        }
    }

    /// One section snapshot from the bridge (DecoDriver.decoSnapSection).
    /// Returns the raw little-endian payload bytes.
    fn snap_section(&self, cx: i32, cz: i32, sec_y: i32) -> Option<Vec<u8>> {
        if self.noup() {
            return None;
        }
        let tbl = unsafe { *self.env };
        unsafe {
            let ivals = [
                Self::val_i(cx),
                Self::val_i(cz),
                Self::val_i(sec_y),
            ];
            let r = ((*tbl).v1_1.CallObjectMethodA)(
                self.env,
                self.obj,
                self.mids.deco_snap_section,
                ivals.as_ptr(),
            ) as jni_sys::jbyteArray;
            if ((*tbl).v1_2.ExceptionCheck)(self.env) {
                ((*tbl).v1_1.ExceptionClear)(self.env);
                return None;
            }
            if r.is_null() {
                return None;
            }
            let len = ((*tbl).v1_1.GetArrayLength)(self.env, r as jni_sys::jarray) as usize;
            if len == 0 {
                ((*tbl).v1_1.DeleteLocalRef)(self.env, r as jni_sys::jobject);
                return None;
            }
            let p = ((*tbl).v1_1.GetByteArrayElements)(self.env, r, std::ptr::null_mut());
            if p.is_null() {
                ((*tbl).v1_1.DeleteLocalRef)(self.env, r as jni_sys::jobject);
                return None;
            }
            let out = std::slice::from_raw_parts(p as *const u8, len).to_vec();
            ((*tbl).v1_2.ReleaseByteArrayElements)(
                self.env,
                r,
                p,
                jni_sys::JNI_ABORT,
            );
            ((*tbl).v1_1.DeleteLocalRef)(self.env, r as jni_sys::jobject);
            Some(out)
        }
    }

    /// WG heightmap handshake: 512 i32 (WS_WG 256 + OF_WG 256, +1
    /// semantics) per region chunk — replaces the per-column height upcalls.
    fn heightmaps(&self, cx: i32, cz: i32) -> Option<Vec<i32>> {
        if self.noup() {
            return None;
        }
        let tbl = unsafe { *self.env };
        unsafe {
            let ivals = [Self::val_i(cx), Self::val_i(cz)];
            let r = ((*tbl).v1_1.CallObjectMethodA)(
                self.env,
                self.obj,
                self.mids.deco_heightmaps,
                ivals.as_ptr(),
            ) as jni_sys::jintArray;
            if ((*tbl).v1_2.ExceptionCheck)(self.env) {
                ((*tbl).v1_1.ExceptionClear)(self.env);
                return None;
            }
            if r.is_null() {
                return None;
            }
            let len = ((*tbl).v1_1.GetArrayLength)(self.env, r as jni_sys::jarray) as usize;
            if len < 512 {
                ((*tbl).v1_1.DeleteLocalRef)(self.env, r as jni_sys::jobject);
                return None;
            }
            let p = ((*tbl).v1_1.GetIntArrayElements)(self.env, r, std::ptr::null_mut());
            if p.is_null() {
                ((*tbl).v1_1.DeleteLocalRef)(self.env, r as jni_sys::jobject);
                return None;
            }
            let out = std::slice::from_raw_parts(p, 512).to_vec();
            ((*tbl).v1_2.ReleaseIntArrayElements)(
                self.env,
                r,
                p,
                jni_sys::JNI_ABORT,
            );
            ((*tbl).v1_1.DeleteLocalRef)(self.env, r as jni_sys::jobject);
            Some(out)
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
        if self.noup() {
            return self.min_y;
        }
        let tbl = unsafe { *self.env };
        unsafe {
            let ivals = [
                Self::val_i(ty as i32),
                Self::val_i(x),
                Self::val_i(z),
            ];
            let tr = self.trace_tick();
            let r = ((*tbl).v1_1.CallIntMethodA)(
                self.env,
                self.obj,
                self.mids.height,
                ivals.as_ptr(),
            );
            let exc = ((*tbl).v1_2.ExceptionCheck)(self.env);
            if tr {
                crate::ffi::log_warn(&format!(
                    "[jni-dbg] height ty={} x={x} z={z} -> {r} exc={exc}", ty as i32
                ));
            }
            if exc {
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
    fn is_air(&self, pos: BlockPos) -> bool {
        self.z_call(
            self.mids.is_air,
            &[Self::val_i(pos.x), Self::val_i(pos.y), Self::val_i(pos.z)],
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
    /// full state string for tree reads (props included). NOTE: the
    /// OverlayLevel wrapper serves its own writes first — this is the
    /// WORLD-side read only.
    fn state_full(&self, pos: BlockPos) -> String {
        let tbl = unsafe { *self.env };
        unsafe {
            let ivals = [
                Self::val_i(pos.x),
                Self::val_i(pos.y),
                Self::val_i(pos.z),
            ];
            let r = ((*tbl).v1_1.CallObjectMethodA)(
                self.env,
                self.obj,
                self.mids.state_full,
                ivals.as_ptr(),
            );
            if r.is_null() {
                return String::new();
            }
            let cstr = ((*tbl).v1_1.GetStringUTFChars)(self.env, r as jni_sys::jstring, std::ptr::null_mut());
            let s = if cstr.is_null() {
                String::new()
            } else {
                std::ffi::CStr::from_ptr(cstr).to_string_lossy().into_owned()
            };
            if !cstr.is_null() {
                ((*tbl).v1_1.ReleaseStringUTFChars)(self.env, r as jni_sys::jstring, cstr);
            }
            s
        }
    }

    fn is_solid_render(&self, pos: BlockPos) -> bool {
        let ivals = [
            Self::val_i(pos.x),
            Self::val_i(pos.y),
            Self::val_i(pos.z),
        ];
        self.z_call(self.mids.is_solid_render, &ivals)
    }

    /// Debug upcall: block registry key at pos (trace-only).
    fn state_name(&self, pos: BlockPos) -> String {
        let tbl = unsafe { *self.env };
        unsafe {
            let ivals = [
                Self::val_i(pos.x),
                Self::val_i(pos.y),
                Self::val_i(pos.z),
            ];
            let r = ((*tbl).v1_1.CallObjectMethodA)(
                self.env,
                self.obj,
                self.mids.state_name,
                ivals.as_ptr(),
            );
            if r.is_null() {
                return String::from("?");
            }
            let cstr = ((*tbl).v1_1.GetStringUTFChars)(self.env, r as jni_sys::jstring, std::ptr::null_mut());
            let s = if cstr.is_null() {
                String::from("?")
            } else {
                std::ffi::CStr::from_ptr(cstr).to_string_lossy().into_owned()
            };
            if !cstr.is_null() {
                ((*tbl).v1_1.ReleaseStringUTFChars)(self.env, r as jni_sys::jstring, cstr);
            }
            s
        }
    }
    fn tag_is(&self, pos: BlockPos, tag: &str) -> bool {
        let Some(js) = self.new_jstring(tag) else { return false };
        self.z_call(
            self.mids.tag_is,
            &[
                Self::val_i(pos.x),
                Self::val_i(pos.y),
                Self::val_i(pos.z),
                Self::val_l(js),
            ],
        )
    }
    fn name_is_solid(&self, name: &str) -> bool {
        // solidity of a written state's block name (overlay reads); cached per
        // name — the JVM parses the state string and asks BlockState.isSolid
        thread_local! {
            static CACHE: std::cell::RefCell<std::collections::HashMap<String, bool>> =
                std::cell::RefCell::new(std::collections::HashMap::new());
        }
        if let Some(v) = CACHE.with(|c| c.borrow().get(name).copied()) {
            return v;
        }
        let Some(js) = self.new_jstring(name) else { return false };
        let v = self.z_call(
            self.mids.name_is_solid,
            &[Self::val_l(js)],
        );
        CACHE.with(|c| c.borrow_mut().insert(name.to_string(), v));
        v
    }
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

/// decoBiomeFeaturesFull(ctx, String[] biomeNames, String[] joinedRefs) —
/// the FULL pack biome -> placed-refs union map, matching the Java
/// BiomeFilter source exactly (including biomes OUTSIDE possibleBiomes —
/// border positions sample neighbor biomes whose JSONs still list the
/// feature). joinedRefs[i] = "ref1\u{1}ref2..." (SOH-separated) or empty.
#[no_mangle]
pub unsafe extern "system" fn Java_com_planeta_crussty_ChunkGenNative_decoBiomeFeaturesFull(
    env: *mut jni_sys::JNIEnv,
    _class: jni_sys::jclass,
    ctx: jlong,
    names: jobjectArray,
    joined: jobjectArray,
) {
    let tbl = *env;
    let read_arr = |arr: jobjectArray| -> Vec<String> {
        let mut out = Vec::new();
        unsafe {
            let len = ((*tbl).v1_1.GetArrayLength)(env, arr as jni_sys::jarray) as usize;
            for i in 0..len {
                let jo = ((*tbl).v1_1.GetObjectArrayElement)(env, arr, i as jni_sys::jsize);
                if jo.is_null() {
                    out.push(String::new());
                    continue;
                }
                let chars = ((*tbl).v1_1.GetStringUTFChars)(env, jo as jni_sys::jstring, std::ptr::null_mut());
                let s = if chars.is_null() {
                    String::new()
                } else {
                    std::ffi::CStr::from_ptr(chars).to_string_lossy().into_owned()
                };
                if !chars.is_null() {
                    ((*tbl).v1_1.ReleaseStringUTFChars)(env, jo as jni_sys::jstring, chars);
                }
                ((*tbl).v1_1.DeleteLocalRef)(env, jo);
                out.push(s);
            }
        }
        out
    };
    let names_v = read_arr(names);
    let refs_v = read_arr(joined);
    // GLOBAL (the chunk deco acquires any ctx from the pool — per-ctx storage
    // would leave the other ctxs with an empty map)
    let mut map = crate::features_snap::BIOME_FULL.write().unwrap();
    map.clear();
    for (i, name) in names_v.iter().enumerate() {
        if name.is_empty() {
            continue;
        }
        let refs: Vec<String> = refs_v
            .get(i)
            .map(|j| j.split('\u{1}').filter(|s| !s.is_empty()).map(|s| s.to_string()).collect())
            .unwrap_or_default();
        map.insert(name.clone(), refs);
    }
}

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
    // outer component type must be String[] (ArrayStoreException otherwise)
    let str_class = ((*tbl).v1_1.FindClass)(env, c"java/lang/String".as_ptr());
    if str_class.is_null() {
        return std::ptr::null_mut();
    }
    let arr_class = ((*tbl).v1_1.FindClass)(env, c"[Ljava/lang/String;".as_ptr());
    if arr_class.is_null() {
        return std::ptr::null_mut();
    }
    let outer = ((*tbl).v1_1.NewObjectArray)(env, d.plan.steps.len() as jni_sys::jsize, arr_class, std::ptr::null_mut());
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
    let ptr = ((*tbl).v1_2.GetPrimitiveArrayCritical)(env, out as jni_sys::jarray, std::ptr::null_mut());
    if ptr.is_null() {
        return -1;
    }
    let slice = std::slice::from_raw_parts_mut(ptr as *mut i32, olen);
    for (k, (step, idx)) in runs.iter().enumerate() {
        slice[k * 2] = *step as i32;
        slice[k * 2 + 1] = *idx as i32;
    }
    ((*tbl).v1_2.ReleasePrimitiveArrayCritical)(env, out as jni_sys::jarray, ptr, 0);
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
    // Hybrid delegate gate: a placed feature whose configured graph contains
    // ANY unsupported node (tree, vegetation_patch, ...) must run ENTIRELY on
    // the vanilla lane (Java PlacedFeature.placeWithBiomeCheck) — a partial
    // native run would desync that feature's own RNG stream. -2 = delegate.
    // Early exit BEFORE the JniLevel/bridge setup (cheap, no upcalls).
    if !crate::features_place::placed_fully_supported(&d.state.reg, &name, 0) {
        return -2;
    }
    // NOTE: env may be attached to a worker thread; direct upcalls only.
    let Some(level) = JniLevel::new(env, bridge, &d.uid_names, origin_y, max_y) else {
        return -1;
    };
    // Chunk-scoped level: overlay (ALL of this chunk's feature writes) ->
    // lazy 3x3 section snapshot -> JNI bridge. The per-feature OverlayLevel
    // is gone: later features must see earlier features' writes (vanilla
    // setBlock is live) AND the point queries must not upcall per query —
    // the snapshot serves them natively (flags computed in Java).
    let ov: &std::cell::RefCell<crate::features_snap::OvMap> = &d.chunk_ov;
    let snap: &std::cell::RefCell<crate::features_snap::SnapRegion> = &d.chunk_snap;
    let hms: &std::cell::RefCell<std::collections::HashMap<(i32, i32), Option<std::sync::Arc<Vec<i32>>>>> =
        &d.chunk_hms;
    let zoom_seed = d.chunk_zoom_seed.get();
    let level_ref = &level;
    let loader = |cx: i32, cz: i32, sec_y: i32| -> Option<crate::features_snap::SnapSection> {
        let bytes = level_ref.snap_section(cx, cz, sec_y)?;
        crate::features_snap::parse_section(&bytes)
    };
    let hm_loader = |cx: i32, cz: i32| -> Option<std::sync::Arc<Vec<i32>>> {
        if let Some(cached) = hms.borrow().get(&(cx, cz)) {
            return cached.clone();
        }
        // the WG handshake must always succeed: 512 ints, O(1) Java
        let loaded = level_ref.heightmaps(cx, cz).map(std::sync::Arc::new);
        if loaded.is_none() {
            static LOGGED: std::sync::atomic::AtomicU32 =
                std::sync::atomic::AtomicU32::new(0);
            let n = LOGGED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if n < 3 {
                crate::ffi::log_warn(&format!(
                    "[deco-hm] heightmaps handshake failed cx={cx} cz={cz} (fallback to per-column JNI)"
                ));
            }
        }
        hms.borrow_mut().insert((cx, cz), loaded.clone());
        loaded
    };
    let chunk_level = crate::features_snap::ChunkLevel::new(
        level_ref,
        ov,
        snap,
        &loader,
        &hm_loader,
        &d.uid_names,
        zoom_seed,
    );
    let olen = ((*tbl).v1_1.GetArrayLength)(env, out as jni_sys::jarray) as usize;
    if olen < 5 {
        return -1;
    }
    // GetPrimitiveArrayCritical: PIN the ops buffer (no copy). The old
    // GetIntArrayElements copied the 1.5MB buffer TO and BACK per feature —
    // 21818 features x 3MB traffic = ~65GB memcpy per 332-chunk pregen —
    // the dominant cost of the native deco path (17ms of the 21.6ms run
    // phase). Writes are short-lived (pin released at feature end).
    let ptr = ((*tbl).v1_2.GetPrimitiveArrayCritical)(env, out as jni_sys::jarray, std::ptr::null_mut());
    if ptr.is_null() {
        return -1;
    }
    let slice = std::slice::from_raw_parts_mut(ptr as *mut i32, olen);
    let n = run_feature_master(
        &d.state,
        &chunk_level,
        deco_seed as i64,
        if paper_seed == -1 { None } else { Some(paper_seed as i64) },
        BlockPos::new(origin_x as i32, origin_y as i32, origin_z as i32),
        &name,
        step as usize,
        gid as usize,
        slice,
    );
    ((*tbl).v1_2.ReleasePrimitiveArrayCritical)(env, out as jni_sys::jarray, ptr, 0);
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

/// decoChunkBegin(ctx, cx, cz, minSectionY) — reset the chunk-scoped overlay
/// + snapshot grid before the feature loop of chunk (cx, cz).
#[no_mangle]
pub unsafe extern "system" fn Java_com_planeta_crussty_ChunkGenNative_decoChunkBegin(
    _env: *mut jni_sys::JNIEnv,
    _class: jni_sys::jclass,
    ctx: jlong,
    cx: jint,
    cz: jint,
    min_sec_y: jint,
    zoom_seed: jlong,
) {
    let guard = SESSIONS.lock().unwrap();
    if let Some(d) = guard
        .as_ref()
        .and_then(|m| m.get(&(ctx as i64)))
        .and_then(|c| c.deco.as_ref())
    {
        d.chunk_ov.borrow_mut().clear();
        d.chunk_snap.borrow_mut().reset(cx, cz, min_sec_y);
        d.chunk_hms.borrow_mut().clear();
        d.chunk_zoom_seed.set(zoom_seed);
    }
}

/// decoOpsDropped(ctx, coords, count) — ops the Java apply loop dropped
/// (postproc marks / unparseable states / out-of-range sections): remove
/// their pending-overlay entries so the chunk overlay stays exact (a phantom
/// entry would serve stale answers for the rest of the chunk — the old
/// per-feature overlay discarded them implicitly at feature end).
#[no_mangle]
pub unsafe extern "system" fn Java_com_planeta_crussty_ChunkGenNative_decoOpsDropped(
    env: *mut jni_sys::JNIEnv,
    _class: jni_sys::jclass,
    ctx: jlong,
    coords: jintArray,
    count: jint,
) {
    if coords.is_null() || count <= 0 {
        return;
    }
    let tbl = *env;
    unsafe {
        let len = ((*tbl).v1_1.GetArrayLength)(env, coords as jni_sys::jarray) as usize;
        if len < (count as usize) * 3 {
            return;
        }
        let p = ((*tbl).v1_2.GetPrimitiveArrayCritical)(env, coords as jni_sys::jarray, std::ptr::null_mut());
        if p.is_null() {
            return;
        }
        let slice = std::slice::from_raw_parts(p as *const i32, len);
        let guard = SESSIONS.lock().unwrap();
        if let Some(d) = guard
            .as_ref()
            .and_then(|m| m.get(&(ctx as i64)))
            .and_then(|c| c.deco.as_ref())
        {
            let mut ov = d.chunk_ov.borrow_mut();
            for k in 0..count as usize {
                ov.remove(&(slice[k * 3], slice[k * 3 + 1], slice[k * 3 + 2]));
            }
        }
        ((*tbl).v1_2.ReleasePrimitiveArrayCritical)(env, coords as jni_sys::jarray, p, jni_sys::JNI_ABORT);
    }
}

/// decoDelegateMark(ctx) — a vanilla delegate feature ran (its writes went
/// through the region directly, invisible to BOTH the overlay and the
/// snapshot). Clear the chunk overlay (its entries may be stale where the
/// delegate overwrote them; the flushed native writes live in the region and
/// come back via the reloaded snapshot) AND bump the snapshot epoch (stale
/// sections reload lazily on the next query).
#[no_mangle]
pub unsafe extern "system" fn Java_com_planeta_crussty_ChunkGenNative_decoDelegateMark(
    _env: *mut jni_sys::JNIEnv,
    _class: jni_sys::jclass,
    ctx: jlong,
) {
    let guard = SESSIONS.lock().unwrap();
    if let Some(d) = guard
        .as_ref()
        .and_then(|m| m.get(&(ctx as i64)))
        .and_then(|c| c.deco.as_ref())
    {
        d.chunk_ov.borrow_mut().clear();
        d.chunk_snap.borrow().bump_epoch();
    }
}
