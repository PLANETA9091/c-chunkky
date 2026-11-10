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
use jni_sys::{jint, jintArray, jlong, jobject, jobjectArray, jstring};

// ---------------------------------------------------------------------------
// run-phase split profiler (CRUS_RUN_PROF=1): where does the decoRunFeature
// wall go — native compute vs the snapshot loader JNI (sections) vs the other
// bridge upcalls. Accumulated across calls; printed + reset at decoChunkBegin
// every PROF_EVERY chunks.
// ---------------------------------------------------------------------------
static PROF_LOADER_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static PROF_LOADER_N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static PROF_PARSE_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static PROF_NATIVE_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static PROF_RUNS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static PROF_STATE_FULL_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static PROF_STATE_FULL_N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static PROF_SOLIDR_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static PROF_SOLIDR_N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static PROF_CHUNKS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
const PROF_EVERY: u64 = 16;

static PROF_EPOCH: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

#[inline]
fn prof_now() -> u64 {
    PROF_EPOCH.get_or_init(std::time::Instant::now).elapsed().as_nanos() as u64
}

fn prof_chunk_tick() {
    let n = PROF_CHUNKS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    if n % PROF_EVERY != PROF_EVERY - 1 {
        return;
    }
    let l = PROF_LOADER_NS.swap(0, std::sync::atomic::Ordering::Relaxed);
    let ln = PROF_LOADER_N.swap(0, std::sync::atomic::Ordering::Relaxed);
    let p = PROF_PARSE_NS.swap(0, std::sync::atomic::Ordering::Relaxed);
    let nat = PROF_NATIVE_NS.swap(0, std::sync::atomic::Ordering::Relaxed);
    let runs = PROF_RUNS.swap(0, std::sync::atomic::Ordering::Relaxed);
    let sf = PROF_STATE_FULL_NS.swap(0, std::sync::atomic::Ordering::Relaxed);
    let sfn = PROF_STATE_FULL_N.swap(0, std::sync::atomic::Ordering::Relaxed);
    let sr = PROF_SOLIDR_NS.swap(0, std::sync::atomic::Ordering::Relaxed);
    let srn = PROF_SOLIDR_N.swap(0, std::sync::atomic::Ordering::Relaxed);
    crate::ffi::log_warn(&format!(
        "[run-prof] chunks=16 runs={} native={:.3}ms loader={:.3}ms/{}sec (jni={:.3} parse={:.3}) state_full={:.3}ms/{} solid_render={:.3}ms/{}",
        runs,
        nat as f64 / 16.0 / 1e6,
        l as f64 / 16.0 / 1e6,
        ln / 16,
        l.saturating_sub(p) as f64 / 16.0 / 1e6,
        p as f64 / 16.0 / 1e6,
        sf as f64 / 16.0 / 1e6,
        sfn / 16,
        sr as f64 / 16.0 / 1e6,
        srn / 16,
    ));
    // ore internals: where the place_ore work goes (exact call counts,
    // cont-118 — the old 1/8 Instant::now sampler distorted x~6 and was
    // JIT/CPU-dependent; wall attribution stays with [run-prof] native=)
    let ore = crate::features_place::ore_prof::drain();
    crate::ffi::log_warn(&format!(
        "[ore-prof] tag={} nodes={} hm={} air={} (calls/window, exact)",
        ore[0], ore[1], ore[2], ore[3],
    ));
    // top-10 features by accumulated native wall (window only)
    let mut top: Vec<(String, u64, u64)> = PROF_FEAT
        .lock()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), v.0, v.1))
        .collect();
    top.sort_by(|a, b| b.1.cmp(&a.1));
    let mut s = String::from("[run-prof-top]");
    for (i, (name, ns, cnt)) in top.iter().take(10).enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&format!(" {}={:.1}us/{}", trim_name(name), *ns as f64 / 16.0 / 1e3, cnt / 16));
    }
    crate::ffi::log_warn(&s);
    PROF_FEAT.lock().unwrap().clear();
}

fn trim_name(n: &str) -> String {
    let short = n.rsplit(':').next().unwrap_or(n);
    short.chars().take(24).collect()
}

/// feature name -> (ns, calls) for the current prof window
static PROF_FEAT: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<String, (u64, u64)>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

/// CRUS_SNAP_RANGE=1 — the sy-1..sy+1 batch loader A/B (default OFF, see the
/// loader comment for the measured numbers).
static SNAP_RANGE_ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

// ---------------------------------------------------------------------------
// JVM upcall bridge (DecoLevel impl over the real WorldGenRegion)
// ---------------------------------------------------------------------------

// cont-135 (Q1, deco-wrapper audit): JniLevel::new resolved 19 method IDs +
// ~19 CString allocs + one std::env::var on EVERY decoRunFeature (~73/chunk)
// and oreRunConfigured (~250/chunk on the ore leg). The bridge class is
// process-stable (one plugin, one classloader, never unloaded), so the table
// is built ONCE into a global ref and reused. jmethodID/jclass are JVM
// process globals — safe to share across gen threads (holder is Send+Sync;
// JNI functions themselves take the caller's env per call).
struct BridgeCache {
    class: jni_sys::jclass, // global ref (kept alive for the mids)
    mids: BridgeMids,
}
unsafe impl Send for BridgeCache {}
unsafe impl Sync for BridgeCache {}
static BRIDGE_CACHE: std::sync::OnceLock<Option<BridgeCache>> = std::sync::OnceLock::new();
static NOUPCALL_ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

