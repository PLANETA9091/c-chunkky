# 26.3 densityfunction layer — port design (cont-48 entry point)

Status after cont-47/47b: noise core (n263.rs) bit-exact vs Paper 26.3 b218, 61/61 tests.
This doc freezes the density-function layer: ALL op semantics read from reference/dec263
(2026-10-24, cont-48) + utils decompiled into reference/dec263_util/ (Mth, Interval,
CubicSpline, BoundedFloatFunction, TerrainProvider).

## Architecture (26.3) — read and frozen

* Tree: `DensityFunction` (nodes) -> `compileSampler(CompileContext)` -> `DensitySampler`
  (executable). Compiler pipeline (DensityFunctionCompiler.getSampler):
  1. optimizerRule = sequence(INLINE_REFERENCE + cache-dedup, SLICE_UNIFORM_AXES(7));
  2. optimized.compileSampler(ctx).
* INLINE_REFERENCE: HolderHolder -> value. Cache dedup: CacheFunction nodes with EQUAL
  input share one PreparedCache(id) -> CachingDensitySampler(id, inputSampler) — memo per
  unique cached subtree. cacheCells array in SamplerContext grows by 16.
* SLICE_UNIFORM_AXES(7): descend tree; when a subtree's domainAxes ⊂ parent's, wrap with
  SliceFunction(X|Y|Z, 0, f) for each missing axis (VALUE-NOOP — f ignores that axis;
  pure perf: broadcasts 1 column). Skip Constant/Gradient. Axis order: X, then Z, then Y.
* Volume model: DensityVolume(sizeX,Y,Z, minBlock*, stepBlock*); buffer index =
  `y + (x + z*sizeX)*sizeY` (Y fastest). Buffers float[], arena-pooled (ScopedDensityBuffer).
* SamplerContext: userFields (Blender/Beardifier context keys) + bufferArena + caches flag.
  CacheCell = { last volume, last buffer, last valueKey(BlockPos.asLong), last value }.
  sampleVolumeCached: if volume equals last -> copyFrom; else recompute into cell buffer.
  sampleValueCached: point inside last volume -> read from buffer; else last-point memo;
  else compute. EMPTY_UNCACHED: caches disabled (pass-through).

## Op semantics (ALL f32 unless noted; point path `sV`, volume path `sVol`)

