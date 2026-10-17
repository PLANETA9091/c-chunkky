# Features: контракт ванильного декорирования (decompile 1.21.8, vineflower src14)

Источник: /tmp/my-project/decomp/src14 (717 классов, mojmap, classpath paper-1.21.8.jar).
Цель: нативный features-движок этапа F по пути stage C (контракт → probe → bit-check → NMS-мост).

## 1. Мастер-цикл (ChunkGenerator.addVanillaDecorations)

- `blockPos` = origin СРЕДНЕЙ секции центрального чанка: `SectionPos.of(chunkPos, minSectionY).origin()`
  → (chunkX<<4, minSectionY, chunkZ<<4)?? нет: origin секции = (chunkX<<4, minY секции, chunkZ<<4).
- `l = worldgenRandom.setDecorationSeed(level.getSeed(), blockPos.getX(), blockPos.getZ())`.
- Биомы: объединение Holder<Biome> всех секций 3x3 чанков вокруг центра (`ChunkPos.rangeClosed(center,1)`),
  `set.retainAll(biomeSource.possibleBiomes())`.
- `max = max(GenerationStep.Decoration.values().length, featuresPerStep.size())`.
- На каждый шаг i (0..max-1), ПОРЯДОК СТРОГИЙ:
  1. Структуры этого шага (map по structure.step().ordinal()), каждая: `worldgenRandom.setFeatureSeed(l, i1, i)` где i1 — ЛОКАЛЬНЫЙ индекс структуры в шаге (0,1,2..), затем placeInChunk в writable area.
  2. Если i < size: IntSet индексов фич = union по всем биомам set из `generationSettings.features()[i]`, каждый holder → `stepFeatureData.indexMapping().applyAsInt(feature)` (глобальный индекс placed feature в шаге). set1 ОТСОРТИРОВАТЬ по возрастанию.
  3. Каждый i3 по возрастанию:
     - Paper: `configFeatureSeed = paperConfig().featureSeeds.features.getLong(placedFeature.feature())`; если != -1 → `featurePopulationSeed = worldgenRandom.setDecorationSeed(configFeatureSeed, x, z)` (СЖИГАЕТ 4 xoroshiro-next(32)! порядок потоков не меняется, но seed другой).
     - `worldgenRandom.setFeatureSeed(featurePopulationSeed, i3, i)`.
     - `placedFeature.placeWithBiomeCheck(level, generator, worldgenRandom, blockPos)`.
- Один общий WorldgenRandom на весь цикл; каждый фича-индекс ре-сидится, НО внутри placeWithContext
  random ТРЕДИНГ-СКВОЗНОЙ через все модификаторы и все позиции (порядок flatMap = порядок потребления RNG).

## 2. RNG-протокол (бит-в-бит)

WorldgenRandom extends LegacyRandomSource, оборачивает XoroshiroRandomSource (unique-seed wrapper,
setSeed делегирует внутрь; next(bits) override: (int)(xoro.nextLong() >>> (64-bits))).

- `setDecorationSeed(levelSeed, minX, minZ)`:
  setSeed(levelSeed) → xoro reseeds = Xoroshiro128PlusPlus(upgradeSeedTo128bit(levelSeed));
  a = WRnextLong()|1; b = WRnextLong()|1;  // WRnextLong = next(32)<<32 + next(32) — ДВА вызова xoro.nextLong на каждый, итого 4
  seed = minX*a + minZ*b ^ levelSeed; setSeed(seed); вернуть seed.
- `setFeatureSeed(decoSeed, index, step)`: setSeed(decoSeed + index + 10000L*step).
- `upgradeSeedTo128bit(s)`: lo = s ^ 7640891576956012809 (SILVER); hi = lo + (-7046029254386353131) (GOLDEN);
  затем mixStafford13 обоих: x=(x^(x>>30))*(-4658895280553007687); x=(x^(x>>27))*(-7723592293110705685); x^=x>>31.