#[derive(Clone, Copy)]
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
    deco_snap_section_range: jni_sys::jmethodID,
    deco_heightmaps: jni_sys::jmethodID,
    /// SOFT resolve (not in the ok[] gate): an older plugin jar without
    /// tagMembers only degrades to the per-cell tagIs bridge (None), the
    /// hard-resolved mids above still gate JniLevel::new.
    tag_members: jni_sys::jmethodID,
    /// SOFT resolve (not in the ok[] gate): freeze_top_layer verdict upcall
    /// (native FreezeTopLayerFeature). An older jar without freezeChecks
    /// degrades the feature to bailing-with-zero-ops at place time (no RNG
    /// consumed — safe), every other feature keeps running.
    freeze_checks: jni_sys::jmethodID,
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
        let no_upcall = *NOUPCALL_ON.get_or_init(|| {
            std::env::var("CRUS_DECO_NOUPCALL").ok().as_deref() == Some("1")
        });
        // Fast path: table built on an earlier call — no JNI resolves, one
        // IsSameObject identity guard (plugin reload swaps classloaders:
        // same method names, DIFFERENT Class objects — stale jmethodIDs
        // would be UB; on mismatch the table is rebuilt and the stale
        // global ref dropped so the old classloader can be collected).
        let bc = 'build: {
            if let Some(Some(bc)) = BRIDGE_CACHE.get() {
                let cur = ((*tbl).v1_1.GetObjectClass)(env, obj);
                let same = !cur.is_null()
                    && ((*tbl).v1_1.IsSameObject)(
                        env,
                        cur as jni_sys::jobject,
                        bc.class as jni_sys::jobject,
                    ) == jni_sys::JNI_TRUE;
                if !cur.is_null() {
                    ((*tbl).v1_1.DeleteLocalRef)(env, cur as jni_sys::jobject);
                }
                if same {
                    break 'build bc;
                }
                ((*tbl).v1_1.DeleteGlobalRef)(env, bc.class as jni_sys::jobject);
            }
            let built: Option<BridgeCache> = unsafe {
                    let class = ((*tbl).v1_1.GetObjectClass)(env, obj);
                    if class.is_null() {
                        None
                    } else {
                        let gref = ((*tbl).v1_1.NewGlobalRef)(env, class);
                        ((*tbl).v1_1.DeleteLocalRef)(env, class);
                        let mid = |name: &str, sig: &str| -> jni_sys::jmethodID {
                            let n = std::ffi::CString::new(name).unwrap();
                            let s = std::ffi::CString::new(sig).unwrap();
                            ((*tbl).v1_1.GetMethodID)(env, gref, n.as_ptr(), s.as_ptr())
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
                            deco_snap_section_range: mid("decoSnapSectionRange", "(IIII)[B"),
                            deco_heightmaps: mid("decoHeightmaps", "(II)[I"),
                            tag_members: mid("tagMembers", "(Ljava/lang/String;)[Ljava/lang/String;"),
                            freeze_checks: mid("freezeChecks", "(II)I"),
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
                            mids.deco_snap_section_range,
                            mids.deco_heightmaps,
                        ]
                        .iter()
                        .all(|m| !m.is_null());
                        if ok && !gref.is_null() {
                            Some(BridgeCache { class: gref, mids })
                        } else {
                            None
                        }
                    }
                };
                // A None (old jar / broken env) is cached too — the same
                // class resolves the same way on every call, so a per-call
                // retry cannot produce a different answer. (Exception: a
                // plugin RELOAD yields a different Class — caught by the
                // IsSameObject guard above, which forces a rebuild.)
                let _ = BRIDGE_CACHE.set(built);
                match BRIDGE_CACHE.get() {
                    Some(Some(bc)) => bc,
                    _ => return None,
                }
        };
        let mids = bc.mids;
        Some(JniLevel {
            env,
            obj,
            mids,
            uid_names,
            min_y,
            max_y,
            trace: std::cell::Cell::new(0),
            no_upcall,
        })
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
    /// cont-135 (Q3, deco-wrapper audit): Java now returns its REUSABLE
    /// serialize buffer (GetArrayLength = capacity >= payload; parse_section
    /// is strictly sequential and tolerates the trailing bytes), so both the
    /// Java-side Arrays.copyOf and the native-side to_vec() copy are gone.
    fn snap_section(&self, cx: i32, cz: i32, sec_y: i32) -> Option<crate::features_snap::SnapSection> {
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
            let parsed = crate::features_snap::parse_section(std::slice::from_raw_parts(
                p as *const u8,
                len,
            ));
            ((*tbl).v1_2.ReleaseByteArrayElements)(
                self.env,
                r,
                p,
                jni_sys::JNI_ABORT,
            );
            ((*tbl).v1_1.DeleteLocalRef)(self.env, r as jni_sys::jobject);
            parsed
        }
    }

    /// Batch variant: n consecutive sections of one chunk column, one JNI
    /// crossing (DecoDriver.decoSnapSectionRange; length-prefixed payloads).
    fn snap_section_range(&self, cx: i32, cz: i32, sec_y: i32, n: i32) -> Option<Vec<u8>> {
        if self.noup() {
            return None;
        }
        let tbl = unsafe { *self.env };
        unsafe {
            let ivals = [
                Self::val_i(cx),
                Self::val_i(cz),
                Self::val_i(sec_y),
                Self::val_i(n),
            ];
            let r = ((*tbl).v1_1.CallObjectMethodA)(
                self.env,
                self.obj,
                self.mids.deco_snap_section_range,
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
        // The Java bridge computes `y < reg.getMinY() || y >= reg.getMaxY()`
        // (DecoDriver.outsideBuildHeight) — pure arithmetic over constants
        // that are ALREADY the min_y/max_y JNI args of this run (runChunk
        // passes oy=reg.getMinY(), maxY=reg.getMaxY()). Every env_scan step
        // calls this (pillar scans: up to 32+32 steps x ~1300 placements per
        // pillar placed-feature) — the JNI crossing was pure waste.
        y < self.min_y || y >= self.max_y
    }
}

impl<'a> DecoLevelExt for JniLevel<'a> {
    /// full state string for tree reads (props included). NOTE: the
    /// OverlayLevel wrapper serves its own writes first — this is the
    /// WORLD-side read only.
    fn state_full(&self, pos: BlockPos) -> String {
        let t0 = prof_now();
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
                PROF_STATE_FULL_NS.fetch_add(prof_now() - t0, std::sync::atomic::Ordering::Relaxed);
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
            PROF_STATE_FULL_N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            PROF_STATE_FULL_NS.fetch_add(prof_now() - t0, std::sync::atomic::Ordering::Relaxed);
            s
        }
    }

    fn is_solid_render(&self, pos: BlockPos) -> bool {
        let t0 = prof_now();
        let ivals = [
            Self::val_i(pos.x),
            Self::val_i(pos.y),
            Self::val_i(pos.z),
        ];
        let r = self.z_call(self.mids.is_solid_render, &ivals);
        PROF_SOLIDR_N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        PROF_SOLIDR_NS.fetch_add(prof_now() - t0, std::sync::atomic::Ordering::Relaxed);
        r
    }

    /// freeze_top_layer per-column verdict (DecoDriver.freezeChecks): the
    /// packed y + shouldFreeze/shouldSnow/snowy-rewrite bitmask, evaluated by
    /// Java with the exact vanilla Biome methods on the live region.
    fn freeze_checks(&self, x: i32, z: i32) -> Option<i32> {
        if self.noup() || self.mids.freeze_checks.is_null() {
            return None;
        }
        let tbl = unsafe { *self.env };
        unsafe {
            let ivals = [Self::val_i(x), Self::val_i(z)];
            let r = ((*tbl).v1_1.CallIntMethodA)(
                self.env,
                self.obj,
                self.mids.freeze_checks,
                ivals.as_ptr(),
            );
            if ((*tbl).v1_2.ExceptionCheck)(self.env) {
                ((*tbl).v1_1.ExceptionClear)(self.env);
                return None;
            }
            Some(r)
        }
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
    /// Full member set of a block tag — the feed for the process-wide tag
    /// registry (features_snap::tag_registry_is): ONE crossing per tag
    /// replaces the per-cell tagIs upcalls inside the 26.3 ore rule tests
    /// ([ore-prof] top holder: tag=4.4ms/chunk, ~2 crossings x ~3K cells).
    /// None = no tagMembers mid (older jar) / the call failed — the caller
    /// keeps the per-cell bridge.
    fn tag_members(&self, tag: &str) -> Option<Vec<String>> {
        if self.noup() || self.mids.tag_members.is_null() {
            return None;
        }
        let Some(js) = self.new_jstring(tag) else {
            return None;
        };
        let tbl = unsafe { *self.env };
        unsafe {
            let ivals = [Self::val_l(js)];
            let r = ((*tbl).v1_1.CallObjectMethodA)(
                self.env,
                self.obj,
                self.mids.tag_members,
                ivals.as_ptr(),
            ) as jni_sys::jobjectArray;
            ((*tbl).v1_1.DeleteLocalRef)(self.env, js as jni_sys::jobject);
            if ((*tbl).v1_2.ExceptionCheck)(self.env) {
                ((*tbl).v1_2.ExceptionClear)(self.env);
                return None;
            }
            if r.is_null() {
                return None;
            }
            let len = ((*tbl).v1_1.GetArrayLength)(self.env, r as jni_sys::jarray) as usize;
            let mut out: Vec<String> = Vec::with_capacity(len);
            for i in 0..len {
                let el = ((*tbl).v1_1.GetObjectArrayElement)(self.env, r, i as jni_sys::jsize);
                if el.is_null() {
                    continue;
                }
                let c = ((*tbl).v1_1.GetStringUTFChars)(self.env, el as jni_sys::jstring, std::ptr::null_mut());
                if !c.is_null() {
                    out.push(std::ffi::CStr::from_ptr(c).to_string_lossy().into_owned());
                    ((*tbl).v1_2.ReleaseStringUTFChars)(self.env, el as jni_sys::jstring, c);
                }
                ((*tbl).v1_1.DeleteLocalRef)(self.env, el);
            }
            ((*tbl).v1_1.DeleteLocalRef)(self.env, r as jni_sys::jobject);
            Some(out)
        }
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

use crate::ffi::sessions_lock;

/// Best-effort panic payload to string (String / &'static str / opaque).
fn panic_msg(p: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = p.downcast_ref::<String>() {
        s.clone()
    } else if let Some(s) = p.downcast_ref::<&str>() {
        (*s).to_string()
    } else {
        "opaque panic payload".to_string()
    }
}

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
    let guard = sessions_lock();
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
    let guard = sessions_lock();
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
    let guard = sessions_lock();
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
    // cont-93 panic guard: a placement panic unwinding across this
    // extern "system" boundary aborts the JVM (run 41324500487 deco lanes
    // died silently exactly this way). Catch inside the sessions_lock scope
    // (no poisoning), log the payload, degrade to an empty plan — Java
    // decorates nothing this chunk but the server lives and the log names
    // the panic.
    let runs = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        chunk_plan(&d.plan, &d.biome_features, &biomes)
    })) {
        Ok(r) => r,
        Err(p) => {
            crate::ffi::log_warn(&format!(
                "deco: chunk_plan PANIC (chunk degrades to 0 runs): {}",
                panic_msg(&p)
            ));
            Vec::new()
        }
    };
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
    let guard = sessions_lock();
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
    let ov_cols: &std::cell::RefCell<std::collections::HashMap<(i32, i32), Vec<i32>>> =
        &d.chunk_ov_cols;
    let snap: &std::cell::RefCell<crate::features_snap::SnapRegion> = &d.chunk_snap;
    let hms: &std::cell::RefCell<
        std::collections::HashMap<
            (i32, i32),
            Option<std::sync::Arc<std::sync::Mutex<Vec<i32>>>>,
        >,
    > = &d.chunk_hms;
    let zoom_seed = d.chunk_zoom_seed.get();
    let level_ref = &level;
    let run_loader: std::cell::Cell<u64> = std::cell::Cell::new(0);
    // A/B: sy-1..sy+1 batch prefetch (CRUS_SNAP_RANGE=1). MEASURED NEGATIVE on
    // the 2-core sandbox: the per-section Java build (~3.5-4.8us) dominates the
    // JNI crossing (~2us), so the 1.78x section overbuild (11013 built vs 6188
    // useful) costs more than the 1.7x fewer crossings save (loader 0.22-0.37ms
    // vs 0.14-0.20ms single). Kept for cross-hardware A/B only; default OFF.
    let snap_range = *SNAP_RANGE_ON.get_or_init(|| {
        std::env::var("CRUS_SNAP_RANGE").as_deref() == Ok("1")
    });
    let loader = |cx: i32, cz: i32, sec_y: i32| -> Option<crate::features_snap::SnapSection> {
        let t0 = prof_now();
        PROF_LOADER_N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let r = if snap_range {
            let range = level_ref.snap_section_range(cx, cz, sec_y - 1, 3);
            let mut target: Option<crate::features_snap::SnapSection> = None;
            if let Some(bytes) = range {
                let mut s = snap.borrow_mut();
                let epoch = s.epoch.get();
                let mut p = 0usize;
                for _ in 0..3 {
                    if p + 4 > bytes.len() {
                        break;
                    }
                    let mut lb = [0u8; 4];
                    lb.copy_from_slice(&bytes[p..p + 4]);
                    let len32 = i32::from_le_bytes(lb);
                    p += 4;
                    if len32 <= 0 || len32 as usize > bytes.len() - p {
                        break; // absent (null build) / malformed — stop, stay lazy
                    }
                    let len = len32 as usize;
                    if let Some(sec) = crate::features_snap::parse_section(&bytes[p..p + len]) {
                        let sy = sec.sec_y;
                        if let Some(slot) = s.slot(cx, cz, sy) {
                            if sy == sec_y {
                                target = Some(sec);
                            } else if s.secs[slot].is_none() {
                                let mut sec2 = sec;
                                sec2.epoch = epoch;
                                s.secs[slot] = Some(sec2);
                            }
                        }
                    }
                    p += len;
                }
            }
            target
        } else {
            let p0 = prof_now();
            let parsed = level_ref.snap_section(cx, cz, sec_y);
            PROF_PARSE_NS.fetch_add(prof_now() - p0, std::sync::atomic::Ordering::Relaxed);
            parsed
        };
        run_loader.set(run_loader.get() + (prof_now() - t0));
        r
    };
    let hm_loader = |cx: i32, cz: i32| -> Option<std::sync::Arc<std::sync::Mutex<Vec<i32>>>> {
        if let Some(cached) = hms.borrow().get(&(cx, cz)) {
            return cached.clone();
        }
        // the WG handshake must always succeed: 512 ints, O(1) Java
        let loaded = level_ref
            .heightmaps(cx, cz)
            .map(|v| std::sync::Arc::new(std::sync::Mutex::new(v)));
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
    let st_flags = |name: &str| -> Option<u8> {
        let idx = *d.state.state_index.get(name)?;
        d.state.state_flags.lock().unwrap().get(idx as usize).copied()
    };
    let chunk_level = crate::features_snap::ChunkLevel::new(
        level_ref,
        ov,
        ov_cols,
        snap,
        &loader,
        &hm_loader,
        &d.uid_names,
        zoom_seed,
        &st_flags,
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
    let t_run = prof_now();
    // cont-93 panic guard: run_feature_master is the largest unproven native
    // surface (feature placement). A panic here unwinds across the
    // extern "system" boundary = JVM abort (run 41324500487 deco lanes: the
    // JVM died silently right after the spawn backlog, both CRUS_NATIVE_DECO
    // lanes, reproducibly). Catch INSIDE the sessions_lock scope (no mutex
    // poisoning), release the pinned ops buffer, log the payload with the
    // feature name, return -1 — the Java loop skips the feature (the n<0
    // protocol branch) and the server lives.
    let run_res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_feature_master(
            &d.state,
            &chunk_level,
            deco_seed as i64,
            if paper_seed == -1 { None } else { Some(paper_seed as i64) },
            BlockPos::new(origin_x as i32, origin_y as i32, origin_z as i32),
            &name,
            step as usize,
            gid as usize,
            slice,
        )
    }));
    let n = match run_res {
        Ok(n) => n,
        Err(p) => {
            // release the critical pin FIRST (an unreleased critical blocks
            // GC and can wedge the JVM a second way)
            ((*tbl).v1_2.ReleasePrimitiveArrayCritical)(env, out as jni_sys::jarray, ptr, 0);
            crate::ffi::log_warn(&format!(
                "deco: run_feature PANIC step={} gid={} feature={} (feature skipped, ops buffer stale): {}",
                step, gid, name,
                panic_msg(&p)
            ));
            return -1;
        }
    };
    let t_run_ns = prof_now() - t_run;
    PROF_RUNS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let rl = run_loader.get();
    PROF_LOADER_NS.fetch_add(rl, std::sync::atomic::Ordering::Relaxed);
    PROF_NATIVE_NS.fetch_add(
        t_run_ns.saturating_sub(rl),
        std::sync::atomic::Ordering::Relaxed,
    );
    if let Ok(mut m) = PROF_FEAT.lock() {
        let e = m.entry(name.clone()).or_insert((0, 0));
        e.0 += t_run_ns.saturating_sub(rl);
        e.1 += 1;
    }
    ((*tbl).v1_2.ReleasePrimitiveArrayCritical)(env, out as jni_sys::jarray, ptr, 0);
    n as jint
}