Binary (add/sub/mul/div/min/max) — compile folding: const left/right -> ConstSampler
(add commutes, sub-const-left = c-x, div-const-right = x*(1/c) via ConstMul);
MIN/MAX collapse whole node to left/right when ranges disjoint (warn). Volume: left fills
out, right in scoped buffer, combine loop (sub: addTo(-right)).
Point shortcuts: mul/div `left==0.0F -> 0.0F`; min: `left <= rightRange.min -> left`;
max: `left >= rightRange.max -> left`. ConstMin/ConstMax volume: `if (c < buf[i]) set(c)` /
`if (c > buf[i]) set(c)`.
Unary 11: abs, square(x*x), cube(x*x*x), sqrt(Mth.sqrt=(float)Math.sqrt(double)),
half_negative/quarter_negative (LeakyReLU x>0?x:x*0.5|0.25), reciprocal(1/x), negate,
squeeze(clamp±1/2 - cube/24), log((float)Math.log(double)), sign(Math.signum).
Volume: in-place elementwise.
Clamp: Mth.clamp(v,min,max) = v<min?min:Math.min(v,max).
Lerp: alpha==0 -> first; alpha==1 -> second; else Mth.lerp = p0 + a*(p1-p0). Const
first/second folding. Volume: alpha in out, first/second in scoped buffers.
Pow: (float)Math.pow((double)base,(double)exp). Const exponent: |e|=0.5/1/2/3 -> sqrt/id/
square/cube (neg e -> Reciprocal wrapper). Const base -> ConstBaseSampler.
Round (floor/round/ceil/truncate), multiple: `multiple==0?input:roundToInt(in/mul)*mul`;
multiple==1.0F -> IntegerMultiple (roundToInt: floor=floor(d), round=Math.round(f),
ceil=ceil(d), truncate = x>0?floor:ceil).
RangeChoice: in-range = `v>=min && v<max`; volume: whenInRange fills out, overwrite with
whenOutOfRange where out. (ConstSampler decomp quirk when BOTH branches const — verify by
javap if ever hit; overworld never has both-const.)
IntervalSelect: input < thresholds[i] -> functions[i] else last; single-threshold special.
Volume: input + ALL branch buffers, then select.
Cache: compile throws if reached before dedup (we model via shared sampler directly).
BlendDensity: Blender from context; empty -> identity (our default path).
Slice(axis,c): point -> input at fixed coord; volume: if volume IS the slice pass through;
else eval input on thin volume (1-wide in axis) and broadcast (Y contiguous: setRange;
XzSampler merges X+Z slices).
Gradient(axis, tiling, from,to, fV,tV): factor = (tV-fV)/(float)(to-from) at compile.
ClampedSampler: rel = clamp(coord,min(from,to))-from; fV + rel*factor. Repeat:
floorMod(rel,range); Mirrored: tile flip. yClampedGradient = Y + CLAMP_TO_EDGE.
Interpolated: FROZEN in cont-47c (volume = cell corners, Z->X->Y-additive; point =
2x2x2 corners + Mth.lerp3(x,y,z alphas); corner-direct shortcut).
FindTopSurface(density, upperBound, lowerBound, cellHeight): compile wraps ENTIRE sampler
in YSampler(0). findSurface: topY = floor(upperBound/cellHeight)*cellHeight; if topY<=lower
-> lower; else probe from topY down by cellHeight until density>0 (first probe wins);
else lower. Point: upperBound sampled at (x,y,z) given (y ignored via wrap).
Spline: f32 THROUGHOUT (locations[], derivatives[], nested values; range() f32 too).
sample: input=coord.apply(c); binarySearch(0,len, i->input<loc[i])-1; <0 or ==last:
linearExtend (der==0? value : value+der*(input-loc[idx])); else x1,x2,t=(in-x1)/(x2-x1),
a=d1*(x2-x1)-(y2-y1), b=-d2*(x2-x1)+(y2-y1), res=lerp(t,y1,y2)+t*(1-t)*lerp(t,a,b).
Volume: per-coordinate buffers cached, per-index apply.
Noise(noise,xzScale,yScale,shiftX,shiftY,shiftZ): point = noise.get(x*xz+sx, y*ys+sy,
z*xz+sz) — scales/sums in f64; shifts are DFs. No shifts -> Sampler(noise); shiftY zero ->
ShiftedXz. Volume: shift buffers filled, then per-point noise.get with f64 adds.
Noise.addToVolume(buffer, vol, xz, ys, amp): per-point buffer.addTo(amp*get(...)) — no
separate volume kernel (same math as point).
ShiftA: ConstMul(Sampler(noise,0.25,0.0),4). ShiftB: noise.get(z*0.25, x*0.25, 0)*4 —
TRANSPOSED (volume: transposed thin buffer, setRange per column). Shift: ConstMul(Sampler(
noise,0.25,0.25),4). range = noise.range * 4. domainAxes ShiftA/B = 5 (X|Z).
SimpleDensityFunction: blend_alpha (ctx Blender.ALPHA_KEY, default 1.0), blend_offset
(OFFSET_KEY, 0.0), beardifier (Beardifier.CONTEXT_KEY, 0.0). Ranges: [0,1], INF, Beardifier.
EndIslands: SimplexNoise(random, true) after consumeCount(17292); NOT needed for overworld
(end only). DistanceToPoint: DistanceMetric.EUCLIDEAN etc. (end main island).
Constant: trivial. remap(): factor=(toMax-toMin)/(fromMax-fromMin),
offset=toMin-fromMin*factor; offset==0 -> mul(factor) else mul+add (BUILD-time float).

## NoiseRouter 26.3 (8 fields)

temperature, vegetation, continents, erosion, depth, ridges(=weirdness), chunkSurfaceLevel,
finalDensity. barrier/fluid/vein GONE; preliminary_surface_level lives in Aquifer.Config.

## Overworld tree (NoiseRouterData.registerTerrainNoises, non-amplified)

* y = yClampedGradient(-4064, 4062, -4064.0, 4062.0); shiftX = cache(shiftA(SHIFT noise));
  shiftZ = cache(shiftB(SHIFT)); temperature/vegetation = shiftedNoise2d(shiftX,shiftZ,
  0.25, noise) = Noise(noise, 0.25, 0.0, shiftX, ZERO, shiftZ); continents/erosion/ridges =
  cache(shiftedNoise2d(...)); ridges_folded = peaksAndValleys(ridges) =
  mul(add(add(abs(ridges), -0.6666667), abs) - 0.33333334, -3.0).
* jaggedNoise = noise(JAGGED, 1500.0, 0.0) (xz=1500, y=0).
* offset = cache(lerp(blend_alpha, blend_offset(0), add(const(-0.50375),
  spline(TerrainProvider.overworldOffset(c,e,ridges,false))))) — blendAlpha=1 normally ->
  lerp(1, 0, X) = X (alpha==1 shortcut; but keep node!).
