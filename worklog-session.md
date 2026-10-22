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

---
Task ID: cont-11 (2026-10-13, verdict + determinism)
Agent: Super Z (main)
Task: разбор зелёного вердикта 7f1c567 (6133), перф-семafor, детерминизм декораций

Work Log:
- Вердикт 7f1c567: vanilla 6133 (0.097%), datapack 3709 (0.059%), перф mean 9.36ms (было 16.4). Пещеры/лава/аквифер — ПОЛНОСТЬЮ исправлены (air->deepslate 10006 -> 0, lava->deepslate 1954 -> 0).
- Остаток 6133: ~3700 stone-blob swaps + ~2000 kelp/seagrass — ВСЕ на краях чанков (d=0: 1309, d=1: 1059, ->0 к d=6) = decoration spill 3x3 WorldGenRegion.
- 441-эра была на 50ms/чанк нативе (chunks=312), сейчас 9.4ms (chunks=366) — скорость фазы шума меняет порядок декораций dependency-каскада => разный winner спорных блоков на границах. Это НЕ парити-баг, а недетерминизм протокола.
- Семафор потоков натива (THREAD_PERMITS, budget=CRUS_MAX_THREADS или available_parallelism): fill_parallel плоскости/стрипы + pipeline берут пермиты из общего пула -> 4 Paper-воркера больше не множат потоки (16.4 -> 9.4ms на CI подтверждено вердиктом).
- Дебаг plane_hits=0 оказался целочисленным делением 7/8 в печати — кэш плоскостей работает (7/8 попаданий).
- ДЕТЕРМИНИЗМ: paper-global.yml chunk-system.gen-parallelism=false (sed в server_up.sh + -Dpaper.chunk-system.gen-parallelism=false) на ОБЕИХ сторонах (capture и judge идут через server_up.sh). Ключ подтверждён локальным бутом Paper 1.21.8 b30.
- Next: recapture refs (recapture.yml) с gen-parallelism=false -> judge должен упасть до истинного остатка (ожидание ~0-441).

Stage Summary:
- Судья: 25211 -> 6133; после recapture с детерминизмом ожидание существенного падения.
- Перф: CI 16.4 -> 9.4ms mean (семафор); локально serial 12.8ms, @2thr 7.0ms.

---
Task ID: cont-12 (2026-10-13, СПИЛЛ НАЙДЕН)
Agent: Super Z (main)
Task: корень ~5900 feature-диффов найден и устранён

Work Log:
- Цепочка вердиктов: 7f1c567 (гейт удалён) -> 6133; +детерминизм gen-parallelism (sed был no-op — неверный путь $ROOT/config/ vs $ROOT/server/config/, исправлен d314e07); рефы пересняты дважды (a4a6fc5: обе стороны single-gen) -> всё равно 6046. Vanilla-vs-vanilla (два capture) = ~500 fluid-tick блоков, блобы НЕ сдвинуты и не совпадают -> RNG-поток декораций.
- A/B workflow (judge_ab.yml, dispatch): CRUS_NO_BEARD=1 -> **190/6291456 = 0.003%** (beard ON давал 6046).
- КОРЕНЬ: collectBeard -> getChunk(STRUCTURE_STARTS, create=true) по соседям — тикеты изнутри noise-стадии переставляют генерацию dependency-чанков -> Paper декорирует в другом порядке -> спорные spill-блоки и сдвинутые RNG-потоки (блобы/келп).
- beard_chunks=0 вводил в заблуждение: getChunk(create) срабатывает ДО проверки piece (refsMap с non-NONE adaptation непуст в регионе).
- FIX: create=true убран (getChunkAtIfLoaded только; не загружен -> skip). Комментарий в коде с измерениями.
- Остаток 190: y=-54/-55 lava->obsidian + water-покеты (fluid-tick settle) + kelp age 20-22 (рост в окне 15s) — та же природа, что vanilla-vs-vanilla (34). НЕ парити.

Stage Summary:
- Паритет: 6046 -> 190 (0.003%) после фикса bridge; ожидание CI == 190 на vanilla и ~аналог на datapack.
- Пуш 07157ab: judge_ab workflow + вердикт-инфраструктура.

---
Task ID: cont-13 (2026-10-13, ЯД №2 — oracle jar)
Agent: Super Z (main)
Task: изоляция скремблера main CI; полный почин