/// decoRunBatch(ctx, decoSeed, paperSeed, originX, originY, originZ, maxY,
/// int[] runs, int nRuns, int startRun, bridge, int[] ops, int[] perRun,
/// int[] meta) -> int opsWritten.
///
/// S1 sub-batch (cont139, H4; kill-switch CRUS_S1_SUBBATCH is JAVA-side — this
/// entry exists unconditionally and is called only when the switch is ON):
/// expands CONSECUTIVE (step, gid) runs of the chunk plan in ONE JNI crossing
/// with the exact per-feature protocol (run_feature_staged = the
/// run_feature_master body; same seeding, same overlay evolution), writing all
/// ops into the shared pinned buffer. Stops at flush boundaries (meta[1]):
///   0 END      — all requested runs expanded
///   1 DELEGATE — runs[startRun+completed] is a vanilla delegate: Java applies
///                the batch, runs delegateVanilla + decoDelegateMark, resumes
///                at +1
///   2 FREEZE   — runs[startRun+completed] is freeze_top_layer with ops
///                pending: flush first; the feature runs at the NEXT call
///                start (opsWritten == 0 there) so its freezeChecks live-region
///                upcalls see the flushed region exactly like the old protocol
///   3 FULL     — ops buffer would overflow: the staged ops carry over in
///                DecoCtx.batch_carry (they are already in the overlay and
///                MUST be applied) and drain first on the next call
/// meta = [completedRuns, boundaryKind, opsWritten]; perRun[k] = ops of run
/// startRun+k (completed entries). Return: opsWritten, or -1 when the ctx /
/// arrays / bridge are unusable (Java falls back to the per-feature loop),
/// or -2 when a feature PANICKED mid-batch (meta carries the valid prefix;
/// Java applies it, skips the panicked run and continues).
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "system" fn Java_com_planeta_crussty_ChunkGenNative_decoRunBatch(
    env: *mut jni_sys::JNIEnv,
    _class: jni_sys::jclass,
    ctx: jlong,
    deco_seed: jlong,
    paper_seed: jlong,
    origin_x: jint,
    origin_y: jint,
    origin_z: jint,
    max_y: jint,
    runs: jni_sys::jintArray,
    n_runs: jint,
    start_run: jint,
    bridge: jobject,
    out: jni_sys::jintArray,
    per_run: jni_sys::jintArray,
    meta: jni_sys::jintArray,
) -> jint {
    if bridge.is_null() || out.is_null() || runs.is_null() || per_run.is_null() || meta.is_null() {
        return -1;
    }
    if n_runs <= 0 || start_run < 0 || start_run >= n_runs {
        return -1;
    }
    let tbl = *env;
    let guard = sessions_lock();
    let Some(d) = guard.as_ref().and_then(|m| m.get(&(ctx as i64)))
        .and_then(|c| c.deco.as_ref()) else {
        return -1;
    };
    // ---- read the requested (step, gid) run pairs -------------------------
    let run_pairs: Vec<(usize, usize)> = unsafe {
        let rlen = ((*tbl).v1_1.GetArrayLength)(env, runs as jni_sys::jarray) as usize;
        let want_end = (n_runs as usize).min(rlen / 2);
        if (start_run as usize) >= want_end {
            return -1;
        }
        let rp = ((*tbl).v1_2.GetPrimitiveArrayCritical)(env, runs as jni_sys::jarray, std::ptr::null_mut());
        if rp.is_null() {
            return -1;
        }
        let slice = std::slice::from_raw_parts(rp as *const i32, rlen);
        let mut v = Vec::with_capacity(want_end - start_run as usize);
        for k in start_run as usize..want_end {
            v.push((*slice.get_unchecked(k * 2) as usize, *slice.get_unchecked(k * 2 + 1) as usize));
        }
        ((*tbl).v1_2.ReleasePrimitiveArrayCritical)(env, runs as jni_sys::jarray, rp, 0);
        v
    };
    // ---- pin the output arrays -------------------------------------------
    let olen = ((*tbl).v1_1.GetArrayLength)(env, out as jni_sys::jarray) as usize;
    if olen < 6 {
        return -1;
    }
    let plen = ((*tbl).v1_1.GetArrayLength)(env, per_run as jni_sys::jarray) as usize;
    let mlen = ((*tbl).v1_1.GetArrayLength)(env, meta as jni_sys::jarray) as usize;
    if mlen < 3 {
        return -1;
    }
    let write_meta = |meta_ptr: *mut i32, completed: usize, kind: i32, written: usize| unsafe {
        // (closure bodies do not inherit the unsafe fn context — explicit block)
        *meta_ptr.add(0) = completed as i32;
        *meta_ptr.add(1) = kind;
        *meta_ptr.add(2) = written as i32;
    };
    // NOTE: env may be attached to a worker thread; direct upcalls only.
    let Some(level) = JniLevel::new(env, bridge, &d.uid_names, origin_y, max_y) else {
        return -1;
    };
    // Chunk-scoped level shared by the WHOLE batch (the old path re-created it
    // per feature): ov/snap/hms live in DecoCtx across the chunk either way —
    // the only per-feature state was the tag memo, whose (slot, paletteIdx)
    // keys stay valid across features of one batch (no epoch bumps and no
    // section reloads can happen inside a batch: delegates/freezes are
    // boundaries and the live region is frozen while ops are pending).
    let ov: &std::cell::RefCell<crate::features_snap::OvMap> = &d.chunk_ov;
    let ov_cols: &std::cell::RefCell<std::collections::HashMap<(i32, i32), Vec<i32>>> =
        &d.chunk_ov_cols;
    let snap: &std::cell::RefCell<crate::features_snap::SnapRegion> = &d.chunk_snap;
    let hms: &std::cell::RefCell<
        std::collections::HashMap<
            (i32, i32),
            Option<std::sync::Arc<std::sync::Mutex<Vec<i32>>>>,
        >,
    > = &d.chunk_hms;
    let zoom_seed = d.chunk_zoom_seed.get();
    let level_ref = &level;
    let run_loader: std::cell::Cell<u64> = std::cell::Cell::new(0);
    let loader = |cx: i32, cz: i32, sec_y: i32| -> Option<crate::features_snap::SnapSection> {
        let t0 = prof_now();
        PROF_LOADER_N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let p0 = prof_now();
        let parsed = level_ref.snap_section(cx, cz, sec_y);
        PROF_PARSE_NS.fetch_add(prof_now() - p0, std::sync::atomic::Ordering::Relaxed);
        run_loader.set(run_loader.get() + (prof_now() - t0));
        parsed
    };
    let hm_loader = |cx: i32, cz: i32| -> Option<std::sync::Arc<std::sync::Mutex<Vec<i32>>>> {
        if let Some(cached) = hms.borrow().get(&(cx, cz)) {
            return cached.clone();
        }
        let loaded = level_ref
            .heightmaps(cx, cz)
            .map(|v| std::sync::Arc::new(std::sync::Mutex::new(v)));
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
    let st_flags = |name: &str| -> Option<u8> {
        let idx = *d.state.state_index.get(name)?;
        d.state.state_flags.lock().unwrap().get(idx as usize).copied()
    };
    let chunk_level = crate::features_snap::ChunkLevel::new(
        level_ref,
        ov,
        ov_cols,
        snap,
        &loader,
        &hm_loader,
        &d.uid_names,
        zoom_seed,
        &st_flags,
    );
    // ---- pin ops + per_run for the whole batch ---------------------------
    let ptr = ((*tbl).v1_2.GetPrimitiveArrayCritical)(env, out as jni_sys::jarray, std::ptr::null_mut());
    if ptr.is_null() {
        return -1;
    }
    let pr_ptr = ((*tbl).v1_2.GetPrimitiveArrayCritical)(env, per_run as jni_sys::jarray, std::ptr::null_mut());
    if pr_ptr.is_null() {
        ((*tbl).v1_2.ReleasePrimitiveArrayCritical)(env, out as jni_sys::jarray, ptr, 0);
        return -1;
    }
    let mt_ptr = ((*tbl).v1_2.GetPrimitiveArrayCritical)(env, meta as jni_sys::jarray, std::ptr::null_mut());
    if mt_ptr.is_null() {
        ((*tbl).v1_2.ReleasePrimitiveArrayCritical)(env, per_run as jni_sys::jarray, pr_ptr, 0);
        ((*tbl).v1_2.ReleasePrimitiveArrayCritical)(env, out as jni_sys::jarray, ptr, 0);
        return -1;
    }
    let slice = std::slice::from_raw_parts_mut(ptr as *mut i32, olen);
    let per_slice = std::slice::from_raw_parts_mut(pr_ptr as *mut i32, plen.min(run_pairs.len()));
    let t_run = prof_now();
    let progress: std::cell::Cell<(usize, usize)> = std::cell::Cell::new((0, 0));
    // cont-93 panic discipline (same as decoRunFeature): a panic must not
    // unwind across the extern "system" boundary — catch inside, release the
    // pins, report the valid prefix (-2) so Java applies it, skips the
    // panicked run and continues.
    let run_res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut carry = d.batch_carry.borrow_mut();
        crate::features_master::run_batch_core(
            &d.state,
            &chunk_level,
            deco_seed as i64,
            if paper_seed == -1 { None } else { Some(paper_seed as i64) },
            crate::features_placement::BlockPos::new(origin_x as i32, origin_y as i32, origin_z as i32),
            &d.plan,
            &run_pairs,
            0usize,
            slice,
            per_slice,
            &mut carry,
            &prof_now,
            &progress,
        )
    }));
    let res = match run_res {
        Ok(r) => r,
        Err(p) => {
            let (completed, written) = progress.get();
            write_meta(mt_ptr as *mut i32, completed, 4, written);
            ((*tbl).v1_2.ReleasePrimitiveArrayCritical)(env, meta as jni_sys::jarray, mt_ptr, 0);
            ((*tbl).v1_2.ReleasePrimitiveArrayCritical)(env, per_run as jni_sys::jarray, pr_ptr, 0);
            ((*tbl).v1_2.ReleasePrimitiveArrayCritical)(env, out as jni_sys::jarray, ptr, 0);
            crate::ffi::log_warn(&format!(
                "deco: run_batch PANIC after {completed} runs / {written} ops (prefix valid, run skipped): {}",
                panic_msg(&p)
            ));
            return -2;
        }
    };
    // per-run op counts were written into per_slice by the core as each run
    // completed (panic-consistent with meta[0]); meta carries the batch result
    write_meta(mt_ptr as *mut i32, res.completed, res.boundary as i32, res.ops_written);
    ((*tbl).v1_2.ReleasePrimitiveArrayCritical)(env, meta as jni_sys::jarray, mt_ptr, 0);
    ((*tbl).v1_2.ReleasePrimitiveArrayCritical)(env, per_run as jni_sys::jarray, pr_ptr, 0);
    ((*tbl).v1_2.ReleasePrimitiveArrayCritical)(env, out as jni_sys::jarray, ptr, 0);
    let t_run_ns = prof_now() - t_run;
    PROF_RUNS.fetch_add(res.completed as u64, std::sync::atomic::Ordering::Relaxed);
    let rl = run_loader.get();
    PROF_LOADER_NS.fetch_add(rl, std::sync::atomic::Ordering::Relaxed);
    PROF_NATIVE_NS.fetch_add(
        t_run_ns.saturating_sub(rl),
        std::sync::atomic::Ordering::Relaxed,
    );
    // per-feature native wall attribution (same granularity as decoRunFeature)
    {
        let mut ridx = start_run as usize;
        if let Ok(mut m) = PROF_FEAT.lock() {
            for (ops_n, ns) in res.per_run.iter() {
                if let Some(&(step, gid)) = run_pairs.get(ridx - start_run as usize) {
                    if let Some(name) = d.plan.steps.get(step).and_then(|v| v.get(gid)) {
                        let e = m.entry(name.clone()).or_insert((0, 0));
                        e.0 += *ns;
                        e.1 += 1;
                    }
                }
                let _ = ops_n;
                ridx += 1;
            }
        }
    }
    res.ops_written as jint
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
    prof_chunk_tick();
    let guard = sessions_lock();
    if let Some(d) = guard
        .as_ref()
        .and_then(|m| m.get(&(ctx as i64)))
        .and_then(|c| c.deco.as_ref())
    {
        d.chunk_ov.borrow_mut().clear();
        d.chunk_ov_cols.borrow_mut().clear();
        d.chunk_snap.borrow_mut().reset(cx, cz, min_sec_y);
        d.chunk_hms.borrow_mut().clear();
        d.chunk_zoom_seed.set(zoom_seed);
        // S1 batch carry never outlives its chunk (a batch sequence aborted
        // mid-way — server stop / lane fallback — must not leak ops into the
        // next chunk's overlay bookkeeping)
        d.batch_carry.borrow_mut().clear();
    }
}