* factor = cache(lerp(blend_alpha, const(10.0), spline(overworldFactor(...))));
  depth = add(yClampedGradient(-64, 320, 1.5, -1.5), offset);
  jaggedness = cache(mul(spline(overworldJaggedness(...)), halfNegative(jaggedNoise)));
  initialDensity = noiseGradientDensity(factor, add(depth, jaggedness)) =
  mul(quarterNegative(mul(depth+jag, factor)), 4.0).
* slopedCheese = cache(add(initialDensity, base_3d_noise)) where base_3d_noise =
  BlendedNoise(0.25, 0.125, 80.0, 160.0, 8.0) (xzScale 0.25, yScale 0.125, xzFreq 80,
  yFreq 160, 8 octaves via FBM smeared).
* preliminary_surface_level = findTopSurface(density, upperBound, -64, 8):
  upperBound = clamp(remap(div(0.2734375, factor) - offset, 1.5,-1.5,-64,320), -40, 320);
  density = add(slideOverworld(add(noiseGradientDensity(factor, offsetToDepth(offset)),
  -0.703125).clamp(-64,64)), -0.390625);
  chunk_surface_level = interpolated(preliminary_surface_level, 16, 1).
* entrances = cache(min(bigEntrances = noise(CAVE_ENTRANCE,0.75,0.5)+0.37+
  yGradient(-10,30,0.3,0), add(spaghettiRoughness, clamp(max(wrap3d(SPA_3D_1),
  wrap3d(SPA_3D_2)) + mappedNoise(SPA_3D_THICKNESS,-0.065,-0.088), -1, 1))));
  spaghettiRoughness = cache(mappedNoise(SPA_ROUGH_MOD,0,-0.1) * (abs(noise(SPA_ROUGH))-0.4)).
  wrapRarity3d: intervalSelect(noise(SPA_3D_RARITY,2,1), [-0.5,0,0.5],
  [noise*0.75 |1.0|1.5|2.0] where noise(n,r)=noise(n, 1/r, 1/r)*r).abs().
* caves = rangeChoice(slopedCheese, -1e6, 1.5625, min(slopedCheese, entrances*5),
  underground(...)); underground = max(min(min(layerized+solidCheese, entrances),
  spaghetti2D+roughness), pillars) with:
  layerized = square(noise(CAVE_LAYER, 8.0))*4; solidCheese = clamp(noise(CAVE_CHEESE,
  0.666..)+0.27, -1,1) + clamp(slopedCheese*(-0.64)+1.5, 0, 0.5);
  spaghetti2D = max(caveNoise, layerRidged).clamp(-1,1) where caveNoise=wrap2d(SPA_2D)+
  thickMod*0.083, layerRidged=abs(cache(mappedNoise(SPA_2D_ELEV,0,floorDiv(-64,8),8.0))+
  yGrad(-64,320,8,-40))+thickMod; cube op; thickMod = cached mappedNoise(SPA_2D_THICK,
  2.0,1.0,-0.6,-1.3); wrap2d: intervalSelect(noise(SPA_2D_MODULATOR,2,1),
  [-0.75,-0.5,0.5,0.75], [n*0.5|0.75|1|2|3]).abs();
  pillars = rangeChoice(cache(mul(noise(PILLAR,25,0.3)*2 + mappedNoise(PILLAR_RARENESS,0,-2),
  cube(mappedNoise(PILLAR_THICKNESS,0,1.1)))), -1e6, 0.03, -1e6, pillars).
* finalDensity = add(min(postProcess(slideOverworld(caves), 4, 8), NOODLE), beardifier);
  postProcess(slide, cxz, cy) = squeeze(interpolated(mul(blendDensity(slide), 0.64), cxz, cy));
  slideOverworld = slide(caves, -64, 384, 80, 64, -0.078125, 0, 24, 0.1171875):
  lerp(yGrad(-64,-40,0,1), 0.1171875, lerp(yGrad(240,256,1,0), -0.078125, caves));
  NOODLE = rangeChoice(yLimInt(Y, noise(NOODLE,1,1), -60,320,-1,4,8), -1e6, 0, 64,
  noodleThickness + max(abs(yLimInt(noise(NOODLE_RIDGE_A,2.6666..))),abs(...B...))*1.5)
  where noodleThickness = yLimInt(mappedNoise(NOODLE_THICKNESS,1,1,-0.05,-0.1),...,4,8);
  yLimInt(y, f, min, max, out, 4, 8) = interpolated(rangeChoice(y, min, max+1, f, const(out)), 4, 8).

