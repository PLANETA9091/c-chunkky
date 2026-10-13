# c-faster worklog

Проект: нативный генератор чанков (Rust .so) для Paper 1.21.8, цель — полный чанк за 1мс даже с датапаками, бит-в-бит паритет с ванилью (судья: reference/*.crsf, снятые с настоящей ванили).

## Статус репо на момент восстановления (2026-10-11)
В c-faster запушено только M1-M3: json.rs, worldgen.rs (реестр), noise.rs, rng.rs, md5.rs.
ВСЯ работа M4/M5 потеряна при сбросе окружения (не была запушена). Восстанавливаем.

## НЕИЗГЛАДИМЫЕ ЗНАНИЯ (проверено против декомпила настоящего Paper 1.21.8 jar)

### Инфраструктура
- Paper 1.21.8-60 — paperclip-лаунчер; настоящий mojmap jar появляется после первого запуска (прогнать с eula).
- JNI: native-метод получает JNIEnv* = ДВОЙНОЙ указатель (*env)->Fn(env). jni-sys 0.4.1: JNIEnv — сырой указатель, vtable-поля v1_1.GetArrayLength, v1_2.{Get,Release}PrimitiveArrayCritical.
- plugin.yml: load: STARTUP (иначе плагин не успевает до создания мира). nether/end отключить в bucket-конфиге.
- Paper PluginRemapper ремапит плагины; oracle-плагин с NMS-вызовами — пропускать ремап через manifest-атрибут.
- Кэш ремапа Paper может устаревать — чистить при странных ошибках.
- RCON: forceload занимает главный поток — длинный таймаут на вызов даёт зависание; лучше короткий таймаут + поллинг.
- `tail -1` лога берёт строку the_end; отключать End/Nether, брать первую строку (overworld).
- Дампы: абсолютный путь (относительный падал в неверную папку).
- Бенчмарк: один вызов Paper (выход по stdin EOF), FIFO-держатель консоли, md5-контроль деплоя .so.
- Перед каждой сверкой: убедиться что .so и jar СВЕЖИЕ (были случаи бинарников-зомби).

### JNI-контракт (заморожен)
- newCtx(seed, minY, height) / fill(ctx, cx, cz, short[98304])
- layout: blocks[(y-minY)*256 + z*16 + x], x + z*16 внутри слоя
- Плaгин берёт только NOISE-стадию, ваниль достраивает сверху. Дефолты Paper API: все стадии false — плагин сам включает ванильные стадии (surface, carvers, features, light).
- CraftChunkData LINKED-режим пишет прямо в ProtoChunk, heightmap обновляется сам (heightmapsAfter(NOISE)) — проблема heightmap решена.

### Критические багфиксы (были найдены, потеряны, применить заново)
1. **rng.rs**: next_f32/f64 — ЛОГИЧЕСКИЙ сдвиг (>>>), не арифметический (ваниль nextInt).
2. **md5.rs**: seedFromHashOf = СЫРОЙ MD5 (Guava Hashing.md5()), БЕЗ UUID-манглинга (никаких version/variant битов!). Java UUID.nameUUIDFromBytes модифицирует байты 6 (|0x30) и 8 (|0x80) — это ДРУГАЯ функция, не использовать.
3. **NormalNoise**: ванильный путь — `create` с useLegacyNetherBiome=TRUE → octave_N-путь, последовательный расход RNG от старшего индекса вниз (legacy). НЕ per-octave fromHashOf.
4. **ImprovedNoise**: lowest_freq_input_factor = 2^(-i) (i.e. 2^firstOctave масштаб), у меня была инверсия.
5. **BlendedNoise**: ванильный getOctaveNoise(i) возвращает noiseLevels[len-1-i] — ОБРАЩЁННЫЙ порядок октав.
6. **BlendedNoise/noise_5**: weirdDeltaY = d4 - d7 — y-снап вычитается из dy, НЕ из dz.
7. **BlockPos**: точная битовая распаковка (декомпилировал, применять).
8. **Сплайн**: вся арифметика f32 (включая локации и деривативы) — критично для паритета.
9. **flat_cache**: вне NoiseChunk — чистый pass-through (грид создаёт только NoiseChunk-обёртка).
10. **FMA-ловушка**: раздельные mul+add, не fmadd (расхождение с ванилью).

### Семантика ванили (подтверждено декомпилом)
- NoiseChunk: интерполяторы с углами 5x49x5, порядок лерпов Y→X→Z; кэши чистая оптимизация (значения совпадают при прямом вычислении).
- final_density через lerp3 (cell-cache путь), vein-функции через цепочку updateForY→X→Z.
- Aquifer полностью разобран. OreVeinifier: RNG на каждый блок от positional factory.
- deepslate — SURFACE-стадия (ваниль), наш fill его НЕ делает. Etалонные дампы — ПОЛНЫЕ ванильные чанки (руды, геоды, шахты — всё от ванильных стадий).
- Bukkit-путь адаптера не обновляет heightmap/не делает postprocessing-маркировку — проверено, LINKED-путь ок.
- RandomState: ServerChunkCache.randomState() публичный — оракул строится на reflection.
- Дерево сидов: upgradeSeedTo128bit константы верны; XOROSHIRO bit-в-бит совпал; consumeCount совпадает.
- PerlinNoise октавы совпали бит-в-бит (после фиксов 2-4). ImprovedNoise.noise бит-в-бит (0x3fd480ae816f25fa).
- 12/13 density-функций были бит-в-бит; последняя дивергенция — sloped_cheese/base_3d (BlendedNoise.compute) → осталось побайтово сверить вклады октав в compute. ЭТА ТОЧКА — МЕСТО ВОЗОБНОВЛЕНИЯ.

### Судейский контур
- Эталоны: 64 чанка .crsf с настоящей ванили (в репо reference/), + контрольный датапак (sea_level 63→40).
- Приём засчитывается только если % совпавших блоков не упал.
- Oracle-плагин: eval-режим, сверка density-функций вживую (RandomState идентичен ванильному).

## План восстановления (worktree = чистый M3)
- [x] clone c-faster, rustup
- [ ] Проверить rng.rs/md5.rs/noise.rs против списка багфиксов (в репо их НЕТ — применить)
- [ ] M4: density.rs (интерпретатор, f32-сплайны)
- [ ] M5: nochunk.rs (NoiseChunk порт: interpolator, aquifer, vein) + ffi.rs (JNI двойная косвенность!) + bench
- [ ] Пере-скачать Paper, декомпилировать NMS классы (для сверки)
- [ ] Oracle-плагин заново
- [ ] Сверка density бит-в-бит → финализировать BlendedNoise.compute
- [ ] Паритет эталона 64 чанков
- [ ] Оптимизация до 1мс/чанк (с датапаками)
- [ ] Коммит+ПУШ после каждого шага!

---
Task ID: cont-1 (2026-10-11, session continuation)
Agent: Super Z (main)
Task: продолжить паритет-работу; тесты перенести на GitHub CI (по требованию юзера)

Work Log:
- Проверил состояние c-faster: последний коммит = бит-в-бит все 15 density-полей против живого Java-оракула (14580/14580). Корень BlendedNoise-бага: terrain seed должен быть WORLD seed (был 0) + per-params инстансы + отсутствие MulOrAdd fold.
- worktree: мусор от прошлого прогона (dumps_plugin перезаписаны, oracle классы переименованы в OracleNoiseStagePlugin) — вычищено/восстановлено.
- ci.yml был ЦЕЛ (ранее показалось 'ain]' из-за ANSI-эскейпа в выводе).
- НОВОЕ: harness/bootstrap_server.sh — поднимает server/ с нуля (paperclip b60, eula, props с rcon+seed, bukkit.yml) для CI/свежих окружений.
- НОВОЕ: harness/judge_gate.py — гейт судьи (fail если diff% вырос).
- plugin_bench.sh: datapack-вайринг (копирует crussty-custom в world datapacks после CRUS_FRESH-вайпа).
- ci.yml: добавлен job 'integration' — на GH-раннере собирает native+plugin, бутает Paper 1.21.8, CRUS_FRESH прогон vanilla+datapack, судья diff_dumps vs reference/dumps/*, гейт diff%<=35, артефакты (логи/дампы/статы) всегда. Кэши cargo+paperclip.
- reference/dumps_plugin снят с учёта (.gitignore) — судейские эталоны в reference/dumps нетронуты.
- Локальные быстрые сборки ок: plugin jar (252b1423...), .so (release, 0.08s incremental), bench perf 8 = mean 46.5ms/чанк (нечопизированный корректный порт).

Stage Summary:
- ПАРИТЕТ-СТАТУС: density бит-в-бит (15/15 полей), блочный судья НЕ перегонялся после серии шумовых фиксов (было 35%) — теперь это делает CI.
- Все длинные тесты (бут Paper, pregen, судья) = GitHub Actions job 'integration'.
- Следующее: push → дождаться CI → по результатам tighten gate (35 → реально достигнутый), затем оптимизация 46ms → 1ms (пропуск невлияющей работы, SIMD, параллель).

---
Task ID: cont-2 (2026-10-11 evening, perf session)
Agent: Super Z (main)
Task: оптимизация нативного fill по требованию юзера (тесты на GitHub CI)

Work Log:
- CI: PaperMC v2 API SUNSET (410) -> переписал bootstrap_server.sh на Fill v3; integration job теперь полноценно гоняет судью.
- ПЕРВЫЙ РЕАЛЬНЫЙ СУДЕЙСКИЙ РЕЗУЛЬТАТ (run 38156910440, commit c30c551):
  vanilla: 95.545% блоков совпало (diff 4.455%) — было 35%; datapack: 97.813%; оба PASS (gate 35%).
  Анализ артефакта: главные диффы = water->air (аквифер!) + каскад surface-правил; сосредоточено у y=32..64 (уровень моря).
- ПРОФАЙЛЕР: нашёл баг фазовых таймеров (self.prof перезаписывался последним fill -> деление на n давало 1/16 правды). Фазы (реальные, до оптимизаций): fill_slice 6.8ms, cellfill 7.6ms, material 6.6ms, loop 1.5ms = ~23ms.
- CSE (hash-consing) при компиляции арены: 6304 -> 217 узлов! + memo per-point DAG eval (Cell<u64> stamps). 45 -> 23.6ms. 14/14 density-полей бит-в-бит.
- БАГ ПАРИТЕТА найден и исправлен: memo serial не бампался в material-пути (aquifer/vein evals) — 128 блоков ячейки делили serial -> чужие значения. Исправлено бампами per-pt во всех получателях pt (ore_vein, calculate_pressure, compute_surface_level, compute_randomized_fluid_surface_level, compute_fluid_type).
- КРИТИЧЕСКИЙ БАГ АКВИФЕРА: cache (fluid status per cell) НЕ сбрасывался между чанками, а grid origin переезжает — чанки читали чужие статусы воды/лавы! begin_chunk теперь сбрасывает и cache. Это главная причина water->air диффов судьи.
- CellFill fast path: bypass memo+eval_fresh (vanilla тоже пересчитывает diamond-узлы) 22.5 -> 18.3; затем КОМПИЛЯЦИЯ CellFill-дерева в плоскую оп-программу (CellOp, post-order, стек-машина) -> cellfill 4.2ms. Отказ от компиляции (fallback) для датапаков с сырыми шумами вне интерполяторов.
- Cross-chunk plane cache: последняя x-плоскость чанка N = первая плоскость N+1 (slice 6.8 -> 5.5ms). Баг сравнения (cell-idx vs block-x) найден через plane_hits счётчик.
- Aquifer 12-cell scan: инкрементальные индексы вместо полного index() на ячейку (порядок i10>i11>i12 сохранён — tie-break!).
- Пайплайн поток (worker на следующую x-плоскость): РЕАЛИЗОВАН, но на этой 2-ядерной машине МЕДЛЕННЕЕ (31ms vs 17ms) — спавн-оверхед + кэш-конфликты; оставлен opt-in CRUS_PIPELINE=1, default off.
- Проверки: каждый шаг — out_hash (FNV по выходному массиву) == старый билд на 8 чанках + 14/14 density-полей против Java-оракула. ВСЁ БИТ-В-БИТ.

Stage Summary:
- СКОРОСТЬ: 45ms -> 17.0ms mean на dev-box (2.65x), целиком бит-в-бит с ванильной семантикой.
- ПАРИТЕТ: судья 95.5%/97.8% (до аквифер-фикса); свежие CI-прогоны должны показать выше.
- Остатки: fill_slice 5.5ms (уникальные сэмплы шума на углах), material 5.8ms (аквифер per-block), cellfill 4.2ms, misc 1.5ms.
- Следующие шаги: дождаться CI с аквифер-фиксом (6 runs в очереди), потом AVX-512 векторизация шума (fill_slice/vein/barrier), оп-программа для Slice-пути, threads если реальное железо даст выигрыш.

---
Task ID: cont-3 (2026-10-11 night)
Agent: Super Z (main)
Task: продолжение оптимизации + CI-верификация

Work Log:
- Отменены 6 промежуточных CI-прогонов (освобождали очередь), остался новейший (f703444).
- Устранён HashMap<String> lookup шума НА КАЖДЫЙ САМПЛ (noise_resolved индексная таблица в Compiled): 17.0 -> 16.5ms.
- Two-pass cellfill (rows hoisting) — ИЗМЕРЕН, нейтрален/хуже — ОТКАТ к single-pass op-программе.
- Пайплайн-поток: opt-in CRUS_PIPELINE=1 (на 2-ядерном dev-box медленнее — 31ms).
- Финальные проверки: BYTE-IDENTICAL на 8 чанках против pre-CSE билда, 14/14 density-полей vs Java-оракул.

Stage Summary:
- 45ms -> 16.5ms (2.7x) standalone; судья 95.5%/97.8% на коммите ДО аквифер-фикса; свежий CI (с фиксом) в работе.
- Остаток времени: fill_slice ~5.0ms (нужен AVX-512 batch eval — главная цель следующей сессии), material ~5.7ms (аквифер per-block: scan+status+pressure), cellfill ~4.2ms (op-программа), misc ~1.5ms.
- К 1ms: AVX-512 (4-6x на fill_slice/cellfill/vein/barrier) + потоки (2x) + возможно пропуск невлияющей работы. Оценка: 16.5 -> ~2-4ms с SIMD+threads; 1ms потребует ещё и векторизации PerlinNoise.

---
Task ID: cont-4 (2026-10-12 night)
Agent: Super Z (main)
Task: 100% паритет — разбор 263 ванильных диффов + sea_level для датапака

Work Log:
- CI run 38160543222 (6f7f5be): vanilla 99.996% (263/6291456), datapack 95.098% (regression от 97.8%).
- Datapack regression root: натив берёт зашитые vanilla noise_settings (sea_level=63), эталон с датапаком = 40. Фикс: newCtx(seed,minY,height,seaLevel) + reflection-resolve NMS getSeaLevel() (NoiseBasedChunkGenerator через delegate-цепочку CustomChunkGenerator), кэш per-world + WorldKey(seed,sea).
- 263 vanilla диффа разобраны по позициям (scripts/diff_detail.py + analyze): 138 fluid (water->air 122, obsidian->lava 12, lava->air 4) + каскады (bubble_column 92, granite 23, kelp 10). НЕ кластерятся у границ чанков (plane cache чист).
- vanilla=water[level=4]/bubble_column/obsidian — ПОСТПРОЦЕССИНГ: LevelChunk.postProcessGeneration тикает fluidState у помеченных позиций (markPosForPostprocessing). Метки: doFill (аквифер flag && fluid), SurfaceSystem (fluid у surface), WorldGenRegion.setBlock (state.hasPostProcess — только MAGMA/MUSHROOMS!), карверы, фичи.
- Paper запускает surface/carver/feature-метки САМ (наши стадии не тронуты) — не хватает ТОЛЬКО noise-stage (aquifer) меток: shouldGenerateNoise=false выкидывает doFill с его метками.
- Порт флага должен быть точным: все ветки computeSubstance (d<=0: d>=FLOWING(-0.76)&&status1!=status; water-above-lava; финал flag/flag1/flag2 + 4-й источник l3+similarity(i6,i9)). Реализовано в nochunk.rs + marks: Vec<i64> (packed BlockPos, cap 65535) + fillMarked JNI (marks long[] + count).
- Плагин: markFluidUpdates — reflection CraftChunkData.weakChunk -> ProtoChunk.markPosForPostprocessing(BlockPos) (Paper does identical ticks at promotion).
- ВАЖНО: bubble_column образуется не из аквифера: MAGMA_BLOCK.hasPostProcess=always -> UnderwaterMagmaFeature.setBlock(magma,2) -> метка -> promotion updateFromNeighbourShapes -> MagmaBlock.updateShape(UP,water)->scheduleTick(20) -> BubbleColumnBlock.updateColumn (рост 5 тиков/блок). Это Paper делает сам; наш вклад — только aquifer-метки.
- ГИПОТЕЗА для остатка (spring-растекания water[level=2..8]): settle 2s (plugin_bench) vs 5s (bench.sh эталон) — мир пойман недотикавшим. Выровнял sleep 5.
- Тесты: marks region-test (валидность, packing round-trip, fluid-only), reset per chunk; 7/7 green; out_hash byte-identical на 5 чанках (e06a3b56... и др.).
- Убраны кэш-баги локальных скриптов (get() по имени без папки — сравнивал эталон с эталоном!), тестовые координаты world vs local.

Stage Summary:
- Натив: флаг+метки (fillMarked), newCtx + seaLevel. Плагин: NMS sea-level resolve + ProtoChunk marking bridge (reflection, fail-safe).
- CI покажет: vanilla ~100%? (settle+метки), datapack ~99.9%? (sea_level=40).
- Next: после CI — паритет до конца, затем 16.5ms -> 1ms (density-gate, SIMD, threads).

---
Task ID: cont-5 (2026-10-12 night 2)
Agent: Super Z (main)
Task: perf spurt + steady-state references

Work Log:
- cellfill: row-hoisting (dx/dy const per 4-block row -> only z-lerp per block) + fixed-array stack machine: 4.9 -> 2.6ms.
- ctx POOL in plugin: one native ctx per Paper chunk worker (tryLock scan + rr fallback) -> parallel fills, no shared mutable state.
- aquifer fast path: all 12 candidate cells share one FluidStatus -> scan provably collapses to (status.at(y), sched=false + y=-54 water-above-lava mark). BUG FOUND during dev: statuses must resolve per SOURCE POSITION (packed, rng inside cell), not cell coords — cell-coord version POISONED the cache and changed hashes; fixed via loc_cache decode on miss + cache-direct lookup. fastdiff=0 on 5 chunks. material 8.6 -> 5.0ms.
- 4-wide batch eval (eval4) for fill_slice: bit-exact lane-identical tree walk; batched ImprovedNoise.noise_5 (per-lane y_max), PerlinNoise/NormalNoise.get_value_batch, BlendedNoise.compute_batch (per-lane skip_min/skip_max). fill_slice 5.1 -> 4.5ms. ALL HASHES BYTE-IDENTICAL.
- Mean fill: 17.9 -> 12.3ms (dev box), warm 11.1ms. Phases: slice 4.5, material 5.0, cellfill 2.6, misc ~0.2.
- Pipeline auto-gate: >=4 cores ON (2-core box measured 2x WORSE), CRUS_NO_PIPELINE/CRUS_PIPELINE override.
- slicecost profile: slot 0 (final_density child) = 4.87ms of 5.5ms; tree = BLENDEDx3 noisex29 splinex9 ap2x125.
- RECATURE workflow (pure vanilla, probe plugin deployed, settle 15s): steady-state refs captured (22/22 bubble columns complete; old ref had 12/25 = mid-growth!). refs committed.
- Judge history: 8c67b2d (marks+sea): vanilla 245, datapack 180 (mid-growth refs); 12ad9e9 (fastpath): 260/183 vs old refs — fast path did NOT regress.
- KEY INSIGHT: mixed settle times make fluid-tick state INCOMPARABLE. Next CI (15s settle both sides + steady refs) = the real verdict. Local diff vs steady ref at 5s settle: 393 vanilla (ref spread further) — expected artifact.

Stage Summary:
- HEAD: steady refs + 15s settle + fast path + eval4 + pool + pipeline gate. Awaiting CI verdict (~100%?).
- Perf: 12.3ms mean (2-core), remaining: material 5.0, slice 4.5, cellfill 2.6. Next levers: material noise batching, threads-in-fill, 8-wide AVX-512.

---
Task ID: cont-7 (2026-10-12 night 3)
Agent: Super Z (main)
Task: judge regression bisect (441 -> 5903) + parallel cells + ShiftB fix

Work Log:
- Pocket analysis flipped: minijudge 4-chunk set showed ours=water[lv8] EXTRA (over-marking); refactored pocket script (scripts/pocket_analysis.py).
- FIX(aquifer): all-same fast path sched must be gated on d67>0 (vanilla d<=0 early-return => sched=false since !S.equals(S)); merged fast+slow scans into one vanilla-order pass tracking i6..i9/l0..l3 + all_same; lazy status resolution once all_same breaks. Hashes unchanged (e06a3b56...).
- PERF(material): base-cell SUMMARY cache (12-neighborhood is a pure function of the base cell): all_same verdict + uniform status memoized => material 7.9 -> 2.9ms, wall 14.9 -> 9.8ms (2-core dev). Deep water-above-lava marks resolved via targeted scan_two_smallest.
- Dead interp update_for_* calls removed from material walk (chain_value only read in Mode::Block): material 2.85 -> 2.28ms.
- PERF(slice): parallel corner evaluation (CRUS_FILL_THREADS; per-thread MemoCtx; corners are pure fns of position): slice 3.9 -> 2.35ms @2thr.
- PERF(cells): parallel x-strips over per-thread ChunkGen clones (thread_clone: fresh memo/aquifer-cells/marks; shared env/compiled(Arc)/prelim(Arc<Mutex>)/aquifer caches(Arc<Atomic*>)); planes precomputed by eval_plane_into workers; OR-merge of out buffers; marks/prof merged.
- BUG FOUND (CRITICAL, correctness): eval4 ShiftB had x/z->y/z args swapped vs vanilla ShiftNoise.compute(z,x,0): scalar get_value(z*.25, x*.25, 0) but batch put x*.25 into the Z slot and 0 into Y. All prior CI runs' pipeline corners were subtly WRONG (eval_plane_into path). Fixed + eval4-vs-eval parity unit test (8/8 green). planecmp: 0 diffs per plane after fix.
- CI archaeology: judge artifacts contain a STALE inner judge.zip (441) alongside the REAL streamed verdicts. Real: e6fe197 = 441 (99.993%); 760c518 = 5903 (99.906%); e824200 = 5906. Regression composition: ~2700 granite/diorite/andesite blob swaps (y 0..31) + kelp/seagrass cascades + the SAME ~400 water-pocket undermarks.
- Ruled out: block output (old vs new native both e06a3b56), marks (old marks 8/2 for 3,-2/2,-2 == new), sea-level, datapack config. Only functional server-path delta e6fe197->760c518: plugin beardifier bridge (collectBeard does synchronous getChunk(STRUCTURE_STARTS, create=true) neighbor re-entrancy during noise stage).
- Bisect running: 6aec118 = CRUS_BEARD gate default OFF (beard collection skipped) -> if verdict returns to ~441, beard bridge guilty.

Stage Summary:
- Perf: mean fill 14.9 -> 9.6ms (2-core dev, 2 threads); phases: slice 3.9 (2.35 @2thr), cellfill 2.7, material 2.3.
- Judge: regression 441 -> 5903 under bisect; verdicts pending (6aec118 beard-gate, 85a1382 ShiftB fix + parallel cells).
- 100%-goal plan: fix the blob/pocket residuals, then thread-scale on CI (4 cores) -> ~3ms/chunk, then SIMD/VM for the rest.

---
Task ID: cont-8 (2026-10-12 night 4)
Agent: Super Z (main)
Task: judge nondeterminism root cause + deterministic protocol

Work Log:
- Bisect results: beard bridge EXONERATED (CRUS_BEARD off = 5896 vs 5903); ShiftB fix neutral (+27); dead-chain removal neutral; blocks+marks bit-identical old-vs-new across 9 chunks (bench marks cmd: old==new 8/2/22/31).
- ROOT CAUSE of the 441 -> ~5900 judge regression: DECORATION SPILL NONDETERMINISM. Ore/blob features write into the 3x3 WorldGenRegion; the final block at contested boundary positions depends on the chunk DECORATION ORDER. Diffs cluster at chunk edges (z=12..15 in chunk_1_-3). The old 50ms/chunk generation accidentally matched vanilla's promotion order (441 = lucky); the fast parallel generation scrambled it (~5900). NOT a code bug — an judge-infrastructure nondeterminism.
- The phantom seed-0 diff (-4,1): stale-binary artifact (SIGPIPE from `| rg | head` killed cargo mid-build; ran an old binary). Fresh e2d7213 build is bit-identical to e6fe197 at seed 0 too. LESSON: never trust builds piped through head.
- FIX (judge protocol): sequential per-chunk forceload (cx asc, cz asc) in BOTH bench.sh (reference capture) and plugin_bench.sh (judge) — both worlds now decorate in the SAME order; judge settle aligned to 15s (was 5s vs refs' 15s — a second mismatch source). Refs re-captured via recapture 38286817425 and committed.
- Bisect-B revert (old native) was accidentally left on main and broke the marks-oracle step (old bench lacks the marks cmd) — restored native+plugin to e2d7213 (8d6971a).
- CRUS_FILL_THREADS=4 exported in plugin_bench (parallel slice corners + cell strips).

Stage Summary:
- Awaiting the clean judge verdict (8d6971a): new code + new refs + deterministic order. Expect granite-family spill diffs to vanish; remaining = real diffs (pockets + kelp cascades).
- Perf on CI pending with 4 fill threads.

---
Task ID: cont-9 (2026-10-13 night)
Agent: Super Z (main)
Task: SIMD-спурт до предела + возобновление судейского вердикта

Work Log:
- CI-археология: прогон 818ad9b падал на "unknown command: marks" (bisect-B бинтер без marks); реставр 8d6971a это чинил, но прогоны отменялись по concurrency. Перезапущен CI; позднее 4 параллельных integration-джобы душили друг друга (>1.5ч in_progress) — отменены все кроме последнего (ce8fba0).
- ОРАКУЛ-ПАРИТЕТ МЕТОК ПОДТВЕРЖДЁН: vanilla NMS marked_true 2/1/8/0 (2,-2/2,-3/3,-2/3,-3) == наши bench marks ровно (2,-2: marks=2, позиции 47,24,-32 / 47,23,-32 бит-в-бит).
- SIMD-фаза 1 (шум): noise_5_batch AVX2 __m256d (32 grad_dot + 3 smoothstep + 7 lerp3 векторно; перм-загрузки скалярно; строгий IEEE, БЕЗ FMA) + бит-в-бит юнит-тесты vs скаляр. Микро: 31.8 -> 18.5 нс/сэмпл (1.72x).
- SIMD-фаза 2 (slice): корень "eval4 не ускорял" — fill_slice звал СКАЛЯРНЫЙ eval+memo. Slot 0 (88% slice) переведён на eval4 + V4Memo ([f64;4] штампы per node per group — без него eval4 терял within-group DAG CSE: 6.17ms). slice 3.90 -> 2.15.
- SIMD-фаза 3 (cellfill): 4-лейн AVX2 стек-машина (dz лейн-вектор, blend short-circuit) 3.25 -> 1.79; двухстрочный раннер (пар стеков A/B) 1.79 -> 1.42; векторизованный rows-хойстинг (dx=[0,.25,.5,.75] лейны, 8 вектор-опов/слот вместо 48 лерпов, rows8 [slot][k][ix]) 1.42 -> 1.03. ДВАЖДЫ ломало хэши (layout) — поймано out_hash, исправлено.
- SIMD-фаза 4 (material): row4 fast path (uniform summary + все subst<=0 => константная строка) 2.82 -> 1.77; all-solid-row (subst>0 строкой + ore инлайн) -> 1.29.
- СТРУКТУРНЫЙ ПРОПУСК: all-solid CELL gate — все 8 углов > 0 => выпуклость трилинейки => все 128 subst>0 => cellfill ячейки скипается целиком (beard пуст) -> 0.78ms.
- eval8/AVX-512: const-generic eval_batch<N>, BatchMemo<const N>, noise_5_batch8 (AVX-512 opt-in: на SKX-классе 512-бит f64 МЕДЛЕННЕЕ 2xAVX2 — 25.5 vs 18.6 нс; даунклок), per-half skip в BlendedNoise, РАНТАЙМ-КАЛИБРОВКА pick_slice_lanes (CRUS_LANES; здесь 4: 6004 vs 7506ns).
- Итог: wall 10.15 -> 4.47ms (2.26x), @2thr 3.98. Фазы: slice 2.16 (чистый шум, латентный пол perm-цепочек), material 1.4, cellfill 0.78, other 0.16.
- Проверки после каждого шага: out_hash 6 чанков + fastdiff + 11/11 тестов. УРОК: fastdiff (seed 0) не ловит баги реального seed — гонять bench fill по реальным чанкам!

Stage Summary:
- Перф: 45 -> 10.15 -> 4.47ms single-thread (10x за проект).
- Паритет: бит-в-бит на всех стадиях; метки = NMS-оракул; судейский вердикт ждём на ce8fba0.
- Next: (1) CI-вердикт -> 100%?; (2) material mixed-строки; (3) select_cell_yz SIMD; (4) CRUS_LANES=8 на CI если калибровка выберет 8.

---
Task ID: cont-10 (2026-10-13, env reset + parity rescue)
Agent: Super Z (main)
Task: восстановление окружения, разбор судейского вердикта a17c69a (0.401%), исправление

Work Log:
- Окружение сброшено (репо/токен потеряны): reclon c-faster, rustup, токен выдан юзером заново (remote URL only).
- CI run 38344402927 (a17c69a) ЗЕЛЁНЫЙ, но судья: vanilla 25211/6291456 = 0.401% (регрессия от 441), datapack 0.389%.
- Состав диффа: 10006 air->deepslate (ВЫЕДЕННЫЕ пещеры!) + ~4700 water->solid + 1954 lava->deepslate + ~4600 blob-swaps (вторично) + kelp-каскады.
- Локальная бисекция: marks(2,-2) 2->0 ровно на 5369a08 (SIMD-спурт); fill расходится там же.
- Инструментированная локализация (cellprobe/oldprobe, дамп плоскостей x=44):
  1) Гейт ce8fba0 "all-solid cell" НЕСОСТОЯТЕЛЕН: subst = ДЕРЕВО 8 интерполированных слотов, а гейт проверял углы ТОЛЬКО fd-слота. fd-углы ячейки (0.117..0.18 > 0) при истинном subst -0.0045 (лава). -> целые клетки камня вместо пещер/лавы на CI (параллельный путь, там плоскости корректны).
  2) Серийный fill_slice (5369a08): slot0 ушёл в BatchMemo и НЕ бампал self.memo.serial; слоты 1-7 (self.eval) читали ЧУЖИЕ штампы старых точек -> плоскости слотов 1-7 мусорные в серийном режиме (bench marks/fill были неверны локально всегда после 5369a08; на CI параллельная ветка со своим MemoCtx была чиста).
- Проверка "плоскости бит-идентичны" была только по слоту 0 — потому и не ловилось.
- ФИКСЫ: гейт удалён полностью (unsound даже по всем слотам из-за сплайнов); в серийном fill_slice перед слотами 1-7 добавлен self.memo.next_serial() per corner point (протокол старого eval_fresh).
- Верификация: marks 2/1/8/0 == оракул NMS бит-в-бит; filldump 7 чанков (2,-2/0,0/1,1/-2,2/3,-3/0,2/-3,0) байт-в-байт с 8d6971a; 11/11 тестов.
- УРОК: fastdiff/out_hash на seed 0 + параллельная ветка маскировали серийный баг; гейт проверялся только хэшами тех же серийных прогонов.

Stage Summary:
- Паритет восстановлен локально; push -> CI должен вернуть судью к ~441-эре (детерминированный протокол) или лучше.
- Перф на dev-box: 12.8ms serial / 7.0ms @2thr (этот Xeon vCPU заметно медленнее прежнего бокса; re-baseline на CI).
- Next: CI-вердикт, затем перф-спурт (slice single-thread латентность, AVX-512 калибровка, threads).
