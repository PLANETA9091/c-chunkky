# FR2-A — Deco wrapper audit (DecoDriver / Java deco path, Paper 26.3, "dptt")

RESEARCH ONLY — no code modified, no builds run, worklog untouched.
All numbers below are cited from files in `c-faster/reference/`, `c-faster/plugin/src/`,
`c-faster/native/src/` and `c-faster/worklog-session.md` (worklog used as evidence
archive only). `ESTIMATE` rows show the arithmetic.

---

## 0. Which "15 ms" is which — three distinct readings, all real

The "~15 ms Java deco wrapper" resolves into three measured quantities that must not
be conflated:

| Reading | Value | Source |
|---|---|---|
| **A. deco=1 vs deco=0 A/B e2e delta (CI, honest windows)** | **+15.1 ms/ch (par4), +16.8 ms/ch (serial)** | worklog cont-98 (worklog-session.md:5696-5715): deco probe e2e_deco 53.51 vs nobeard 36.71 (serial), par4deco 45.70 vs jfr_par 30.58. One host (Xeon 8370C, 4c), one run. |
| **B. Java half of CI e2e** | **~15 ms/ch** | cont-131 (worklog-session.md:7366): "wrapper-v2 (Java-половина e2e, 15ms на CI-раннерах) — крупнейший резерв для цели e2e<=6ms". CI e2e band 20.84–22.8 ms/ch (STATUS.md cont-129: par4 22.8, band 20.84–21.91; cont-130: 21.69 48x48) minus native fill p50 5.51–6.16 ms (worklog-session.md:7307, 7359). |
| **C. DecoStats-measured DecoDriver CPU wall (2-core sandbox, CRUS_NATIVE_DECO=1)** | **3.2–4.9 ms/chunk mean, p50 1.7–2.7 ms** | prof_session_prod_deco1.txt:15 (mean 3222.0us, p50 1781.1), prof_session_deco_fast1.txt:15 (3336.5/1723.1), prof_session_dpttb8.txt:15 (3738.7/1739.8), prof_session_dpttb2.txt:15 (4931.8/2689.8). chunks=324 (window 144 + populate margin). |

Reconciliation of A (the +15 ms the wrapper actually costs e2e on CI), per cont-98
(worklog-session.md:5717-5723):

| Component of the +15.1..16.8 ms | ms/ch | Confidence |
|---|---|---|
| Native deco compute (native= per 16-ch run-prof) | 0.47 | High (cont-98:5718) |
| CarverDriver.populate in deco=1 lane (incl. ctx-pool wait) | 4.30 | High (cont-98 table:5699) |
| Deco drain (deco_tail delta, window-amortized) | ~2.0 | High (cont-98:5719-5720) |
| **Pipeline stagnation** — fills starve behind the slower decorate stage (cover 36%→26%, maxconc 4→1) | **~8** | Medium (cont-98:5720-5723, residual) |
| Sum accounted | ≈6.8 | |
| DecoDriver CPU wall itself (sandbox-measured, CI similar/lower) | ≈3–5 | Medium |

**Key structural fact:** the single largest Java-CPU item on the CI runner is *vanilla
decorate in the prod deco=0 default*, not DecoDriver: JFR par4 window →
`generateFeatures` = 55% of Java-CPU, `OreFeature.place+doPlace` = 37% (~3.3 ms/ch),
`buildTerrain` 14%, starlight 5% (worklog cont-106:5987-5990; same shape on Platinum:
53% / 34% ≈ 3.3 ms/ch, cont-103:5934). In the deco=1 CI run 41491819685 the run-top
attributed **≈14.6 ms/ch to Java OreFeature** vs ≈0.5 ms native (cont-100:5926) — this
is the prize wrapper-v2 chases: making deco=1 cheap enough to become the default and
delete Java decorate.

---

## 1. Cost model table (deco=1 path; per *decorated* chunk unless noted)