/// oreRunConfigured(ctx, rngLo, rngHi, cornerX, minY, cornerZ, maxY,
/// featX, featY, featZ, configuredName, bridge, out) -> int ops.
///
/// (cornerX, cornerZ) = the chunk origin block coords (the ops-rel base,
/// same as decoRunFeature's origin); (featX, featY, featZ) = the ABSOLUTE
/// FeaturePlaceContext.origin() — the position the vanilla placement
/// modifiers produced.
///
/// cont-99 front 3 (deco=0 ore takeover): the vanilla decorate loop runs in
/// Java and the placement modifiers (Count/InSquare/HeightRange/...) have
/// ALREADY consumed their draws from the shared WorldgenRandom by the time
/// OreFeature.place() is invoked on the swapped CrusstyOreFeature instance.
/// This entry expands ONLY the configured feature graph at the given origin,
/// continuing the Java RNG stream from the RAW Xoroshiro128PlusPlus engine
/// state (seedLo/seedHi read by reflection at place() entry). Everything else
/// mirrors decoRunFeature: per-run chunk scope (fresh overlay + lazy 3x3
/// snapshot — earlier ore/vanilla writes are applied to the live region
/// before this call and therefore visible), JNI upcalls for the rare
/// queries the snapshot cannot serve, ops as 6-i32 records.
///
/// Return protocols: n>=0 = ops written; -1 = ctx/bridge/env failure;
/// -3 = configured name absent from the native registry (the Java hook
/// falls back to super.place() — the Java RNG is untouched by this entry
/// in every non-success case, so the fallback is bit-exact).
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "system" fn Java_com_planeta_crussty_ChunkGenNative_oreRunConfigured(
    env: *mut jni_sys::JNIEnv,
    _class: jni_sys::jclass,
    ctx: jlong,
    rng_lo: jlong,
    rng_hi: jlong,
    corner_x: jint,
    origin_y: jint,
    corner_z: jint,
    max_y: jint,
    feat_x: jint,
    feat_y: jint,
    feat_z: jint,
    configured: jstring,
    bridge: jobject,
    out: jni_sys::jintArray,
) -> jint {
    if bridge.is_null() || out.is_null() || configured.is_null() {
        return -1;
    }
    let tbl = *env;
    let guard = sessions_lock();
    let Some(d) = guard.as_ref().and_then(|m| m.get(&(ctx as i64)))
        .and_then(|c| c.deco.as_ref()) else {
        return -1;
    };
    // configured name -> utf8
    let clen = ((*tbl).v1_1.GetStringUTFLength)(env, configured) as usize;
    let cptr = ((*tbl).v1_1.GetStringUTFChars)(env, configured, std::ptr::null_mut());
    if cptr.is_null() {
        return -1;
    }
    let name = std::ffi::CStr::from_ptr(cptr as *const i8).to_string_lossy().into_owned();
    ((*tbl).v1_1.ReleaseStringUTFChars)(env, configured, cptr);
    let _ = clen;
    // -3: the native registry has no such feature (decode gap) — the Java
    // hook falls back to the vanilla expansion for this feature.
    // 26.3: the Java-side registry has NO configured layer — the hook binds
    // the FEATURE registry name ("minecraft:ore_dirt"), which is the PLACED
    // name for every ore. Accept both: configured names expand directly,
    // placed names resolve through reg.placed -> configured.
    let cfg_name = if d.state.reg.configured.contains_key(&name) {
        name.clone()
    } else {
        match d.state.reg.placed.get(&name) {
            Some(pd) if d.state.reg.configured.contains_key(&pd.feature) => {
                pd.feature.clone()
            }
            _ => return -3,
        }
    };
    let cfg_name = cfg_name.as_str();
    // per-run scope (deco=0 ore hook): between two ore place() calls the
    // vanilla decorate loop wrote non-ore features through the live region
    // and this hook's own earlier ops were applied Java-side. A stale
    // overlay would shadow those region writes for THIS run and a stale
    // snapshot would miss them entirely (canPlaceOre isAdjacentToAir drift)
    // — enforce the exactness contract here: fresh overlay + epoch bump +
    // fresh heightmaps every run. Idempotent with the Java-side
    // decoChunkBegin retarget (chunk change = same clears + snap.reset).
    d.chunk_ov.borrow_mut().clear();
    d.chunk_ov_cols.borrow_mut().clear();
    d.chunk_snap.borrow().bump_epoch();
    d.chunk_hms.borrow_mut().clear();
    let Some(level) = JniLevel::new(env, bridge, &d.uid_names, origin_y, max_y) else {
        return -1;
    };
    let ov: &std::cell::RefCell<crate::features_snap::OvMap> = &d.chunk_ov;
    let ov_cols: &std::cell::RefCell<std::collections::HashMap<(i32, i32), Vec<i32>>> =
        &d.chunk_ov_cols;
    let snap: &std::cell::RefCell<crate::features_snap::SnapRegion> = &d.chunk_snap;
    let hms: &std::cell::RefCell<
        std::collections::HashMap<
            (i32, i32),
            Option<std::sync::Arc<std::sync::Mutex<Vec<i32>>>>,
        >,
    > = &d.chunk_hms;
    let zoom_seed = d.chunk_zoom_seed.get();
    let level_ref = &level;
    let run_loader = std::cell::Cell::new(0u64);
    let loader = |cx: i32, cz: i32, sec_y: i32| -> Option<crate::features_snap::SnapSection> {
        let t0 = prof_now();
        PROF_LOADER_N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let p0 = prof_now();
        let parsed = level_ref.snap_section(cx, cz, sec_y);
        PROF_PARSE_NS.fetch_add(prof_now() - p0, std::sync::atomic::Ordering::Relaxed);
        run_loader.set(run_loader.get() + (prof_now() - t0));
        parsed
    };
    let hm_loader = |cx: i32, cz: i32| -> Option<std::sync::Arc<std::sync::Mutex<Vec<i32>>>> {
        if let Some(cached) = hms.borrow().get(&(cx, cz)) {
            return cached.clone();
        }
        let loaded = level_ref
            .heightmaps(cx, cz)
            .map(|v| std::sync::Arc::new(std::sync::Mutex::new(v)));
        hms.borrow_mut().insert((cx, cz), loaded.clone());
        loaded
    };
    let st_flags = |name: &str| -> Option<u8> {
        let idx = *d.state.state_index.get(name)?;
        d.state.state_flags.lock().unwrap().get(idx as usize).copied()
    };
    let chunk_level = crate::features_snap::ChunkLevel::new(
        level_ref,
        ov,
        ov_cols,
        snap,
        &loader,
        &hm_loader,
        &d.uid_names,
        zoom_seed,
        &st_flags,
    );
    let olen = ((*tbl).v1_1.GetArrayLength)(env, out as jni_sys::jarray) as usize;
    if olen < 6 {
        return -1;
    }
    let ptr = ((*tbl).v1_2.GetPrimitiveArrayCritical)(env, out as jni_sys::jarray, std::ptr::null_mut());
    if ptr.is_null() {
        return -1;
    }
    let slice = std::slice::from_raw_parts_mut(ptr as *mut i32, olen);
    let t_run = prof_now();
    // same panic discipline as decoRunFeature (cont-93): a panic must not
    // unwind across the extern "system" boundary (JVM abort) — catch inside,
    // release the pin, log, return -1 (Java falls back to the vanilla
    // expansion with the untouched Java RNG).
    let run_res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut rng = crate::features_rng::WorldgenRandom::from_state(
            rng_lo as i64,
            rng_hi as i64,
        );
        let pctx = crate::features_place::PlaceCtx {
            reg: &d.state.reg,
            level: &chunk_level,
            center_x: corner_x as i32,
            center_z: corner_z as i32,
            state_index: &d.state.state_index,
            uid_index: &d.state.uid_index,
        };
        let mut ops: Vec<crate::features_place::DecoOp> = Vec::new();
        let placed = crate::features_place::place_configured(
            &pctx,
            cfg_name,
            &mut rng,
            crate::features_placement::BlockPos::new(
                feat_x as i32,
                feat_y as i32,
                feat_z as i32,
            ),
            &mut ops,
            0,
        );
        let _ = placed;
        let mut n = 0usize;
        for op in &ops {
            if n * 6 + 5 >= slice.len() {
                break;
            }
            slice[n * 6] = op.x as i32;
            slice[n * 6 + 1] = op.y as i32;
            slice[n * 6 + 2] = op.z as i32;
            slice[n * 6 + 3] = op.state as i32;
            slice[n * 6 + 4] = op.flags as i32;
            slice[n * 6 + 5] = op.aux as i32;
            n += 1;
        }
        n as i32
    }));
    let n = match run_res {
        Ok(n) => n,
        Err(p) => {
            ((*tbl).v1_2.ReleasePrimitiveArrayCritical)(env, out as jni_sys::jarray, ptr, 0);
            crate::ffi::log_warn(&format!(
                "ore: run_configured PANIC feature={} (fallback to vanilla): {}",
                cfg_name,
                panic_msg(&p)
            ));
            return -1;
        }
    };
    let t_run_ns = prof_now() - t_run;
    PROF_RUNS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let rl = run_loader.get();
    PROF_LOADER_NS.fetch_add(rl, std::sync::atomic::Ordering::Relaxed);
    PROF_NATIVE_NS.fetch_add(t_run_ns.saturating_sub(rl), std::sync::atomic::Ordering::Relaxed);
    if let Ok(mut m) = PROF_FEAT.lock() {
        let e = m.entry(format!("ore:{cfg_name}")).or_insert((0, 0));
        e.0 += t_run_ns.saturating_sub(rl);
        e.1 += 1;
    }
    ((*tbl).v1_2.ReleasePrimitiveArrayCritical)(env, out as jni_sys::jarray, ptr, 0);
    n as jint
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
        let guard = sessions_lock();
        if let Some(d) = guard
            .as_ref()
            .and_then(|m| m.get(&(ctx as i64)))
            .and_then(|c| c.deco.as_ref())
        {
            {
                let mut ov = d.chunk_ov.borrow_mut();
                let mut cols = d.chunk_ov_cols.borrow_mut();
                for k in 0..count as usize {
                    let (x, y, z) = (slice[k * 3], slice[k * 3 + 1], slice[k * 3 + 2]);
                    ov.remove(&(x, y, z));
                    if let Some(v) = cols.get_mut(&(x, z)) {
                        v.retain(|&yy| yy != y);
                        if v.is_empty() {
                            cols.remove(&(x, z));
                        }
                    }
                }
            }
            // revert the WG heightmap raise the dropped write may have caused
            // (the op never lands in the region — vanilla Heightmap.update
            // would never have seen it). Treat each dropped pos as a removal:
            // rescan down when it sat exactly at top-1. Rare path (drops are
            // zero in healthy runs) — correctness over speed.
            let st_flags = |name: &str| -> Option<u8> {
                let idx = *d.state.state_index.get(name)?;
                d.state.state_flags.lock().unwrap().get(idx as usize).copied()
            };
            let probe = |x: i32, y: i32, z: i32, mask: u8| -> Option<bool> {
                if let Some(st) = d.chunk_ov.borrow().get(&(x, y, z)) {
                    return st_flags(st).map(|f| f & mask != 0);
                }
                d.chunk_snap.borrow().flag(x, y, z, mask as u16)
            };
            let mut hms = d.chunk_hms.borrow_mut();
            for k in 0..count as usize {
                let (x, y, z) = (slice[k * 3], slice[k * 3 + 1], slice[k * 3 + 2]);
                let Some(Some(arc)) = hms.get_mut(&(x >> 4, z >> 4)) else {
                    continue;
                };
                let mut hm = arc.lock().unwrap();
                if hm.len() < 512 {
                    continue;
                }
                let min_y = d.chunk_snap.borrow().min_sec_y * 16;
                for (off, mask) in [(0usize, 1u8), (256, 2)] {
                    let idx = off + ((z & 15) as usize) * 16 + (x & 15) as usize;
                    let first = hm[idx];
                    if first - 1 != y {
                        continue;
                    }
                    let mut yy = y - 1;
                    let mut done = false;
                    while yy >= min_y {
                        match probe(x, yy, z, mask) {
                            Some(true) => {
                                hm[idx] = yy + 1;
                                done = true;
                                break;
                            }
                            Some(false) => yy -= 1,
                            None => break, // unsnapshotted: keep the frozen value
                        }
                    }
                    if !done && yy < min_y {
                        hm[idx] = min_y;
                    }
                }
            }
        }
        ((*tbl).v1_2.ReleasePrimitiveArrayCritical)(env, coords as jni_sys::jarray, p, jni_sys::JNI_ABORT);
    }
}