- Xoroshiro128PlusPlus.nextLong: l2 = rotl(lo+hi,17)+lo; hi^=lo; lo=rotl(l,49)^hi^(hi<<21); hi=rotl(hi,28); вернуть l2.
  Конструктор: если (lo|hi)==0 → lo=GOLDEN, hi=SILVER.
- XoroshiroRandomSource:
  - nextInt() = (int)nextLong();
  - nextInt(bound): Lemire c rejection: u=toUnsignedLong(nextInt()); m=u*bound; low=m&0xFFFFFFFF;
    if low<bound: rejectThreshold = 2^32 % bound = remainderUnsigned(~bound+1,bound);
    while (low < rejectThreshold) { u=toUnsignedLong(nextInt()); m=u*bound; } вернуть (int)(m>>32).
    (декомпил пишет for с телом переназначения u,m и l2=l1&0xFFFFFFFF в инкременте — эквивалент while.)
  - nextLong = xoro.nextLong(); nextBoolean = nextLong&1;
  - nextFloat = (float)(nextLong()>>>(64-24)) * 5.9604645E-8F;
  - nextDouble = (double)((float)(nextLong()>>>(64-53)) * 1.110223E-16F)  ← приведение к float СНАЧАЛА (квантизация!), потом double;
  - consumeCount(n) = n × nextLong.
- LegacyRandomSource (для LegacyRandomSource-wrapped вызовов, напр. populate): LCG mod 2^48:
  next(size): s1=(s*25214903917+11)&(2^48-1); вернуть (int)(s1>>(48-size)). setSeed: (seed^25214903917)&mask.
  nextInt(bound) power-of-two: bound*next(31)>>31; иначе rejection mod. nextLong = next(32)<<32 + next(32).
- MarsagliaPolarGaussian (nextGaussian) — пока НЕ нужен ни одному топ-типу переписи; реализовать при первом появлении.

## 3. Цепочка placement-модификаторов

placeWithBiomeCheck → placeWithContext: stream=[origin]; для КАЖДОГО модификатора по порядку:
stream = stream.flatMap(pos -> modifier.getPositions(ctx, random, pos)). Затем на каждую позицию
configuredFeature.place(level, generator, random, pos) — random тот же сквозной.

- InSquarePlacement: x=pos.x+nextInt(16), z=pos.z+nextInt(16) (X ПОТОМ Z), y наследуется.
- CountPlacement(RepeatingPlacement): n=count.sample(random); позиции = n×та же pos (flatMap → n копий).
- RarityFilter(PlacementFilter.shouldPlace): nextFloat() < 1/chance.
- BiomeFilter: biome(level,pos) ∈ hasFeature(topFeature) — topFeature из PlacementContext (только placeWithBiomeCheck несёт его; plain place → empty → IllegalStateException).
- HeightmapPlacement(type): h=ctx.getHeight(type,x,z); вернуть пусто если h<=ctx.getMinY(), иначе pos.y=h.
- HeightRangePlacement: pos.atY(height.sample(random, ctx)) — UniformHeight/TrapezoidHeight + VerticalAnchor (см. heightproviders).
- RandomOffsetPlacement: x+=xz.sample(); y+=y.sample(); z+=xz.sample() — xz семплится ДВАЖДЫ (X потом Z).
- BlockPredicateFilter: shouldPlace = predicate.matches(level,pos) (blockpredicates/, без RNG если нет random внутри).
- SurfaceWaterDepthFilter: WORLD_SURFACE-OCEAN_FLOOR <= maxWaterDepth.
- EnvironmentScanPlacement: скан вверх/вниз по направлению + predicate filter (см. файл).
- CountOnEveryLayerPlacement (@Deprecated, встречается): do-while по слоям: на слой n=count.sample, на каждую: x=nextInt(16), z=nextInt(16), h=MOTION_BLOCKING, findOnGroundYPosition (пусто над не-пусто, не bedrock, i==layer → y+1); повторяет слои пока что-то нашлось.
- NoiseBasedCountPlacement: count = ceil((Biome.BIOME_INFO_NOISE.getValue(x/noiseFactor, z/noiseFactor,false)+noiseOffset)*noiseToCountRatio) — Perlin без RNG.
- NoiseThresholdCountPlacement: аналогично через порог.
- FixedPlacement / SurfaceRelativeThresholdFilter: см. файлы.
- PlacementContext.getHeight → level.getHeight (proto-чанк heightmaps ЧУЖИХ чанков 3x3!), getBlockState → чужие чанки.