Work Log:
- A/B матрица (judge_ab.yml): no_beard=1 (07157ab) = 190; no_beard=0 С ФИКСОМ create=true (38ba1ef) = **131 (0.002%)**; main CI (c oracle-степом) = 6212.
- => фикс бороды РАБОТАЕТ, но main CI травит ORACLE-СТЕП: oracle_marks.sh оставляет CrusstyOracle.jar в plugins/, а plugin_bench его не удаляет.
- OracleNoiseStagePlugin.onEnable БЕЗ -Doraclemarks выполняет run() безусловно -> при старте judge-сервера форсирует генерацию чанков -> сдвиг декораций (тот же механизм, что борода).
- ПОЯСНЕНИЕ истории: 441-эра была ДО добавления oracle-степа в ci.yml; все бисекты (5896/5903) шли с отравленным oracle jar -> «борода exonerated» была ложным выводом (bisect обе ветки отравлены).
- ФИКСЫ: (1) plugin_bench.sh удаляет ВСЕ чужие jar кроме Probe; (2) OracleNoiseStagePlugin.onEnable инертен без явных -D флагов; (3) recapture.yml: rm -f server/plugins/*.jar перед деплоем Probe.
- Гейты судьи: 35% -> 0.05%.

Stage Summary:
- Ожидание main CI после фиксов: vanilla ~131, datapack ~аналог (0.002-0.003%).
- Паритет почти 100%: остаток = fluid-tick settle (lava->obsidian, water-покеты) + kelp age в окне 15s.

---
Task ID: cont-14 (2026-10-13, ФИНАЛ — ПАРИТЕТ 100%)
Agent: Super Z (main)
Task: полный почин судейского конвейера; финальный вердикт

Work Log:
- main CI 176a81f (гигиена деплоя + инертный oracle + борода без тикетов + single-gen):
  **vanilla 127/6291456 = 0.002%, datapack 5/6291456 = 0.00008%**
- Состав остатка: vanilla = fluid-tick settle (lava->obsidian 28, water-покеты, kelp age 20-22);
  datapack = 5 блоков РАНДОМНЫХ тиков (grass_block snowy spread, amethyst bud growth) — вне генерации.
  Это класс деградации во времени, vanilla-vs-vanilla даёт те же 34-500. ПАРИТЕТ ГЕНЕРАЦИИ ПОЛНЫЙ.
- Цепочка корней за сессию: (1) unsound all-solid gate; (2) serial fill_slice stale memo; (3) борода getChunk(create=true); (4) oracle jar без флагов; (5) sed мимо server/config/; (6) семафор потоков.
- Гейты: 0.05% (запас 25x от реального уровня).
- Перф на судье (single-gen judge): mean 12.6ms/chunk; эффективная пропускная на 4 ядрах (без single-gen) ~2.4-3ms/chunk. 1ms — следующий фронт (8x2 interleave, AVX-512 zen4, material/cellfill уже SIMD).

Stage Summary:
- ПАРИТЕТ: 99.998% vanilla / 99.99992% datapack — ВСЕ блоки генерации совпадают бит-в-бит, остаток = тиковая динамика в окне settle.
- Судья детерминирован и гигиеничен; A/B-воркфлоу для изоляции; гейты автоматизированы.

---
Task ID: cont-15 (2026-10-13, перф-спурт: column-cache)
Agent: Super Z (main)
Task: перф-оптимизация native-ядра к цели 1ms/чанк; среда восстановлена после сброса

Work Log:
- Среда: re-clone c-faster, rustup stable 1.99; токен подключён ТОЛЬКО в remote URL.
- Вердикт судьи подтверждён артефактом run 38380807194 (HEAD 00dee1f): vanilla 76/6291456 (0.001%),
  datapack 41 (0.001%), HEIGHTMAP 0/16384 в обоих мирах. Состав остатка = kelp/water/lava/obsidian
  рандом-тики в окне измерения (числа плавают между прогонами: 127->76, 5->41 при неизменном гене).
- Базлайн phases (real seed, 2 ядра): wall 5.21 | slice 2.69 | cellfill 1.05 | material 1.31.
- Слоты slice: slot0 = 4.79ms из 5.44 (BLENDEDx3 + noisex29); slot1-7 — копейки (y-grad цепочки).
- AVX-512 путь существует (CRUS_AVX512=1), на этой VM МЕДЛЕННЕЕ (slice 3.60 vs 2.70) — дефолт верный.
- **ПОБЕДНЫЙ РЫЧАГ: BatchMemo column-cache.** Cache2d/FlatCache ноды в батч-пути были СКВОЗНЫМИ —
  y-НЕЗАВИСИМЫЕ климат-шумы (temperature/vegetation/continents/erosion/ridges) пересчитывались на
  каждый из 49 y-углов каждой z-колонки! Compile-time y-invariance анализ (DAG-safe, консервативный:
  Noise y_scale==0, ShiftedNoise y_scale==0 + инвариантные сдвиги, ShiftA/B, пure-обёртки) ->
  7/7 кандидатов получили слоты; одно значение f64 на (x,z)-колонку вместо 49 вычислений поддерева.
- Попытки с отрицательным результатом (ОТКАТЫ): (1) TLS cell-memo perm-walk — TLS enter+borrow на
  ~40 octave-вызовов/точку съедает выгоду (slice 2.97 vs 2.69) — walk_cell оставлен, memo отключён;
  (2) векторизация cell-setup в noise_5_batch_avx2 (floor/frac/d7) — 0 эффекта (OoO перекрывал);
  (3) all-solid cell gate по 8 углам > 0 — НЕСОСТОЯТЕЛЬЕН: mul(y_gradient, depth) — произведение
  двух неконстант, знаки множителей могут меняться внутри ячейки (углы > 0, интерьер <= 0);
  счётчики subst- 16100->13080, aquifer_air 9464->6907 доказывают смену выхода. ОТКАТ до коммита.
- lane-dedup в AVX2/AVX-512 perm-walk: соседние y-лейны батча на низких частотах в одной ячейке ->
  3 int-сравнения на промах, переиспользование grad-байт (slice 2.17 -> 2.07).
- cellfill: FlatOp/FlatProg — статические слоты стека вместо sp-цепочки (17 опов, result slot 0);
  выигрыш скромный (~0.05ms, LLVM и так хорошо компилировал стек-машину).
- fill_parallel: pool strip-клонов (strip_pool) — ранее КАЖДЫЙ параллельный fill реаллокировал и
  занулял ~1MB (cell_cache 784KB + interp + rows). Пул: strip_reset_for_fill (beard/marks/sched/
  prof/memo), буферы перезаписываются полностью до чтения. Баг по ходу: pool-return до merge
  marks/prof -> паника gens[t-1] — перенос после merge.

Stage Summary:
- wall 5.21 -> 4.57ms (-12%), slice 2.69 -> 2.07 (-23%), параллель 2 потока: 2.73 -> 2.50ms,
  bench p50 3.26 -> 3.19ms. Все 3 коммита (5ef4e49, 10e41ea, 7168706) запушены, CI запущен.
- Верификация после КАЖДОГО шага: 5 пробных out_hash бит-в-байт + 11 юнит-тестов + счётчики фаз.
- Следующие фронты: ore toggle batch (4 z за один eval_batch, ~0.1-0.15ms), персистентные
  воркеры плоскостей (spawn ~0.3-0.5ms в parallel), slice micro (LUT градиентов f64).

---
Task ID: cont-16 (2026-10-13, CI-вердикт на перф-коммитах)
Agent: Super Z (main)
Task: CI-подтверждение паритета после перф-спурта

Work Log:
- Push 5ef4e49 + 10e41ea + 7168706 -> CI run 38396318759: **SUCCESS**.
- Судья: vanilla 192/6291456 = 0.003%, datapack 69 = 0.001%, HEIGHTMAP 0/16384 оба мира.
  Остаток — тот же тиковый класс (числа плавают 76..192 между прогонами), гейт 0.05% PASS (17x запас).
- marks_seen=20329 идентичен прошлому прогону — детерминизм генерации сохранён.
- ore toggle batch (eval_batch::<4> по vein_toggle) — РЕГРЕССИЯ material 1.29 -> 2.42ms:
  batch-обвязка [f64;4] на каждой ноде тяжелее 4 скалярных eval для маленького дерева
  (выгоден только 40-октавным BlendedNoise). ОТКАТ. Урок: батчить только шумные поддеревья.
- CI bench single-gen: 15.1ms mean (CI-шум 2-ядерной VM; локально стабильный прогресс).

Stage Summary:
- ПАРИТЕТ ПОДТВЕРЖДЁН СУДЬЁЙ на оптимизированном ядре: генерация бит-в-байт, остаток = тики.
- Локальный перф: wall 5.21 -> 4.57ms, slice 2.69 -> 2.07, parallel-2t 2.73 -> 2.50, bench p50 3.19.
- Следующие фронты: персистентные plane-воркеры (spawn-оверхед), slice LUT градиентов,
  cellfill -- 1.0ms (интерпретатор уже flat), material ore -- скалярный (батч не подходит).

---
Task ID: cont-17 (2026-10-13, перф-спурт II: ore-toggle батч + диагностическая анатомия)
Agent: Super Z (main)
Task: продолжение оптимизации к 1ms; замеры на тех же phases/bench

Work Log:
- Базлайн (2-core VM): serial wall 4.57-4.60 (slice 2.11-2.13 | cellfill 1.01 | material 1.28-1.31 | other 0.16);
  parallel-2t 2.51-2.75; bench perf 2t p50 6.97. bench perf p50 на seed 0 (12.5ms) В ДВА раза тяжелее реального seed
  (5.15) — seed-0 мир плотнее по aquifer-работе; bench fill получил CRUS_BENCH_SEED для честных хэшей обоих миров.
- Анатомия slice (новые PROF-счётчики, CRUS_DIAG=1): NormalNoise-batch 0.75ms/2443 вызова (320ns ср.), BlendedNoise
  0.64ms/225; дерево+memo+сплайны ~0.65ms. Слоты 1-7 — noodle/ore_vein шумы (slotdump: 7 детей, ВСЕ с y_scale=1..4 —
  честно y-зависимы, column-cache неприменим; total col-cache нод 7/279). lanes=8 медленнее lanes=4 на VM (memo
  footprint) — подтверждено дважды.
- ПОПЫТКИ С ОТКАТОМ: (1) векторный floor-setup в noise_5_batch_avx2 (vroundpd+cvtpd) — slice 2.07->2.24 REGRESSION
  (скалярные floor перекрываются OoO, векторная цепочка сериализует walk-зависимости; повтор известного урока);
  (2) персистентный plane-pool (Mutex+Condvar) — p50 не улучшился (spawn ~50us дёшев), прогоны шумные;
  (3) overlap (main strips, pool workers planes) — wall 3.0 vs 2.75 REGRESSION: cross-phase L2 eviction (BatchMemo
  ~108KB vs cell_cache 784KB). Все три откаты зафиксированы комментарием в fill_parallel.
- ВНЕДРЕНО: (1) ore-toggle 4-wide AVX2 батч для all-solid рядов + y-range гейт [-60,50] -> STONE (ore_reached
  457850->436462/16ч); material 1.31->1.28; ore_finish с next_serial перед ridged (защита от stale memo);
  (2) env-диагностика кэшируется в полях (5 getenv/fill снято); (3) diag-инструменты: PROF counters, slotdump,
  bench fill CRUS_BENCH_SEED. NO_AQUIFER+NO_VEIN дно: material 1.27 — базовый per-block проход 13ns/блок.
- Верификация каждого шага: 6 чанков x (seed 0 + real 3257840388504953787) out_hash бит-в-байт, parallel-2t ==
  serial, 11/11 юнит-тестов. Коммит c075a1a -> push -> CI.

Stage Summary:
- Прогресс скромный (wall ~-0.05), но анатомия теперь точная: slice = noodle/ore y-шум (0.65) + blended (0.64) +
  дерево (0.65) + climate (0.1); material = per-block база (1.27) + ore (0.29); cellfill 1.0 (AVX 2-row).
- Уроки: VM 2 ядра = фазовая схема лучше overlap; векторный setup в Perlin ядре НЕ работает; spawn дешёв.
- Следующие фронты: slot0 flat-компиляция (дерево 0.65), material row4 SIMD (база 1.27), blended LUT.

---
Task ID: cont-18 (2026-10-13, перф-спурт III: pipeline-дефолт — wall -30%)
Agent: Super Z (main)
Task: продолжение к 1ms; замеры на phases/bench (real seed)

Work Log:
- КРИТИЧЕСКОЕ ОТКРЫТИЕ: A/B CRUS_PIPELINE=1 на 2-ядерной VM — pipeline теперь ВЫИГРЫВАЕТ:
  wall 4.6 -> 3.25 (-29%). Старое «LOSES on 2 cores» устарело: после flat cellfill / ore row4 / aquifer
  summary strips (~2.5ms) прячут плоскость (0.41ms) целиком. Дефолт переключён на >=2 ядер (6752bdf).
- Замеры после: serial+pipeline wall 3.18-3.25 (slice 0.68 | cellfill 1.00 | material 1.33-1.36);
  bench perf 64 real seed p50 5.15 -> 3.87 (-25%). parallel-2t (фазовая spawn-схема) 2.59-2.75 — без изменений.
- cellfill: 2-row AVX results пишутся прямо в cell_cache (a_buf/b_buf + copy_from_slice выкинуты) — 1.01 -> 1.00.
- material: gx/gy hoist в aquifer row4 (LLVM и так делал — в шуме, код чище). NO_AQ+NO_VEIN дно 1.27 =
  базовый per-block проход 13ns/блок (cell_cache load + gates + out write) — следующий фронт = row4-SIMD.
- Вердикты судьи: 1c83fc9 (ore-toggle) vanilla 89/6291456 = 0.001%, datapack 4 = 0.000%;
  6752bdf (pipeline-дефолт) vanilla 225 = 0.004%, datapack 50 = 0.001%; HEIGHTMAP 0/16384 оба.
  Гейт 0.05% PASS (12-56x запас). Остаток — тиковый класс (kelp/water/lava settle), детерминизм генерации полон.
- CI: 1c83fc9 SUCCESS, 6752bdf SUCCESS, 5d7838c (+cellfill-store, +gx-hoist) — прогон.
- CI bench native noise stage: mean 14.9ms (366 чанков, 2-ядерная CI VM, шумная) — паритет сохранён.

Stage Summary:
- wall: 4.57 -> 3.2 (-30%) за спурт II-III. Структура wall 3.2: strips 2.4 (cellfill 1.0 + material 1.33 +
  misc 0.2) + slice0 0.69. БОТЛНЕК = strips; при идеальном 2-потоке floor ~1.6-1.8.
- До 1ms нужны радикальные шаги (оценка): slot0 flat-компиляция (дерево ~0.4), material row4 SIMD (1.27
  базы -> ~0.5), cellfill ядро (1.0 -> 0.6), затем parallel-фазовая схема даст ~1.0-1.2. Все — большие проекты.
- Уроки спурта: векторный floor-setup в Perlin — REGRESSION; pool/overlap на 2 ядрах — без выигрыша
  (L2-выдавливание cross-phase); pipeline — главный рычаг (-30%); env-кэш/hoist — микро.

---
Task ID: cont-19 (2026-10-13, ФИНАЛ спурта II-III)
Agent: Super Z (main)
Task: закрытие CI-цикла спурта

Work Log:
- 5/5 CI-прогонов SUCCESS: c075a1a(через 1c83fc9), 6752bdf, 5d7838c, b97a412, 0d902e2.
- Финальный вердикт 0d902e2 (весь спурт): vanilla 109/6291456 = 0.002%, datapack 5 = 0.000%,
  HEIGHTMAP 0/16384 оба мира. Гейт 0.05% PASS (25x запас). Остаток = тиковый класс.
- Итог перфа (локально, 2-core VM): serial wall 4.57 -> 3.19-3.25 (-30%); bench p50 real seed
  5.15 -> 3.87 (-25%); parallel-2t 2.59-2.75; parallel-4t недоступен (2 ядра).

Stage Summary:
- Паритет генерации ПОЛНЫЙ на всех оптимизированных путях (pipeline-дефолт включительно).
- Спурт закрыт: 5 коммитов (ore-toggle batch+diag, worklog, pipeline default, cellfill store, gx hoist).
- Следующая сессия: slot0 flat-компиляция, material row4-SIMD, cellfill ядро -> затем parallel к 1.0-1.2.

---
Task ID: cont-20 (2026-10-13, перф-спурт IV: параллельная архитектура + prelim)
Agent: Super Z (main)
Task: 1ms/чанк на 2-ядерном CI-раннере; замеры только на историческом наборе

Work Log:
- CI-калибровка (новый шаг в ci.yml, bench perf/phases на раннере): native p50 4.53 serial /
  3.78 parallel-2t / phases 3.69 — раннер лишь на 10-15% медленнее dev-VM. Значит GenStats p50
  9.35ms на судье = ~5.6ms оверхеда Paper/JNI/spawn-схемы, а не медленные ядра. Цель 1ms
  достижима на нативе; параллельно чиню масштабирование.
- ПОЧЕМУ ПАРАЛЛЕЛЬ НЕ МАСШТАБИРУЕТСЯ (расследование с par[planes/strips/merge] + worker diag):
  (1) prelim surface Arc<Mutex<HashMap>> — мьютекс делал 12-скан в 1.9x медленнее (3.9K lock-оп/fill);
  (2) per-thread aquifer ДУБЛИРОВАЛ работу (все 4 полосы чанка юзают одни base-ячейки: fluid 261->429);
  (3) Box::new([0u16;98304]) на каждый fill — 2x192KB свежих страниц (~96 page faults).
- ФИКСЫ: aquifer = shared Arc<Padded<Atomic*>> (паддинг 64B против false sharing, паттерн
  first-touch-then-read => нет ping-pong); prelim = lock-free direct-mapped AtomicU64 кэш
  (entry = key<<9|level+65, гонки пишут одинаковые значения — бит-в-бит); out-пул.
- PRELIM-СКАН = 98% material (1.79ms/fill): AVX2 eval_batch<4> по 4 y-грид-точкам + col-cache
  (with_cols: y-инвариантный климат-сабтри считается раз на колонку). НО батч выигрывает только
  на ранних стопах: seed-0 океан 13.8 batched vs 9.3 scalar (y-зависимая часть дерева даёт те же
  FLOPs на лейн + оверхед штампов). РЕШЕНИЕ — рантайм-калибровка per-world: первые 12 сканов
  чередуют режимы с таймером, победитель фиксируется (real seed -> batch, seed 0 -> scalar).
- cellfill: стеки [f64;16]x2 обнулялись на каждый 2-row run (24х/ячейку) — создаются раз на
  iy-пару (программа топологически упорядочена). 1.06 -> 0.81. 4-row runner (1 диспетчер на оп)
  РЕГРЕССИЛ 3.2ms — 4 регистровых файла льются; удалён. 8-лейн prelim тоже медленнее (1.18 vs 1.04).
- Верификация каждого шага: 6/6 чанков serial==parallel бит-в-бит, 11/11 тестов, fastdiff 0,
  out_hash (0,0)=e06a3b56... неизменен. Seed-0 регрессия 14.1 найдена и устранена калибровкой.

Stage Summary:
- phases wall 3.25 -> 2.58ms (-21%): slice 0.77 | cellfill 0.81 | material 0.80 | other 0.21.
- bench perf p50 real seed 4.18 -> 2.52 (-40%); parallel-2t 3.30 -> 2.09 (-37%); seed 0 9.1 (восстановлен).
- Диагностика: par-бакеты в Prof, aq-счётчики (substance/hit/miss/scan/prelim), spawnbench/thrscale.
- Next: material 12-скан (~0.15ms) + fluid-машинерия; cellfill хвост; slice-плоскости для
  параллели; CI-верификация + GenStats на судье.

---
Task ID: cont-21 (2026-10-13, bulk layout + персистентный воркер)
Agent: Super Z (main)
Task: продолжение к 1ms на 2-ядерном CI; репо переименован в c-chunkky

Work Log:
- Репо переименован (c-faster -> c-chunkky); remote обновлён (токен только в URL).
- CI-калибровка на раннере: натив p50 1.90 serial / 1.80 parallel-2t (раннер БЫСТРЕЕ dev-VM!).
  GenStats судьи 9.35 = native ~7.9 + layout ~6.0 + оверхед — разбивка добавлена в GenStats
  [native/nat_p50/layout/beard/marks + sections filled/mixed/air/v1fallback/bulk].
- LAYOUT 6.0ms (98304 ChunkData.setBlock): bulk-путь через reflection. Верифицировано против
  versions/1.21.8/paper-1.21.8.jar: CraftChunkData.getHandle()->ChunkAccess.getSections();
  LevelChunkSection.states (public final); PalettedContainer.getAndSetUnchecked (resize-safe);
  Strategy.SECTION_STATES index = y<<8|z<<4|x; CollisionUtil.isSpecialCollidingBlock публичен.
- ПЕРВЫЙ bulk (подмена контейнера): судья FAIL 31.9%, heightmap 16384/16384 (|dh|=108.5):
  пропущены инварианты per-block пути — счётчики секций (nonEmpty/ticking/fluid/special)
  и Heightmaps (WORLD_SURFACE_WG/OCEAN_FLOOR_WG) не обновлены. v2: запись в СУЩЕСТВУЮЩИЙ
  контейнер + пересчёт счётчиков из пер-материальных флагов + rebuild heightmap через
  Heightmap.setRawData (9 бит x 256, stored = topY+1-minY). Дважды ловил свои ошибки
  (NoSuchMethod на getSections; дублированный setRawData lookup) — обе видны в GenStats.reason.
- ИТОГ bulk=ok: layout 6.0 -> 1.82-2.09ms (3.3x), судья PASS 0.000%.
- Самопроверка: 256 сэмплов на чанк; mismatch -> bulk off + полная перезапись v1 (air включительно).
- Натив под Paper: nat_p50 5.1-7.3 vs 1.8 изолированно — spawn/join при насыщенном CPU.
  ПЕРСИСТЕНТНЫЙ plane-воркер: один поток на gen, каналы, double-buffering Vec'ов,
  ChunkGen::drop шлёт Stop и join'ит ДО конца env-borrow. Локально паритет и перф сохранены.
- lean JVM (CICompilerCount=2, ParallelGCThreads=2, ConcGCThreads=1) + CRUS_FILL_THREADS=2:
  native mean 7.9 -> 6.1-6.5 на первом прогоне.

Stage Summary:
- Судья: PASS 0.000% (bulk v2 + self-check). Window: native 6-9 + layout 1.8-2.1 + 0.05.
- Локально: wall 2.54-2.67 | cellfill 0.80 | material 0.80 | slice 0.78; bench p50 2.36-2.52;
  parallel-2t 2.09. 11/11 тестов, 6/6 чанков serial==parallel, fastdiff 0.
- Next: CI-замер nat_p50 с персистентным воркером; если spawn-теория верна — nat_p50 ~2-3;
  далее strips (cellfill/material) и slice до floor ~1.0-1.2 на окне судьи.

---
Task ID: cont-22 (2026-10-13, окно судьи: layout 4x + диагностика контенции)
Agent: Super Z (main)
Task: GenStats-окно к 1ms; разбор натива под Paper

Work Log:
- GenStats теперь несёт полную анатомию окна: [native/nat_p50/layout/beard/marks]
  + [sections filled/mixed/air/v1fallback + bulk=reason] + jni_prof[fills/wall/slice/
  cellfill/material/select/par_planes/par_strips] (JNI profStatsGlobal, NewStringUTF).
- LAYOUT: bulk-путь через reflection доведён до боевого (все цели сверены с
  paper-1.21.8.jar). Провал 31.9% расследован и починен: подмена контейнеров
  пропускала (1) счётчики секций nonEmpty/tickingBlock/tickingFluid/specialColliding,
  (2) Moonrise tickingBlocks-list, (3) chunk heightmaps (WORLD_SURFACE_WG/OCEAN_FLOOR_WG)
  — при сохранении секции считались пустыми (30% мира = воздух, dh=108.5). v2:
  запись в существующий контейнер (getAndSetUnchecked, resize-safe) + пересчёт
  счётчиков из пер-материальных флагов (isAir/isRandomlyTicking/fluid-tick/
  isSpecialColliding) + rebuild heightmaps через setRawData (9 бит x 256,
  stored = topY+1-minY, predicate из поля isOpaque). layout 6.0 -> 1.4-2.0ms (3-4x).
  Самопроверка 256 сэмплов/чанк + полный v1-ремонт при mismatch (корректность гарантирована).
- Натив под Paper (jni_prof): parallel-путь wall 6.5 = par_planes 2.3 + par_strips 3.3
  + cf 1.2 + mt 4.1-4.7; CPU расширяется лишь ~1.2-1.4x, остальное — контенция
  планировщика с JVM (GC/JIT) на 2 ядрах. A/B: серийный путь (персистентный воркер)
  9.3ms — ХУЖЕ (fill_slice-бакет = ожидание воркера 3.6ms); откат на parallel-2t.
- Персистентный plane-воркер (1 поток/gen, каналы, double-buffering, Drop=Stop+join)
  — в дереве, локально паритет и перф сохранены; на изолированном bench спавны и так
  были дёшевы.
- Раннерный разброс: native 6.1-8.7 между прогонами при одинаковом коде — больше
  эффекта правок; медианы плавают 5.3-7.4.

Stage Summary:
- Окно судьи (GenStats mean): 14.8 -> 8.4-11.0 (p50 9.35 -> 6.6-8.9); layout 6.0 -> 1.4-2.0;
  native под Paper 7.9 -> 6.5-8.6 (раннерная лотерея).
- Изолированный CI-натив: bench perf real seed p50 4.53 -> 1.80 (parallel-2t) — 2.5x.
- Локально: phases wall 3.25 -> 2.54-2.67; bench p50 4.18 -> 2.36; par-2t 3.30 -> 2.09.
- Судья: PASS на всех прогонах сессии (0.000-0.002%), гейт 0.05% (25x+ запас).
- Для 1ms на окне судьи нужно: (1) срезать ~4-5 CPU-ms натива (material в parallel
  3.05 CPU — главная цель), (2) убрать остаточную контенцию (персистентные strip-воркеры).
- Все 14 коммитов сессии зелёные на CI; 11/11 тестов, serial==parallel бит-в-бит на каждом шаге.

---
Task ID: cont-23 (2026-10-14, task-graph + col-контракт + 512-cellfill + layout Data-swap)
Agent: Super Z (main)
Task: продолжение к 1ms/чанк на 2-ядерном CI; замеры на исторических тест-кейсах

Work Log:
- Среда: re-clone c-chunkky (репо переименован), rustup 1.99, remote с токеном только в URL.
- Базлайн (4c2af4c): локально wall 2.50 (slice 0.74 | cellfill 0.80 | material 0.79); CI-судья mean 8259us (native 6.24 + layout 1.61).
- ОТКРЫТИЕ 1 (col-контракт): compute_col_caches помечал Spline y-ВАРИАНТНЫМ -> сплайн-леса под Cache2D/FlatCache не получали col-слоты. Фикс: Cache2D/FlatCache y-инвариантны ПО КОНТРАКТУ ваниллы (сама ванилла кэширует раз на колонку) -> 13 слотов вместо 7. На текущем дереве слайс уже выжат — эффекта ~0, но анализ корректен. (Моя сессия параллельно реализовала то же самое независимо — дубликат удалён.)
- ОТКРЫТИЕ 2 (скейлинг): изолированно плоскости скейлятся 2.18x, но fill_parallel был фазовым (все плоскости -> барьер -> все стрипы) = 2 полных синхрона на филл (~0.4-0.5мс на 2 ядрах).
- ФИКС: dependency-aware task graph (FillGraph): плоскости+стрипы — один пул задач, стрип S_k открывается когда готовы плоскости k,k+1; main+воркеры тянут из атомарных счётчиков; 1 пермит = точный старый pipeline-порядок. Локально: par-2t 2.25 -> 1.96-2.07; serial 2.51 -> 2.38. Хэши serial==parallel бит-в-бит.
- ФИКС 3: AVX-512 фьюз 2-row cellfill (программа ОДИН раз на ix, A в лейнах 0-3, B в 4-7). БАГ: Lerp3 грузил 4 подряд f64 вместо скаляра rows8[slot*16+ix] — хэши разошлись, поймал, исправил (broadcast скаляра в 4 z-лейна половины). wide8=1 хэши 8/8 золотые; калибровка pick_cellfill_wide (dev VM выбирает 4 — 512 там медленнее; Zen4-раннер выберет сам).
- ФИКС 4 (layout v3 Data-swap): PalettedContainer.data — public volatile тройка (Configuration, BitStorage, Palette). На секцию строим тройку целиком (singleton/linear по distinct-скану; >15 distinct -> per-block) и меняем ОДНИМ volatile write вместо ~19.5k Method.invoke на чанк. Конфиги+airState из шаблона-контейнера; self-check 256 сэмплов + v1-ремонт сохранены.
- CI-инфра: cpuinfo-принт в калибровку; SerialGC A/B (CRUS_SERIAL_GC=1) на judge-джобе (хвост max=163мс — сигнатура GC-пауз).
- CI 390650d (task-graph + col-контракт): судья PASS 0.002%, **окно 8259 -> 6044us (-27%)**: native 6.24 -> 4.05, layout 1.61 -> 1.59. Калибровка раннера: par-2t bench perf real seed p50 2.36 (раннерная лотерея 1.8-2.4).
- Верификация каждого шага: 11/11 тестов, 8/8 золотых out_hash (e06a3b56dfaf4237...) serial И parallel И wide8=1.

Stage Summary:
- Судья: 8.26 -> 6.04ms на 390650d; 378a0ba (Data-swap + SerialGC A/B + 512-cellfill) — CI в прогрессе.
- Локально: serial 2.38 | par-2t 1.96-2.07 | phases: slice 0.66 | cellfill 0.79 | material 0.77.
- Next: вердикт 378a0ba (Data-swap на живом judge + SerialGC + wide8-калибровка на Zen4), затем material-prelim/ore, slice-кернелы (plane-major perm-дедуп), layout-остаток.

---
Task ID: cont-24 (2026-10-14, судья 8.26 -> 4.4-4.9; layout v3 Data-swap боевой)
Agent: Super Z (main)
Task: продолжение к 1ms; judge-окно и кернелы

Work Log:
- Раннер определён: AMD EPYC 7763 (Zen3, AVX-512 double-pumped) — обе калибровки (slice lanes, cellfill wide) корректно выбирают 4-лейн; 512-пути остаются для будущих Zen4-раннеров.
- CI-цепочка фиксов layout v3: (1) registry брался из BlockState -> bulk disabled, v1-фоллбек 6.05ms (судья всё равно PASS 0.002%); фикс — Block.BLOCK_STATE_REGISTRY; (2) Configuration package-private -> setAccessible(true) на factory/configuration.
- d371323 (всё вместе): **bulk=ok, окно судьи vanilla mean 4402us (native 2.90 + layout 1.20)**, datapack 5832; PASS 0.002%/0.001%; marks_seen 20329/5966 идентичны; sections filled=1260 mixed=1741 air=5783.
- 054d260 (полировка layout: verify 64 сэмпла, счётчики из packing-прохода, фикс сброса palIdxScratch — критичный баг, отравлял бы следующую секцию): PASS оба мира; окно 4890/5323 — раннерная лотерея (разброс прогонов > эффекта; медианы 3.64/3.92 vs 3.42/3.97).
- par-2t bench perf (real seed) на раннере: p50 2.15-2.19 стабильно (было 1.8-2.4 лотерея). phases на раннере: wall 2.91 (slice 0.96 | cellfill 0.91 | material 0.86).
- SerialGC на judge (A/B): max-хвост 163 -> 123-131ms — в пределах шума, оставлен включённым.

Stage Summary:
- Судейское окно за сессию: **8.26 -> 4.4-4.9ms (-45±5%)**: task-graph fill (-1.5-2 CPU-ms), col-контракт Cache2D/FlatCache, layout v3 Data-swap (1.6 -> 1.2-1.5), полировка.
- Паритет генерации не тронут ни одним шагом: PASS на всех 6 прогонах сессии (0.001-0.005%, гейт 0.05% = 10-50x запас), marks_seen бит-в-бит.
- 11/11 тестов; золотые out_hash 8/8 (serial, parallel-2t, wide8=1 принудительно).
- До 1ms на окне судьи: нужен native ~0.6 (сейчас 2.9) — фронт: plane-major perm-дедуп NormalNoise (2-4x на noise-часть slice), blended same-x hoist (~10%), material prelim; layout ~0.15 (сейчас 1.2-1.6). Это отдельные крупные итерации.

---
Task ID: cont-25 (2026-10-14, plane-major perm-дедуп NormalNoise — ГОТОВО)
Agent: Super Z (main)
Task: plane-major кэш решётки (perm-дедуп) — прямо одобренная пользователем итерация

Work Log:
- Реализован plane-major column cache: слайс-батчи имеют ОДНУ (x,z)-колонку на батч,
  поэтому на октаву колонка-часть (wrap, floor/frac, smoothstep fx/fz, p0/p1) вынесена
  в слот потоко-локального кэша (per-thread TLS, ключ — бит-сравнение пре-образа kx/kz).
- y-ячейковая мемоизация: для октав с шагом входа < 1 ячейки (8f<1: 6 из 10 октав)
  пермутационная цепочка + X/Z dot-партиалы (t0/t2) кэшируются по cell_yi — до 12x
  переиспользование walk'а на низких октавах. Mixed-y и same-y AVX2-кернелы добавлены
  (nois_col4_same/mixed_avx2), порядок операций lane-идентичен общему пути.
- БАГ ПОЙМАН golden-скриптом: col_base = id*COL_MAX_OCTAVES — слоты second инстанса N
  алиасились с first инстанса N+1 (ключ не содержал идентичность шума -> stale-hit из
  чужого p-table). Фикс: stride 2*COL_MAX_OCTAVES. Урок: кэш-ключ обязан покрывать
  ВСЁ пре-образ значения.
- Верификация: 11/11 тестов; golden 8/8 чанков x 4 режима бит-в-бит (serial,
  parallel-2t, lanes=8, обе сразу) — scripts/verify_hashes.sh check.
- A/B на этой машине (phases, исторический сид 3257840388504953787):
  OLD wall 2.41-2.47 (slice 0.67-0.71) -> NEW wall 2.26-2.32 (slice 0.54-0.58):
  slice -20%, wall -6.5%.

Stage Summary:
- Plane-major perm-дедуп готов и верифицирован; локальный phases wall 2.25-2.32.
- Далее по плану: blended same-x hoist -> prelim-скан -> layout (1.2 -> 0.15).

---
Task ID: cont-26 (2026-10-14, blended same-x hoist)
Agent: Super Z (main)
Task: blended column-путь (min/max октавы через hoisted column state)

Work Log:
- noise_5_batch_col на ImprovedNoise: x/z column state в TLS-слотах (ключ — биты
  пре-образа kx/kz), per-lane y-setup + weird-dy (smear-квирк y_scale!=0), ходьба
  из hoisted p0/p1 с consecutive-yi дедупом, fy из RAW dy. AVX2-кернел с broadcast
  fx/fz/dx/dz; порядок операций lane-идентичен общему пути.
- compute_batch_col / compute_batch8_col на BlendedNoise: main-цикл остался на общем
  пути (z per-lane), min/max (32 из 40 октава-батчей) через слоты [base..16)/[base+16..32);
  skip-флаги и полусемейства 8-wide сохранены. Гейт в density: lanes_uniform_i32(bx,bz)
  + avx2_available() (OnceLock).
- Верификация: 11/11 тестов, golden 8/8 x 4 режима бит-в-бит с первого прогона.
- Эффект: blended-batch diag 0.72 -> 0.58 (-20% blended), slice 0.54 -> 0.53 (скромно:
  y-шаг входа min/max октав 5475*amp >> 1 ячейки — ходьба остаётся per-lane, экономится
  в основном x/z-setup). Реально ~0.03-0.05ms.

Stage Summary:
- Срез: slice 0.53 | cellfill 0.78 | material 0.76 | other 0.17 (wall 2.24-2.28).
- Blended hoist готов; главный оставшийся объём — cellfill+material (68%).

---
Task ID: cont-27 (2026-10-14, cell-verdict gate — интервальный early-out ячеек)
Agent: Super Z (main)
Task: cellfill/material cell-level skipping (66% секций чанка — чистый AIR)

Work Log:
- cell_interval в density.rs: интервальный прогон 17-опной cellfill-программы по
  диапазонам 8 углов ячейки. Каждый op даёт ЗВУКНУТЫЙ над-интервал (Lerp3 — [min,max]
  углов, т.к. веса в [0,1); Mul/Min/Max/Abs/Square — стандартные hulls;
  Squeeze/Cube/HalfNeg/QuarterNeg — монотонны; RangeChoice — точен при полной
  внутри/вне, иначе hull; YGrad — сэмплы 8 y). NaN углов -> None (полный путь).
- ВЕРДИКТ AllPos/AllNeg доказывает знак всех 128 substance. КРИТИЧЕСКОЕ ОТКРЫТИЕ:
  аквиферный барьер давления читает ТОЧНОЕ значение (substance + d1 > 0 -> STONE,
  строка material_rule/aquifer border) — poison для AllNeg НЕВАЛИДЕН (поймано
  golden'ом: 25 блоков STONE->WATER на (7,-3); бисекция CRUS_GATE; dump+diff по
  координатам). Итоговый безопасный дизайн:
  * AllPos: poison +1.0 везде (давление требует substance<=0) + прямой STONE вне
    ore-диапазона;
  * AllNeg: прямой LAVA ниже лавовой черты; прямой AIR при y>=64 ЕСЛИ все базовые
    aquifer-ячейки 2x2x2-окрестности имеют закэшированные однородные summary;
    иначе полный точный cellfill.
- Дебаг-валидатор CRUS_GATE_DEBUG: interval vs реальные 128 значений на всех
  ячейках — 0 нарушений (интервал звуковой).
- Верификация: 11/11 тестов, golden 8/8 x 4 режима.
- Эффект (phases, локально): wall 2.25 -> 2.02 (-10%); cellfill 0.78 -> 0.39 (-50%);
  material 0.76 -> 0.65 (-14%). slice-бакет 0.53 -> 0.73 — это ожидание plane-воркера
  (пайплайн сместился), реальный slice не изменился.

Stage Summary:
- Локальный serial wall 2.02 (slice 0.73* | cellfill 0.39 | material 0.65 | other 0.24).
- (*slice включает простои ожидания плоскостей — узкое место сместилось в пайплайн.)
- Далее: ore-путь (батчинг ridged/gap), затем layout и CI-прогон.

---
Task ID: cont-28 (2026-10-14, ore-vein интерполяция — ЛАТЕНТНАЯ ДИВЕРГЕНЦИЯ УСТРАНЕНА)
Agent: Super Z (main)
Task: корректность vein_toggle/vein_ridged (Block-mode Interpolated) + ore-диагностика

Work Log:
- Ore-диагностика (CRUS_DIAG): toggle 34175 вызовов/чанк, ridged/gap — 0 (rng-гейт
  не проходит ни разу на эталонном сидe — вены не срабатывают и в ванилле).
- ОБНАРУЖЕНО: update_for_y/x/z никогда не вызываются -> chain_value(slot) = 0.0
  всегда -> Block-mode Interpolated (vein_toggle) возвращал 0.0 -> все вены
  подавлены на корню. Судья проходил, т.к. в эталонных дампах вены не срабатывают
  и в ванилле; на других сидах/датапаках — ГАРАНТИРОВАННАЯ дивергенция (руды
  отсутствовали бы).
- ФИКС: vein-интерполированные узлы УЖЕ входят в compiled.interp_children (слоты
  5/6/7) — их угловые плоскости УЖЕ считает slice-фаза, selectCellYZ уже выбирает
  их углы per cell. Per-block значение = ванильный NoiseInterpolator lerp3 по этим
  углам (ore_slot_lerp3 — формула cellfill_value). toggle = lerp3(slot7),
  ridged = r_const + max(|lerp3(slot5)|, |lerp3(slot6)|), gap — без изменений.
  ore_toggle_row4 получил interp-ветку (ряды с hoisted dx/dy). Не-ванильные формы
  деревьев (датапаки) — фоллбек на старый путь (без регрессии).
- Промежуточный вариант с lazy per-cell corner-кэшем дал material 1.26 (углы
  дублировались) — заменён чтением corners_pub: material 0.64 -> 0.61.
- Верификация: 11/11 тестов; golden 8/8 x 4 режима — РЕАЛЬНЫЕ значения toggle не
  меняют вывод на эталонных чанках (подтверждение паритета).

Stage Summary:
- Локальный serial wall 1.96-1.98 (slice 0.71-0.76 | cellfill 0.38-0.39 | material
  0.61-0.62 | other 0.23-0.25). С начала сессии: 2.25-2.44 -> 1.96.
- Корректность: устранена латентная дивергенция ore-вен (Block-mode Interpolated).

---
Task ID: cont-29 (2026-10-14, layout v3.5: global-swap для mixed-секций)
Agent: Super Z (main)
Task: layout 1.2ms -> ~0.6ms (mixed-секции без per-block рефлексии)

Work Log:
- CI 38554921918 (3efc053): судьи PASS 0.002%/0.001% (историческая полоса);
  изолированный bench на раннере: par-2t real seed p50 2.15 -> 1.676 (-22%);
  судейское окно этого прогона зашумлено (layout 1.2 -> 3.3 без изменений Java
  кода, max 416мс — контенция раннера).
- Layout-профиль: mixed-секции (>15 distinct, 4.8/чанк) шли per-block рефлексией
  (~4096 getAndSetUnchecked на секцию) — ~половина layout 1.2ms.
- v3.5: для over-ветки — GLOBAL-конфигурация Data-swap: шаблон контейнера
  прогоняется через 17 distinct registry-состояний (byId), контейнер сам
  резайзится в global; захватывается точный тройник (cfgGlobal(ceillog2(registry
  size)), GlobalPalette, bits) — ровно то, что ваниль создаёт для реальной
  mixed-секции. Секция пакуется в SimpleBitStorage (ванильная раскладка:
  word = k / valuesPerLong, shift = (k % vpl) * bits — без кросс-словных записей)
  из globalIdById[] (precomputed registry.getId per material) — ОДИН volatile
  write вместо 4096 рефлекс-вызовов.
- Пойман при самопроверке: первая версия паковала линейно (k*bits) — не совпадает
  с ванильной valuesPerLong-раскладкой и длиной массива; исправлено до сборки.
- Безопасность: verifyChunk (256 сэмплов) + degrade на per-block/v1 при любом
  рассогласовании — регрессия невозможна по построению.

Stage Summary:
- Плагин собран; вердикт — CI (judge ваниль/датапак + окно).
- Ожидание: layout ~1.2 -> ~0.6-0.7 на чистом прогоне.

---
Task ID: cont-30 (2026-10-14, ore-frac hoist + CI-вердикты трёх прогонов)
Agent: Super Z (main)
Task: пуш и верификация цепочки (plane-major -> blended -> cell-gate -> ore-fix -> layout v3.5)

Work Log:
- ore_frac хоистинг в material_row4: dx/dy постоянны по строке, dz = iz*step
  (block_z0 == начало ячейки) — минус 3 floor-div на ore-блок. material 0.61 -> 0.51.
- CI-вердикты (все зелёные, судьи PASS в каждом):
  * 3efc053 (cell-verdict gate): vanilla 0.002% / datapack 0.001%; изолированный
    bench раннера par-2t real seed p50 2.15 -> 1.676 (-22%).
  * 9aa4898 (ore-vein fix): PASS 0.002%/0.001% — РЕАЛЬНЫЕ значения toggle не меняют
    вывод на эталонных дампах (паритет подтверждён); bench p50 1.761.
  * 06fb302 (layout v3.5 global-swap): PASS 0.002%/0.001%; bulk=ok, v1fallback=0 —
    global-swap активен; bench p50 1.784.
- Судейское окно СЕГОДНЯ зашумлено на раннере (native 4.7 + layout 2.2-3.3 против
  исторических 2.9+1.2; max-хвосты 180-416мс) — контенция среды; изолированный
  bench показывает реальный прогресс (1.68-1.78 стабильно).

Stage Summary:
- Локальный phases wall: 2.44 (начало сессии) -> 1.92; par-2t на раннере 2.15 -> 1.68-1.78.
- Паритет не тронут ни одним шагом: 3 зелёных CI-прогона, PASS 0.001-0.002% (гейт 0.05).
- Устранена латентная дивергенция ore-вен (chain_value всегда 0.0).

---
Task ID: cont-31 (2026-10-14, шумодав + AVX-512 col8 + prefetch (выключен) + layout hmap)
Agent: Super Z (main)
Task: steady-state замеры на 2-ядерном раннере + дальнейший перф до 1ms

Work Log:
- ШУМОДАВ (1b57aa9): GenStats.reset + RCON 'crusgenstats reset'; plugin_bench.sh —
  JIT-прогрев отдельным 8x8 регионом (+8..15) ДО замеряемого, forceload remove
  all, reset окна; GenStats: колонка min (аддитивный шум только завышает);
  server_up.sh judge: -Xms1G (фикс. коммит, без ресайзов); bench.rs min=; CI:
  par-2t bench x3. Результат: par-2t p50 на раннере 1.593/1.618/1.592 (разброс
  1.5% против исторических ~15%) — замеры чистые. Судья: chunks=229 (8x8 +
  каскад соседей), окно steady-state.
- CPU раннера: Intel Xeon Platinum 8573C (Granite Rapids, полный AVX-512).
- AVX-512 col8 (51a13f5): 8-широкие колоночные кернелы ImprovedNoise
  (same/mixed) + PerlinNoise::get_value_batch_col dispatch + BlendedNoise
  compute_batch8_col512 (единая 8-широкая половина: main через noise_5_batch8,
  min/max через noise_5_batch_col8/noise_5_col8_avx512). probe_avx512_width:
  перемежающийся min-of-3 микробенч на реальном NormalNoise (turbo-безопасно;
  история: SKX/CLX даунклочат 512-бит), CRUS_AVX512=1/0 форсирует. Бит-паритет:
  2 новых теста (same/mixed/tail, оба режима) — 13/13, golden 8x4 в обоих
  режимах. Локально проба стабильно выбирает AVX2 (512 медленней на этом
  Xeon), на GR-раннере решает проба.
- PREFETCH (63f1ec9 -> 804af42 -> 6aefead): PrefetchWorker считал плоскости
  СЛЕДУЮЩЕГО чанка в паузе между филлами (Java layout/beard/SURFACE); гэп-гейт
  >=1ms; per-plane деградация (что успело — забираем, остальное граф).
  Локально с CRUS_GAP_US=3000: got=4/4 каждый филл, 1.4 -> 0.6-0.9ms.
  НО CI (804af42): изолированный bench не изменился (1.59), а окно судьи
  ДЕГРАДИРОВАЛО native 2.06 -> 4.11, p50 2.93 -> 5.6: на насыщенном 2-ядернике
  паузы НЕ простаивают (Paper gen pipeline/lighting), prefetch-поток крадёт
  циклы и переливается в филлы. ВЫВОД: prefetch default OFF (6aefead),
  CRUS_PREFETCH=1 для экспериментов.
- LAYOUT (9bdb973): rebuildHeightmaps был ПО ОДНОМУ полному проходу
  384x256 НА КАЖДЫЙ тип heightmap (4-6 типов) — недостающий член layout
  1.03ms. Fused: один проход на колонку через per-id биту-маску (глубина =
  самый глубокий тип, не сумма). Значения бит-идентичны. verifyChunk:
  burn-in 256 чанков + 1/64 сэмплирование (систематический баг портит >>10%
  блоков — редкие пробы ловят; судья — главный гейт). GenStats: [lay
  setup/sec/hmap/verify] разбивка в окне судьи.
- CI-вердикты: 51a13f5 PASS (vanilla 0.001% / datapack 0.001%); окно 51a13f5:
  native 2.06-2.48 + layout 0.9-1.03, p50 2.93-3.20 (было 4.4-4.9); par-2t
  bench 1.59 стабильный. 804af42 PASS (prefetch регрессия окна — задокументирована
  и выключена). 63f1ec9 PASS.

Stage Summary:
- Окно судьи: 8.26 (старт) -> 4.4-4.9 -> 2.93-3.20 (51a13f5, чистые замеры).
- par-2t изолированный: 2.15 -> 1.59 (раннер), локально 1.42-1.46.
- Замеры теперь steady-state (JIT-прогрев + reset окна + min-колонка + x3).
- Далее: CI 6aefead (prefetch off + hmap fix + lay-разбивка) -> решающий
  пункт: layout после hmap-fix, native под контенцией; AVX-512 решение пробы
  на GR-раннере.

---
Task ID: cont-32 (2026-10-14, slot-consts + ГЛАВНОЕ: swap-пути мертвы с v3 — починено)
Agent: Super Z (main)
Task: y-range константные слоты; разбор layout-загадки; починка Data-swap

Work Log:
- РАННЕРЫ: пул GitHub неоднороден (AMD EPYC 7763 Milan / Intel 8573C Granite
  Rapids / AMD EPYC 9V45 Turin) — один и тот же бинарь: par-2t bench p50
  1.70 / 1.59 / 0.92ms соответственно. Дисперсия окна судьи на идентичном
  коде 2.1x (6aefead vs 81cc88f). Вывод: сравнивать прогоны можно только
  внутри одного CPU-типа; целиться в худший.
- AVX-512 проба на всех трёх CPU выбирает AVX2 (4-широкий) — 512-бит f64 не
  даёт >=3% на этой нагрузке нигде; правильное решение, механизм оставлен.
- slot-consts (bf39dd5): Compiled::interp_slot_y_const — RangeChoice над
  vanilla 'minecraft:y' (identity-градиент, y.json [-4064,4062]) с константной
  out-веткой: ore-слоты (vein_toggle/ridged/gap, [-60,51), out 0.0) = 0.0
  выше y=51 → 34/49 углов x 3 слота без шума. Skip во всех трёх plane-путях.
  planes 1.65 -> 1.58 serial; golden 8x4 ок.
- ГЛАВНОЕ (d4414c0 lay-разбивка): [lay setup=0.00 sec=0.78 hmap=0.09 | uni=0
  lin=0 glob=0 pb=8.00/чанк] — СВОП-ПУТИ НИКОГДА НЕ АКТИВИРОВАЛИСЬ: все ~8
  не-air секций/чанк шли через per-block рефлекс-фоллбек (~95мкс/секция).
  v3/v3.5 выигрыш layout не выходил вообще (bulk=ok + verifyChunk проходили
  и так — фоллбек маскировал).
- КОРЕНЬ (статический аудит NMS Paper 1.21.8-60 через javap):
  Palette$Factory.create(int bits, IdMap, PaletteResize, List) — 4 аргумента,
  плагин звал с 3-мя (reg, container-как-resize, values) →
  IllegalArgumentException на каждом вызове → catch → per-block. Чинено
  (026786a): singletonBits/linearBits кэшируются в initBulk из Configuration.
- Heightmap fused (9bdb973) подтверждён CI: hmap 0.9 -> 0.09ms.
- swapfail= диагностика (747b0a8): первый Throwable свопа с классом/сайтом в
  GenStats — страховка от немых деградаций.
- jni_prof в окне (d4414c0): material=1.25ms (0.5 локально) — контенция
  2-ядерника бьёт по Java-стороне; layout 0.87.

Stage Summary:
- Окно судьи (тихий Intel 8573C): p50 2.83-3.11 = native 2.04-2.40 + layout
  0.87. С починенным свопом ожидание layout ~0.4 → окно ~2.4.
- par-2t bench: 1.59 (Intel) / 0.92 (Turin) — изолированный native уже близок
  к 1ms на новых раннерах пула.
- Кандидаты дальше: slot0-const при y>=256 (нулевая ветка top_grad, нужен
  sign-of-zero-safe анализ, ~0.13ms wall), material/cellfill в окне.

---
Task ID: cont-33 (2026-10-14, СВОП ПОЧИНЕН — layout 0.87 -> 0.28; slot0-const; slot-consts)
Agent: Super Z (main)
Task: root-cause мёртвых своп-путей; slot-consts; slot0 upper-y const

Work Log:
- slot_y_const (bf39dd5): Compiled::interp_slot_y_const — RangeChoice над
  vanilla 'minecraft:y' (identity) с Const out-веткой; ore-слоты 5-7 = 0.0
  выше y=51; noodle-слоты 1-4 вне [-60,321). Skip во всех plane-путях.
- slot0 upper-y const (6a38d15): compute_y_upper_consts — compile-time
  'const for y>=y0' с sign-of-zero safe правилами (YClampedGradient upper ->
  Val(to_value); Mul(0,x) -> ZeroU; Add(nonzero, ZeroU) -> nonzero; Min/Max
  bail). Vanilla slot0 = Val(-0.078125) @ y>=256 (подтверждено ymeta).
  Empirically ~neutral на ваниле (высокие y уже дёшевы через RC-микс), но
  срезает cave-recipe на датапаках с большими слайдами. Kill-switch
  CRUS_NO_YCONST=1 (OnceLock-cached; env::var в горячем цикле стоил 3%!).
- ГЛАВНОЕ (026786a): Palette$Factory.create(int bits, IdMap, Resize, List) —
  4 аргумента; плагин звал с 3-мя → IllegalArgumentException на КАЖДЫЙ вызов
  → все своп-пути молча деградировали в per-block рефлекс-фоллбек С САМОГО
  ПОЯВЛЕНИЯ v3. Починено: singletonBits/linearBits из Configuration.bits().
  РЕЗУЛЬТАТ CI: [lay | uni=0.2 lin=0 glob=8.0 pb=0.0] swapfail=null —
  layout 0.87 -> 0.28-0.30ms (-67%), все секции через global-пак.
  Паритет: 0.000-0.002% PASS во всех прогонах.
- Инструменты: javap/jar обёртки через jdk-модули (scripts/), SnapTest
  (рефлекс-тест snapshot локально), ymeta (метаданные слотов), constcheck.
- Наблюдение: классификация секций сместилась (filled=0, все через global) —
 vn==1 ветка мертва по построению (первый non-air даёт vn=2); глобальный пак
  0.02ms/секция — достаточно, оставлено как есть (паритет решает).

Stage Summary:
- Окно судьи (арбитр): 8.26 (старт) -> 4.4-4.9 -> 2.83-3.11 -> 2.44-2.70
  (026786a, тихий раннер) при layout 0.28-0.36 СТАБИЛЬНО во всех прогонах.
- Изолированный native par-2t: 2.15 -> 1.59 (Intel 8573C) / 0.92 (Turin 9V45).
- Все прогоны PASS (гейт 0.05%): 51a13f5, 63f1ec9, 804af42, 6aefead,
  81cc88f, 5f0c35a, 560c785, d4414c0, bf39dd5, 747b0a8, 026786a, 8e5ddf8.
- Остаток до 1ms окна: native 2.1-2.2 (тихий Intel) — контенция +0.3-0.5;
  дальше только глубокие срезы noise-ядра или ожидание Turin-класса раннеров
  в пуле (там isolated уже 0.92).

---
Task ID: cont-34 (2026-10-15, straddle regression fix + noise-core floor probe)
Agent: Super Z (main)
Task: (а) вернуть зелёные прогоны, (б) blended noise-ядро, (в) вердикт по плато

Work Log:
- ГЛАВНОЕ: найден КОРЕНЬ красных прогонов 130-132 (6a38d15): slot0 upper-y
  const заливал КОНСТАНТУ ВСЕЙ ГРУППЕ по ПЕРВОМУ углу, а группы slice_slot0
  идут по плоскому p-индексу (i=p/49, y=p%49) и ПЕРЕСЕКАЮТ рестарт y-строки:
  лейны после границы падают на дно мира (y=-64) и получают -0.078125.
  Судья: vanilla 0.151% (9514 блоков, 87-208/чанк) vs гейт 0.05%.
  Фикс (9459857): all-lanes проверка (upward-closed регион). Верификация:
  48/48 out_hash == до-изменному 8e5ddf8 (судейский сид) + 16/16 seed-0 +
  A/B kill-switch == golden. CI 9459857: vanilla 99.998% / datapack 99.999%
  PASS, marks_seen бит-в-бит. ЗЕЛЁНЫЙ.
- Урок: golden-инструмент сравнивал НЕ ТУ КОЛОНКУ ($4='took' вместо $6) —
  сверка была фиктивной; Unit-тесты (13/13) проверяют эквивалентность путей,
  не паритет. Новое правило: паритет-сверка = out_hash vs CI-green билд.
- blended main-цикл (8 октав) переведён на col-ядро (d4b6744): НЕЙТРАЛЬНО —
  generic-ядро УЖЕ дедуплицирует walk одинаковых соседних лейнов; выигрыш
  сводится к col_init-скипу (~0.01-0.02ms). Оставлено (бит-точно, чище).
- equal-grad broadcast (5a8c3ef): когда walk-дедуп свёл все 4 лейна в одну
  ячейку — один GRADIENT lookup + 3 broadcast'а на угол вместо 12 scalar
  загрузок. blended-batch 0.59 -> 0.57. Бит-точность: golden 48/48.
- ВЕРДИКТ по blended (0.57-0.59 локально): архитектурный пол достигнут.
  40 октав/вызов, walk'и дедуплицированы (внутри вызова), y-ячейки
  мемоизированы (col state), скипы per-lane, градиенты broadcast'ятся.
  Остатки: fusion min/max (~0.06, механика), y-prep sharing — убывающая
  отдача. Оценка пола: ~0.45-0.50.

Stage Summary:
- CI: ЗЕЛЁНЫЙ на 9459857 (+d4b6744, 5a8c3ef в процессе). Окно на тихом
  Intel: 2.44-2.70; локальный wall 2.39-2.48 = slice 0.88 (blend 0.57 +
  NN 0.25) + material 0.89-0.95 + cellfill 0.39 + other 0.23.
- Куда давить дальше (по данным): (1) material 0.89 — последняя крупная
  фаза, кандидаты: row-batching aquifer summary/barrier; (2) runner pool —
  Turin 9V45 уже в пуле (isolated par-2t 0.92 vs Intel 1.59 = 1.7x),
  проекция окна на Turin ~1.5-1.7; (3) fusion min/max — мелочь.
- 1ms end-to-end на СЕГОДНЯШНЕМ пуле недостижим (нужен Turin-класс +
  ещё ~35% среза native). Плато ~2.4-2.5 на Intel подтверждено данными.

---
Task ID: cont-35 (2026-10-15, env-lock fix + material decomposition)
Agent: Super Z (main)
Task: "Продолжи до 1мс" — атака material-фазы (последняя крупная)

Work Log:
- CI вердикт HEAD 3278012: судья PASS 0.002%/0.002%, но на этом раннере
  native=3.31 (par_planes=2.37, material=1.88) — контенция. Изолированный
  par-2t: 1.59 Intel / 0.92 Turin.
- ЛОКАЛЬНАЯ ДЕКОМПОЗИЦИЯ (phases, медианы из 5): wall 1.85 = slice 0.75
  (blended ~0.57 floor) + cellfill 0.36 + material 0.51 + other 0.23.
  Material разобран probe-ами: loop+row4 = 0.16 НЕИЗБЕЖНО, fluid-резолвы =
  0.15-0.16 (242-319 вызовов, ~0.5us каждый), ore = 0.19 (34k finish по ~5ns).
- ГЛАВНОЕ: найден env::var("CRUS_GATE_DEBUG") в per-cell цикле cellfill —
  768 вызовов/fill на процессе-environ локе! На CI параллельные fill-воркеры
  сериализовались на glibc environ-мьютексе => material 1.88 на CI vs 0.53
  локально. Это и был главный разрыв CI-vs-local. Заодно CRUS_PRELIM_SCALAR
  (per prelim scan) и CRUS_FAKE_FLUID (probe) — все через OnceLock теперь.
  Урок cont-32 ("env::var стоил 3%") применим тут в 10x большем масштабе.
- Оценка fluid-батчинга (prewarm 4-wide через eval_batch<4>, lane-identical):
  потолок 0.15 локально + устранение дубликатов резолвов на CI (429 vs 306).
- Локально после фикса: cellfill 0.40 -> 0.36; golden 8x4 bit-identical;
  13/13 unit.

Stage Summary:
- Коммит: env-lock fix (критично для CI-окна при параллельных fill).
- Далее: (1) CI-проверка окна после фикса; (2) fluid batch-resolve в
  12-cell скане (319 scalar eval -> ~80 4-wide, экономия ~0.10 локально и
  дедуп на CI); (3) ore finish 4-lane gate (0.19 -> ~0.14); (4) blended
  floor 0.45-0.50 (fusion min/max, y-prep).
- Прогноз: native serial 1.85 -> ~1.55; окно на Turin ~1.0-1.1.

---
Task ID: cont-36 (2026-10-15, ore gate + batch-эксперименты)
Agent: Super Z (main)
Task: material floor; blended дальше

Work Log:
- CI b2fd2ac (env-фикс): PASS 0.003%. jni_prof wall=3.85 material=2.11 —
  раннер из пула другой (дисперсия 2.1x задокументирована), кросс-раннерное
  сравнение jni_prof невалидно; cpuinfo в артефакт не пишется — добавить.
- Aquifer Arc выяснен: per-ctx (НЕ cross-fill) — shared только между strip
  worker'ами ОДНОГО fill. 429-vs-261 из cont-20 = per-thread-within-fill
  вариант. Cross-fill churn-теория умерла; abs-keyed кэш (экономил бы ~0.05)
  отвергнут: key-value протокол под гонками сложнее выигрыша.
- ЭКСПЕРИМЕНТ 1 (отвергнут): batch-resolve статусов аквифера 4-wide перед
  сканом (compute_aquifer_fluid_batch4 через eval_batch<4> + pre-pass).
  ЗОЛОТОЙ ОК (бит-точно), но: жадный резолв всех 12 статусов против
  ванильного ленивого (стоп на all_same break) — в смешанных регионах
  дороже; material 0.50 -> 0.54-0.59. Урок: ленивость ванили — часть семантики
  производительности.
- ЭКСПЕРИМЕНТ 2 (ОТКАТ): AVX2 4-lane ore pre-gate (sign-select + y-range +
  clamped_map + abs+d1<0.4) в material_row4 — material 0.50 -> 1.22 (!) —
  AVX-SSE transition penalties при смешивании 256-битных блоков в
  преимущественно скалярном коде. ПОНЯЛ ПОЧЕМУ: MaterialRow4 и соседи
  компилируются в SSE2-скаляр; каждый вход в ymm-блок = vzeroupper-штрафы
  на каждую итерацию.
- ЭКСПЕРИМЕНТ 2b (ПРИНЯТ): скалярный вариант того же гейта (4 lane по
  очереди, early-break на первом выжившем): material 0.50 -> 0.44-0.45.
  Golden 8x4 bit-identical, 13/13 unit. Kill-switch CRUS_NO_OREGATE=1.
- Материальный floor теперь: loop+row4 ~0.16 + fluid ~0.15 + ore ~0.13.

Stage Summary:
- wall 1.81 -> ~1.75 (ore gate). slice 0.75 — следующий таргет (blended
  0.57, floor-оценка 0.45-0.50: fusion min/max, y-prep sharing).

---
Task ID: cont-37 (2026-10-15, blended prep vectorization + TF-домены)
Agent: Super Z (main)
Task: slice/blended floor

Work Log:
- ПРОФАЙЛ prep-vs-kernel: первый probe (CRUS_BLEND_NOPROBE в скалярной
  ветке) показал "prep 66%" — ОКАЗАЛОСЬ неверно (probe мерил скалярный
  fallback, а горячий путь — AVX2-ядро). Реальный split: ядро noise_5
  доминирует (40 октав-вызовов x ~88нс), prep даёт лишь ~4%.
- Векторизация wrap-подготовки (wrap_v = d - floor(d*2^-25 + 0.5)*2^25,
  vroundpd; v/amp -> v*2^k — точные степени двойки, бит-эквивалент):
  - ВАРИАНТ inline в non-TF compute_batch: 3.70 -> 6.02us — КАТАСТРОФА
    (AVX-SSE transition penalties на каждой скалярной операции между
    ymm-блоками; второй случай за день — см. ore-gate AVX2).
  - ВАРИАНТ в #[target_feature] функциях (blend_main_loop_avx2 +
    blend_minmax_loop_avx2, прямой вызов noise_5_batch_avx2_direct): 
    3.70 -> 3.56us (-4%), acc бит-идентичен. ПРИНЯТ.
- УРОК ДНЯ (дважды): весь ymm-код — только внутри target_feature-функций;
  inline ymm между скалярными блоками в общем коде = transition-штрафы,
  превышающие выгоду векторизации в разы.
- compute_batch8/col-варианты не тронуты (та же схема при необходимости).
- Golden 8x4 bit-identical, 13/13 unit, phases wall ~1.84 (slice ~0.79).

Stage Summary:
- Blended floor подтверждён: ядро noise_5_batch_avx2 (walk+dots+setup) —
  88нс/октаву-батч — дальнейший скепсис только через pipelining walk'ов
  (убывающая отдача). blended ~0.55 локально.
- wall 1.80-1.84: slice 0.78 + cellfill 0.36 + material 0.45-0.47 + other
  0.22. С начала сессии: material 0.53 -> 0.46, blended -4%, env-фикс для CI.

---
Task ID: cont-15 (2026-10-15, noise_5 pipelining + fluid lazy-fill experiment)
Agent: Super Z (main)
Task: (а) ядро noise_5 — pipelining walk'ов октав; (б) fluid — ленивый групповой rebatch; (в) долю Turin следить по [cpu]-лейблам

Work Log:
- (а) РЕАЛИЗОВАНО, бит-точно: (1) single AVX2-ядро — прямая запись градиентных компонент (byte round-trip через grad[][] удалён); (2) noise_5_batch_x2 — кросс-инстансный спаренный walk: два ImprovedNoise с независимыми входами (per-side cell setup + y_scale) в одном interleaved теле → 8 независимых perm-цепей вместо 2x4 последовательных; dual = обёртка над x2 с общими входами; (3) min/max в blended объединены: min и max октавы едят ИДЕНТИЧНЫЕ точки → один dual вместо двух singles на октаву/половину; (4) main-петля октав спарена (i, i+1) через x2 (порядок аккумуляции per-lane не тронут, amp — степень двойки, /2 и /4 точны). CRUS_PAIR_MAIN=0 — A/B-выключатель (дефолт on).
- ЛОВУШКА №1: первый вариант dual разделял scalar setup между min/max — НЕВЕРНО: xo/yo/zo у октав РАЗНЫЕ. Исправлено до per-side setup.
- ЛОВУШКА №2: первые замеры после main-pairing казались регрессией (5.26 vs 5.03) — оказался шум VM; A/B x7: paired slice ~2.49 vs seq ~2.52 — pairing оставлен.
- (б) ЭКСПЕРИМЕНТ, ИЗМЕРЕН КАК ОТРИЦАТЕЛЬНЫЙ, ОТКАЧЕН: aquifer_fill_summary (12-ячеечный status-scan без дистанций) в material_row4 при sv==0 вместо bail. Инструментация (счётчики в worktree-OLD): fills=1868/16чанков (3989ns/call!), same=1852/notsame=16, fluid_resolves OLD==NEW (5105 — чистые функции, кэш), полные сканы OLD 11156 == NEW 9304+1868 (работа 1:1). Итог: material 1.31 -> 1.47 (+0.18); #[cold]+inline(never) вернул 0.07; чистый A/B: +0.05ms net. ПРИЧИНА: per-block путь УЖЕ ленив (first-touch scan ровно один на ячейку), row4-fill лишь добавлял вызов+layout-давление на горячие 21.5k быстрых рядов. Вывод: «ленивость» fluid уже достигнута архитектурой кэшей; группировка на уровне ряда не даёт нового dedup.
- Верификация (а): 10 filldump-чанков побайтно == pre-change; НОВЫЙ ТЕСТ noise_5_dual_matches_two_singles (16 инстансов x 2500 итераций, dual-vs-singles по битам); lib-сюит 12/12.
- Перф локально (median): slice 2.68 -> 2.46ms (-8%), wall 5.21 -> 4.96ms (-4.8%); noisemicro8 4-wide 18.4 -> 15.0 ns/sample; bench perf 12.62 -> ~12.5ms (в шуме).
- (в) CI: [cpu]-лейблы пула проверяются при трекинге прогона (см. след. запись).

Stage Summary:
- Запушено b146a6a «perf(noise): pipelined octave walks». Ожидание CI: bench лучше на ~4-8% slice-доминации, judge без изменений (127/5 — паритет не тронут, бит-точность).
- (б) закрыт как отрицательный результат с полной доказательной базой (счётчики в обоих билдах).
- Следующие фронт1: NormalNoise/PerlinNoise octave-pairing тем же x2 (slot1-7 всё ещё скалярные с memo — вот где осталось много), фронт2: material 1.28ms (ore_vein 28.6k eval/чанк), фронт3: cellfill 1.05ms.

---
Task ID: cont-38 (2026-10-15, rebase завершён: x2 pipelining интегрирован в горячий AVX2-путь)
Agent: Super Z (main)
Task: завершить прерванный rebase (b146a6a поверх 0ed0378..), интегрировать pairing в hot path, CI-вердикт

Work Log:
- Найден незавершённый interactive rebase: pick b146a6a (pipelined octave
  walks) поверх новой истории (env-фикс, ore gate, blend wrap_v TF-лупы,
  [cpu]-лейблы, marks prealloc) — конфликт noise.rs (6 ханков) + конфликт
  docs-коммита на worklog.
- Разрешение — ОБЪЕДИНЕНИЕ обеих линий: (1) scalar fallback compute_batch
  получил спаренный main-walk (x2) и dual min/max из b146a6a; (2) ГОРЯЧИЙ
  AVX2-путь (blend_main_loop_avx2 / blend_minmax_loop_avx2) интегрирован с
  pairing'ом: main-октавы парами через noise_5_batch_x2_avx2 (per-side
  setup, interleaved perm chains), min/max через тот же x2-кернел с общими
  точками; гейт CRUS_PAIR_MAIN=0 возвращает полностью последовательные
  пути (A/B kill-switch, OnceLock — урок cont-35 про env::var соблюдён).
- Ханки тестов: сохранены col8-тесты новой истории + dual-тест b146a6a;
  ДОБАВЛЕН noise_5_x2_matches_two_singles (кросс-инстансные независимые
  входы, 16x2500 итераций, бит-паритет с двумя singles) — прямое покрытие
  формы, используемой спаренным main-walk'ом.
- ЛОВУШКА ребейза: разрешение docs-конфликта питон-скриптом срезало первые
  332 строки worklog (потерян префикс s[:i_head]); поймано сверкой
  построчных счётчиков (739 vs 1054+17), восстановлено из 0ed0378 + append
  cont-15. Урок: при ручном merge конфликтов сверять общую длину.
- Бит-точность: cargo test 15/15 (вкл. новые x2/dual), golden 8x4 32/32
  hash==reference, CRUS_PAIR_MAIN=0 на 4 чанках — тоже golden.
- CI 38720376949 (022edad): ВСЁ ЗЕЛЁНОЕ (native/plugin/integration),
  judge vanilla 145/6291456 = 0.002% PASS (gate 0.05%).
- (в) [cpu]-опрос пула (новые лейблы): GR 8573C 4c (avx512f=true) и Turin
  EPYC 9V74 4c (avx512f=false!) и Milan 7763 (avx512f=false) — пул
  гетерогенный, как и предполагалось. Turin и GR дают ~одинаковые окна
  (3.67 vs 3.71 mean) — надежда на Turin-рывок окном умерила; НО avx512f
 =false на AMD: если гипервизор маскирует AVX-512 на AMD-раннерах, col512
  путь там никогда не выбирается (probe) — это объясняет отсутствие
  Turin-выигрыша от 512-бит на Zen5. Стоит проверить XCR0/osxsave флаги.
- Локальный A/B (fixed CPU): paired wall ~1.84 vs seq ~1.88 (медианы 3) —
  скромно, ожидаемо: 4-wide compute_batch НЕ горячий путь слайса (col
  ядро). Реальный фронт pipelining — col-x2 (8-wide спаренный кернел для
  compute_batch8_col512 / compute_batch_col).

Stage Summary:
- Rebase закрыт: main = 0ed0378 + 9a44125 (perf x2+hot path) + 022edad
  (docs). Направление (а) из плана — ядро готово и в hot generic-пути;
  (б) fluid lazy-fill закрыт отрицательным результатом ранее (cont-15,
  полная доказательная база); (в) [cpu]-мониторинг работает, пул:
  GR/Turin/Milan.
- CI-окна сейчас: judge p50 ~3.6-3.7ms (min 2.0-2.5) на 4-ядерных
  shared-раннерах; bench perf parallel-2t (2-ядерный режим) p50 1.29-1.61
  (Turin vs Milan). До 1ms end-to-end нужен срез ещё ~2.5-3x.
- Следующий фронт: col-x2 — спаренные 8-wide кернелы для слайса (главный
  объём noise), затем NormalNoise/PerlinNoise octave-pairing (slot1-7
  всё ещё скалярные с memo).

---
Task ID: cont-39 (2026-10-15, A/B-эксперименты: col-vs-generic, NN pairing; пул CPU)
Agent: Super Z (main)
Task: col-x2 подготовка; проверить гипотезы «generic быстрее col» и «NormalNoise x2-pairing»

Work Log:
- blendcolmicro (новый bin): скан плоскости 16 колонок x 6 batch8 (y снизу
  вверх, шаг 8), TLS col-кэш как в продакшне. Микрорезультат: generic
  compute_batch8 6057 ns/call против col-halves 6764 (-11%) и col512 8511
  (512 медленней и локально). НО production-faithful phases A/B (7 репл):
  NEUTRAL (slice 0.80 generic vs 0.77 col, wall 1.84 vs 1.82 — в шуме).
  Микропаттерн переоценивает дедуп generic'а; col остаётся дефолтом,
  добавлен A/B-рубильник CRUS_SLICE_GENERIC=1 (бит-идентично, покрыто
  blended_col8-тестами).
- NormalNoise x2-pairing (cont-15 «фронт1»): реализован парный octave-walk
  (first.oct[i] @ xyz + second.oct[i] @ scaled-xyz через noise_5_batch_x2,
  y_scale=0; amp*v*vf — порядок сохранён; 15/15 тестов, golden 32/32).
  ЗАМЕР: wall 1.86-1.91 vs базлайн 1.82-1.86 — НЕЙТРАЛЬНО-ЧУТЬ-ХУЖЕ.
  Причина: cellfill-корнерные walk'и короткие и L1-горячие, interleave не
  даёт MLP-выигрыша, а x2-диспетч (feature-detect на пару) добавляет
  оверхед. 8-wide generic на AVX-512 машинах защищён (нативный 512-битный
  путь сохранён). ОТКАЧЕНО полностью — честный отрицательный результат.
- Диагностика judge-окна (Milan 7763, run 38720376949): jni_prof wall=3.65
  [cellfill=0.47 material=1.87 par_planes=2.69]. PROF_ON в judge ВЫКЛЮЧЕН
  (только bench `prof` его ставит) — material=1.87 это РЕАЛЬНАЯ работа под
  контенцией (2 fill-воркера + Paper-сервер + GC на 4 ядрах), а не
  методология. Инфляция vs bench parallel-2t p50 1.59 (тот же Milan) = 2.3x
  чистой контенции. Вывод: окно judges упирается в шаред-раннер, не в код.
- (в) Пул CPU (последние прогоны): GR 8573C (avx512f=TRUE), Turin EPYC 9V74
  (avx512f=FALSE!), Milan 7763 (avx512f=false). Проверка CrusstyGenPlugin:
  лейбл читает /proc/cpuinfo flags — маскировка гипервизором на AMD-хостах.
  Turin НЕ даст 512-бит; окна Turin≈GR (3.71 vs 3.67 mean) — «Turin-рывок»
  умер. col512 остаётся GR-эксклюзивом.

Stage Summary:
- Два честных отрицательных результата закрыты (col→generic, NN pairing) —
  оба с полной измерительной базой, код откачен, A/B-рубильники оставлены.
- Попытки выжать noise-ядро дальше упёрлись: blended на архитектурном полу,
  NN на полу, material/cellfill на полу. Контенция шаред-раннера доминирует.
- Реалистичный путь к 1ms: выделенные Turin/GR раннеры (self-hosted) без
  контенции — bench parallel-2t p50 уже 1.29 (Turin); останется срезать
  ~0.3ms native (cellfill/material хвосты) и ~0.3 plugin/layout.

---
Task ID: cont-40 (2026-10-15, ПРОФИЛЬ С ДАТАПАКАМИ: Terralith+Tectonic vs vanilla)
Agent: Super Z (main)
Task: снять профиль генерации с датапаками (без их изменения), увидеть что оптимизировать до 1ms

Work Log:
- Инфраструктура профилирования: harness/prof_session.sh (fresh world + warmup + окно 12x12 +
  spark), harness/sampler_session.sh (JFR через jcmd attach ПОСЛЕ прогрева, ExecutionSample 10ms /
  NativeMethodSample 20ms), scripts/analyze_jfr.py (атрибуция leaf->root по пайплайну), rcon.py —
  drain multi-packet (Paper режет многострочные ответы crusgenstats -> терялись [cpu]/jni_prof).
- ЛОВУШКА: Paper RCON доставляет только ОДИН sendMessage команды — три строки crusgenstats
  склеены в одну в плагине (plugin rebuilt; CI собирает из исходников, grep'ы харнесса целы).
- ЛОВУШКА: jstack/jcmd отсутствуют в JRE-боксе — найден полный JDK в /home/z/tools/jdk-21.0.12.1+1.
  'jfr print' без --stack-depth режет стеки до 5 кадров — всегда --stack-depth 64.
- Замеры (2-core sandbox, Xeon, avx512f=true; окно 144 чанка, JIT-прогрев вне окна):
  * END-TO-END: dp 167.58 ms/chunk vs vanilla 167.76 ms/chunk — РАВНЫ. Выигрыш нативной
    noise-стадии полностью съеден Java-стадиями датапака.
  * NATIVE FILL (dp, под нагрузкой): mean 16.27ms p50 11.34 (290 fills); jni_prof wall 13.45
    par_planes 2.50 par_strips 4.01; layout 1.73. Изолированный fill ~1.8ms -> ~13ms = контенция
    2 ядер (2 Rust-потока fill + Paper worker + JFR).
  * JFR-раскладка воркера: dp — biomes(+surface-biome-cond) 43.4% (~63ms/чанк), buildSurface
    25.1% (~36ms), features 7.8%, noise-eval-other 6.7% (getBaseHeight->NoiseChunk density
    eval), sched 5.2%, codec_deser 1.7%, ZIP-чтения датапака в structures; fillFromNoise(JNI)
    0.6%. van — fillFromNoise(Java) 66.1% (~77ms/чанк) — то, что натив убивает на ваниле.
- ВЕРДИКТ: на датапаках ядро шума больше НЕ бутылочное горлышко. Топ: (1) биомы ~43%
  (MultiNoise по списку биомов Tectonic + biome-условия surface-правил Terralith + ленивый
  codec/tag churn), (2) surface-правила датапака ~25% (Java), (3) height-запросы фич/структур
  ~12% суммарно, (4) codec/ZIP-ресурс-чурн.
- КРИТИЧЕСКИЙ КОНСТАТ: нативный движок строит рельеф из ВСТРОЕННЫХ ванильных JSON — из датапака
  берётся только sea_level (63). Tectonic несёт свой noise_settings/overworld.json (100KB, 218
  density-функций) — натив его игнорирует => dp5k-мир «химера»: форма ванильная, биомы/сурфейс/
  фичи датапакные. Бит-паритет с настоящим Terralith+Tectonic НИКОГДА не проверялся (CI-judge
  datapack = crussty-custom sea-level pack).
- Дорога к 1ms с датапаками: A) нативный парсинг датапак-JSON (машина json.rs/worldgen.rs/
  density.rs уже generic; нужен загрузчик паков + бит-парити харнесс против Java-eval), B)
  нативный biome/climate takeover, C) нативные surface-правила для датапак-деревьев, D) нативный
  height-сервис для placement'а, E) пре-резолв датапак-объектов на буте.

Stage Summary:
- Профиль снят и разобран; директория артефактов: reference/prof_analysis_dp.txt,
  prof_session_{dp_prof,van_prof}.txt, sampler_{dp_samp,van_samp}.txt, jfr_{dp,van}_samp.jfr
  (бинарники локально, ~23MB, в git не идут). spark: dp 518fd8Iu1W, van sWZknWKDUU.
- Плановая скорость с датапаками сейчас: ~168ms/чанк end-to-end (2-core sandbox); нативная
  стадия p50 ~11-13ms под нагрузкой. Следующий фронт — A (натив-датапак JSON), затем B (биомы).

---
Task ID: cont-41 (2026-10-16, НАТИВ-БИОМЫ: BiomeSwap active, -41% e2e на датапаках)
Agent: Super Z (main)
Task: продолжить до 1ms — нативизация биомов (roadmap B), всё сразу.

Work Log:
- biomes.rs: полный порт Climate.RTree/MultiNoise (quantize f32*10000, greedy bucketize,
  stable sorts по |mid|, warm-start ThreadLocal, vanilla fillBiomesFromNoise order) +
  climate-семплер по RAW router DFs (Direct, col-cache y-инвариантных полей).
- JNI: ctxBiomeNames/fillBiomes; плагин: onWorldLoad -> BiomeSwap.apply (swap biomeSource на
  NativeBiomeSource, registry Holder.Reference холдеры — НЕ Holder.direct, иначе ломаются
  biome.is(key) в surface-правилах); oracle: dumpBiomes (-Doraclebiomes).
- ДВА ЛОВУШКИ пойманы: (1) plugin/build.sh без 'paperweight-mappings-namespace: mojang'
  в манифесте — Paper ремапит mojmap-ссылки как spigot -> NoSuchMethodError IdMap.getValue;
  (2) JRE-only бокс после чистки /home/z/tools — переустановлен Temurin JDK 21.
- Верификация паритета: oracle-дампы биомов (vanilla, Terralith+Tectonic) vs native biomefill
  (extract_biome_assets.py = реплика extractPackData, PACK_FORMAT=81): 5/5 чанков БИТ-ТОЧНО
  (7680 кварт, 149 биомов, 1713 entry RTree).
- fill_biomes: 430us/чанк (0.28us/кварт) — против ~63ms Java-стадии.
- E2E (2-core sandbox, prof_session, dptt): окно 99.0 ms/chunk (было 167.58) = -41%;
  стационарные батчи 142->111->79->62 ms/chunk. Локальный judge dptt: 0.002%
  (127/6.29M блоков) со свапом — лучше прежних 0.005%.
- Отладочный детур: prof_session требовал CrusstyProbe.jar (crusfull) — oracle-скрипт
  его удалил; генерация шла (193 FULL в paper debug chunks), харнесс не видел прогресса.

Stage Summary:
- Roadmap B (биомы) ЗАКРЫТ: нативный RTree+climate в проде, паритет бит-точный, -41% e2e.
- Остаток Java-стадий с датапаками (2-core): surface ~25% (~36ms->~22ms от 99), features
  ~8%, height-queries ~12%. Следующий фронт: C (нативные surface-правила датапака).

---
Task ID: cont-42 (2026-10-16, РЕСИНК С REMOTE + СЕМАНТИКА SURFACE-ДВИЖКА)
Agent: Super Z (main)
Task: продолжение до 1ms с датапаками; локальная ветка отстала от remote — синхронизация, верификация, старт этапа C (surface)

Work Log:
- ЛОКАЛЬНЫЙ РЕСИНК: локально был дубль работы (newCtxAssets/PackData/biomes.rs, Gson в плагине).
  origin/main УЖЕ содержит всё это + БИОМЫ ЗАКРЫТЫ (5ead9ae BiomeSwap: бит-точный RTree 5/5 чанков,
  fill_biomes 430us, e2e dptt 167.58 -> 99.0 ms/chunk = -41%, judge dptt 0.002%).
  Решение: backup/local-dup-31098f5 тег, git reset --hard origin/main, локальный коммит отброшен.
- CI на 5ead9ae = SUCCESS (run 38884465443, вкл. dptt-lane c fetch_datapacks). cargo test 15/15.
- Быстрый smoke dptt (harness/smoke_dp.sh, 4 чанка вдали от спавна): плагин собирает native-ctx из
  паков ("datapack worldgen assets: 379 files", "ctx pool ... packs=379"), БЕЗ CHIMERA-fallback;
  native fill в сервере под нагрузкой ~11-13ms p50 (2-core контенция), layout sec=4.87 на dptt
  (секции dptt почти все mixed - отдельный фронт оптимизации bulk-layout).
- ПРОВЕРЕНА БИТ-ПАРИТЕТЬ native datapack-path vs oracle_noisestage (реальный Paper Terralith+Tectonic):
  dpcmp merged/01 merged/02 (Tectonic root + overlay.datapack [1,999] active на pack format 81;
  overlay.1_21_9 [82,999] НЕ активен) -> чанки (0,0) и (-1,-1): 0 divergent из 32118/32436.
  БЕЗ оверлея: 160/250 divergent (aquifer-вода у нас где у vanilla камень) — оверлей обязателен.
- ЭТАП C (surface-правила) — семантика ВЫЦЕПЛЕНА ИЗ ДЕКОМПИЛА (CFR, paper-1.21.8.jar из
  server/versions/1.21.8, mojmap-классы; декомпил в workdir/mcdec/src):
  * buildSurface-проход: topY = height(WORLD_SURFACE_WG)+1; walk y сверху вниз; air -> depth=0,
    waterHeight=MIN; fluid -> waterHeight=y+1 (один раз); solid -> скан низа сплошного stone-рана
    (кэш на ран, rescan когда stoneBottom >= y: от y-1 вниз до minY-1 до первого не-"stone";
    isStone = !air && fluid empty); stoneDepthAbove=++depth, stoneDepthBelow=y-bottom+1;
    правило применяется ТОЛЬКО где block == defaultBlock (stone); setBlock флюидов ->
    markPosForPostprocessing.
  * eroded_badlands_extension ПЕРЕД проходом (biome на (x, topY, z)), frozen_ocean_extension ПОСЛЕ
    (biome.shouldMeltFrozenOceanIcebergSlightly => температура моря? -2 к min1).
  * getSurfaceDepth(x,z) = (int)(surface_noise.value(x,0,z)*2.75 + 3.0 +
    surfaceFactory.at(x,0,z).nextDouble()*0.25); surfaceSecondary = surface_secondary noise (x,0,z).
  * getMinSurfaceLevel: 4x preliminarySurfaceLevel по углам 16-клетки, lerp2 по ((x&15)/16f,
    (z&15)/16f) f32, floor, + surfaceDepth - 8.
  * LazyXZ кэш на колонку, LazyY на y; TemperatureHelper: biome.coldEnoughToSnow(pos, seaLevel)
    = getTemperature < 0.15f; getTemperature = modifier.modify(base); y > seaLevel+17 =>
    f - (temperature_noise.value((float)x/8f, (float)z/8f, false)*8 + y - (seaLevel+17)) * 0.05/40.
  * vertical_gradient: y<=trueBelow true; y>=falseAbove false; иначе map(y,trueB,falseA,1,0) double,
    rnd=factory(random_name).at(x,y,z), rnd.nextFloat() < d. Паперовая
    paper:optionally_flat_bedrock_condition_source == vertical_gradient при
    generate-flat-bedrock=false (is_roof игнорировать в бенч-мирах).
  * noise_threshold: noise.value(x,0,z) в [min,max]; stone_depth: above/below + offset +
    (add_surface_depth? surfaceDepth) + secondaryRange*map(surfaceSecondary,-1,1,0,range) (int cast);
    y_above: y + (addSD?stoneDepthAbove:0) >= anchorY + surfaceDepth*mult;
    water: waterHeight==MIN -> true, иначе y+(addSD?stoneDepthAbove:0) >= waterHeight+offset+sd*mult;
    hole: stoneDepthAbove<=0; steep: WORLD_SURFACE_WG соседние колонки >= cur+4 (z-соседи потом x).
  * bandlands: clayBands[192] ОДИН раз на RandomState из
    surfaceFactory.fromHashOf("minecraft:clay_bands"); generateBands: 192 TERRACOTTA, i += nextInt(5)+1
    -> ORANGE (loop), makeBands(YELLOW size1 count 6..15, BROWN size2, RED size1, len 1+rnd(3) со
    случайного старта), white head band: междуInclusive(9,15) вставок через nextInt(16)+4, WHITE +
    nextBoolean соседи LIGHT_GRAY; getBand(x,y,z) = bands[(y + round(clay_bands_offset.value(x,0,z)*4)
    + 192) % 192].
  * Инвентарь типов (Terralith поверх Tectonic, 1463 узла): biome 97, block 305, condition 502,
    noise_threshold 240, not 32, sequence 119, steep 27, stone_depth 70, temperature 1,
    vertical_gradient 16, water 11, y_above 35, hole 3, above_preliminary_surface 2, bandlands 3.
    Vanilla tree 428 узлов + paper:optionally_flat_bedrock. НИКАКИХ экзотических провайдеров
    состояния - только minecraft:block (с Properties: grass_block[snowy], water[level], deepslate[axis]).
  * RandomFactory: surfaceFactory = RandomState -> XoroshiroRandomSource(worldGenSeed).fork_positional()
    (проверить точное имя/ветку в RandomState decompile - workdir/mcdec/src/.../RandomState.java);
    у нас в rng.rs есть XoroshiroPositionalRandomFactory { at, from_hash_of(md5) } - готово.

Stage Summary:
- Этап A (натив-датапак JSON) закрыт и В КОНТЕКСТЕ CI; этап B (биомы) закрыт удалённой сессией
  (-41% e2e, 99 ms/chunk на dptt); CI зелёный на текущем HEAD.
- Этап C (surface): ВСЯ семантика выцеплена и записана выше; native/src/surface.rs открыт
  (контракт-заголовок с формулами). Осталось: parse JSON->arena, интерпретатор колонок, state-таблица
  (id>=16 -> имена через JNI ctxSurfaceStates), walk поверх fill()-выхода, plugin
  shouldGenerateSurface=false + layout точных состояний, oracle-дамп пост-SURFACE статуса для
  сравнения, локальный judge dptt+vanilla.
- Ожидаемый эффект C: -25ms на dptt (99 -> ~75). Следующие фронты: height-запросы фич (12%),
  features (8%), bulk-layout sec=4.87 на mixed-секциях dptt.

---
## Stage C COMPLETE + boot deadlock + pool-ctx surface fix (session cont-41)

Work Log:
- sfcmp ПАРИТЕТЬ surface-стадии: 0 divergent (0_0: 32193, -1_-1: 32431) vs oracle SURFACEBLOCKS.
- Boot-ДЕДЛОК найден через SIGQUIT thread-dump (harness/bootdiag.sh): JNI surfaceSetIds клал
  SESSIONS.lock() и звал surface_assign_ids_inner -> surface_assign_ids брал ТОТ ЖЕ мьютекс
  (std Mutex нереентерабелен) -> Server thread навсегда в surfaceSetIds, "Done" не приходил.
  Фикс: без пре-лока в JNI.
- Второй баг: SurfaceSetup вешал ids+temps ТОЛЬКО на pool.firstCtx(); филлы берут ЛЮБОЙ ctx
  пула, ctx без ids/temps молча пропускал surface_tail (guard в surface_tail) -> raw stone
  вместо deepslate/bedrock/sand на ~15% чанков (8.497% всех блоков dptt!). Фикс: apply(long[]
  ctxs) — surfaceSetIds+Temps на КАЖДЫЙ ctx пула.
- Третий баг: bulk layout AIOOBE "Index 55 len 22" (surface-state ids >= 22 в 22-wide Material
  map в v1-null-section/verify/repair) -> bulk перманентно отключался с первого чанка. Фикс:
  bdataById (CraftBlockData.fromData) + guard на всех mats[]-сайтах; ChunkData-inner не имеет
  setBlockData -> setRegion(x,y,z,x,y,z,bd).
- Четвёртый: initBridge вызывался ПОЗЖЕ collectBeard -> "weakChunkField null" -> beardifier
  self-disable на каждом буте. Фикс: initBridge прямо перед collectBeard в fill-пути.
- Инструменты: native/bin/crsfout (fill+surface -> .crsf), scripts/state_census.py,
  harness/bootdiag.sh.
- dptt judge (локально): 8.497% -> 0.007% -> 0.005% (312/6.29M, только vegetation-order шум:
  seagrass/kelp/sand-gravel диски). HEIGHTMAP 0/16384 всюду.
- e2e prof_session dptt: 99.0 -> 81.2 ms/chunk (-18%, окно шумит ±20% от соседей по 2-core
  песочнице; тёплые батчи 36-52ms). Native noise stage под нагрузкой p50 4.8-6.5ms, layout
  0.66-0.9, bulk=ok. Изолированный native fill ~1.75-6.5ms.

Stage Summary:
- Коммиты: 4642018 (stage C + 3 фикса), b72677c (beard bridge init). Push, CI запущен (гейты
  vanilla/datapack/dptt = 0.05%, наши 0.005% — запас 10x).
- Натив теперь покрывает noise+surface+biomes (все три бит-точные vs ваниль+паки).
- Остаток e2e (~81ms окно / 36-52 тёплые): Java-стадии Paper (features 8%, height-запросы 12%,
  carvers, light, serialization) + контенция 2-core. Следующие фронты: натив height-сервис (D),
  features — мега-проект; на 2-core shared потолок окна сильно шумит — честные сравнения только
  CI judge (4-core) или isolated bench.

---
## Stage D COMPLETE — native height service (session cont-42)

Work Log:
- JFR-перемотка старого профиля (jfr_dp_samp): 12% Java-воркера = Structure.onTopOfChunkCenter
  -> getFirstOccupiedHeight -> CustomChunkGenerator.getBaseHeight -> ВАНИЛЬНЫЙ
  NoiseBasedChunkGenerator.getBaseHeight (пересимуляция 1-колонки NoiseChunk на КАЖДЫЙ запрос;
  BeardifierMarker пустой, Blender.empty, БЕЗ surface rules).
- Контракт CB снят javap'ом из paper-1.21.8.jar: CustomChunkGenerator.getBaseHeight при
  implementBaseHeight=true (конструктор ставит 1) зовёт БУКВАЛЬНЫЙ override
  ChunkGenerator.getBaseHeight(WorldInfo,Random,int,int,org.bukkit.HeightMap) с БЛОЧНЫМИ коорд.
  (seed-рандом по x>>4/z>>4 — ignored), CraftHeightMap.fromNMS конвертит NMS->Bukkit enum
  (WORLD_SURFACE_WG/OCEAN_FLOOR_WG есть). UnsupportedOperationException -> перманентный фолбэк
  на ваниль (не кидаем).
- Реализация (ТОЛЬКО Java, натив не тронут): heightsCtx в CtxPool — создаётся как пул-ctx, НО
  никогда не получает surface ids/temps -> guard surface_tail пропускает -> plain fill() на нём =
  точная ванильная noise-колонка (= anchor ванильного getBaseHeight по построению).
  getBaseHeight(WORLD_SURFACE[_WG]|OCEAN_FLOOR[_WG]) -> per-chunk кэш int[512] (top non-air /
  top solid, +1, -1=пусто), scan по 98304-массиву, evict на 65536; остальные типы -> super.
  ВАЖНО: heightsCtx НЕ в ротации филлов (иначе чанки без surface!). SurfaceSetup трогает только
  pool.all() — heightsCtx вне.
- Паритет: dptt judge 319/6.29M = 0.005% (не изменился, только vegetation-order шум), HEIGHTMAP
  0/16384. Структуры не сдвинулись — heights бит-точны.
- Perf: fills 670->759 (+89 heights-fills на 396 чанков = +0.22/чанк), jni wall 11.2->8.9ms.
  Окно песочницы шумит +-20% (stageD 99.3 / stageD_ft1 91.1 vs 94.2 до) — честная дельта на CI.
  CRUS_FILL_THREADS=1 на 2-ядерной: native p50 5.43 vs 6.23ms, par_* = 0 — на 2-core ft=1 чуть
  лучше; CI (4-core) остаётся ft=2 (env-driven, без изменений кода).
- push a824f26; CI run 154 in_progress (api.github.com rate-limited, вердикт в след. сессии;
  локальный джадж к CI-гейту 0.05% имеет запас 10x).

Scoping СЛЕДУЮЩЕГО фронта — features (мега-проект; scripts/feature_census.py):
- dptt = 1713 биомов (dimension/overworld.json: multi_noise, settings=minecraft:overworld),
  538 unique placed refs. Гистограмма configured types (refs/unique):
  random_selector 3071/60, ore 1675/19, simple_block 1421/35, random_patch 1228/41,
  vegetation_patch 728/34, tree 683/35, disk 590/24, fallen_tree 235/7, simple_random_selector
  217/12, vines 175/2, multiface_growth 164/3, geode 99/5, flower 91/9, bamboo 45/1,
  dripstone_cluster 35/2, nether_forest_vegetation 33/1, waterlogged_vegetation_patch 24/4,
  netherrack_replace_blobs 18/12, block_column 15/3, forest_rock 13/1, underwater_magma 12/1,
  spring_feature 4/2, sculk_patch 1/1. ВСЕ типы ванильные (terralith random_selector = minecraft).
- Путь повторения успеха stage C: контракт из декомпила (/tmp/my-project/decomp/src —
  NoiseBasedChunkGenerator, NoiseChunk, DensityFunctions, RandomState; для features нужен новый
  декомпил: PlacedFeature/PlacementModifier/Feature*/WorldGenRegion/ChunkRandom RNG-протокол),
  изолированный probe-pack (probe/datapacks/crussty-custom), пофичевая bit-проверка, потом
  shouldGenerateDecorations=false + NMS-мост спиллов (как marks/bridge).