| # | Component | Evidence (file:line / run id) | ms/chunk, 2-core | ms/chunk, CI-4c | Confidence |
|---|---|---|---|---|---|
| 1 | `plan` = regionBiomes (3x3 biome union) + decoChunkPlan JNI | prod_deco1.txt:15 `plan=0.09ms`; planbio=15.15ms/324=0.047 | 0.09 | n/m | High |
| 2 | `struct` = runStructures (vanilla placeInChunk); **~2/3 phantom** = ~12 margin-chunk ReportedException walls ~20 ms each per window | prod_deco1:15 `struct=0.82ms`; struct-split `func=265.12ms` vs inner `gate+setup+loop=72.6ms`; DecoStats.java:61-66 | 0.82 mean (p50 ≪) | n/m | High (mean), High (phantom cause) |
| 3 | `run` = Σ decoRunFeature JNI wall (native compute + all upcalls + snapshot loader) | prod_deco1:15 `run=2.02ms`; deco_fast1 1.90; dpttb2 2.94 | 1.9–2.9 | ~ (in A/B) | High |
| 3a | — native-only floor of run (CRUS_DECO_NOUPCALL) | worklog:1747 (f1) `run=0.17ms` — "JNI-масса ~90% run" (f1); worklog:1777 native 0.385–0.39 (f1 full upcalls) | f1: 0.17 floor vs 0.86–0.90 with upcalls | n/m | High |
| 3b | — snapshot loader inside run (JNI ~2us + Java build 1.3–4.8us × ~63 sections/ch) | features_jni.rs:1047-1051 comment; prod_deco1 `snap-sec=20455/26.91ms`=1.32us/call; dpttb2 30537/64.42ms=2.1us/call | 63×(2+1.3..2.1)us = **0.21–0.26** EST | n/m | Medium (arithmetic) |
| 3c | — point-query upcalls inside run (106/ch measured; JNI ~2us + Java getBlockState) | prod_deco1 `upcalls=34267… other=34254` → 105.8/ch; 2us/crossing features_jni.rs:1049 | 106×(2+1..3)us = **0.32–0.53** EST | n/m | Medium (arithmetic) |
| 3d | — **per-run fixed overhead: JniLevel::new = 19× GetMethodID + ~19 CString allocs + 1× `env::var("CRUS_DECO_NOUPCALL")` + sessions_lock, on EVERY decoRunFeature** | features_jni.rs:185-259 (called per run at :1026), env::var at :257, sessions_lock at :1008; 72.9 runs/ch (features=23620/324) | 73×(19×~0.1–0.3us + ~1us) = **0.25–0.55** EST | n/m | Medium-low (arithmetic, unmeasured) |
| 4 | `apply` = Java op-apply loop (applyOpStatic → region.setBlock / section write) | prod_deco1 `apply=0.01ms`; ops=11026/324=34 ops/ch; f1: 168 ops, 1.2us/op (worklog:1778) | 0.01 (dptt) / 0.17–0.21 (f1) | n/m | High |
| 5 | `deleg` = delegateVanilla (vanilla PlacedFeature.placeWithBiomeCheck + decoDelegateMark JNI + snapshot epoch bump) | prod_deco1 `deleg=0.23ms` (8.0 del/ch); dptt_vis:15 `deleg=1.61ms` (26/ch); snap-sec 39642 vs 20455 | 0.20–1.61 (∝ delegate count) | n/m | High |
| 6 | DecoDriver wall total (populate → recordChunk) | prod_deco1 mean 3.222 / p50 1.781; dpttb2 4.93/2.69; deco_slow1 max 89.4 | 3.2–4.9 mean | ≈3–5 (inferred) | High |
| 7 | CarverDriver (deco=1 lane; ctx-pool contention incl.) | cont-98:5699 `carver=4.30`; cont-102:5930 ore-leg `carver=9.77` = ctx-wait "молотит ore-хук (250 ранов/чанк)" | n/m (0 in these sandbox files) | 4.3 (deco=1) / 9.77 (ore=1) | High |
| 8 | Deco drain / deco_tail | cont-98:5719 (+2.0); cont-123 (STATUS.md) deco-tail 221–329ms/window = 1.5–2.3 ms/ch | n/m | 1.5–2.3 | Medium |
| 9 | Pipeline stagnation (fills starve; cover 36→26%, maxconc 4→1) | cont-98:5720-5723 | n/m | ~8 | Medium (residual) |
| 10 | **Context: vanilla decorate in prod deco=0** (what wrapper-v2 replaces) | cont-103:5934 (53% Java-CPU; OreFeature 34% ≈3.3ms/ch); cont-106:5987-5990 (55%/37%); cont-100:5926 (run-top OreFeature ≈14.6ms/ch in deco=1 run) | n/m | 5–8 Java-CPU / up to 14.6 run-top | High |
| 11 | Context: noise stage native fill | prod_deco1:15 `native=12.30ms` (2-core loaded); cont-131:7359 pack-path 5.96 p50; cont-129 STATUS 6.07–6.16 | 12.3 (loaded) | 5.5–6.2 | High |

Window totals (sandbox, prod_deco1.txt:15, per 144-chunk window, 324 decorated):
ops=11026, features=23620 (72.9/ch), delegate=2588 (8.0/ch), upcalls=34267
(h=0, stateis=0, fluidis=11, isair=0, survive=2, other=34254), snap-sec=20455,
snap-hm=1766 (5.45/ch), planbio=15.15ms, struct func=265.12ms.

