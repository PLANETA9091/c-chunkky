# STATUS — 1ms e2e chunk-gen campaign (Paper 26.3, native Rust takeover)

Компактная карточка состояния кампании. Полная хроника — `worklog-session.md`
(записи cont-XX по датам). Этот файл обновляется при изменении КАРТЫ, не при
каждом тике. Последнее обновление: cont-126.

## Цель и инвариант

Устранить e2e-задержку генерации чанков Paper 26.3 переводом стадий
генерации в нативный Rust (`native/`, JNI-хуки через плагин
`plugin/CrusstyGen.jar`). НЕПРИКОСНОВЕННЫЙ ИНВАРИАНТ: **бит-точность** —
все нативные поверхности обязаны давать блок-в-блок тот же мир, что
ваниль; верификация = гейты `harness/ab263_ci.sh` (ore-сигнатура 24
материала в допуске max(4%,500) + orebase strict bit-equal + blockdump
fingerprint deco-noise класса). Не-бит-точная территория отвергнута.
ВЕКТОР ПОЛЬЗОВАТЕЛЯ (cont-124): однониточные техники оптимизации —
«параллельность — НЕ ускорение»; фронт атаки = CPU-стоимость нативного
филла (класс memo/hash cont-115/116).

## Карта фронтов (обновлена cont-124 — perlin-ядро ускорено)

| Поверхность | Статус |
| --- | --- |
| noise / surface / biomes / carvers | **натив, бит-точен, в проде** |
| heights | fill-based, в проде |
| ore-хук (per-cell deco) | **OFF** (дефолт `CRUS_NATIVE_ORE=OFF`); memo (cont-115) + OvHasher (cont-116) забанкованы бит-точными |
| interp_cell: perlin-ядро | **УСКОРЕНО cont-124 + cont-125** (монофоморфизация + f32-таблица градиентов −13.8% ns/pt; Y-CHAIN HOIST колонка-инвариантной f64 y-цепочки = ядро −53% ns/pt-vol, fill COLD −19.7%, серверный филл COLD −18.8%; mask-форма и x2-interleave замерены — mask отвергнута, interleave −16% отложен как кандидат) |
| interp_cell: SIMD-батчинг | заморожено (gather-доминанта 92%, cont-112) |
| decorate | фронт закрыт |
| light / serialization | Paper-side, вне зоны кампании |
| par4 платформенная невязка | открыт, но runner-bound (см. ниже) |

## Ключевые вердикты hold-mode

- **per-cell ore фронт ЗАКРЫТ** (cont-117): структурный остаток
  unpack+loop+RNG ~60-80ns/кандидата на 1-2 порядка ниже CI-шума.
- **run-prof исключён из регрессионных сигналов** (cont-117/118):
  одиночные абсолюты на shared-раннерах нечитаемы; ci116-аномалия —
  артефакт Instant::now-сэмплера (заменён точными счётчиками в cont-118,
  разброс ±30% → ±2-4%).
- **Честные метрики**: same-run гейты + A/B-дельты + контролируемый
  локальный A/B на тёплой VM (memo −22% native wall, hasher −7%).
- **CI e2e-дельта хука runner-bound**: +10.64 → +19.05 → +17.73 → +17.35
  → +15.91 → +15.52 (полоса x1.5-1.8, пять ранов подряд), семантика
  инвариантна — атрибуция только по гейтам.
- **Paper 26.3 = build 222** (sha256 f649771a..., md5 f49e29961...);
  PaperMC v2 API sunset — только fill v3 (ключ `server:default`).

## Граница цели 1ms e2e (квантифицирована cont-123)

Чистый 4-рановый ряд prod-ноги (ген-код идентичен с cont-118): native
30.1–36.2ms/chunk, cover (at-least-one-busy доля стены) 68.9–73.8%,
deco-хвост 221–329ms, e2e 23.98–28.31.

| Метрика | Значение |
| --- | --- |
| native CPU на окно (144 чанков) | 4.34–5.22 CPU-s (avg 4.71) |
| CPU-утилизация par4 (4 воркера) | **31.0–32.0%** (avg 31.5%) |
| Идеальный 4-воркерный пол | **7.54–9.05 ms/chunk** (avg 8.18) |
| Воркеров для 1ms/chunk amortized | **30.1–36.2** (avg 32.7) |