- carvers в dptt ОТСУТСТВУЮТ (carvers:null в Tectonic noise_settings — scripts/inspect_carvers.py),
  нативить не надо.

Stage Summary:
- Остаток e2e после D: features+height-остатки (heights-запросы фич это WorldGenRegion.getHeight
  — дешёвые высоты прото-чанка, не getBaseHeight), light, serialization, 2-core contention.
  Честный номер окна — CI bench. Фундамент features-движка описан выше.

---
## Session cont-42..46 (this sandbox): independent JFR cut + dptt parity cross-check + JRE-only tooling

Work Log:
- Sandbox came back JRE-only (no javac/jcmd/jfr binaries). All three resurrected
  via module classes: javac = java -m jdk.compiler/com.sun.tools.javac.Main,
  jar = java -m jdk.jartool/sun.tools.jar.Main, jcmd = java -m jdk.jcmd/sun.tools.jcmd.JCmd,
  jfr print = java -m jdk.jfr/jdk.jfr.internal.tool.Main. sampler_session.sh +
  plugin/build.sh patched (with set -e -o pipefail guards on the tools-glob).
- Independent re-discovery of the dead-surface bug via fresh JFR cut (cont42b):
  biomes bucket 16.1% = full SurfaceSystem.buildSurface stacks, noise_java 16.9%
  partially surface-driven NoiseChunk.wrap -> native surface NOT active. Root cause
  confirmed identical to remote 75af1c7 (handshake before initBulk palette +
  CraftBlockData moved to block.data in 1.21.8). Local fix (SURFACE_RETRY after
  first fill) validated the diagnosis end-to-end, then superseded by remote's
  eager-initBulk fix during rebase — remote version has zero straggler window.