TerrainProvider splines: structurally IDENTICAL to 1.21.8 (same points/derivatives), all
f32; amplified transformers only. overworldOffset continents knots: -1.1:0.044,
-1.02:-0.2222, -0.51:-0.2222, -0.44:-0.12, -0.18:-0.12, -0.16:beach, -0.15:beach,
-0.1:low, 0.25:mid, 1.0:high; erosion sub-splines via buildErosionOffsetSpline + ridge
splines (see TerrainProvider.java in dec263_util — port point-by-point).

## Compiler folding (Interval) — needed to mirror compiled shape

Interval: f32 min/max; add/sub/mul(mulBound: 0*anything=0)/div=mul(left,recip(right))/
min/max/clamp/abs/square/pow/lerp/mapMonotonic/encapsulating; NaI NaN-poisoning; constants
ZERO_TO_ONE / NEGATIVE_ONE_TO_ONE canonicalization in of(). MIN/MAX collapse + remap()
are the only value-affecting uses.

## Noise parameters (vanilla263/data/minecraft/worldgen/noise/*.json)

Format 26.3: {base_amplitude: f64, base_octave: i32, octave_count: i32,
amplitude_modifiers: f64[] (optional)}. NormalNoise263 (n263.rs) already implements
modifier+halving semantics (cont-47b). Per-noise random = root_factory.fromHashOf(
"minecraft:<name>") (Noises.instantiate; same scheme as 1.21.8 worldgen.rs).

## Oracle & goldens