---

## 2. JNI crossing census (per chunk)

### 2.1 Noise stage (always, 1–3 crossings)
| Crossing | Marshalling | Ref |
|---|---|---|
| `fillBeard(ctx,cx,cz,short[98304] out, long[65536] marks, int[] beard)` ×1 | out 192KB **zero-copy** (Rust writes into pinned array); marks cap 65536×8B=512KB (≈119 marks landed/ch: marks_seen=17153/144); beard ≈ [1+8n+1+3n] ints (beard_pieces=0 in all windows) | ChunkGenNative.java:226; CrusstyChunkGenerator.java:932; ChunkGenNative.java:35-42 |
| `fillBiomes(ctx,cx,cz,int[1536] out)` ×1 (only CRUS_BIOME_GRID / BiomeSwap filler) | 6KB out (24 sections × 64 quarts) | ChunkGenNative.java:100; CrusstyChunkGenerator.java:775 |
| `fill(heightsCtx,…)` via getBaseHeight cache miss ×0–1 | 192KB out zero-copy, cached per chunk column (heightCache ≤65536) | ChunkGenNative.java:84; CrusstyChunkGenerator.java:342,326 |

### 2.2 Deco stage, deco=1 (measured dptt: **≈258 crossings/chunk**)
| Crossing | Count/ch | Marshalling | Ref |
|---|---|---|---|
| `decoDecoSeed` | 1 | 3 scalars → long | ChunkGenNative.java:160 |
| `decoChunkBegin` | 1 | 5 scalars | :166 |
| `decoChunkPlan` | 1 | String[] regionBiomes (3x3 biome union, few strings) + int[PLAN_TOTAL*2+16] out — **fresh alloc per chunk** | :146; DecoDriver.java:319-320 |
| `decoRunFeature` | **72.9** | 9 scalars + bridge jobject + `int[6*65536]` ops buffer (1.5MB/thread, pinned zero-copy via GetPrimitiveArrayCritical; only n×24B written) | :154; features_jni.rs:1143-1148; DecoDriver.java:331-335 |
| `decoDelegateMark` | 8.0 | 0 args; bumps snapshot epoch (stale-section reload storm: 26 del/ch → snap-sec 123/ch; 8/ch → 63/ch) | :177; DecoDriver.java:374; worklog:1809,1845 |
| `decoOpsDropped` | 0–73 (unmeasured) | int[ndrop*3] coords (≤12KB buf), pinned | :172; DecoDriver.java:400-402 |
| **Upcalls native→Java**: point queries (height/stateIs/fluidIs/isAir/wouldSurvive/isReplaceable/isSolid/hasSturdyFace/biomeHasFeature) | **105.8** | (III)Z scalars; stateIs/fluidIs carry String[]; h=0 since the WG-heightmap handshake | prod_deco1:15; features_jni.rs:203-223 |
| Upcall `decoSnapSection` / Range | **63.1** | (III)→byte[] ~2.6–3.2KB typical (5-bit storage: 320 longs=2560B + headers+names); Java `Arrays.copyOf` per call | DecoDriver.java:1142-1186,1345; ChunkGenNative.java:— (bridge) |
| Upcall `decoHeightmaps` | 5.45 | (II)→int[512] = 2KB (WS_WG+OF_WG, +1 semantics) | DecoDriver.java:1127-1140 |
| **Total** | **≈258** | | |

Pure crossing overhead at the measured ~2μs (features_jni.rs:1049):
258 × 2μs ≈ **0.52 ms/chunk ESTIMATE**; plus per-run fixed overhead 0.25–0.55 (row 3d)
→ **wrapper JNI mass ≈ 0.8–1.1 ms/ch ESTIMATE** (consistent with the f1 floor gap:
run 0.86–0.90 vs native-only 0.17, worklog:1747/1777).

### 2.3 Carver stage (CRUS_NATIVE_CARVERS=1, 3 crossings)
| Crossing | Marshalling | Ref |
|---|---|---|
| `carve(ctx,cx,cz,seed,int[72] meta, long[] raw, short[] pal)` | Java packs 24 sections: bits[24]+rawOff[24]+palOff[24] meta, concatenated BitStorage raw longs (8–196KB), palette tables | ChunkGenNative.java:257; CarverDriver.java:112-181 |
| `carveOps(ctx,long[n])` | n = changed cells (idx<<16|newId; thousands when carved) | :261 |
| `carveMarks(ctx,long[1024])` | ≤1024 packed BlockPos | :264 |