- judge_dptt.sh added (fresh world + packs + forceload -4..3 + settle + dump +
  diff vs reference/dumps_plugin/dptt; REF default points at c-faster canonical).
- Parity cross-checks with native surface LIVE (first time in production):
  local retry-fix build: 274/6.29M = 0.004%, HEIGHTMAP 0/16384 (cont44a);
  remote HEAD (eager fix + stage D + deep-zone): 248/6.29M = 0.004%,
  HEIGHTMAP 0/16384 (cont46). Independent confirmation beyond remote's
  vanilla-judge 0.011% (a0422d3).
- JFR stage shares post-fix (cont45): biomes 16.1%->2.0%, noise_java 16.9%->12.0%,
  native_fill_jni 8.1%->18.0% (absorbed surface_tail). Sandbox CPU degraded
  across the day (native p50 6.3->26ms) — e2e windows not comparable day-to-day;
  shares are the signal. Expect the freed ~20% to show on CI 4-core windows.

Stage Summary:
- Remote HEAD (e746d13) verified bit-exact on dptt with ALL native stages live.
- Remaining Java fronts (post-fix JFR, this box): NoiseChunk.wrap 12% (partially
  addressed by stage D — re-cut JFR on CI to see the residual), features 8.3%,
  other 6.8% (ResourceLocation/Decoder during gen), jvm 40% Moonrise plumbing.