oracle263/RouterOracle263.java: bootStrap -> BuiltInRegistries.DENSITY_FUNCTION keys
(minecraft:overworld/*) -> NoiseRouter -> RandomState.create(noises, SEED, false, stone,
63, router). Dumps (reference/oracle263_df.txt):
* pt <name> <x> <y> <z> <hexbits>  — point grids via EMPTY_UNCACHED;
* vol <name> <cx> <cz> <index> <hexbits> — full-chunk volume (16x384x16) via cached ctx.
Rust df263.rs mirrors tree + compiler; goldens compared bit-exact (f32.to_bits).

## Rust plan (native/src/df263.rs)

enum Df263 (Rc-based DAG); Sampler263 trait { sample_volume(ctx, buf, vol),
sample_value(ctx, x,y,z) }; Arena263 buffers; CacheCtx263 (cells per id);
compile-from-tree with same folding; TerrainProvider263 splines; overworld router builder;
noise params table (transcribed); goldens vs oracle263_df.txt. Integration into fill +
worldgen.rs-263 JSON loader = next steps (roadmap items 3+).

## STATUS cont-48: ENGINE BIT-EXACT vs Paper 26.3

df263.rs + df263_router.rs: 432/432 point samples + 3,538,944/3,538,944 volume samples
(22 full-chunk fills, 18 router functions incl. final_density) bit-identical to the
compiled vanilla router (oracle263/RouterOracle263.java, seed 3257840388504953787).
Binary: native/src/bin/df263golden.rs (goldens: reference/oracle263_df.txt full /
oracle263_df_strided.txt committed). Runtime 3.2s release for 22 chunk volumes.
Known residual: 149,376 signed-zero-only diffs (all jaggedness y*0.0 underflow,
-0.0 vs +0.0; inert for generation since -0.0>0 == 0.0>0).
Bugs found by the goldens (all fixed): (1) BlendedNoise 26.3 scales z by xzMultiplier
(compiled = NoiseFunction.Samplers, unlike 1.21.8 get()); (2) FindTopSurface volume
path must emulate its YSampler(0) compile wrap; (3) elevation noise yScale 0.0 (NOT -8);
(4) cave_layer/cave_cheese 2-arg noise = yScale (JSON: xz 1.0 / y 8.0|0.667);
(5) rarity branch scales must divide in f64 (1.0/(double)rarity).

## Reading queue — DONE (cont-48)

1. InterpolatedFunction (cont-47c) ✅ 2. Slice/TilingMode ✅ 3. DensityFunctions ✅
4. DensityFunctionCompiler ✅ 5. NoiseRouterData ✅ 6. FindTopSurface ✅ 7. NoiseRouter/
RandomState ✅ 8. material/MaterialSystem — NEXT (surface step) 9. moonrise chunk_gen —
with fill wiring. Utils: Mth/Interval/CubicSpline/TerrainProvider ✅ (dec263_util).

## Stage F-263 scoping — features/deco (cont-56c, decompile-verified)

Prize: features = 16-23% of gen-worker Java time (JFR cont-52b). Engine legacy
(stage F1, 1.21.8) готов: features_*.rs ~16k строк, CRUS_NATIVE_DECO opt-in,
DecoDriver handshake. Ниже — что изменилось в 26.3 и план адаптации.

### Deltas 1.21.8 -> 26.3

1. Placement driver переписан (placement/FeaturePlacer.java): рекурсивный lazy-stream
   pull -> итеративный DFS на явных стеках positions+modifierIndices; модификаторы
   теперь consumer-based: modify(ctx, random, pos, Consumer<BlockPos>). ТЕОРЕМА
   (трасса на 3 модификаторах): реверс-пуш даёт порядок feature.place идентичный
   lazy-pull (p,q,r,s обоими способами) — при сохранении per-modifier порядка
   выдачи. Модель порядка легаси (expand_rec, features_placement.rs) остаётся
   валидной; рекурсивный драйвер менять не нужно.
2. Feature JSON schema ПЛОСКАЯ: 1.21.8 {"type","config":{}} -> 26.3 {"type",...config
   inline...} (feature/ore_dirt.json). Данные: reference/vanilla263 — 235 feature /
   273 placed_feature JSON. features_json.rs нужен 26.3-режим (strict unknown->skip).
3. placed_feature schema БЕЗ ИЗМЕНЕНИЙ; PlacementContext той же формы.
4. addVanillaDecorations (ChunkGenerator.java:388): та же структура шагов x
   sorted-index; сидирование не изменилось (setDecorationSeed(level.getSeed(),
   origin.x, origin.z); setFeatureSeed на фичу; paper featureSeeds override есть).
   Дельты: FeaturePlacer-инстанс per call; possibleBiomes из 3x3 секций (GlobalPalette
   спец-кейс) + retainAll(biomeSource.possibleBiomes()); debugVoidTerrain guard.
5. GenerationStep.Decoration: те же 11 шагов.
6. НОВЫЙ тип placement: CuboidPlacement (registered "cuboid", класс в feature/,
   ванильными данными не используется, но датапак-видим -> нужен для dptt).
7. Использование модификаторов в ванильных данных 26.3: biome 211, count 201,
   in_square 198, block_predicate_filter 155, heightmap 111, offset 93, height_range 86,
   rarity_filter 55, surface_water_depth_filter 26, environment_scan 12,
   count_on_every_layer 8, noise_threshold_count 6, noise_based_count 4,
   surface_relative_threshold_filter 3, fixed_placement 1. Не используются ванилью:
   random_chance, randomly_selected, repeating, cuboid.
8. Покрытие легаси: ядро RNG-модификаторов в features_placement.rs (InSquare, Count,
   RarityFilter, RandomOffset, HeightRange); level-модификаторы в features_deco.rs
   (SurfaceWaterDepth, SurfaceRelativeThreshold, NoiseBasedCount, ...). Гэпы:
   count_on_every_layer (runtime), random_chance / randomly_selected / repeating /
   cuboid; для "json-only" набора сверить наличие runtime-реализаций.
9. Структуры: CRUS_NATIVE_DECO гейт выключает addVanillaDecorations ЦЕЛИКОМ
   (структуры И фичи) — F-263 должен решить: ре-ранн structuresByStep внутри
   DecoDriver или пере-скоупить гейт (сначала проверить форму гейта в Paper 26.3
   CustomChunkGenerator).

### Build order (предложение)

1. Данные: 26.3 vanilla feature/placed JSON -> features_vanilla_data (нормализация
   плоской схемы во внутренний Cfg; strict unknown->skip как в 1.21.8-загрузчике).
2. Modifier parity sweep: декомпил-сверка каждого из 15 используемых модификаторов
   (порядок RNG-дров per modifier); реализовать count_on_every_layer + cuboid
   (+ random_chance/randomly_selected/repeating для полноты реестра).
3. Драйвер: рекурсивная модель порядка легаси остаётся; голдены ChainRef.java +
   новые 26.3-голдены на эквивалентность итеративному стеку.
4. Handshake: DecoDriver/FeaturesMaster против NMS 26.3 (Identifier-ререйны,
   Registry.get вместо getHolderOrThrow); политики структур; kill-switch
   CRUS_NATIVE_DECO_263.
5. E2E: третья пара ab263_ci.sh `deco` (CRUS_NATIVE_DECO_263=0/1); deco-бакеты гейта
   уже обкатаны на surface/carvers парах.