### 2.4 Ore hook (deco=0 + CRUS_NATIVE_ORE=1) — the rejected shape
`oreRunConfigured` ×**~250 runs/chunk**, each holding a pool ctx through the whole
native call + apply; per-run scope reset bumps snapshot epoch → snapshot-storm via
JNI; ~250 ctx-acquires hammer the same pool carvers use (cont-102:5930). Native
expansion itself is cheap (0.43–0.50 ms/ch, cont-100:5926) — the wrapper architecture
killed it (+19.1 ms/ch e2e). **Do not rebuild this shape.**

### 2.5 Setup-time only (per world, irrelevant to per-chunk)
newCtxAssets×(POOL+1), decoStates, decoPlan, decoBiomeFeaturesFull, ctxBiomeNames,
surfaceStates, surfaceSetIds, surfaceSetTemps, decoSetFlags (once).

---

## 3. Ordered lever list for wrapper-v2

### Quick wins (days; zero-to-low bit-exactness risk)

**Q1. Cache `JniLevel` method-IDs + the `CRUS_DECO_NOUPCALL` flag.**
`features_jni.rs:185-259` runs 19× `GetMethodID` + ~19 `CString` allocations + one
`std::env::var` (a process-lock + alloc) **on every `decoRunFeature` call** (~73/ch).
A `static OnceLock<BridgeMids>` + `OnceLock<bool>` (the codebase already has the
pattern: `SNAP_RANGE_ON`, features_jni.rs:1052-1054) removes it.
Expected: **0.25–0.55 ms/ch EST** (row 3d). Risk: none (same IDs, same class).
Validate: filldump bit-parity (f1/gold probe) + deco p50 A/B.

**Q2. ThreadLocal `runsBuf` + reused biome-name array for `decoChunkPlan`.**
`DecoDriver.java:319-320` allocates `int[PLAN_TOTAL*2+16]` per chunk; `regionBiomes`
builds a fresh `TreeSet`+`String[]` (DecoDriver.java:788-796). Cache both TL.
Expected: 0.02–0.05 ms/ch. Risk: none. Validate: filldump parity.

**Q3. Kill the `Arrays.copyOf` on snapshot serialization.**
`decoSnapSection0` returns `Arrays.copyOf(buf.array(), pos)` (DecoDriver.java:1345)
and the range variant copies again (:1182) — ~63 copies × ~2.7KB/ch. Return into a
preallocated thread-local byte[]/direct ByteBuffer and pass the live length.
Expected: 0.03–0.06 ms/ch. Risk: none (JNI copies out regardless).
Validate: filldump parity + snap-sec ns counter.

**Q4. Measure & batch `decoOpsDropped`.**
Every run with `ndrop>0` triggers an extra JNI + native overlay sync
(DecoDriver.java:400-402). FLAG_POSTPROC marks (DiskFeature) are drops *by design*.
First add a counter (evidence gap, §5); if frequent, piggy-back the drop triples
onto the *next* `decoRunFeature` call (new arg). Risk: medium-low — overlay must be
exact before the next run's queries (same-chunk order preserved).
Validate: marks parity (marks_seen==landed discipline, GenStats) + judge.

**Q5. Decouple deco/carver ctx contention.**
CarverDriver and DecoDriver both `decoAcquire` from DECO_POOL; fills hold ctxs
5–12 ms, so populate waits (cont-102: carver=9.77ms on the ore leg is pure ctx-wait;
4.3 ms on deco=1). Options: dedicated deco ctx pool, or tryLock-fallback-to-fresh-ctx.
Expected: up to ~4 ms/ch on CI deco=1 lanes. Risk: none for parity (timing only;
native scratch is ctx-bound — a fresh ctx just has cold caches).

**Q6. Keep the structures phase as-is** (bit-exactness: vanilla structures must stay;
already fast-gated on 3x3 references and cached step-grouping — DecoDriver.java:486-521).
The 0.82 ms mean is ~2/3 margin-chunk exception walls, not removable work.

### Structural (wrapper-v2 proper)

**S1. Sub-batch `decoRunFeature` between delegates (73 → ~9 crossings/ch).**
The master loop must stay order-exact (DecoDriver.java:72-75: ops of feature k
applied BEFORE feature k+1 expands). Group maximal native-supported run sequences
between delegate (`-2`) features; one native call per group; Java applies ops of the
group, then the delegate runs exactly where it did before. Native overlay already
models Java-applied ops (decoOpsDropped keeps it exact). Expected: per-run fixed
overhead (row 3d) collapses ×8 → **0.2–0.4 ms/ch saved**; JNI count 258→~40.
Risk: MEDIUM — ordering contract + drop-sync timing. Validate: filldump bit-parity
deco=1, ops-count equality probes (f1 probe precedent: "ops=33011 identical",
worklog:1776), ab263 deco gates, judge dptt.