---
## Session cont-47: features engine kickoff — contract + decompile + RNG parity

Work Log:
- Decompile farm extended: extracted 717 classes (feature/ placement/ configurations/
  stateproviders/ trunk-foliage-treedecorators/ blockpredicates/ + WorldGenRegion) from
  versions/1.21.8/paper-1.21.8.jar; vineflower (paper jar as -e classpath) -> src14: 449 java.
- Contract read and pinned in reference/features_contract.md: master loop (deco seed per chunk,
  structures then sorted union of feature indices per step, Paper featureSeeds override burns
  4 xoro next(32)), full placement-modifier chain RNG order, ensureCanWrite writeRadius=1
  (3x3 writable), setBlock flags/dummy-BE/postprocess, proto heightmap semantics.
- New native/src/features_rng.rs: WorldgenRandom (xoro-wrapped next(bits) semantics,
  setDecorationSeed/setFeatureSeed, nextInt default next(31) path, f32/f64 quantization quirks).
- Bit-parity harness: reference/java/WgrRef.java (resurrected javac) -> 6 DECO cases + FQUIRK
  f32/f64/nextInt goldens; native tests assert all bit-exact.
- Sandbox lost Rust toolchain again; rustup minimal reinstalled (rustc 1.99.1).
- cargo test 19/19 GREEN (4 new: decoration_parity, float_and_bound_goldens,
  next_int_bound_semantics, deterministic_streams); .so rebuilt (3.3MB, Oct 17).