## 4. Feature.place и запись блоков

- `Feature.place(config, level, gen, random, origin)` = `level.ensureCanWrite(origin) && feature.place(ctx)`; ctx.topFeature=empty.
- `ensureCanWrite`: |secX-centerX|<=writeRadius && |secZ-centerZ|<=writeRadius (decoration step = 1 → писать можно во ВСЕ 9 чанков 3x3!); при upgrading ещё isOutsideBuildHeight(y).
- `WorldGenRegion.setBlock(pos,state,flags)`: ensureCanWrite → chunk.setBlockState(pos,state,flags) → hasBlockEntity: для proto (LEVELED) — dummy NBT {x,y,z,id:"DUMMY"} или removeBlockEntity; hasPostProcess && !(flags&16) → markPosForPostprocessing.
- Features пишут с flags=2 (обновление клиентам не нужно в gen; 2 = send to clients в обычном мире, в gen игнорируется кроме флагов 16/…); scheduleTick идёт в proto ticks.
- `WorldGenRegion.getHeight(type,x,z)` = chunk(чужой).getHeight(type,x&15,z&15)+1 — ЧИТАЕТ heightmap proto-чанка (у нас уже бит-точны: HEIGHTMAP 0/16384).
- getBiome(pos) — quart-биом; у нас натив-биомы уже живые.

## 5. Staged план (по переписи dptt: 538 unique placed, 1713 биомов)

Порядок по (refs, сложность) — «сначала длинный хвост простых»:
- F1 placement-ядро + NoOpFeature + simple_block (1421 ref) + replace_single_block: RNG-ядро,
  модификаторы InSquare/Count/Rarity/BiomeFilter/Heightmap/HeightRange/RandomOffset/BlockPredicate/
  SurfaceWaterDepth, stateproviders (simple/rotated/noise/weighted), canSurvive.
- F2 random_selector (3071) + random_patch (1228, = feature loop + tries через CountOnEveryLayer?)
  + simple_random_selector (217) + random_boolean_selector: диспетчеры — чистый RNG-протокол.
- F3 ore (1675, scattered+обычный): pools + size/discard chance + vein RNG.
- F4 disk (590) + block_column (15) + block_blob/basalt.
- F5 tree/fallen_tree (683+235): trunk/foliage/decorators — самый большой; после него vegetation_patch (728).
- F6 хвост: vines/multiface/gеоде/dripstone/bamboo/spring/nether_* и пр.
Пропуск: carvers отсутствуют в dptt (carvers:null у Tectonic).
Мост: shouldGenerateDecorations=false в нашем CrusstyChunkGenerator + вызов нативного
applyDecorations(ctx, cx, cz) с палитрой блоков (bdataById из stage C) и записью через
CraftChunkData-подобный буфер. Чужие чанки 3x3 для height/block-запросов — уже есть в pool-сессии.

## 6. Верификация

- Референс-оракул: vanilla-place per-feature (judge gate 0.05% наш локальный с 10x запасом).
- per-feature probe: включать по одному типу, дифф по изменённым блокам; сначала на probe/datapacks/crussty-custom, потом dptt.
- Критерий каждой стадии: judge_dptt доля не растёт, HEIGHTMAP 0/16384, CRUS_* kill-switch.