/// decoDelegateMark(ctx) — a vanilla delegate feature ran (its writes went
/// through the region directly, invisible to BOTH the overlay and the
/// snapshot). Clear the chunk overlay (its entries may be stale where the
/// delegate overwrote them; the flushed native writes live in the region and
/// come back via the reloaded snapshot), bump the snapshot epoch (stale
/// sections reload lazily on the next query) AND drop the cached WG
/// heightmaps (the region maps moved under them — vanilla updates the WG
/// heightmaps on every setBlock, so the next query re-handshakes with the
/// post-delegate values; the cont-56 0.555% judge residual).
#[no_mangle]
pub unsafe extern "system" fn Java_com_planeta_crussty_ChunkGenNative_decoDelegateMark(
    _env: *mut jni_sys::JNIEnv,
    _class: jni_sys::jclass,
    ctx: jlong,
) {
    let guard = sessions_lock();
    if let Some(d) = guard
        .as_ref()
        .and_then(|m| m.get(&(ctx as i64)))
        .and_then(|c| c.deco.as_ref())
    {
        d.chunk_ov.borrow_mut().clear();
        d.chunk_ov_cols.borrow_mut().clear();
        d.chunk_snap.borrow().bump_epoch();
        d.chunk_hms.borrow_mut().clear();
    }
}