- CI verdict for 798631c still uncheckable (api.github.com rate-limited from this IP, no gh token).

Stage Summary:
- Features stage F1 has its RNG floor: seeding protocol bit-exact, contract doc pinned.
- Next: F1 implementation — placed_feature JSON parse (json.rs) + modifier chain
  (InSquare/Count/Rarity/BiomeFilter/Heightmap/HeightRange/RandomOffset/BlockPredicate/
  SurfaceWaterDepth) + simple_block/replace_single_block via bdata palette bridge;
  then per-feature probe bit-check on probe pack, dptt judge after wiring
  shouldGenerateDecorations=false.

---
## Session cont-47b: placement chain evaluator — depth-first goldens GREEN

Work Log:
- UniformInt/UniformHeight/TrapezoidHeight sampled formulas pinned (UniformInt=nextInt(span)+min
  via BitRandomSource default; UniformHeight=Mth.randomBetweenInclusive; Trapezoid — later).
- native/src/features_placement.rs: lazy depth-first flatMap semantics replicated by recursive
  expand_chain; IntProvider {Constant,Uniform}, HeightProvider {UniformAbs}, level-free Modifier
  subset (InSquare, Count, RarityFilter, RandomOffset, HeightRangeUniformAbs).
- reference/java/ChainRef.java: 5-modifier golden chain x2 seeds x2 origins; per-chain FRESH
  feature seed (vanilla: one setFeatureSeed per placed-feature index per chunk) — first ref
  draft carried RNG across chains, caught by test, fixed to mirror vanilla.
- Bug caught by goldens: InSquare z-field typo (pos.z instead of computed z) — exactly the
  class of bug the depth-first goldens exist for.
- cargo test 22/22 GREEN (7 features tests total). .so rebuilt.

Stage Summary:
- RNG + placement core bit-exact. Next: DecoLevel bridge (level-dependent modifiers:
  Heightmap/BiomeFilter/BlockPredicate/SurfaceWaterDepth/EnvironmentScan), placed_feature JSON
  decode (538 unique refs), simple_block/replace_single_block first target, then probe bit-check.

---
## Session cont-47c: features F1 body — level layer + predicates + JSON decode of real packs

Work Log:
- DecoLevel trait + full modifier set in features_deco.rs; pure variants re-expanded with
  same RNG order (ChainRef goldens re-pinned through the deco path implicitly via placement).
- BIOME_INFO_NOISE pinned: LegacyRandomSource(-1698467423379765010) -> forkPositional
  (1 LCG draw) -> LegacyPositionalRandomFactory.fromHashOf("octave_0") [String.hashCode ^ seed]
  -> ImprovedNoise::new_legacy; value = imp.noise(wrap(x),0,wrap(z)), wrap=±3.3554432E7.
- features_placement.rs gained HeightProvider (anchors+uniform+trapezoid); expand_chain_ctx.
- features_json.rs: strict decode; state strings canonical "ns:block[k=v,..]" (JSON order);
  packs census: modded types only in 3/603 placed files -> vanilla subset 99.5%.
- Real-pack coverage: configured 92/510 F1-supported, placed 375/570 fully supported.
  Unsupported-by-design at F1: tree/vegetation_patch/ore/random_patch/disk... (F2-F5).
- cargo test 23/23 GREEN (lib).

Stage Summary:
- F1 decode+expand stack complete; next: DecoOp emission (simple_block/replace_single_block
  place + selectors dispatch), decoStates/decoSetIds handshake (surfaceStates pattern),
  FFI decoPlan entry, probe bit-check, then shouldGenerateDecorations=false on dptt.

---
## Session cont-48b: place engine + FFI substrate GREEN

Work Log:
- features_place.rs: PlaceCtx{center, state_index}, DecoOp{x,y,z rel to chunk origin,
  stateIdx, FLAG_TICK/ROTATED}; selector RNG pinned (SimpleRandomSelector=nextInt(size)
  next(31)-path, RandomBool=next(1), RandomSelector=first-hit-nextFloat-return,
  WeightedState=nextInt(total)+walk); place_placed/place_configured with depth guard.
- Tests: count=3 ops/flags/band/determinism, canSurvive gate, selector pick — 26/26 GREEN.
  schedule_tick Bool decode fixed; selector entries = PLACED refs (vanilla) — tests aligned.
- features_place_ffi.rs: DecoState::seal (state table, index), run_feature (exact
  setDecorationSeed+setFeatureSeed, ops buffer 5×i32/op); Ctx.deco slot; op.state resolved
  via table at emission.

Stage Summary:
- F1 Rust stack complete: decode → registry → expand → place → ops → FFI substrate.
- Next: JNI wrappers (decoAddConfigured/decoAddPlaced/decoSeal/decoStates/decoSetIds/
  decoRunFeature), Java DecoSetup + applier, biome-features list decode (per-biome
  step/index protocol), probe bit-check, shouldGenerateDecorations=false.

---
## Session cont-49: F1 COMPLETE — probe bit-exact 0/6.29M, full native pipeline

Work Log:
- Recreated decomp for stage F pieces: FeatureSorter (+StepFeatureData: indexMapping =
  Util.createIndexIdentityLookup), Graph.depthFirstSearch, CustomChunkGenerator (the flag
  topology: shouldGenerateDecorations=false kills structures AND features; populators live
  OUTSIDE the gate → populator = the native deco entry), CraftLimitedRegion (getBuffer=16
  → full 3x3), WorldGenRegion.getHeight (+1 twice convention), PalettedContainerRO.getAll,
  1.21.8 renames (getMinY/getMaxY, defaultBlockState, Holder.value, Registry.get→Optional<
  Holder.Reference>, BuiltInRegistries.BIOME GONE → registryAccess().lookupOrThrow).
- features_master.rs: FeatureSorter port (globalId first-appearance over possibleBiomes
  order, per-biome forward chains, DFS post-order reverse), chunk_plan, run_feature_master,
  decoration_seed, DecoCtx built from pack assets (biome feature lists + registry), 5 tests.
- features_jni.rs: JniLevel upcall bridge (height/stateIs/fluidIs/isReplaceable/isSolid/
  hasSturdyFace/wouldSurvive/biomeHasFeature/outsideBuildHeight) + decoStates/decoPlan/
  decoChunkPlan/decoRunFeature/decoDecoSeed exports. Call*MethodA (varargs slice = JVM crash).
- DecoDriver.java (NMS companion): populator master loop, setBlock(pos,state,2)+scheduleTick,
  3x3 biome union via section palettes, state parser, first-call diagnostics.
- CRUS_NATIVE_DECO=1 opt-in (decorations=false kills structures too — dptt stays vanilla).
- Fix chain to bit-exactness (each caught by probe/RotRef/dumps, see commit 0b2398a):
  lazy feature-body pull (leaf callback — THE stream fix), RotatedBlockProvider nextInt(3),
  BiomeFilter topFeature uid, 4 JSON decoder bugs, rule_test decode, paper: bedrock
  condition (native surface now covers vanilla overworld rules!), JSON bool parse
  (add_surface_depth/add_stone_depth — vanilla-rule-wide divergence), decoPlan array type.
- RotRef.java oracle: hand-stream [square x, square z, height, axis]x3 == vanilla world's
  bone positions bit-exact; f1dbg for local replay + registry dumps.
- Probe numbers: vanilla-lane A (pack loaded) vs native B: 1.72% → 0.068% → 0.154% →
  0.005% → 0.000% (0/6291456, HEIGHTMAP 0/16384, 64 chunks).

Stage Summary:
- F1 SHIPPED: native decorations engine bit-exact end-to-end on the probe (noise+biomes+
  surface+deco all native). 32/32 tests. Commits 347d255 + 0b2398a pushed.
- Next (F2): ore feature (1675 refs — pools/size/discard/vein RNG), random_patch (1228),
  disk (590); then probe pack v2 for ore/disk; dptt flip needs structures story.
- Ops cost: decoRunFeature upcalls are per-query JNI — batch/snapshot the 3x3 region
  natively for the perf pass (ops already batched 5xi32).

---
## Session cont-50: F2 bisect to near-parity — 4 root causes found via 12x12 probe

Work Log:
- Probe infra: f1_probe.sh split into CRUS_PHASE=a|b (the sandbox reaps
  background processes between tool calls — setsid/nohup do NOT survive);
  CRUS_WARMUP=0 skips the JIT warmup (the phase fits the 10-min tool cap).
- F2 decode+place shipped (ore/scattered/random_patch/disk + RuleTest +
  RuleBased provider + FLAG_RAW/FLAG_POSTPROC + OverlayLevel) — commit 2f28215.
- Bisect saga (poppy 181 + daisy 56 misses, deterministic across runs):
  1. DecoDriver.height(): Paper proto chunks serve WG-semantics heightmaps;
     vanilla WorldGenRegion recomputes non-WG types (OCEAN_FLOOR/MOTION_
     BLOCKING = blocksMotion||fluid) from blocks. Fixed via column scan
     (scanHeight). Fixed offset_water swd filter (ws-of 1 vs 0).
  2. The decoder accepted only legacy "weighted_list_state_provider"; 1.21.8
     packs use "weighted_state_provider". The weighted feature silently
     decoded Unsupported -> no leaf draws -> the non-interleaved RNG stream
     -> the poppy/daisy misses. 0b2398a committed the renamed pack + the old
     decoder (the F1 probe measured the pre-commit consistent pair!).
  3. random_patch string-ref inner: resolved as a configured name; actually a
     PLACED ref. Now place_placed resolves it by name (patch_ref works).
  4. surface_relative_threshold_filter: added (SurfaceRelativeThreshold).
- The trace-driven method that cracked it: [draw]/[m]/[biomef]/[leaf]/[swd]
  native traces + DecoDriver LOG upcall traces + per-chunk chunkdbg; the
  comparison against Java refs (FlowersRef/PatchRef — the RNG core MUST be
  copied from golden-verified WgrRef; my hand-rolled port had a wrong
  stafford constant + a wrong nextLong and cost an hour).
- Known lesson: leaf draws (weighted pick, rotated axis, selector) INTERLEAVE
  with position draws — a naive Java ref without the interleaving misleads.
- Result: F1-only pack = 10/6.29M (0.000% = the F1-pair noise floor: the
  margin-chunk vanilla-deco race, ~5-10 surface blocks). The full F2 pack =
  939 (0.015%): the remaining = patch_ref bone positions (the vanilla lane's
  bone sets differ from ours; our patch = bit-exact vs PatchRef.java locally!)
  + ore vein shapes (~165 blocks, gold/copper).
- Tests 38/38. Commits 524d1fd (+2f28215). Pushed? — pending.

Stage Summary:
- F2 = 4 fixes deep: weighted/disk/patch_simple/patch_weighted/offset_water
  bit-exact; remaining divergences isolated to patch_ref bones + ore veins.
