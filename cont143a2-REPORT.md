# cont-143a2 — window-time java CPU: attribution + levers

Task: reduce WINDOW-TIME JAVA CPU in the chunk-gen path on Paper Common Worker #0/#1.
Branch `cont143-structgrid`, worktree `/home/z/my-project/wt_cont143a`.
JFR source: `reference/jfr_cont143/jfr_print.txt` (windows = startTime >= 17:11:00;
2 windows x 144 inside chunks = 288; **1 sample ≈ 10 ms java CPU ⇒ ms/ch = n/288*10**).

Parser: `agent-scratch/cont143a2_jfr_attr.py` (+ `_tail.py`), full output preserved in
`agent-scratch/cont143a2_attr_out.txt`. Window cut reproduces 870 samples total,
746 on the two chunk workers.

## 1. Reproduced window-time attribution (workers only, ms/ch = n/288*10)

| whale (leaf-first) | my samples | ms/ch | brief | owner / call path (walked) |
|---|---|---|---|---|
| layoutBulk (any frame, leaf lines 1719×14 1708/1711/1542/1543) | **31** | **1.08** | 28≈1.0 | OURS: `ChunkStatusTasks.buildTerrain:109 → NoiseBasedChunkGenerator.buildTerrain:395 → CustomChunkGenerator.fillFromNoise → CrusstyChunkGenerator.generateNoise:1028 → layout:1039 → layoutBulk` |
| save-path (walk-up: `NewChunkHolder.saveChunk → SerializableChunkData.write`) | **57** | **1.98** | ≈1.1 | Paper: PaletteReadoutCache.readout 10, SimpleBitStorage.<init> 6, ListTag.getIdToWrite 4(7 w/ server-th), PalettedContainer.get/pack ~8, moonrise FastNBT CompoundTag.writeWithoutId ~5, RecordCodecBuilder/codec glue ~20 scattered |
| streams/spliterator (~34 in brief) | **36** | **1.25** | ≈1.2 | **ATTRIBUTED: vanilla** — every sample (workers) sits under `WorldGenRegion.<init>:139 → EnvironmentAttributeSystem.Builder.addStaticLayers → addBiomeLayer` (ReferencePipeline FlatMap + IntStream.range spliterators + fastutil ObjectArrayList spliterator), re-run PER STATUS STEP: generateBiomes 10, generateSpawn 8, generateFeatures 7, buildTerrain 7, generateStructureReferences 4. NOT our DecoDriver (0 samples) |
| NoiseStack$Perlin.get (19) | **19** | **0.66** | 19 | **8 = GeodeFeature.place:136 crack noise** (via DecoDriver.delegateVanilla:659 — agent B's geode port removes); **11 = vanilla aquifer ctor**: `NoiseChunk.<init>:47 → Aquifer$NoiseBasedAquifer.<init>:159 → maxSurfaceLevel:181` inside `createNoiseChunk` (java-side surface pre-pass, runs before our JNI fill; not plugin-reachable) |
| GeodeFeature.place leaf | 9 | 0.31 | 13 | agent B (PORT3 geode+lakes) |
| StarLightEngine.performLightIncrease | 13 | 0.45 | 14 | Paper starlight INIT_LIGHT/LIGHT steps (no plugin hook) |
| StaticCache2D get/init + WorldGenRegion.lambda$new$2 | 15+8 | 0.80 | 8 | vanilla StaticCache2D per WorldGenRegion |
| CarverDriver.populate | 3 | 0.10 | 6 | ours, small |
| ForwardLogHandler.publish (workers) | 1 | 0.03 | 6 | logging; harness rate-log, not plugin per-chunk |

Differences vs brief: my save-path walk-up counts **57 (1.98 ms/ch)** — the brief's 32
undercounted the scattered codec glue; my layoutBulk-any-frame is 31 (1.08). Trust mine
(same file, stricter walk-up). NoiseStack-19 and streams-34 fully attributed as above.

## 2. Chosen levers

* **L1 layoutBulk fast pack (OURS — implement, env `CRUS_LAYOUT_FAST`, default OFF=old):**
  hot code = 3 full 4096-passes per non-air section (any-scan 1542, distinct-collect 1582,
  pack 1699) + per-block 3 flag-array loads (matTicking/matFluidTicking/matSpecial,
  lines 1708/1711/1714) + per-section `Constructor.newInstance` (SimpleBitStorage 1719,
  Data 1729) + per-section `Method.invoke` palette create. JFR samples at 1719 are
  safepoint-biased out of the counted pack loops (classic pattern) — fix targets the loop
  work AND the reflection: fuse scan1+scan2, single-read `palIdxLoc[id]` (drop 2 loads),
  fold 3 flag arrays → 1 byte[] (drop 2 loads), ticking-list indices collected during pack
  (drop 4th scan), LambdaMetafactory-spun ctor factories (no Object[]+boxing per call).
  Est. cut 0.3–0.6 ms/ch java.
* **L2 save-path — REFUTED (plugin side):** Paper already runs its fast path
  (moonrise `FastNBTSerialiser`/`writeWithoutId`, `PaletteReadoutCache`); remaining
  1.98 ms/ch is intrinsic pack+encode+NBT on the worker pool, triggered by
  unload-on-ticket-drop during waves. Plugin cannot (a) change serialization without
  breaking region byte-compat (HARD CONSTRAINT), (b) defer saves without holding every
  wave chunk loaded (memory + moves CPU, doesn't cut it — measurement gaming if windows
  only). Needs a Paper-fork patch (out of repo scope). Numbers above; not chased.
* **L3 streams/envattr — REFUTED (plugin side):** 36 samples are vanilla
  `EnvironmentAttributeSystem.addStaticLayers` re-run on EVERY `WorldGenRegion`
  construction (one per status step per chunk, 5 constructions/chunk). A Bukkit plugin
  cannot patch vanilla classes (no mixin); fix belongs to a Paper fork.
  Documented for the campaign; not chased.

NoiseStack-19: 8 = geode (agent B), 11 = vanilla aquifer ctor — neither reachable from
the plugin; JNI crossing shrink (short[98304] memcpy ≈ 10–20 µs vs 1 ms layoutBulk)
refuted as <0.1 ms/ch.

## 3. Implementation

(filled in after implementation — see sections below)

## 4. Headless evidence

(filled in after implementation)

## 5. Verdicts

(filled in after implementation)

## 3. Implementation — ORCHESTRATOR TAKEOVER + VERDICT (cont-143)

Agent stream died post-implementation; orchestrator took over build/fix/measure.
Three launch bugs found and fixed on the branch:
1. findConstructor with Object-return MethodType → NoSuchMethodException on JDK 25
   (must be void-return ctor descriptor; switched to unreflectConstructor over the
   legacy-resolved ctors).
2. generic T erasure made invokeExact() type as ()Object vs ()Sam
   WrongMethodTypeException → invoke() with explicit cast.
3. metafactory samMethodType must be the SAM's ERASED abstract type
   ((Object,Object,Object)Object), not the specialization — otherwise the lambda
   implements only the bridge → AbstractMethodError on make() (seen in prod as
   swapfail).

## 4. Headless/e2e evidence (server paired A/B, same jar per leg, WIPE, rect 11700)

Leg OFF (legacy): honest 52.14, layout=0.27ms, bulk=ok, swapfail=null.
Leg ON attempt 3 (engaged): **swapfail=ArrayIndexOutOfBoundsException:22@2270**
AND **bulk verify CORRUPTION: want=AIR got=DEEPSLATE at x=15 y=-43 z=14**,
layout mean inflated 4.76ms (global path glob=4.01), sections filled=5075
(global-path wholesale swaps), honest e2e 59.75 (worse), max chunk 383ms.
The production verify caught the wrong-block write — bit-exactness VIOLATED.

## 5. Verdict: L1 = REJECTED (UNSAFE)

The fast pack writes wrong blocks in the >15-distinct global path and throws AIOOBE
in the linear path on real terrain. Bit-exactness is a HARD gate: the branch is NOT
merged; kill-switch default OFF keeps main safe. Code preserved on branch
cont143-structgrid (commit history: 035f07b4 + spin fixes) for future reference.
Estimated win (0.3-0.6ms/ch) does not justify the correctness risk without a full
rewrite of the global/linear packing paths with per-section verify.