/// decoSetFlags(ctx, byte[] flags) — the decoStates handshake tail: one flag
/// byte per state string (bit0=isAir, bit1=blocksMotion), computed Java-side
/// from the resolved BlockStates. Feeds the WG heightmap update-on-write.
///
/// Q5 (cont-136): the flags FAN OUT to every ctx with a deco engine, not
/// just the addressed one. Pre-Q5 only the ctx that happened to run the
/// first chunk deco received the handshake (DecoDriver FLAGS_SENT one-shot)
/// — every other pool ctx kept all-zero flags, a latent ctx-dependent
/// divergence in the WG heightmap update-on-write (only observable for
/// air/blocksMotion writes landing exactly at top-1, which is why the A/B
/// judges never caught it). The dedicated deco pool (Q5) — and equally the
/// shared-pool kill-switch — requires all deco-capable ctxs to initialize
/// identically, so the handshake is world-global now. Ctxs with a different
/// states-table length (other worlds, other packs) are skipped; each world's
/// own first populate re-sends its own table before any of its deco runs.
#[no_mangle]
pub unsafe extern "system" fn Java_com_planeta_crussty_ChunkGenNative_decoSetFlags(
    env: *mut jni_sys::JNIEnv,
    _class: jni_sys::jclass,
    ctx: jlong,
    flags: jni_sys::jbyteArray,
) {
    let tbl = *env;
    let len = ((*tbl).v1_1.GetArrayLength)(env, flags as jni_sys::jarray) as usize;
    let guard = sessions_lock();
    // addressed-ctx length check first (pre-existing warn keyed on the caller's ctx)
    if let Some(d) = guard
        .as_ref()
        .and_then(|m| m.get(&(ctx as i64)))
        .and_then(|c| c.deco.as_ref())
    {
        if len != d.state.states.len() {
            crate::ffi::log_warn(&format!(
                "[deco-flags] length mismatch: got {len}, want {} (flags ignored)",
                d.state.states.len()
            ));
            return;
        }
    }
    let Some(map) = guard.as_ref() else {
        return;
    };
    if len == 0 {
        return;
    }
    let p = ((*tbl).v1_1.GetByteArrayElements)(env, flags as jni_sys::jarray, std::ptr::null_mut());
    if p.is_null() {
        return;
    }
    let bytes = std::slice::from_raw_parts(p as *const u8, len);
    for c in map.values() {
        let Some(d) = c.deco.as_ref() else {
            continue;
        };
        if d.state.states.len() != len {
            continue;
        }
        d.state.state_flags.lock().unwrap().copy_from_slice(bytes);
    }
    ((*tbl).v1_1.ReleaseByteArrayElements)(
        env,
        flags as jni_sys::jarray,
        p as *mut i8,
        0,
    );
}