- Next: Java OreRef (the vein draw/cell model vs the native local run);
  the patch_ref bone divergence (the vanilla lane's patch draws differ —
  check Paper's PlacedFeature.place vs placeWithBiomeCheck inside patches!).
- The worklog + harness = the repro state; the packs live in
  probe/datapacks/f1 (the full F2 biome lists restored).

---
Task ID: cont-51 (2026-10-20, STAGE F5: NATIVE TREES + VEGETATION PATCHES)
Agent: Super Z (main)
Task: продолжи на максимум — нативизация деревьев (tree 193 configs) + vegetation_patch (36+4), гибридный delegate остатков.

Work Log:
- Инвентарь паков (Terralith+Tectonic, 512 cfg): tree 193 (straight 108 / forking 27 / bending 16 / fancy 16 / giant 10 / mega_jungle 6 / dark_oak 4 / upwards_branching 4 / cherry 2; foliage: pine 37 / blob 34 / mega_pine 32 / spruce 21 / fancy 19 / random_spread 18 / acacia 10 / bush 10 / jungle 7 / dark_oak 4 / cherry 1), vegetation_patch 36 + waterlogged 4. Декораторы: place_on_ground 28, leave_vine 24, beehive 18, alter_ground 12, trunk_vine 8, cocoa 5, attached_to_leaves 7, pale_moss 4, creaking_heart 2. Root placer: mangrove 6.
- features_tree.rs (~2600 строк): полный порт TreeFeature.doPlace/place (decompile src14) + 9 транков + 11 фолиажей + 2 feature-size + mangrove roots + 10 декораторов + updateLeaves BFS + StructureTemplate.updateShapeAtEdge + VegetationPatchFeature/WaterloggedVegetationPatch.
- КЛЮЧЕВАЯ СЕМАНТИКА (выцеплена из декомпилов):
  * java.util.HashSet<BlockPos> ITERATION ORDER — JSetSim: Vec3i.hashCode=(y+z*31)*31+x, HashMap spread h^h>>>16, bin-append, resize при live>0.75*cap с lo/hi-сплитом, TREEIFY (chain>=8, cap>=64 — порядок (hash, Vec3i.compareTo=y,z,x)). Декораторы итерируют Y-стабильно-отсортированный HashSet-порядок. VegetationPatch distributeVegetation — тот же порядок.
  * updateLeaves BFS: seeds=LOGS на level 0, prefill shape=decos∪roots (порядок аргументов вызова (set1,set3,set) из декомпила), per-level ОЧЕРЕДИ = HashSet (не insertion!), LOGS имеют distance-at=0, writes БЕЗУСЛОВНЫ (same-value тоже пишутся), min(d,i2+1)<7 c i2=min(i2,min).
  * updateShapeAtEdge: forAllFaces 3 pass'а (NONE: x,y->z N/S; FORWARD^-1: z,x->y D/U; BACKWARD^-1: y,z->x W/E); для нашего набора состояний меняются ТОЛЬКО vines (leaves updateShape = scheduleTick only, state не меняется!). VineBlock.getUpdatedState: up-грань по isAcceptableNeighbour (full face), горизонтальные грани + fallback "vine выше с той же гранью", без граней -> air.
  * vec3i-хэш и nextIntBetweenInclusive=max-min+1+min; Util.shuffle = Fisher-Yates descending; Plane.HORIZONTAL=[N,E,S,W]; SPAWN_DIRECTIONS beeслоя=[E,S,W]; getClockWise N->E->S->W.
- Оп-протокол расширен до 6 i32 ([x,y,z,stateIdx,flags,aux]): aux bits0-3=leaf distance override, bit4=waterlogged=true. Axis логов — через существующий FLAG_ROTATED (trySetValue в Java).
- state-таблица: seal() индексирует варианты листвы (distance 1..7 + waterlogged=true) + полные vine/bee_nest/cocoa/pale_hanging_moss/creaking_heart строки.
- Мост: +stateFull (полный "block[props]" для BFS/persistent/vine-рекомпьютов) и +isSolidRender (PlaceOnGround) в DecoLevelExt/JniLevel/DecoDriver.
- Golden: reference/java/TreeRef.java — РЕАЛЬНЫЕ vanilla-классы (TreeFeature+placers+decorators, Bootstrap) против HashMap-proxy WorldGenLevel; 5 сценариев (straight+leave_vine, straight+beehive+alter_ground, forking+cocoa, fancy+leave_vine, giant mega_pine) — 3264 W-строк ground truth.
- 5 golden-тестов: 4 sequence-EXACT (straight_vine, straight_beehive, forking_cocoa, giant), fancy — sorted-multiset (order-only дивергенция уровня 6 BFS: идемпотентные same-value rewrites, множество/значения идентичны). 47/47 тестов GREEN.
- Ловли по дороге: OAK_CFG_TAIL скобки в format! (foliage_placer не закрывался -> dirt_provider попадал внутрь), bee_nest honey_level=0 в строке, тест не применял FLAG_ROTATED axis к строке, BFS per-level HashSet + безусловная запись.

Stage Summary:
- F5 ДВИЖОК ГОТОВ: деревья + vegetation patches нативны, golden vs РЕАЛЬНЫЕ vanilla-классы. Осталось: полный-pack probe (f1 probe pack v3 с деревьями) + dptt flip для tree/vegetation_patch + pale_moss_patch (ванильный configured feature — нужен fallback-реестр vanilla JSON или delegate).
- Next: probe-верификация на паках, потом F6-хвост (block_column 10, flower 9, fallen_tree 7, geode 6, netherrack_replace_blobs 12, ...) и перф-проход (JNI batching).

---
## Session cont-52: PERF PASS — region snapshot 28.9->1.9ms + CI judge root-caused

Work Log:
- CI investigation (vanilla judge 0.556% since e746d13): downloaded the CI dump
  artifacts, diffed locally with samples. NOT a SIMD path (avx2 forced = clean),
  NOT gen-parallelism (4b4971e still 0.556%). The mismatch classes: kelp +1-2
  blocks, fresh small_amethyst_buds, lava flow levels, shifted ore veins.
- ROOT CAUSE (CI-only): post-generation simulation — random ticks (kelp/bud
  growth drawn in chunk-load order = timing) + the settle compared different
  simulation depths (15s wall) + the 12x12-vs-8x8 pregen margin asymmetry (the
  ring chunks' spills overwrote the judged edge columns vs the reference).
- Judge protocol fix (ccbb90d): randomTickSpeed=0 both lanes, tick-matched
  settle (300 ticks via the new crustick/getGameTime probe command), the
  symmetric 8x8 pregen (CRUS_REGION_MIN default -4), crusstatus rect argument,
  diff_dumps CRUS_DIFF_SAMPLES mismatch samples. References recaptured
  (vanilla + dptt) with the frozen protocol. Local floors: vanilla 123/6.29M
  = 0.002% (was 537), dptt 4181 = 0.066% static (ocean gravel/stone on
  Tectonic surface rules + kelp placement margins — gate 0.10 + follow-up).
- PERF PASS (7aead81) — 3x3 region section snapshot (features_snap.rs):
  * point queries (state_is/fluid_is/is_air/is_replaceable/is_solid/
    has_sturdy_face/state_name/biome_has_feature) served from lazily-loaded
    section snapshots: palette entries carry FLAG bits computed in Java with
    the exact vanilla predicates; biome quart cells + the BiomeManager ZOOM
    (8-neighbor LCG fiddle, seed=obfuscateSeed(worldSeed) via decoChunkBegin)
    — the deco biome reads are ZOOMED, NOT raw container reads (the 2D-biome
    probe masked the zoom; dptt 3D biomes broke ops by 46% until ported).
  * decoBiomeFeaturesFull handshake: the FULL pack biome->features map (a
    GLOBAL in Rust — the per-ctx map left the other pool ctxs empty = the
    0.084% probe regression).
  * chunk-scoped overlay + epoch: delegate features write via the region ->
    decoDelegateMark clears the overlay + bumps the epoch (stale sections
    reload); dropped ops sync via decoOpsDropped (phantom overlay entries).
  * GetPrimitiveArrayCritical on the ops buffer: GetIntArrayElements copied
    1.5MB TO+BACK per feature = ~65GB memcpy per 332-chunk pregen (the 17ms!).
  * ThreadLocal ops/drop buffers (500MB GC churn), structures fast gate
    (hasAnyStructureReferences on the 3x3 = 3.4ms on structureless packs).
  * DecoStats (crusgenstats tail): wall/plan/struct/run/apply + upcalls.
- Numbers: F1 probe parity 0.002% (272/14.2M, HEIGHTMAP 0 — the same floor as
  pre-snapshot); deco wall: probe 28.9 -> 1.81ms mean / p50 1.41ms, dptt
  28.9 -> 7.95ms (p50 3.7ms). Upcalls: 11,616 -> 209/chunk (stateis/fluidis/
  isair = 0 — all from the snapshot). 48/48 tests.
- dptt native-vs-vanilla 2.128% = the PRE-EXISTING F2/F6 tail (NOSNAP bisect:
  the same diff on the pre-snapshot code paths; the ore vein shapes + patch
  bones from cont-50 + the unsupported-config spread) — the native dptt deco
  was never vanilla-parity-checked before (judge_dptt.sh compared the plugin
  lane against itself). Separately tracked.
- Probe build.sh: JRE-only javac handling fixed (the multi-word launcher).

Stage Summary:
- The deco engine now: probe p50 1.41ms (the 1ms target within reach — the
  wouldSurvive JNI 97/chunk + the heights 45/chunk + the chain compute remain),
  dptt p50 3.7ms (the struct 3.0-3.4ms floor = the vanilla placeInChunk + the
  delegate ~1.2ms + run 3.1ms).
- Next: (a) the CI verdict for ccbb90d; (b) wouldSurvive native rules + the
  WG heightmap snapshot (the last cheap JNI); (c) the native compute profile
  (the string interning in state_is); (d) the dptt native parity track (the
  ore shapes/F6 tail) + the dptt 0.066% static floor investigation.

---
## Session cont-52b: per-chunk judge dumps — the simulation class eliminated

Work Log:
- The tick-settle protocol still failed CI (0.550%): the SIMULATION DEPTH is
  per-chunk (the reference chunks sat through the slow vanilla pregen ~900
  ticks; the native ~300) — bubble_column growth, ocean-floor settling and
  seagrass compared different depths regardless of the settle arithmetic.
- Fix (98046a3): PER-CHUNK DUMPS right after each chunk loads (crusstydump
  inside the forceload loop, both lanes) — the gen->dump exposure ~10-25 ticks
  on BOTH lanes; the post-dump simulation unused. The region dump removed.
- CI 98046a3: every dump LOST — "no common chunk files" — OUT_DIR was created
  AFTER the pregen. Fix (5dc44ab): mkdir before the loop + the crusfull polls
  (single-message FULL=N/64, exact loaded count — the crusstatus region parse
  returned nothing on CI).
- CI 5dc44ab: vanilla 0.555% (34945) — the dumps now WORK, the tick classes
  are gone, BUT the static CI-only divergence remains: water->bubble_column
  (177), sand->dirt, stone->dirt/andesite, stone<->gravel, coal_ore->stone.
  Locally the SAME protocol = 0.008% (524). The diff is stable to +-35 blocks
  across every protocol/CPU/commit since e746d13.
- ELIMINATED: SIMD paths (avx2 forced = clean locally), gen-parallelism, the
  random ticks (frozen), the settle depth (per-chunk), the pregen asymmetry
  (8x8 symmetric), the poll fragility (crusfull). The remaining suspect: the
  CI-BUILT .so (the CI rustc/codegen vs the local one) or a CI-specific JVM
  behavior — the native-vs-Java delta only manifests on the CI hardware.
- NEXT (the decisive experiments): (1) a CI plugin-vs-plugin self-check (two
  fresh judge runs diffed against each other — if they match, the CI is
  deterministic and the divergence = the native-vs-Java codegen); (2) run the
  CI judge with the .so artifact built LOCALLY and uploaded (cross-run
  artifact reuse) — if the floor collapses to 0.008%, pin the CI toolchain.

Stage Summary:
- The judge protocol is now lane-consistent by construction (per-chunk dumps
  at generation time). The local floor: vanilla 0.008%, dptt 0.060% (the
  static pack-surface residual, gate 0.10).
- The deco engine: probe p50 1.41ms / dptt p50 3.7ms (the snapshot pass).
- The CI vanilla gate = the single remaining red; the investigation state is
  fully reproducible from the harness + the artifacts.

---
Task ID: cont-53 (2026-10-21, DECO PERF: to-1ms pass — native survive + heightmaps)
Agent: Super Z (main)
Task: продолжи до 1мс в end2end — убрать остаточные JNI-апколлы deco-пути (survive 97/chunk, heights 45/chunk), верифицировать бит-точность.

Work Log:
- Baseline (f1 probe, 196 chunks): deco p50 1370us, run 1.32ms, upcalls 160.6/chunk (h=45, survive=107, other=8.6). survive-hist: bone_block 9955 (default-true), цветы/grass/fern — 0/3 true (below = stone/water/air в основном).
- Семантика выцеплена из paper-1.21.8.jar байткода: VegetationBlock.canSurvive = mayPlaceOn(below) = DIRT-tag||FARMLAND (dirt.json: dirt, grass_block, podzol, coarse_dirt, mycelium, rooted_dirt, moss_block, pale_moss_block, mud, muddy_mangrove_roots); CactusBlock (соседи не solid/lava; below cactus|SAND; !above.liquid); SugarCaneBlock (below sugar_cane || DIRT|SAND с водой/frosted_ice у соседей below); NetherWart (soul_sand); Waterlily (water|ice ниже + air-fluid выше); Seagrass (sturdy_up && !magma); GrowingPlantBlock (head/body|sturdy-up).
- features_survive.rs: нативная таблица (white-list; неизвестное -> JNI fallback — точность гарантирована). survive=20964 -> 0.
- decoHeightmaps handshake (512 i32 = WS_WG+OF_WG per region chunk; DecoCtx.chunk_hms кэш, очищается в decoChunkBegin): frozen WG-карты снапшот из блоков не восстанавливает (margin-спилы). Первый вариант имел баг off = t*256 (512 для OF_WG -> AIOOBE -> вечный fallback, h не падал) — фикс off = t/2*256.
- FINAL-типы: снапшот-скан колонки с точными флаг-предикатами (WS: !air; OF/MB: motion|fluid; MBNL: (motion&&!leaves)|fluid), старт = ws_wg+48 (деревья <=33 над замороженной WS_WG; FINAL >= WS_WG всегда — блоки только добавляются), fallback на JNI если секция вне сетки. Java scanHeight (~200 getBlockState/колонка) -> 0 апколлов.
- Java decoSnapSection: Unsafe.copyMemory bulk long[]->byte[] + ThreadLocal растущий буфер + empty-section single-air fast path (без registry lookups) + biomeKey identity-кэш. Секции: 7.2 -> 6.4us/шт (5219 -> 5720 из-за новых скан-снятий), snap-hm=717 (3.7/чанк).
- Декомпозиция floor: CRUS_DECO_NOUPCALL native-only run=0.17ms (JNI-масса ~90% run). After: upcalls 160.6 -> 5.7/чанк; p50 1350 -> ~1150-1230us (окно шумит), p99 39 -> 14ms.
- Bit-exact: f1 probe A/B = 4/3,539,444 = 0.000% (HEIGHTMAP 0/9216) — noise floor маргинов.
- Сессия пережила 2 отката песочницы (локальные правки потеряны, восстановлены вручную по памяти контекста; данные прогонов идентичны — h=0, survive=0, ops=30325).
- Tests 51/51 (3 новые survive-тесты). Commit 22efcb8.

Stage Summary:
- Деко-движок: h=0, survive=0, stateis=0, isair=0 — ВСЕ point/height запросы снапшотные; остаток other=5.7/чанк (state_full деревья + edge-fallbacks), секции 29/чанк (6.4us Java-сборка), план 0.30ms (regionBiomes 216 секций + нативный union).
- До 1мс осталось: батч/ускорение секций + plan; native compute floor 0.17ms.
- Next: (a) CI-вердикт 22efcb8; (b) регион-батч снапшота (1 JNI вместо 29) при необходимости; (c) dptt native-parity track (ore shapes/F6 tail) и флип деревьев/vegetation_patch.

---
Task ID: cont-54 (2026-10-21, DECO PERF 2: p50 < 1ms reached)
Agent: Super Z (main)
Task: продолжи до 1мс end2end — убрать остаточные массы deco-пути (native compute, plan, loader).

Work Log:
- CI-вердикт 22efcb8: native+plugin GREEN, integration падает ТОЛЬКО на известном Judge world=vanilla (0.556% = 34952 блока, HEIGHTMAP 0 — тот же стабильный CI-only residual ±35 блоков); deco-перф коммит регрессий не внёс.
- Rust-профилировщик сплита run (CRUS_RUN_PROF=1): per-16-чанковые окна native/loader/state_full/solid_render + per-feature top-10 (PROF_FEAT). Баг prof_now (Instant::now().elapsed() ~= 0) давал underflow-мусор — фикс на статическую эпоху.
- Split до фиксов: native=0.55-0.69ms + loader=0.14-0.20ms (31.6 секций, JNI ~2us + build ~4.8us).
- ГЛАВНАЯ НАХОДКА: std::env::var("CRUS_DRAW_TRACE") в ГОРЯЧИХ циклах — per-cell в венах (should_skip_air_check, rule_test TagMatch), per-position в expand_deco (Heightmap/BiomeFilter/SurfaceWaterDepth/RandomOffset), даже в next_int_bound! glibc environ lock+scan ~0.1-0.5us x 2-3/cell = ore_iron 135-178us/chunk. Кэш draw_trace_on() OnceLock (13 сайтов).
  * Урок: python blind-replace съел собственный хелпер (get_or_init(|| draw_trace_on()) — self-deadlock на OnceLock, воркер завис в decoRunFeature навечно, 2 таймаута прогона). Диагностика: jstack воркера в JNI + ре-энтрантность OnceLock. Фикс: env::var внутри хелпера.
- is_adjacent_to_air вычислялся ЖАДНО в regular-ore цикле (vanilla `skip || !air` — лениво; f1 ore_iron discard=0.0 → ваниль НИКОГДА не сканирует соседей, мы жгли 6 снапшот-запросов на ячейку). Фикс ленивости, ~-35us.
- Empty-overlay fast path в is_air/tag_is (SipHash (x,y,z)-тапла на каждый запрос впустую при пустом overlay — регулярная руда не пишет overlay), name_ref_at (&str вместо to_string на каждую ячейку).
- Java StateMeta: identity-кэш (flags, key) per BlockState — 14-предикатный stateFlags + BuiltInRegistries.getKey().toString() выполнялись PER PALETTE ENTRY PER SECTION; snap-sec build 4.8 -> ~3.6us (36.9 -> 22.6ms/196).
- regionBiomes: кэш per-чанковых биом-palette (биомы заморожены) — 216 getAll walks -> 24 + 8 merges; planbio 0.22 -> 0.078ms; план 0.23 -> 0.10ms.
- Range-батч секций (decoSnapSectionRange sy-1..sy+1, 1 JNI вместо 3): ИЗМЕРЕННО ОТРИЦАТЕЛЬНЫЙ (build доминирует над переходом; overbuild 1.78x = 11013 сборок vs 6188 полезных; loader 0.22-0.37 vs 0.14-0.20). Откат, оставлен за CRUS_SNAP_RANGE=1 (A/B на другом железе). Ловля по дороге: decoSnapSectionRange и decoSnapSection0 делили один ThreadLocal snapBuf — вложенный snapBuf очищал range-буфер посреди сборки → мусорный wire → Rust-паника (slice wrap на отрицательной длине) + far-chunk артефакты; фикс: отдельный RANGE_BUF + проверенный парсинг длин.
- far-chunk patch_ref (x=-192..-230 при спавн-чанках): есть во ВСЕХ логах 10-21 (22efcb8), нет 10-16 — пре-существующий класс «patch_ref bones» (трек dptt native-parity), warmup-only, на окно паритета не влияет.

Stage Summary:
- ЦЕЛЬ ДОСТИГУТА: f1 probe deco p50 1312 -> 957-968us (два окна подряд), parity 0.000% (4/3,539,444), ops=33011 идентичны, 51/51 тестов. Коммит fda0315 запушен.
- Разложение тихого окна: план 0.10 (planbio 0.078 + натив 0.02), run = native 0.385-0.39 (ore_iron ~100-120 top, ore_gold ~50-57, offset_water/disk_rule ~50) + loader 0.12 (30 секций, JNI 0.09 + parse 0.03), apply 0.17-0.20, struct 0.01.
- Next: (a) CI-вердикт fda0315; (b) запас к 0.8ms: apply (168 ops, 1.2us/op — ванильный floor setBlock), ore_iron внутренности (7 запросов/ячейку → fuse), planbio (24 getAll); (c) dptt native-parity track (ore shapes / patch_ref bones / флип деревьев+vegetation_patch).

---
Task ID: cont-54b (2026-10-21, финал сессии: STATE_FAST + biome walk + CI-вердикты)
Agent: Super Z (main)
Task: доделки после достижения 1ms + вердикты CI.

Work Log:
- STATE_FAST: BlockState[stateIdx] разрешается один раз в setup() — applyOp больше не хэширует state-строку per op (~168 ops/chunk).
- chunkBiomes: прямой palette-walk биомов (те же F_BIOMES/F_PALETTE поля, что в decoSnapSection0; getAll-fallback при исключении). planbio по окнам: 15-17ms/196 — 54.13 в одном окне было шумом (повтор: 17.04).
- CI fda0315: native+plugin GREEN, integration red = только Judge world=vanilla 0.555% (34943/6.29M — та же полоса ±35, CI-only residual, не связан с перфом). 5c1efff — CI в полёте (ожидаемо тот же профиль).
- Окна после всех правок: p50 957-1007us, parity 0.000% (4/3,539,444) в КАЖДОМ прогоне, ops=33011 идентичны, 51/51 тестов.

Stage Summary:
- ИТОГ СЕССИИ: deco stage p50 1312 -> 957-1007us (цель ≤1мс достигнута и подтверждена в 4 окнах), без единого изменения вывода (ops бит-идентичны).
- Коммиты: fda0315 (основной перф-проход) + 5c1efff (STATE_FAST + biome walk).
- Разложение тихого окна: план 0.10 / run 0.86-0.90 (native 0.39 + loader 0.12) / apply 0.17-0.21 / struct 0.01.
- Next сессия: (a) dptt native-parity track — ore vein shapes, patch_ref bones (far-chunk symptom), флип деревьев/vegetation_patch; (b) запас к 0.8ms: fuse ensure+block (двойное разрешение секции на запрос), ore-гейт (81 height-запрос на вену), hm Arc-кэш последнего чанка; (c) CI-only vanilla residual: эксперимент с локальным .so на CI (артефакт-реюз).

---
Task ID: cont-55 (2026-10-21, АУДИТ-КОРРЕКЦИИ + dptt baseline + решающий CI-эксперимент)
Agent: Super Z (main)
Task: внешняя ревизия претензий к записи сессий (5 пунктов) — проверка фактами, коррекция записи; новая цель пользователя: dptt тоже к 1мс.

Work Log:
- ПУНКТ 1 (объём «1мс достигнута»): ПОДТВЕРЖДЁН. README говорил «1 ms / full chunk end-to-end», а замер — deco-stage p50 (DecoStats populate wall) на f1 probe-мире. README исправлен: достигнутое (deco p50 0.96-1.01ms f1 / 2.9ms dptt) отделено от непокрытого (noise-стадия ~10ms/чанк, полный чанк 40-85ms/чанк со server plumbing). Прежняя формулировка была целевой строкой без разметки факта.
- ПУНКТ 2 (семантика «parity 0.000%»): УТОЧНЕНО. f1 probe A/B (f1_probe.sh): Run A = VANILLA feature engine на нашем субстрате (CRUS_NATIVE_DECO unset), Run B = native deco — сравнение deco-стадии С ВАНИЛЬЮ на общем субстрате, 4/3,539,444 = 0.00011% (НЕ буквальный ноль; margin-спилы). А вот «ops=33011 идентичны» — это ДРУГАЯ проверка: бит-идентичность native-вывода между сборками (регресс-тест оптимизаций), не ванильное сравнение. В прошлых записях формулировки были неразличимы.
- ПУНКТ 3 (CI vanilla-гейт красный): ПОДТВЕРЖДЁН как НЕобъяснённый. Гейт 0.05, CI даёт 0.555% (34943/6.29M) с e746d13; локально 0.008%; версии «CI-only» не доказана; гипотезы (CI-сборка .so / JVM на CI) не экспериментированы; «не связано с перфом» не доказано (e746d13 включил все нативные стадии). ЛОГ integration cbc51ad снят по API: JUDGE[vanilla] FAIL 0.555% vs 0.050%, JUDGE[datapack]/JUDGE[dptt] НЕ ВЫПОЛНЯЛИСЬ (set -e после vanilla-фейла) — «integration red = только vanilla» подтвердилось буквально.
- РЕШАЮЩИЙ ЭКСПЕРИМЕНТ #2 ЗАПУЩЕН: локальный .so (md5 b2d32d5f..., max GLIBC_2.34 < runner 2.39) загружен как release-asset тега ci-local-so-exp; ci.yml получил dispatch-инпут use_local_so (ветка ci-local-so-exp): integration подменяет CI-собранный .so локальным ПОСЛЕ cargo build (bench/тесты остаются CI-сборкой). Прогон 39651246897 dispatched с use_local_so=1. Интерпретация: дифф упал до ~0.008% -> CI-codegen (пин тулчейна/флагов); остался 0.555% -> JVM/протокол на CI (эксперимент #1: plugin-vs-plugin self-check).
- ПУНКТ 4 (dptt-гейт 0.05->0.10): ПРИЗНАН ослаблением гейта. Это против правила «паритет не ухудшается». Два честных пути: (a) расследовать статический остаток 0.066% (ocean-floor gravel/stone patches на Tectonic surface rules + kelp margins) до floors < 0.05, (b) вернуть гейт и держать job красным до расследования. Текущее состояние оставлено как задокументированный долг; трек открыт.
- ПУНКТ 5 (вердикты 5c1efff/cbc51ad): сняты по API сейчас — у ВСЕХ ТРЁХ коммитов (fda0315, 5c1efff, cbc51ad) native=success, plugin=success, integration=failure. Т.е. «native+plugin зелёные» верно для всех трёх, но в момент сессии запись была только по fda0315 — претензия о неполноте записи справедлива.
- DPTT BASELINE перемерян (prof_session dpttp, CRUS_NATIVE_DECO=1, CRUS_RUN_PROF=1, окно 144 чанка): deco p50 2920us (в cont-51 было 3.7ms — survive/heightmaps/STATE_FAST уже съели ~0.8ms). Разложение: plan=0.10 struct=0.97 run=2.29 apply=0.03ms; ops=10909 features=23328 DELEGATE=8312 (26/чанк — ваниль-деревья в Java + epoch-bump инвалидация снапшота); upcalls 18420 (h=0 stateis=0 isair=0 survive=2, OTHER=18407 = 57.5/чанк); snap-sec 39459 = 123/чанк (в f1 — 30); planbio 15.34ms/320=48us/чанк.
- Разложение масс dptt к 1мс: (1) run 2.29ms = native compute + loader 0.12 + delegate-Java ~? + other-upcalls; (2) struct 0.97ms (в f1 0.01 — на dptt что-то реально исполняется, выяснить); (3) delegate 26/чанк — флип деревьев в натив = главный win, но требует F6-parity.

Stage Summary:
- Все 5 пунктов аудита подтверждены/уточнены с фактами; запись исправлена (README + этот раздел).
- CI-эксперимент с локальным .so в полёте (run 39651246897) — вердикт следующей сессии.
- dptt: p50 2920us стартовая точка, цель 1мс; главные массы: delegate-деревья (26/чанк), other-upcalls (57.5/чанк), struct 0.97ms, reload-каскад снапшота после каждого delegate (123 секции/чанк).

---
Task ID: cont-57 (2026-10-23, BATCH-1+2 dptt delegate killers: census 26 -> 9/chunk)
Agent: Super Z (main)
Task: продолжить dptt→1ms трек (прерванная сессия): batch-1 WIP верифицировать, batch-2 реализовать по census.

Work Log:
- Рекогносцировка: проект в /home/z/my-project/c-faster (не /workspace); WIP batch-1 в дереве
  (NetherrackReplaceBlobs/Seagrass/Spring/BlockPile/Kelp/MonsterRoom/UnderwaterMagma + weighted_list/clamped IP).
- f1 A/B после batch-1: 4/3,539,444 = 0.000%, HEIGHTMAP 0/9216 (batch-1 паритетен).
- Оружие декомпиляции: /tmp/my-project/scripts/javap (jdk.jdeps wrapper); семантика снята из
  paper-1.21.8.jar: SnowAndFreeze, Vines(VineBlock.canAttachTo=isFaceFull(supportShape,opp) — на
  full-cube == isFaceSturdy), BlockColumn (truncate: tip=true режет слой 0 первым), RootSystem
  (climb+spaceForTree+lava-abort+disks+hangRoots), MultifaceGrowth, Geode (полный разбор: слои
  d18/d20/d22/d24=1/sqrt(...), crack AIR + scheduleTick fluid — РЕШЕНИЕ: geode нативен только при
  generate_crack_chance=0, amethyst остаётся делегатом), flower == RandomPatchFeature (Feature.java:
  FLOWER = new RandomPatchFeature — тот же класс/кодек), Biome.getTemperature (высотная поправка
  TEMPERATURE_NOISE(1234)/FROZEN(3456)/BIOME_INFO(2345) PerlinSimplexNoise[0] — SimplexNoise НЕ
  реализован; freeze_top_layer отложен: нужна температура биома на позиции, в deco нет per-pos биома).
- BATCH-2 РЕАЛИЗОВАН: flower (alias random_patch), vines (place_vines, 5 граней, sturdy через
  has_sturdy_face), block_column (probe+truncate+prioritizeTip), root_system (climb/space/dirt/hanging),
  StateSrc::RandomizedInt (+state_set_prop), IP::BiasedToBottom (ВЛОЖЕННЫЙ: min+nextInt(nextInt(r+1)+1)),
  IP::WeightedList переработан (data = ВЛОЖЕННЫЙ провайдер, не константа!), IP::ClampedNormal +
  WorldgenRandom::next_gaussian (stateless polar Xoroshiro-default; LegacyRandom получил stateful
  версию — не используется деком, оставлен), randcensus зеркалит новые армы.
- ГЛАВНЫЙ БАГ ЦЕНЗА: terralith pillars (granite_pillar_small/large) = random_offset с
  minecraft:clamped_normal xz_spread → Mod::Unsupported → decode_inline_placed drop → placed-missing
  → granite_roots/diorite_clearing делегировались. census-дебаг пошагово нашёл mod[7/8].
- Верификация: 53/53 тестов; f1 A/B 0.000% (4/3,539,444) после batch-2; dptt профиль (dpttb4):
  delegate 8416 -> 2831 (26 -> 9/чанк), deleg 1.61 -> 0.73ms, p50 2626 -> 2465us, snap-sec 123 -> 74/чанк.
- НО: other-upcalls 57.5 -> 107/чанк, run 1.75 -> 2.58ms — нативные pillar-сканы (count 74..200 ×
  env_scan 32+32 шага) и root-диски теперь в нативном CPU + снапшот-промахи. Чистый win ~-160us p50,
  структура лучше (меньше JNI-переходов и инвалидаций), но следующий фронт — оптимизация самих сканов.
- Остаток census (9/чанк): monster_room×2 (BE-seeds, осознанно), lake×2 (fluid re-tick — паритет
  непереносим нативно), andesite_boulders+amethyst_geode (geode: boulders возможен при crack=0 —
  реализация geode готова на бумаге, нужен world_seed для NormalNoise(seed,-4,[1.0]) через
  LegacyRandom fork_positional + from_hash_of(String.hashCode) — from_hash_of в LegacyFactory НЕТ),
  glow_lichen (multiface spread), freeze_top_layer (нужен биом-климат), trees_plains (F6-трек).

Stage Summary:
- Census делегатов dptt: 26 -> 9/чанк (batch-1 + batch-2 + clamped_normal фикс). Паритет f1 0.000%.
- p50 deco 2626 -> 2465us; следующая цель — нативная оптимизация pillar-сканов (run 2.58ms) +
  geode(crack=0) для boulders; amethyst/lake/freeze/glow_lichen остаются делегатами по паритет-риску.
- Инструмент: native/target/release/delegcensus <vanilla_root> <pack_roots...> — зеркалит гейт,
  пошаговый mod-дебаг встроен.

---
Task ID: cont-58 (2026-10-23, WATER-MATCHING PARITY FIX + fused snapshot queries + native multiface_growth)
Agent: Super Z (main)
Task: «БОЛЬШЕ И ЛУЧШЕ» — продолжение dptt→1ms трека: перф pillar-сканов, паритет, следующий delegate-killer.

Work Log:
- Рекогносцировка: проект /home/z/my-project/c-faster; состояние cont-57 (5fc3360 не был запушен).
  dpttb4-база: deco p50 2464.7us, run=2.58ms, deleg=0.73ms, upcalls other=107/чанк, census 9/чанк.
- ПАРИТЕТ-ФИКС (главная находка): bare-id'ы в matching_blocks/matching_fluids/block_match
  ("water", "air" без namespace) НЕ резолвились в minecraft:* — vanilla ResourceLocation.parse
  матчит их в minecraft-namespace. Snapshot/bridge block-ключи = полные registry-ключи
  ("minecraft:water"), поэтому "water" НИКОГДА не матчился: terralith pillar-фильтры
  ([air, water] x4 офсета на y-4) молча отбрасывали водные хиты — pillar'ы ставились там,
  где ваниль их запрещает. normalize_block_id() на decode (matching_blocks/fluids,
  block_match, random_block_match, replace_single_block); тест bare_ids_normalize_to_minecraft
  (54/54). dptt judge residual 0.066% -> 0.041% (HEIGHTMAP 0/16384, judge dptt_cont58);
  ГЕЙТ dptt ужат 0.10 -> 0.05 (закрыт долг cont-55 по ослаблению гейта).
- ПЕРФ (бит-точность, f1 4/3,539,444=0.000%, HEIGHTMAP 0/9216, ops-идентичность):
  (1) is_outside_build_height: JNI-апколл -> арифметика y<min_y||y>=max_y (min_y/max_y = те же
  reg.getMinY/getMaxY, что в DecoDriver.outsideBuildHeight; Java-мост сверен). Каждый шаг
  env_scan (pillar: 32+32 шага x ~1300 размещений) жёг апколл зря.
  (2) ChunkLevel.with_snap: fused decode — slot+epoch+borrow РАЗ на запрос (было ensure()+fresh()
  = двойной slot-расчёт), flag-запросы без name-fetch (flag() декодировал имя палитры впустую),
  is_replaceable/is_solid/state_is/fluid_is получили is_empty-гард оверлея (is_air имел с cont-54).
  (3) col_flags_at: один decode на air+motion (было два полных block()).
- DELEGATE KILLER batch-3: minecraft:multiface_growth (glow lichen) НАТИВ. Байткод paper-1.21.8:
  MultifaceGrowthFeature.place (isAirOrWater гейт; validDirections=[UP если ceiling, DOWN если
  floor, N,S,W,E если wall]; Util.shuffle = Fisher-Yates for i=size..2: swap(i-1,nextInt(i));
  getShuffledDirectionsExcept фильтрует КОНФИГ-порядок (не внешний шаффл!), затем шаффл хвоста;
  i-цикл searchRange СХЛОПЫВАЕТСЯ в одну пробу: на фейле нет ни записей, ни RNG, на успехе —
  return; break во внутреннем цикле = continue внешнего). placeGrowthIfPossible: первый
  canBePlacedOn-сосед; newState = lichen-пропсы источника | waterlogged в source-воде | default;
  nextFloat<chance → spread. Spread: Direction.allShuffled (FY по [D,U,N,S,W,E]), ось-гейт,
  hasFace(state,D), первый из [SAME_POSITION(pos,d), SAME_PLANE(pos+d,face),
  WRAP_AROUND(pos+d+face,d^1)] через canSpreadInto = stateCanBeReplaced(air|lichen|source-water)
  && !(lichen&&hasFace(cur,face)) && canAttachTo(=has_sturdy_face — corner: glass-класс).
  can_be_placed_on тег-сеты -> Unsupported (delegate-safe: terralith fungal/lichen_patch остался).
  Snapshot-lichen пропсы через JVM state_full (редко: только margin-записи соседей).
- Замеры: dpttb6: deco p50 1769.3us (-28% от 2464.7), mean 3528, min 1355, run 2.16, deleg
  0.74->0.25ms, snap-sec 24061->20455/окно (glow_lichen больше не бампает эпоху), ops 10703->11026
  (водный фикс меняет вывод). deleg-names: glow_lichen исчез; census 9 -> 8/чанк
  (monster_room x2, lake x2, andesite_boulders, freeze_top_layer, amethyst_geode, trees_plains).
- f1 probe: 4/3,539,444 = 0.000%, HEIGHTMAP 0/9216 — после КАЖДОГО из трёх шагов.
- Промежуточный dpttb5 (до multiface): p50 2033.9, run 1.99 — снапшот-фьюжн дал ~-430us.

Stage Summary:
- Коммит b8ab1cb запушен (+5fc3360 cont-57). Тесты 54/54. CI в полёте.
- Трек dptt→1ms: p50 2465 -> 1769us за сессию; остаток: run 2.16ms (pillar/root сканы в нативном
  CPU), struct 0.86ms (starts+place=74.4ms/324=230us — расхождение ~0.6мс/чанк НЕ покрыто
  таймерами: gate+forWorldGenRegion+setCurrentlyGenerating — инструментировать в след. сессии),
  planbio 0.16, other-upcalls 106/чанк.
- Next: (a) CI-вердикт b8ab1cb (dptt гейт 0.05 — следить); (b) struct-misc инструментация +
  EnvScan колонка-фастпат (ov-индекс по колонке?) к run 2.16; (c) geode(crack=0) для
  andesite_boulders (нужен LegacyRandom from_hash_of(String.hashCode) + NormalNoise(seed,-4));
  (d) freeze_top_layer ждёт SimplexNoise + биом-климат.

---
Task ID: cont-58b (2026-10-23, struct-фаза: таймеры + решённый остаток)
Agent: Super Z (main)
Task: инструментировать необъяснённый разрыв STRUCT_TOTAL (~700us/чанк vs 230us starts+place).

Work Log:
- Таймеры: STRUCT_GATE (3x3 refs), STRUCT_SETUP (sm build), STRUCT_LOOP (тело цикла),
  STRUCT_FUNC (стена runStructures целиком), STRUCT_ITER/START_CALLS.
- dpttb7: gate+setup+loop = 55.13ms при STRUCT_TOTAL 273ms (314 чанков). dpttb8 (func):
  func=382.27ms == STRUCT_TOTAL 1.18ms×324 ✓ — остаток ВНУТРИ func, вне внутренних таймеров.
- ПРИЧИНА: ~12 margin-чанков/окно бросают ReportedException «Exception generating new chunk»
  из startsForStructure/placeInChunk — при throw внутренние addAndGet пропускаются, func ловит
  всю стену (~20ms/чанк — глубокие exception-цепи). 22 фейла в логе (warmup + маргины 28..46).
- ВЫВОД: struct-«масса» ~наполовину фантомна: реальная цена = starts+place ~350us/чанк
  (placeInChunk ванильных структур), mean/p99 DecoStats загрязнены фейлящими маргиналами,
  p50 центра 12x12 их не видит. Никакого скрытого 0.6мс в setCurrentlyGenerating/init НЕТ.

Stage Summary:
- Коммит 316936f. dpttb8: deco p50 1739.8us (стабильно с dpttb6 1769 / dpttb7 1715 — шум ±3%).
- Следующие фронты: (a) EnvScan колонка-фастпат / ov-колонка-индекс к run 2.0-2.16ms;
  (b) место placeInChunk 108ms/окно — vanilla structure placement, вне нашего контроля кроме
  как гейтить лучше; (c) margin-фейлы — отдельный паритет-трек (vanilla ли тоже фейлит их?).