**S2. Single-crossing chunk deco with Java delegate upcall mid-stream.**
Native drives the whole chunk; at a delegate position it upcalls Java
`runDelegate(step,gid)` after an `applyPending(ops)` boundary crossing (Java region
must see earlier ops before the vanilla feature reads it). JNI count → ~15–20/ch.
Expected: wrapper JNI mass → ~0.1–0.2 ms/ch. Risk: MEDIUM-HIGH (same contract, more
moving parts). **Calibration warning (cont-115:6432-6446):** on the ore leg the live
profile showed wrapper overhead ≈ 0 (loader 0.02–0.07 ms/ch) — the ceiling of S1+S2
is bounded by rows 3c+3d (≈0.6–1.1 ms), NOT by tens of ms. Build S1 first and
re-measure before S2.

**S3. Replace full-epoch snapshot invalidation with touched-section invalidation.**
Each `decoDelegateMark` bumps the epoch → ALL warm sections reload on next native
query (evidence: 26 del/ch ↔ 123 snap-sec/ch in dptt_vis; 8/ch ↔ 63 in prod_deco1;
glow_lichen fix already halved it once, worklog:1901 `snap-sec 24061→20455`).
Track per-delegate touched sections instead. Expected: snap-sec 63→~30/ch ≈
0.15–0.2 ms/ch + p99 tail cut. Risk: MEDIUM — staleness rule must stay conservative
(a section a delegate wrote must reload). Validate: filldump parity + deleg/snap-sec
counters.

**S4. The actual prize: make deco=1 the prod default (delete Java decorate).**
Even at zero wrapper overhead, the A/B delta stays ~+15 ms/ch until: carver symmetry
(Q5), drain amortization (already 0.16 ms/ch at 2304-window scale, cont-98:5709),
native compute fronts (dripstone_cluster top 1.5–3.7 ms/16ch, cont-98:5718; fused
ore per-cell queries, worklog:1778), and the ~8 ms/ch pipeline stagnation — which is
Paper scheduling physics (populate slower than fill ⇒ cover collapse), fixable only
by making populate itself cheaper (S1+S3+compute). Note the negative precedent:
batch-with-deferred-ops is UNSOUND for bit-exactness — deferring op application shifts
the block-write interleaving visible to later steps/neighbors, and Paper 26.3 has no
delegateable decorate-boundary hook (cont-115:6439-6445).

**S5. Explicitly dead: "batch all ore of a chunk into one call" (Front O).**
Closed by cont-115 with live profile: native wall 17.5–19.3 ms/ch was the whole
+19.1 delta; loader/wrapper ≈ 0; the cost was the per-cell read path (tag-bucket),
fixed separately (tag-memo cont-115, OvHasher cont-116); remaining open item is the
2–3% drift verdict, not the wrapper (worklog:6419-6446). Keep OFF.

---

## 4. Evidence gaps → follow-up perf run checklist

1. **No CI-4c DecoStats phase dump in reference/** — every `prof_session_*.txt` is the
   2-core sandbox; the CI deco numbers exist only as worklog prose (runs
   41458088635 / 41582756447 / 41585182091). Run one `e2e_ci.sh` deco=1 lane with a
   `crusgenstats` snapshot artifact so plan/run/apply/deleg are on-record for 4 cores.
2. **`decoOpsDropped` frequency** never counted (Q4 needs it) — add an AtomicLong.
3. **JniLevel::new cost** never measured directly (row 3d is arithmetic) — cheap:
   time 10⁶ calls or A/B a cached-BridgeMids build.
4. **JNI crossing μs on CI silicon** — the ~2μs figure is 2-core sandbox
   (features_jni.rs:1049); Milan/9V74 may differ.
5. **Per-feature native compute top on current HEAD** — `RUN_BY_NAME` ([run-top] tail)
   exists in DecoStats (DecoStats.java:188-202) but no saved prof_session file shows
   it; all cited tops are stale (dptt_p56-era). One CRUS_RUN_PROF=1 window would name
   the current compute hotspots for the S4 roadmap.
6. **Light/serialization share** of the CI e2e never decomposed (cont-103 stopped at
   starlight 12%); needed before declaring the wrapper the only remaining lever.

## 5. Constraints honored
- Read-only: no code, no builds, no worklog writes. This file is the only artifact.
- Every number above is file-cited; `ESTIMATE` rows show the arithmetic and the
  μs/units assumed.