**Вердикт**: 1ms e2e на Paper 26.3 + par4 недостижима внутри инварианта
бит-точности. Путь «30x срез CPU нативного филла» противоречит
структурным вердиктам (SIMD заморожена, per-cell фронт закрыт);
путь «32.7 воркеров» — dependency-волны chunk-системы Paper (Paper-side,
вне зоны репо). Реалистичный потолок in-scope — пол 8.18ms/chunk, и тот
закрыт Paper-планировщиком. ПОПРАВКА cont-124/125: однониточные техники
продолжают срезать CPU-стоимость ядра (perlin −13.8% ns/pt, затем Y-CHAIN
HOIST — ядро −53%, fill COLD −19.7%, nf[perlin] 3.61→1.52ms/ч) — граница
сдвигается вниз пропорционально, но порядок величины (пол ~8ms) не меняется.

## Индекс собранных CI-ранов (repo id 1412863908)

| Ран | Commit | Вердикт |
| --- | --- | --- |
| 41875709282 | 8134f36d (база 438) | SUCCESS |
| 42061238509 | b4c29cf6 (cont-115) | SUCCESS |
| 42064468247 | aad5d267 (cont-116) | SUCCESS |
| 42071601599 | 6af75d73 (cont-117) | SUCCESS |
| 42084876496 | 8801ac9e (cont-118) | SUCCESS |
| 42098787317 | 8f6333e6 (cont-119) | SUCCESS |
| 42105093915 | f294a207 (cont-119 add.) | SUCCESS |
| 42177866803 | 24903aa6 (cont-120) | SUCCESS |
| 42231520636 | 7a9fc2cc (cont-121) | SUCCESS |
| 42271100713 | be438653 (cont-122) | SUCCESS |
| 42279356696 | e46f357f (cont-123) | SUCCESS |
| 42284831079 | b5820b42 (cont-123 docs) | SUCCESS |
| 42303293298 | 019bd9d0 (cont-124 perlin −13.8%) | SUCCESS |
| 42348486359 | a4ce187c (cont-125 y-chain hoist, ядро −53%, fill COLD −19.7%) | SUCCESS |
| 42360150406 | 682af190 (cont-126: 1.21.8 lane-probe fix + blended scratch-пул, бит-нейтрален) | SUCCESS |

Гейты PASS на каждом; runs-паттерн канонический (3632/3857-3871);
fallbacks=0; loader 0.3-0.55ms/окно; ore-prof exact в CI = tag 106-120k,
nodes ≈1.4k, hm ≈3.4-4.5k, air ≈1.1-1.8k (±2-4% между окнами).

## Протокол тика (дисциплина)

1. state-check: HEAD, tree, cargo/JDK25/server, последние раны CI.
2. Откат песочницы → git fetch + `git reset --hard origin/main` →
   `bash harness/sandbox_recover.sh` (идемпотентен, догоняется
   повторными запусками; git-синк вручную, если HEAD ушёл дальше коммита
   самого скрипта).
3. Вердикты: если есть ран без вердикта — скачать артефакт
   (`artifacts/<id>/zip` через repo-id endpoint + Bearer token) и сверить
   гейты/e2e-пару.
4. Работа тика → запись cont-XX в worklog (рус.) → commit → push.
5. **Без раннего финиша**: дождаться вердикта рана своего пуша
   (poll `runs/<id>` до completed) — прямое требование пользователя.

## Команды восстановления и проверки

```bash
# восстановление после отката (9 стадий, идемпотентно)
bash harness/sandbox_recover.sh [--skip-git]

# health-чек окружения после восстановления (ore-пара, N=8)
JAVA_HOME=/home/z/tools/jdk-25.0.5+8 SRV_DIR=$PWD/server \
  bash harness/ab263_ci.sh ore
# ожидание: "GATE: PASS (all strict materials bit-equal)" или
# "GATE: PASS (N materials within the deterministic ore signature)";
# blockdump divergent lines 88-288 = deco-noise класс (норма)
```

## Известные грабли (закодированы в recover-скрипте)

- **fill263/stage/surf_calls-профиль — spawn-загрязнён CPU-учёт**
  (cont-123): fills=400 при inside=144 — спавн-чанки интерливятся в окно;
  per-fill means (terrain/surface/interp_cell) — смешанная выборка,
  годны только как относительные веса. Чистые числа — только honest
  window (region-bounded) в GenStats.
- PaperMC v2 sunset → fill v3, ключ `downloads["server:default"]`.
- Maven repo1 429 на jetbrains-annotations → aliyun-зеркало в probe263.
- bootstrap_server.sh пинит build только при самостоятельной загрузке
  jar → paper.build форсится скриптом.
- jar-сборки не байт-воспроизводимы (timestamp) — md5 плагина/probe
  меняется между сборками, canonical = committed jar.
- N=12 локально захлёбывается в statdump (446МБ trace + 2 ядра) —
  локальные пары только N=8.
