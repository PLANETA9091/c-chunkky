package com.planeta.crussty;

import org.bukkit.Bukkit;
import org.bukkit.Material;
import org.bukkit.World;
import org.bukkit.generator.ChunkGenerator;
import org.bukkit.generator.WorldInfo;

import java.lang.reflect.Constructor;
import java.lang.reflect.Field;
import java.lang.reflect.Method;
import java.lang.ref.WeakReference;
import java.util.Map;
import java.util.Random;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.locks.ReentrantLock;
import java.util.logging.Logger;

/**
 * Takes over ONLY the vanilla noise stage: one native call fills the whole
 * chunk (16*16*height u16 block ids), then we lay them into ChunkData.
 * Vanilla continues on top: surface rules, carvers, features, lighting —
 * preserving block-for-block parity with the vanilla reference dumps.
 */
public final class CrusstyChunkGenerator extends ChunkGenerator {

    /** all live ctx ids (for freeAll); guarded by itself. */
    private static final java.util.Set<Long> CTX = ConcurrentHashMap.newKeySet();
    /** serializes native fill per ctx (native scratch is ctx-bound for now). */
    private static final Map<Long, ReentrantLock> LOCKS = new ConcurrentHashMap<>();

    private static final int ARRAY_SIZE = 16 * 16 * 384; // 98304

    /** fluid-update mark buffer (packed BlockPos), sized to native MARK_CAP + 1. */
    private static final int MARK_CAP = 65536;

    private final ThreadLocal<short[]> buf = ThreadLocal.withInitial(() -> new short[ARRAY_SIZE]);
    private static final ThreadLocal<long[]> MARK_BUF =
            ThreadLocal.withInitial(() -> new long[MARK_CAP]);

    /** world key: seed + resolved sea level + datapack asset fingerprint
     *  (same seed, different datapacks -> different ctx). */
    private record WorldKey(long seed, int seaLevel, int packHash) {
    }

    /** World datapack worldgen assets, extracted once per world.
     *  null = no packs (or CRUS_DP_NATIVE=0) -> embedded-vanilla ctx. */
    private record PackData(String[] paths, byte[][] blobs, int hash) {
    }

    private static final Map<String, PackData> PACK_CACHE = new ConcurrentHashMap<>();
    /** A/B escape hatch: CRUS_DP_NATIVE=0 restores the embedded-vanilla ctx
     *  (chimera mode) even when datapacks are present. */
    private static final boolean DP_NATIVE = !"0".equals(System.getenv("CRUS_DP_NATIVE"));

    private static final Map<WorldKey, CtxPool> POOLS = new ConcurrentHashMap<>();
    private static final Map<String, Integer> SEA_CACHE = new ConcurrentHashMap<>();
    /** parallel contexts per world (matches Paper's chunk worker parallelism). */
    private static final int POOL_SIZE = Math.max(2,
            Runtime.getRuntime().availableProcessors());

    /**
     * Pool of independent native contexts for one world. Paper generates
     * chunks on several worker threads; each worker acquires a free ctx so
     * fills run in parallel (contexts share nothing mutable). Acquisition
     * prefers a ctx whose lock is currently free, falling back to
     * round-robin (which then blocks on that ctx's lock as before).
     */
    private static final class CtxPool {
        private final long[] ctxs;
        private final ReentrantLock[] locks;
        private final java.util.concurrent.atomic.AtomicInteger rr =
                new java.util.concurrent.atomic.AtomicInteger();

        CtxPool(int n, long seed, int minY, int height, int sea, PackData packs) {
            this.ctxs = new long[n];
            this.locks = new ReentrantLock[n];
            for (int i = 0; i < n; i++) {
                long ctx;
                if (packs != null) {
                    ctx = ChunkGenNative.newCtxAssets(seed, sea, packs.paths(), packs.blobs());
                    if (ctx == 0) {
                        Logger.getLogger("CrusstyGen").warning(
                                "native datapack worldgen failed to parse; using embedded vanilla "
                                + "(CHIMERA MODE - terrain will NOT match the packs)");
                    }
                } else {
                    ctx = 0;
                }
                if (ctx == 0) {
                    ctx = ChunkGenNative.newCtx(seed, minY, height, sea);
                }
                if (ctx == 0) {
                    // free the ones already created
                    for (int j = 0; j < i; j++) {
                        ChunkGenNative.freeCtx(ctxs[j]);
                    }
                    throw new IllegalStateException(
                            "crussty_chunkgen ctx_new failed for seed " + seed + " sea " + sea);
                }
                ctxs[i] = ctx;
                locks[i] = new ReentrantLock();
                synchronized (CTX) {
                    CTX.add(ctx);
                }
            }
        }

        /** Returns [ctx, lock] — prefers an unlocked ctx. */
        long acquire(ReentrantLock[] lockOut) {
            // fast path: first free lock (scan from round-robin cursor)
            int n = ctxs.length;
            int start = Math.floorMod(rr.getAndIncrement(), n);
            for (int k = 0; k < n; k++) {
                int i = (start + k) % n;
                if (locks[i].tryLock()) {
                    lockOut[0] = locks[i];
                    return ctxs[i];
                }
            }
            // all busy: block on the round-robin one
            int i = (start) % n;
            locks[i].lock();
            lockOut[0] = locks[i];
            return ctxs[i];
        }

        void freeAll() {
            for (long ctx : ctxs) {
                ChunkGenNative.freeCtx(ctx);
            }
        }
    }

    /** We own the noise stage (native); vanilla keeps everything else. */
    @Override
    public boolean shouldGenerateNoise() {
        return false;
    }

    /** Vanilla surface rules (deepslate, biome surface, bedrock floor) run on top of our blocks. */
    @Override
    public boolean shouldGenerateSurface() {
        return true;
    }

    @Override
    public boolean shouldGenerateBedrock() {
        return true;
    }

    /** Vanilla carvers (caves/ravines) cut into our blocks. */
    @Override
    public boolean shouldGenerateCaves() {
        return true;
    }

    @Override
    public boolean shouldGenerateDecorations() {
        return true;
    }

    @Override
    public boolean shouldGenerateStructures() {
        return true;
    }

    @Override
    public boolean shouldGenerateMobs() {
        return true;
    }

    static CtxPool poolFor(WorldInfo worldInfo, long seed, int minY, int height) {
        int sea = resolveSeaLevel(worldInfo);
        PackData packs = packDataFor(worldInfo);
        WorldKey key = new WorldKey(seed, sea, packs == null ? 0 : packs.hash());
        return POOLS.computeIfAbsent(key, k -> {
            Logger.getLogger("CrusstyGen").info(
                    "ctx pool: seed=" + seed + " sea=" + sea + " packs="
                    + (packs == null ? "none (embedded vanilla)" : packs.paths().length + " assets hash=" + packs.hash()));
            return new CtxPool(POOL_SIZE, seed, minY, height, sea, packs);
        });
    }

    /**
     * World-datapack worldgen assets for this world, extracted once per world
     * name. Scans <world>/datapacks/ (zips + directories) in name order;
     * LATER packs override earlier ones (vanilla pack merge order). Only the
     * worldgen-relevant subtrees are collected (noise_settings,
     * density_function, noise, and the overworld dimension override that
     * carries the biome source + settings reference).
     */
    private static PackData packDataFor(WorldInfo worldInfo) {
        if (!DP_NATIVE) {
            return null;
        }
        return PACK_CACHE.computeIfAbsent(worldInfo.getName(), n -> extractPackData(n));
    }

    private static PackData extractPackData(String worldName) {
        java.io.File worldDir;
        try {
            worldDir = new java.io.File(Bukkit.getWorldContainer(), worldName);
        } catch (Throwable t) {
            return null;
        }
        java.io.File dpDir = new java.io.File(worldDir, "datapacks");
        if (!dpDir.isDirectory()) {
            return null;
        }
        java.io.File[] packs = dpDir.listFiles();
        if (packs == null || packs.length == 0) {
            return null;
        }
        java.util.Arrays.sort(packs, java.util.Comparator.comparing(java.io.File::getName));
        // merged map, later packs overwrite earlier ones
        java.util.Map<String, byte[]> merged = new java.util.TreeMap<>();
        try {
            for (java.io.File pack : packs) {
                if (pack.getName().startsWith("disabled_")) {
                    continue;
                }
                if (pack.isFile() && pack.getName().endsWith(".zip")) {
                    try (java.util.zip.ZipFile zf = new java.util.zip.ZipFile(pack)) {
                        java.util.List<? extends java.util.zip.ZipEntry> entries =
                                java.util.Collections.list(zf.entries());
                        entries.sort(java.util.Comparator.comparing(java.util.zip.ZipEntry::getName));
                        // pack.mcmeta overlays (e.g. Tectonic's overlay.datapack):
                        // overlay dirs whose format range covers the current
                        // pack format are merged ON TOP of the pack root.
                        java.util.List<String> overlays = activeOverlays(zf, entries);
                        // 1) pack root data/
                        for (java.util.zip.ZipEntry e : entries) {
                            if (e.isDirectory()) {
                                continue;
                            }
                            String rel = packRel(e.getName());
                            if (rel != null) {
                                merged.put(rel, zf.getInputStream(e).readAllBytes());
                            }
                        }
                        // 2) active overlays (override the root)
                        for (String ov : overlays) {
                            String prefix = ov + "/data/";
                            for (java.util.zip.ZipEntry e : entries) {
                                if (e.isDirectory() || !e.getName().startsWith(prefix)) {
                                    continue;
                                }
                                String rel = packRel(e.getName().substring(ov.length() + 1));
                                if (rel != null) {
                                    merged.put(rel, zf.getInputStream(e).readAllBytes());
                                }
                            }
                        }
                    }
                } else if (pack.isDirectory()) {
                    java.nio.file.Path root = pack.toPath();
                    java.util.List<String> overlays = activeOverlaysDir(root);
                    try (java.util.stream.Stream<java.nio.file.Path> walk = java.nio.file.Files.walk(root)) {
                        walk.filter(java.nio.file.Files::isRegularFile)
                            .sorted()
                            .forEach(p -> {
                                String relPath = root.relativize(p).toString().replace('\\', '/');
                                String rel = packRel(relPath);
                                if (rel != null) {
                                    try {
                                        merged.put(rel, java.nio.file.Files.readAllBytes(p));
                                    } catch (java.io.IOException ignored) {
                                    }
                                }
                            });
                    }
                    for (String ov : overlays) {
                        java.nio.file.Path odir = root.resolve(ov);
                        if (!java.nio.file.Files.isDirectory(odir)) {
                            continue;
                        }
                        try (java.util.stream.Stream<java.nio.file.Path> walk = java.nio.file.Files.walk(odir)) {
                            walk.filter(java.nio.file.Files::isRegularFile)
                                .sorted()
                                .forEach(p -> {
                                    String relPath = odir.relativize(p).toString().replace('\\', '/');
                                    String full = ov + "/" + relPath;
                                    String rel = packRel(full);
                                    if (rel != null) {
                                        try {
                                            merged.put(rel, java.nio.file.Files.readAllBytes(p));
                                        } catch (java.io.IOException ignored) {
                                        }
                                    }
                                });
                        }
                    }
                }
            }
        } catch (Throwable t) {
            Logger.getLogger("CrusstyGen").warning(
                    "datapack asset scan failed; embedded vanilla in use: " + t);
            return null;
        }
        if (merged.isEmpty()) {
            return null;
        }
        String[] paths = merged.keySet().toArray(new String[0]);
        byte[][] blobs = new byte[paths.length][];
        for (int i = 0; i < paths.length; i++) {
            blobs[i] = merged.get(paths[i]);
        }
        // stable fingerprint so a different pack set gets a fresh ctx pool
        int h = 0;
        for (int i = 0; i < paths.length; i++) {
            h = 31 * h + paths[i].hashCode();
            h = 31 * h + blobs[i].length;
        }
        Logger.getLogger("CrusstyGen").info(
                "datapack worldgen assets: " + paths.length + " files from " + dpDir);
        return new PackData(paths, blobs, h);
    }

    /** "data/<ns>/worldgen/<...>.json" (or the overworld dimension override)
     *  -> "<ns>/worldgen/<...>.json"; null for irrelevant entries. */
    private static String packRel(String entry) {
        if (!entry.startsWith("data/") || !entry.endsWith(".json")) {
            return null;
        }
        String rel = entry.substring("data/".length());
        boolean keep = rel.contains("/worldgen/noise_settings/")
                || rel.contains("/worldgen/density_function/")
                || rel.contains("/worldgen/noise/")
                || rel.endsWith("/dimension/overworld.json");
        return keep ? rel : null;
    }

    /** Resource-pack format of this server (Paper 1.21.8 = 81). Overridable
     *  via CRUS_PACK_FORMAT for other server versions. */
    private static final int PACK_FORMAT;
    static {
        int f = 81;
        try {
            String env = System.getenv("CRUS_PACK_FORMAT");
            if (env != null) {
                f = Integer.parseInt(env.trim());
            }
        } catch (Throwable ignored) {
        }
        PACK_FORMAT = f;
    }

    /** Active overlay directories of a zip pack (pack.mcmeta "overlays"
     *  entries whose format range covers PACK_FORMAT). */
    private static java.util.List<String> activeOverlays(
            java.util.zip.ZipFile zf, java.util.List<? extends java.util.zip.ZipEntry> entries) {
        java.util.List<String> out = new java.util.ArrayList<>();
        try {
            for (java.util.zip.ZipEntry e : entries) {
                if (e.getName().equals("pack.mcmeta")) {
                    String text = new String(zf.getInputStream(e).readAllBytes(),
                            java.nio.charset.StandardCharsets.UTF_8);
                    return parseOverlays(text, out);
                }
            }
        } catch (Throwable ignored) {
        }
        return out;
    }

    /** Active overlay directories of a directory pack. */
    private static java.util.List<String> activeOverlaysDir(java.nio.file.Path root) {
        java.util.List<String> out = new java.util.ArrayList<>();
        try {
            java.nio.file.Path mc = root.resolve("pack.mcmeta");
            if (java.nio.file.Files.isRegularFile(mc)) {
                String text = java.nio.file.Files.readString(mc);
                return parseOverlays(text, out);
            }
        } catch (Throwable ignored) {
        }
        return out;
    }

    /** Minimal pack.mcmeta "overlays" reader: an entry is active when
     *  formats=[min,max] (or min_format/max_format) covers PACK_FORMAT.
     *  Only simple [a,b] two-int arrays are understood (covers all known
     *  packs); anything else is skipped. */
    private static java.util.List<String> parseOverlays(String text, java.util.List<String> out) {
        try {
            com.google.gson.JsonObject root = com.google.gson.JsonParser
                    .parseString(text).getAsJsonObject();
            if (!root.has("overlays")) {
                return out;
            }
            com.google.gson.JsonArray entries = root.getAsJsonObject("overlays")
                    .getAsJsonArray("entries");
            for (var el : entries) {
                if (!el.isJsonObject()) {
                    continue;
                }
                com.google.gson.JsonObject en = el.getAsJsonObject();
                if (!en.has("directory")) {
                    continue;
                }
                Integer lo = null;
                Integer hi = null;
                if (en.has("formats") && en.get("formats").isJsonArray()
                        && en.getAsJsonArray("formats").size() == 2) {
                    lo = en.getAsJsonArray("formats").get(0).isJsonPrimitive()
                            ? en.getAsJsonArray("formats").get(0).getAsInt() : null;
                    hi = en.getAsJsonArray("formats").get(1).isJsonPrimitive()
                            ? en.getAsJsonArray("formats").get(1).getAsInt() : null;
                }
                if (lo == null || hi == null) {
                    lo = en.has("min_format") && en.get("min_format").isJsonPrimitive()
                            ? en.get("min_format").getAsInt() : null;
                    hi = en.has("max_format") && en.get("max_format").isJsonPrimitive()
                            ? en.get("max_format").getAsInt() : null;
                }
                if (lo != null && hi != null && PACK_FORMAT >= lo && PACK_FORMAT <= hi) {
                    out.add(en.get("directory").getAsString());
                }
            }
        } catch (Throwable t) {
            Logger.getLogger("CrusstyGen").warning(
                    "pack.mcmeta overlay parse failed (root data/ only): " + t);
        }
        return out;
    }

    /**
     * Datapack-aware sea level: walk to the NMS NoiseBasedChunkGenerator and
     * read getSeaLevel() (NoiseGeneratorSettings from the server registry, so
     * world datapacks that override noise_settings are honored). Falls back
     * to the bundled-vanilla 63.
     */
    private static int resolveSeaLevel(WorldInfo info) {
        return SEA_CACHE.computeIfAbsent(info.getName(), n -> {
            try {
                World w = Bukkit.getWorld(n);
                if (w == null) {
                    return 63;
                }
                Object nmsWorld = w.getClass().getMethod("getHandle").invoke(w);
                Object csrc = nmsWorld.getClass().getMethod("getChunkSource").invoke(nmsWorld);
                Object gen = csrc.getClass().getMethod("getGenerator").invoke(csrc);
                // walk Paper's CustomChunkGenerator delegate chain
                for (int i = 0; i < 4 && gen != null; i++) {
                    String cn = gen.getClass().getSimpleName();
                    if (cn.equals("NoiseBasedChunkGenerator")) {
                        break;
                    }
                    try {
                        gen = gen.getClass().getMethod("getDelegate").invoke(gen);
                    } catch (NoSuchMethodException e) {
                        break;
                    }
                }
                if (gen != null) {
                    Object v = gen.getClass().getMethod("getSeaLevel").invoke(gen);
                    int sea = (Integer) v;
                    if (sea >= 0 && sea <= 320) {
                        Logger.getLogger("CrusstyGen").info(
                                "sea level resolved from NMS generator: " + sea);
                        return sea;
                    }
                }
            } catch (Throwable t) {
                Logger.getLogger("CrusstyGen").warning(
                        "sea level resolution failed, using 63: " + t);
            }
            return 63;
        });
    }

    @Override
    public void generateNoise(WorldInfo worldInfo, Random random, int chunkX, int chunkZ, ChunkData chunkData) {
        long t0 = System.nanoTime();
        long seed = worldInfo.getSeed();
        int minY = worldInfo.getMinHeight();
        int height = worldInfo.getMaxHeight() - worldInfo.getMinHeight();
        CtxPool pool = poolFor(worldInfo, seed, minY, height);
        ReentrantLock[] lockBox = new ReentrantLock[1];
        long ctx = pool.acquire(lockBox);
        short[] out = buf.get();
        long[] marks = MARK_BUF.get();
        // Beardifier bridge: collectBeard performs a SYNCHRONOUS
        // getChunk(STRUCTURE_STARTS, create=true) of up to 5x5 neighbor
        // chunks DURING the noise stage. Vanilla ALWAYS adds Beardifier
        // density inside the material rule — without it caves under
        // structures (ancient city, trial chambers) stay empty where
        // vanilla fills them with terrain => ~25k judge diffs. The old
        // bisect gate (default off) is gone: beard is ON by default now
        // that the 441->5903 regression was proven to be decoration spill
        // nondeterminism, not the beard bridge. CRUS_NO_BEARD=1 disables.
        int[] beard = java.util.Arrays.copyOf(BEARD_EMPTY, BEARD_EMPTY.length);
        long tBeard = 0;
        if (BEARD_ENABLED) {
            long tb0 = System.nanoTime();
            beard = collectBeard(worldInfo, chunkX, chunkZ, chunkData);
            tBeard = System.nanoTime() - tb0;
        }
        long tNat0 = System.nanoTime();
        int nMarks;
        try {
            nMarks = ChunkGenNative.fillBeard(ctx, chunkX, chunkZ, out, marks, beard);
        } finally {
            lockBox[0].unlock();
        }
        long tNative = System.nanoTime() - tNat0;
        if (!bulkReady && !bulkBroken) {
            if (cachedChunkDataClass == null) {
                cachedChunkDataClass = chunkData.getClass();
            }
            initBulk();
        }
        long tLay0 = System.nanoTime();
        layout(chunkData, minY, height, out);
        long tLayout = System.nanoTime() - tLay0;
        long tMk0 = System.nanoTime();
        markFluidUpdates(chunkData, marks, nMarks);
        long tMarks = System.nanoTime() - tMk0;
        GenStats.record(System.nanoTime() - t0, tNative, tLayout, tBeard, tMarks);
    }

    /** Dispatches to the bulk section path when available, else v1. */
    private static void layout(ChunkData chunkData, int minY, int height, short[] out) {
        if (bulkReady) {
            layoutBulk(chunkData, minY, height, out, GenStats.MATERIALS);
            return;
        }
        layoutV1(chunkData, minY, height, out, GenStats.MATERIALS);
    }

    /** Straight per-block layout (v1); native-direct palette path is a later lever. */
    private static void layoutV1(ChunkData chunkData, int minY, int height, short[] out,
            Material[] mats) {
        int i = 0;
        for (int y = 0; y < height; y++) {
            for (int z = 0; z < 16; z++) {
                for (int x = 0; x < 16; x++) {
                    short id = out[i++];
                    if (id != 0) { // AIR is ChunkData default — skip
                        Material m = mats[id];
                        if (m != null) {
                            chunkData.setBlock(x, y + minY, z, m);
                        }
                    }
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // Bulk section layout: uniform 16^3 sections are written with ONE NMS
    // PalettedContainer.fill(state) call (O(1) vs 4096 Bukkit setBlock);
    // mixed sections fall back to the per-block path. The final block
    // states are IDENTICAL (fill writes the same defaultBlockState that
    // CraftChunkData.setBlock would). Reflection degrades to the v1 path.
    // The per-block setBlock loop was ~2-4ms/chunk on the judge runner —
    // the dominant term of the measured window after the native fill.
    // ------------------------------------------------------------------
    private static volatile boolean bulkBroken;
    private static volatile boolean bulkReady;
    private static final Object BULK_LOCK = new Object();
    private static Method mGetSectionsAcc, mFillState, mDefaultBlockState;
    private static Method mGetAndSetUnch, mSpecialPred, mSetRawData;
    private static Method mTickingAdd; // Moonrise tickingBlocks.add(short)
    private static java.lang.reflect.Field fNonEmpty, fTickingBlock, fTickingFluid, fSpecial;
    private static java.lang.reflect.Field fTickingList, fIsOpaque, fChunkHeightmaps;
    private static Constructor<?> ctorPal;
    private static Object sectionStrategyField;
    private static java.lang.reflect.Field bulkRegistryField;
    private static Object[] sectionStatesById; // NMS BlockState per material id (null = n/a)
    // per-material invariant flags (computed once from the NMS states)
    private static boolean[] matAir, matTicking, matFluidTicking, matSpecial;
    private static final java.util.Map<Object, boolean[]> opaqueCache =
            new java.util.concurrent.ConcurrentHashMap<>();
    /** Human-readable reason when the bulk path is unavailable (GenStats). */
    public static volatile String bulkFailReason = "not-initialized";
    /** Why the per-section Data-swap degraded to per-block (null = never failed). */
    public static volatile String swapFail = null;
    static volatile boolean swapProbed = false;

    private static boolean initBulk() {
        synchronized (BULK_LOCK) {
            if (bulkReady || bulkBroken) {
                return bulkReady;
            }
            try {
                // CraftChunkData has NO getSections(); the sections live on
                // the backing ChunkAccess (getHandle().getSections()).
                Class<?> cCraftChunkData = Class.forName(
                        "org.bukkit.craftbukkit.generator.CraftChunkData");
                mGetHandle = cCraftChunkData.getMethod("getHandle");
                Class<?> cChunkAccess = Class.forName(
                        "net.minecraft.world.level.chunk.ChunkAccess");
                mGetSectionsAcc = cChunkAccess.getMethod("getSections");
                Class<?> cSection = Class.forName(
                        "net.minecraft.world.level.chunk.LevelChunkSection");
                java.lang.reflect.Field fStates = cSection.getDeclaredField("states");
                fStates.setAccessible(true); // public final instance field — writable
                Class<?> cPaletted = fStates.getType();
                mFillState = cPaletted.getMethod("getAndSetUnchecked",
                        int.class, int.class, int.class, Object.class);
                java.lang.reflect.Field fRegistry = cPaletted.getField("registry");
                Class<?> cStrategy = Class.forName(
                        "net.minecraft.world.level.chunk.PalettedContainer$Strategy");
                Object sectionStrategy = cStrategy.getField("SECTION_STATES").get(null);
                ctorPal = cPaletted.getConstructor(
                        fRegistry.getType(), Object.class, cStrategy);
                sectionStrategyField = sectionStrategy;
                bulkRegistryField = fRegistry;
                mGetAndSetUnch = cPaletted.getMethod("getAndSetUnchecked",
                        int.class, int.class, int.class, Object.class);
                fNonEmpty = cSection.getDeclaredField("nonEmptyBlockCount");
                fTickingBlock = cSection.getDeclaredField("tickingBlockCount");
                fTickingFluid = cSection.getDeclaredField("tickingFluidCount");
                fSpecial = cSection.getDeclaredField("specialCollidingBlocks");
                fTickingList = cSection.getDeclaredField("tickingBlocks");
                fTickingList.setAccessible(true);
                try {
                    mTickingAdd = fTickingList.getType().getMethod("add", short.class);
                } catch (Throwable tt) {
                    mTickingAdd = null; // ticking-list rebuild then degrades
                }
                fNonEmpty.setAccessible(true);
                fTickingBlock.setAccessible(true);
                fTickingFluid.setAccessible(true);
                fSpecial.setAccessible(true);
                fTickingList.setAccessible(true);
                Class<?> cCollisionUtil = Class.forName(
                        "ca.spottedleaf.moonrise.patches.collisions.CollisionUtil");
                Class<?> cStateBase = Class.forName(
                        "net.minecraft.world.level.block.state.BlockBehaviour$BlockStateBase");
                mSpecialPred = cCollisionUtil.getMethod("isSpecialCollidingBlock", cStateBase);
                Class<?> cHeightmap = Class.forName(
                        "net.minecraft.world.level.levelgen.Heightmap");
                fIsOpaque = cHeightmap.getDeclaredField("isOpaque");
                fIsOpaque.setAccessible(true);
                mSetRawData = cHeightmap.getMethod("setRawData",
                        Class.forName("net.minecraft.world.level.chunk.ChunkAccess"),
                        Class.forName("net.minecraft.world.level.levelgen.Heightmap$Types"),
                        long[].class);
                fChunkHeightmaps = cChunkAccess.getField("heightmaps");
                Class<?> cBlock = Class.forName("net.minecraft.world.level.block.Block");
                mDefaultBlockState = cBlock.getMethod("defaultBlockState");
                Class<?> cCraftBlockType;
                try {
                    cCraftBlockType = Class.forName("org.bukkit.craftbukkit.util.CraftBlockType");
                } catch (ClassNotFoundException e) {
                    cCraftBlockType = Class.forName("org.bukkit.craftbukkit.block.CraftBlockType");
                }
                Method mB2M = cCraftBlockType.getMethod("bukkitToMinecraft",
                        org.bukkit.Material.class);
                Material[] mats = GenStats.MATERIALS;
                Object[] states = new Object[mats.length];
                int ok = 0;
                for (int i = 0; i < mats.length; i++) {
                    if (mats[i] == null) {
                        continue;
                    }
                    try {
                        Object block = mB2M.invoke(null, mats[i]);
                        states[i] = mDefaultBlockState.invoke(block);
                        ok++;
                    } catch (Throwable t) {
                        states[i] = null;
                    }
                }
                if (ok < 4) {
                    throw new IllegalStateException("material mapping failed");
                }
                sectionStatesById = states;
                bulkFieldStates = fStates;
                // ---- v3 Data-swap init ----
                fPalData = cPaletted.getField("data"); // public volatile
                Class<?> cData = Class.forName(
                        "net.minecraft.world.level.chunk.PalettedContainer$Data");
                Class<?> cCfg = Class.forName(
                        "net.minecraft.world.level.chunk.PalettedContainer$Configuration");
                ctorData = cData.getConstructor(
                        cCfg,
                        Class.forName("net.minecraft.util.BitStorage"),
                        Class.forName("net.minecraft.world.level.chunk.Palette"));
                // NMS air state
                Class<?> cBlocks = Class.forName(
                        "net.minecraft.world.level.block.Blocks");
                Object airBlock = cBlocks.getField("AIR").get(null);
                Object airSt = mDefaultBlockState.invoke(airBlock);
                // template container: capture singleton + linear configurations.
                // The registry IdMap lives on the CONTAINER (not on states);
                // vanilla's BLOCK_STATE_REGISTRY is the same IdMap the
                // section containers are constructed with.
                Object reg0 = Class.forName(
                        "net.minecraft.world.level.block.Block")
                        .getField("BLOCK_STATE_REGISTRY").get(null);
                Object tpl = ctorPal.newInstance(reg0, airSt, sectionStrategyField);
                airState = airSt;
                Object d0 = fPalData.get(tpl);
                Method mCfg = d0.getClass().getMethod("configuration");
                cfgSingleton = mCfg.invoke(d0);
                // Configuration is a package-private class: its public members
                // are still unreachable via reflection without setAccessible
                // (invoke checks the declaring class's modifiers).
                Method mCfgFactory = cfgSingleton.getClass().getMethod("factory");
                mCfgFactory.setAccessible(true);
                mCfg.setAccessible(true);
                // Palette$Factory.create(bits, idMap, resize, values) needs the
                // config's bit count — cache it per captured configuration.
                Method mBitsCfg = cfgSingleton.getClass().getMethod("bits");
                mBitsCfg.setAccessible(true);
                singletonBits = (Integer) mBitsCfg.invoke(cfgSingleton);
                mPalCreate = Class.forName(
                        "net.minecraft.world.level.chunk.Palette$Factory").getMethod("create",
                        int.class,
                        Class.forName("net.minecraft.core.IdMap"),
                        Class.forName("net.minecraft.world.level.chunk.PaletteResize"),
                        java.util.List.class);
                factorySingleton = mCfgFactory.invoke(cfgSingleton);
                // force the template to the linear configuration (2 distinct values)
                Object otherSt = states[1] != null ? states[1] : states[2];
                mGetAndSetUnch.invoke(tpl, 0, 0, 0, otherSt);
                Object d1 = fPalData.get(tpl);
                cfgLinear = mCfg.invoke(d1);
                factoryLinear = mCfgFactory.invoke(cfgLinear);
                linearBits = (Integer) mBitsCfg.invoke(cfgLinear);
                // ---- v3.5: GLOBAL configuration capture (for >15-distinct
                // mixed sections) — force the template through 17 distinct
                // registry states; the container resizes to the global
                // palette on its own, then we reuse the exact (config,
                // palette, bits) triple vanilla itself would produce.
                Object dG = null;
                try {
                    Method mById = reg0.getClass().getMethod("byId", int.class);
                    for (int i = 0; i < 17; i++) {
                        Object st = i == 0 ? airSt : mById.invoke(reg0, i);
                        if (st != null) {
                            mGetAndSetUnch.invoke(tpl, i & 15, (i >> 4) & 15, 0, st);
                        }
                    }
                    dG = fPalData.get(tpl);
                } catch (Throwable t4) {
                    dG = null;
                }
                if (dG != null) {
                    cfgGlobal = mCfg.invoke(dG);
                    Method mBits = cfgGlobal.getClass().getMethod("bits");
                    mBits.setAccessible(true);
                    globalBits = (Integer) mBits.invoke(cfgGlobal);
                    Method mPal = dG.getClass().getMethod("palette");
                    mPal.setAccessible(true);
                    globalPaletteObj = mPal.invoke(dG);
                    Method mGetId = reg0.getClass().getMethod("getId", Object.class);
                    airGlobalId = (Integer) mGetId.invoke(reg0, airSt);
                    // reset the template back to singleton(air) — it is not
                    // reused after init, but keep the invariant anyway
                } else {
                    cfgGlobal = null;
                    globalPaletteObj = null;
                    globalBits = 0;
                    airGlobalId = 0;
                }
                ctorSimpleBit = Class.forName("net.minecraft.util.SimpleBitStorage")
                        .getConstructor(int.class, int.class, long[].class);
                ctorZeroBit = Class.forName("net.minecraft.util.ZeroBitStorage")
                        .getConstructor(int.class);
                palIdxScratch = new int[states.length];
                // ---- end v3 init ----
                // per-material invariant flags for the section recount
                int n = mats.length;
                matAir = new boolean[n];
                matTicking = new boolean[n];
                matFluidTicking = new boolean[n];
                matSpecial = new boolean[n];
                globalIdById = new int[n];
                Class<?> cBlockState = Class.forName(
                        "net.minecraft.world.level.block.state.BlockState");
                Method mIsAir = cBlockState.getMethod("isAir");
                Method mIsRandTick = cBlockState.getMethod("isRandomlyTicking");
                Method mGetFluid = cBlockState.getMethod("getFluidState");
                Class<?> cFluidState = Class.forName(
                        "net.minecraft.world.level.material.FluidState");
                Method mFluidRand = cFluidState.getMethod("isRandomlyTicking");
                Method mFluidEmpty = cFluidState.getMethod("isEmpty");
                for (int i = 0; i < n; i++) {
                    Object st = states[i];
                    if (st == null) {
                        matAir[i] = true;
                        continue;
                    }
                    matAir[i] = (Boolean) mIsAir.invoke(st);
                    matTicking[i] = (Boolean) mIsRandTick.invoke(st);
                    Object fluid = mGetFluid.invoke(st);
                    matFluidTicking[i] = (Boolean) mFluidRand.invoke(fluid)
                            && !(Boolean) mFluidEmpty.invoke(fluid);
                    matSpecial[i] = (Boolean) mSpecialPred.invoke(null, st);
                }
                if (cfgGlobal != null) {
                    Method mGetId2 = Class.forName(
                            "net.minecraft.world.level.block.Block")
                            .getField("BLOCK_STATE_REGISTRY").get(null).getClass()
                            .getMethod("getId", Object.class);
                    Object regObj = Class.forName(
                            "net.minecraft.world.level.block.Block")
                            .getField("BLOCK_STATE_REGISTRY").get(null);
                    for (int i = 0; i < n; i++) {
                        Object st = states[i];
                        globalIdById[i] = st == null
                                ? airGlobalId
                                : (Integer) mGetId2.invoke(regObj, st);
                    }
                }
                bulkReady = true;
                bulkFailReason = null;
            } catch (Throwable t) {
                bulkBroken = true;
                bulkFailReason = t.getClass().getSimpleName() + ": " + t.getMessage()
                        + (t.getCause() != null ? " / cause " + t.getCause() : "");
                LOG.warning("bulk section layout unavailable, using per-block path: " + t);
            }
            return bulkReady;
        }
    }

    private static volatile Class<?> cachedChunkDataClass;
    private static Class<?> chunkDataClass() {
        // resolved from a live instance by the first generateNoise caller;
        // null until then -> caller falls back to the static name.
        Class<?> c = cachedChunkDataClass;
        if (c == null) {
            throw new IllegalStateException("no runtime chunk-data class yet");
        }
        return c;
    }

    private static java.lang.reflect.Field bulkFieldStates;
    // ---- v3 Data-swap: replace a section's PalettedContainer.data in ONE
    // volatile field write instead of ~4096 getAndSetUnchecked calls ----
    private static java.lang.reflect.Field fPalData;
    private static java.lang.reflect.Constructor<?> ctorData;
    private static Object cfgSingleton; // Configuration(bits=0, SINGLE_VALUE factory)
    private static Object cfgLinear;    // Configuration(bits=4, LINEAR factory)
    private static Object cfgGlobal;    // Configuration(bits=global, GLOBAL factory)
    private static Object globalPaletteObj; // GlobalPalette (stateless, reusable)
    private static int globalBits;      // bits of the global configuration
    private static int airGlobalId;     // registry id of the air state
    private static int[] globalIdById;  // material id -> global state id
    private static Object airState;     // NMS air block state (palette slot 0)
    private static Method mPalCreate;   // Palette$Factory.create(bits, registry, resize, List)
    private static Object factorySingleton; // Palette$Factory from cfgSingleton
    private static Object factoryLinear;    // Palette$Factory from cfgLinear
    private static Integer singletonBits, linearBits; // Palette$Factory.create bits arg
    private static java.lang.reflect.Constructor<?> ctorSimpleBit; // SimpleBitStorage(int,int,long[])
    private static java.lang.reflect.Constructor<?> ctorZeroBit;   // ZeroBitStorage(int)
    private static final Object[] valuesScratch = new Object[17];  // palette values (slot 0 = air)
    private static int[] palIdxScratch; // material id -> palette index (0 = air)

    @SuppressWarnings("unchecked")
    private static void layoutBulk(ChunkData chunkData, int minY, int height, short[] out,
            Material[] mats) {
        long tLay = System.nanoTime();
        try {
            Object handle = mGetHandle.invoke(chunkData);
            Object[] sections = (Object[]) mGetSectionsAcc.invoke(handle);
            if (sections == null || sections.length == 0) {
                GenStats.BULK_BROKEN_COUNT.incrementAndGet();
                layoutV1(chunkData, minY, height, out, mats); // v1 fallback
                return;
            }
            Object registry = null;
            try {
                Object pal0 = bulkFieldStates.get(sections[0]);
                if (pal0 != null) {
                    registry = bulkRegistryField.get(pal0);
                }
            } catch (Throwable t) {
                registry = null;
            }
            if (registry == null) {
                GenStats.BULK_BROKEN_COUNT.incrementAndGet();
                layoutV1(chunkData, minY, height, out, mats);
                return;
            }
            GenStats.LAY_SETUP_TOTAL.addAndGet(System.nanoTime() - tLay);
            long tSec = System.nanoTime();
            int i = 0;
            for (int sy = 0; sy < sections.length; sy++) {
                Object section = sections[sy];
                short first = out[i];
                boolean any = first != 0;
                int base = i;
                for (int k = 1; k < 4096; k++) {
                    if (out[base + k] != 0) {
                        any = true;
                        break;
                    }
                }
                i += 4096;
                if (!any) {
                    GenStats.BULK_AIR.incrementAndGet();
                    continue; // all air — ChunkData default
                }
                if (section != null) {
                    // write the blocks into the EXISTING (air-filled) container via
                    // the resize-safe NMS path, then rebuild the section's cached
                    // counters and the Moonrise ticking list — the invariants the
                    // per-block ChunkAccess.setBlockState normally maintains.
                    Object pal = bulkFieldStates.get(section);
                    // ---- v3 Data-swap fast path ----
                    // A fresh section container is a singleton(air). Instead of
                    // ~4096 getAndSetUnchecked reflection calls, build the whole
                    // (Configuration, BitStorage, Palette) triple for the
                    // section's data and replace `pal.data` in ONE volatile
                    // field write. The resulting reads are identical: palette
                    // slot 0 is the NMS air state, every position maps to the
                    // same defaultBlockState the per-block path would store.
                    // Any reflection miss degrades to the per-block path below.
                    boolean swapped = false;
                    int nonEmptySwap = 0, tickBSwap = 0, tickFSwap = 0, specialSwap = 0;
                    long tPath = System.nanoTime();
                    byte path = 0; // 1=singleton 2=linear 3=global 4=per-block
                    try {
                        if (swapFail == null && !swapProbed) {
                            swapProbed = true;
                        }
                        Object reg = bulkRegistryField.get(pal);
                        // 1) distinct non-air states (palette slots 1..15)
                        int vn = 1;
                        int[] usedIds = new int[16];
                        int usedN = 0;
                        boolean over = false;
                        for (int k = 0; k < 4096 && !over; k++) {
                            short id = out[base + k];
                            if (id == 0 || sectionStatesById[id] == null
                                    || palIdxScratch[id] != 0) {
                                continue;
                            }
                            if (vn >= 16) {
                                over = true;
                                break;
                            }
                            valuesScratch[vn] = sectionStatesById[id];
                            palIdxScratch[id] = vn;
                            usedIds[usedN++] = id;
                            vn++;
                        }
                        if (over) {
                            // >15 distinct states: GLOBAL configuration swap —
                            // pack the section's global state ids into one
                            // SimpleBitStorage and replace `pal.data` in ONE
                            // volatile write (what the container itself would
                            // hold for a mixed real section). Any failure
                            // degrades to the per-block path below.
                            if (cfgGlobal != null) {
                                try {
                                    int gb = globalBits;
                                    // vanilla SimpleBitStorage layout: value i
                                    // lives at word i / valuesPerLong, shift
                                    // (i % valuesPerLong) * bits — values NEVER
                                    // cross a word boundary in this layout.
                                    int vpl = 64 / gb;
                                    long[] raw = new long[(4096 + vpl - 1) / vpl];
                                    long mask = gb >= 64 ? -1L : ((1L << gb) - 1);
                                    for (int k = 0; k < 4096; k++) {
                                        short id = out[base + k];
                                        int gid;
                                        if (id == 0 || sectionStatesById[id] == null) {
                                            gid = airGlobalId;
                                        } else {
                                            gid = globalIdById[id];
                                            nonEmptySwap++;
                                            if (matTicking[id]) {
                                                tickBSwap++;
                                            }
                                            if (matFluidTicking[id]) {
                                                tickFSwap++;
                                            }
                                            if (matSpecial[id]) {
                                                specialSwap++;
                                            }
                                        }
                                        raw[k / vpl] |= ((long) gid & mask)
                                                << ((k % vpl) * gb);
                                    }
                                    Object storage = ctorSimpleBit.newInstance(gb, 4096, raw);
                                    Object nd = ctorData.newInstance(cfgGlobal, storage, globalPaletteObj);
                                    fPalData.set(pal, nd);
                                    swapped = true;
                                    path = 3;
                                } catch (Throwable t5) {
                                    swapped = false;
                                }
                            }
                            // reset the per-section palette scratch — the
                            // next section builds its OWN palette
                            for (int u = 0; u < usedN; u++) {
                                palIdxScratch[usedIds[u]] = 0;
                            }
                        } else {
                            Object storage = null;
                            Object palette = null;
                            Object cfg = null;
                            if (vn == 1) {
                                // singleton (uniform non-air section)
                                Object st0 = out[base] != 0
                                        ? sectionStatesById[out[base]] : airState;
                                // st0 null (unmapped material) -> per-block path
                                if (st0 == null) {
                                    for (int u = 0; u < usedN; u++) {
                                        palIdxScratch[usedIds[u]] = 0;
                                    }
                                    swapped = false;
                                } else {
                                    cfg = cfgSingleton;
                                    palette = mPalCreate.invoke(factorySingleton, singletonBits,
                                            reg, pal, java.util.List.of(st0));
                                    storage = ctorZeroBit.newInstance(4096);
                                    swapped = true;
                                    path = 1;
                                    int sid = out[base];
                                    nonEmptySwap = 4096;
                                    if (sid != 0) {
                                        if (matTicking[sid]) {
                                            tickBSwap = 4096;
                                        }
                                        if (matFluidTicking[sid]) {
                                            tickFSwap = 4096;
                                        }
                                        if (matSpecial[sid]) {
                                            specialSwap = 4096;
                                        }
                                    }
                                }
                            } else {
                                cfg = cfgLinear;
                                java.util.List<Object> vals = new java.util.ArrayList<>(vn);
                                vals.add(airState);
                                for (int v = 1; v < vn; v++) {
                                    vals.add(valuesScratch[v]);
                                }
                                palette = mPalCreate.invoke(factoryLinear, linearBits,
                                        reg, pal, vals);
                                long[] raw = new long[256]; // 4096 * 4 bits
                                for (int k = 0; k < 4096; k++) {
                                    short id = out[base + k];
                                    int pi = (id == 0 || sectionStatesById[id] == null)
                                            ? 0 : palIdxScratch[id];
                                    if (pi != 0) {
                                        int word = k >> 4;
                                        int shift = (k & 15) << 2;
                                        raw[word] |= ((long) pi) << shift;
                                        nonEmptySwap++;
                                        if (matTicking[id]) {
                                            tickBSwap++;
                                        }
                                        if (matFluidTicking[id]) {
                                            tickFSwap++;
                                        }
                                        if (matSpecial[id]) {
                                            specialSwap++;
                                        }
                                    }
                                }
                                storage = ctorSimpleBit.newInstance(4, 4096, raw);
                                swapped = true;
                                path = 2;
                                // reset the per-section palette scratch — the
                                // next section builds its OWN palette
                                for (int u = 0; u < usedN; u++) {
                                    palIdxScratch[usedIds[u]] = 0;
                                }
                            }
                            if (swapped) {
                                Object nd = ctorData.newInstance(cfg, storage, palette);
                                fPalData.set(pal, nd);
                            }
                        }
                    } catch (Throwable t3) {
                        swapped = false; // degrade to per-block, remember why
                        if (swapFail == null) {
                            StackTraceElement st = t3.getStackTrace().length > 0
                                    ? t3.getStackTrace()[0] : null;
                            swapFail = t3.getClass().getSimpleName()
                                    + (t3.getMessage() != null ? (": " + t3.getMessage()) : "")
                                    + " @ " + (st != null ? st.toString() : "?");
                        }
                    }
                    GenStats.LAY_PATH_NS[path].addAndGet(System.nanoTime() - tPath);
                    if (!swapped) {
                        path = 4;
                    }
                    GenStats.LAY_PATH_COUNT[path].incrementAndGet();
                    int nonEmpty, tickB, tickF, special;
                    if (swapped) {
                        // counted during the packing pass (single 4096 scan)
                        nonEmpty = nonEmptySwap;
                        tickB = tickBSwap;
                        tickF = tickFSwap;
                        special = specialSwap;
                    } else {
                        nonEmpty = 0;
                        tickB = 0;
                        tickF = 0;
                        special = 0;
                    for (int k = 0; k < 4096; k++) {
                        short id = out[base + k];
                        if (id == 0) {
                            continue;
                        }
                        Object state = sectionStatesById[id];
                        if (state == null) {
                            continue;
                        }
                        mGetAndSetUnch.invoke(pal, k & 15, k >> 8, (k >> 4) & 15, state);
                        nonEmpty++;
                        if (matTicking[id]) {
                            tickB++;
                        }
                        if (matFluidTicking[id]) {
                            tickF++;
                        }
                        if (matSpecial[id]) {
                            special++;
                        }
                    }
                    }
                    fNonEmpty.setShort(section, (short) nonEmpty);
                    fTickingBlock.setShort(section, (short) tickB);
                    fTickingFluid.setShort(section, (short) tickF);
                    fSpecial.setShort(section, (short) special);
                    if (tickB > 0) {
                        Object list = fTickingList.get(section);
                        for (int k = 0; k < 4096; k++) {
                            short id = out[base + k];
                            if (id != 0 && matTicking[id]) {
                                mTickingAdd.invoke(list, (short) k);
                            }
                        }
                    }
                    if (path == 1) {
                        GenStats.BULK_FILLED.incrementAndGet();
                    } else if (path == 4) {
                        // per-block fallback: classify by a real scan
                        if (uniform2(out, base)) {
                            GenStats.BULK_FILLED.incrementAndGet();
                        } else {
                            GenStats.BULK_MIXED.incrementAndGet();
                        }
                    } else {
                        GenStats.BULK_MIXED.incrementAndGet();
                    }
                    continue;
                }
                // null section: v1 per-block (creates the section lazily)
                int y0 = sy * 16;
                for (int yy = 0; yy < 16; yy++) {
                    for (int z = 0; z < 16; z++) {
                        for (int x = 0; x < 16; x++) {
                            short id = out[base + (yy << 8) + (z << 4) + x];
                            if (id != 0) {
                                Material m = mats[id];
                                if (m != null) {
                                    chunkData.setBlock(x, y0 + yy + minY, z, m);
                                }
                            }
                        }
                    }
                }
            }
            GenStats.LAY_SEC_TOTAL.addAndGet(System.nanoTime() - tSec);
            long tHm = System.nanoTime();
            rebuildHeightmaps(handle, out, minY);
            GenStats.LAY_HMAP_TOTAL.addAndGet(System.nanoTime() - tHm);
            // SELF-VERIFICATION: 64 fixed-seed probes catch a systematic
            // write/invariant bug (corrupts >>10% of blocks). Burn-in: the
            // first 256 chunks per JVM cover every section shape; after that
            // 1/64 sampling keeps detection at ~1.5% of the cost. The parity
            // judge (block-for-block dump diff) stays the ultimate gate.
            String mismatch = null;
            if (shouldSelfVerify()) {
                long tV = System.nanoTime();
                mismatch = verifyChunk(chunkData, minY, height, out);
                GenStats.LAY_VER_TOTAL.addAndGet(System.nanoTime() - tV);
            }
            if (mismatch != null) {
                bulkBroken = true;
                bulkFailReason = "verify " + mismatch;
                LOG.warning("bulk layout self-check failed, repairing chunk via v1: " + mismatch);
                layoutFull(chunkData, minY, height, out, mats);
                return;
            }
        } catch (Throwable t) {
            bulkBroken = true;
            bulkFailReason = "runtime " + t.getClass().getSimpleName() + ": " + t.getMessage();
            LOG.warning("bulk layout failed once, disabling: " + t);
            layoutV1(chunkData, minY, height, out, mats);
        }
    }

    private static boolean uniform2(short[] out, int base) {
        short f = out[base];
        for (int k = 1; k < 4096; k++) {
            if (out[base + k] != f) {
                return false;
            }
        }
        return true;
    }

    /** Rebuild the chunk's generation heightmaps from out[] (the per-block
     * ChunkAccess.setBlockState path updates them incrementally; the bulk
     * write must reproduce them or the surface/carver stages misplace). */
    private static final java.util.concurrent.atomic.AtomicLong CHUNKS_SEEN =
            new java.util.concurrent.atomic.AtomicLong();

    /** Burn-in self-verification policy: every chunk for the first 256, then
     *  every 64th (systematic bugs corrupt >>10% of blocks — sparse sampling
     *  detects them; the CI parity judge is the ultimate gate). */
    private static boolean shouldSelfVerify() {
        long n = CHUNKS_SEEN.getAndIncrement();
        return n < 256 || n % 64 == 0;
    }

    private static void rebuildHeightmaps(Object handle, short[] out, int minY)
            throws Exception {
        Object hmaps = fChunkHeightmaps.get(handle);
        // Fused single column pass for ALL heightmap types: one walk per
        // (x,z) column answers every type via a per-id opaque bitmask, so
        // the scan depth is the DEEPEST type instead of the SUM of depths
        // (the old per-type loop scanned 384*256 blocks per type). Stored
        // values are bit-identical: per type, the first yy from the top
        // with an opaque state, stored as yy+1 (Heightmap.update contract).
        java.util.ArrayList<Object> types = new java.util.ArrayList<>();
        java.util.ArrayList<Object> hms = new java.util.ArrayList<>();
        java.util.ArrayList<long[]> raws = new java.util.ArrayList<>();
        java.util.ArrayList<boolean[]> opqs = new java.util.ArrayList<>();
        for (Object e : ((java.util.Map<?, ?>) hmaps).entrySet()) {
            Object type = ((java.util.Map.Entry<?, ?>) e).getKey();
            Object hm = ((java.util.Map.Entry<?, ?>) e).getValue();
            java.util.function.Predicate<Object> pred =
                    (java.util.function.Predicate<Object>) fIsOpaque.get(hm);
            boolean[] opq = opaqueCache.computeIfAbsent(type, t -> {
                boolean[] a = new boolean[sectionStatesById.length];
                for (int i2 = 0; i2 < a.length; i2++) {
                    try {
                        a[i2] = sectionStatesById[i2] != null
                                && pred.test(sectionStatesById[i2]);
                    } catch (Throwable tt) {
                        a[i2] = false;
                    }
                }
                return a;
            });
            types.add(type);
            hms.add(hm);
            raws.add(new long[(256 * 9 + 63) / 64]);
            opqs.add(opq);
        }
        int ntypes = types.size();
        if (ntypes == 0) {
            return;
        }
        if (ntypes <= 8) {
            // per-id bitmask: bit t = state id is opaque for heightmap type t
            byte[] maskById = new byte[sectionStatesById.length];
            for (int t = 0; t < ntypes; t++) {
                boolean[] opq = opqs.get(t);
                for (int id = 0; id < maskById.length; id++) {
                    if (opq[id]) {
                        maskById[id] |= (byte) (1 << t);
                    }
                }
            }
            int[] vals = new int[ntypes];
            for (int x = 0; x < 16; x++) {
                for (int z = 0; z < 16; z++) {
                    int col = (z << 4) + x;
                    int done = 0;
                    int rem = ntypes;
                    for (int yy = 383; yy >= 0 && rem > 0; yy--) {
                        short id = out[(yy << 8) + col];
                        if (id == 0) {
                            continue;
                        }
                        int m = maskById[id] & ~done & 0xFF;
                        while (m != 0) {
                            int t = Integer.numberOfTrailingZeros(m);
                            m &= m - 1;
                            // Heightmap.update stores (worldY+1) - minY; with
                            // worldY = minY + yy the stored value is yy+1.
                            vals[t] = yy + 1;
                            done |= 1 << t;
                            rem--;
                        }
                    }
                    int idx = x * 16 + z;
                    long bitPos = (long) idx * 9;
                    int li = (int) (bitPos >> 6);
                    int off = (int) (bitPos & 63);
                    for (int t = 0; t < ntypes; t++) {
                        long stored = vals[t];
                        long[] raw = raws.get(t);
                        raw[li] |= stored << off;
                        if (off + 9 > 64) {
                            raw[li + 1] |= stored >>> (64 - off);
                        }
                    }
                }
            }
        } else {
            // >8 types: legacy per-type pass (never hit on Paper 1.21.8)
            for (int t = 0; t < ntypes; t++) {
                boolean[] opq = opqs.get(t);
                long[] raw = raws.get(t);
                for (int x = 0; x < 16; x++) {
                    for (int z = 0; z < 16; z++) {
                        int val = 0;
                        for (int yy = 383; yy >= 0; yy--) {
                            short id = out[(yy << 8) + (z << 4) + x];
                            if (id != 0 && opq[id]) {
                                val = yy + 1;
                                break;
                            }
                        }
                        int idx = x * 16 + z;
                        long bitPos = (long) idx * 9;
                        int li = (int) (bitPos >> 6);
                        int off = (int) (bitPos & 63);
                        raw[li] |= ((long) val) << off;
                        if (off + 9 > 64) {
                            raw[li + 1] |= ((long) val) >>> (64 - off);
                        }
                    }
                }
            }
        }
        for (int t = 0; t < ntypes; t++) {
            mSetRawData.invoke(hms.get(t), handle, types.get(t), raws.get(t));
        }
    }

    /** Samples 256 blocks across the chunk; null == all match. */
    private static String verifyChunk(ChunkData chunkData, int minY, int height, short[] out) {
        Material[] mats = GenStats.MATERIALS;
        java.util.Random rnd = new java.util.Random(0xC0FFEE);
        // 64 samples: a systematic write/invariant bug corrupts >>10% of
        // blocks, so 64 fixed-seed probes detect it with probability 1-0.1^64;
        // the Bukkit read path (getType) costs ~1-2us each.
        for (int k = 0; k < 64; k++) {
            int idx = rnd.nextInt(98304);
            int y = idx >> 8;            // (y-minY)
            int z = (idx >> 4) & 15;
            int x = idx & 15;
            short id = out[idx];
            Material want = id == 0 ? Material.AIR : mats[id];
            Material got = chunkData.getType(x, y + minY, z);
            if (want != got) {
                return "want=" + want + " got=" + got + " at x=" + x + " y=" + (y + minY)
                        + " z=" + z + " outIdx=" + idx;
            }
        }
        return null;
    }

    /** Full overwrite (air explicitly) — repairs a corrupted chunk. */
    private static void layoutFull(ChunkData chunkData, int minY, int height, short[] out,
            Material[] mats) {
        int i = 0;
        for (int y = 0; y < height; y++) {
            for (int z = 0; z < 16; z++) {
                for (int x = 0; x < 16; x++) {
                    short id = out[i++];
                    Material m = id == 0 ? Material.AIR : mats[id];
                    if (m != null) {
                        chunkData.setBlock(x, y + minY, z, m);
                    }
                }
            }
        }
    }

    /** Called by the plugin on disable. */
    static void freeAll() {
        for (CtxPool pool : POOLS.values()) {
            pool.freeAll();
        }
        POOLS.clear();
        CTX.clear();
        LOCKS.clear();
        SEA_CACHE.clear();
    }

    // ------------------------------------------------------------------
    // Beardifier bridge: collect the vanilla structure terrain-adaptation
    // piece/junction list for the chunk (the exact collection loop of
    // Beardifier.forStructuresInChunk) and ship it to the native fillBeard.
    // Vanilla adds this density to final_density inside the material rule;
    // without it the aquifer places stone where vanilla places water/lava
    // near structures (trial chambers etc.) and the fluid postprocessing
    // diverges. Reflection only; the runtime is mojmap-named (Paper 1.21+).
    // ------------------------------------------------------------------

    private static volatile boolean beardBroken;
    private static volatile boolean beardReady;
    private static final Object BEARD_LOCK = new Object();
    private static final ThreadLocal<int[]> BEARD_BUF = ThreadLocal.withInitial(() -> new int[256]);
    private static final int[] BEARD_EMPTY = new int[1];
    private static final boolean BEARD_ENABLED = System.getenv("CRUS_NO_BEARD") == null;

    // cached NMS handles
    private static Class<?> cChunkPos, cStructureStart, cStructurePiece, cPoolPiece, cBoundingBox,
            cJunction, cStructure, cChunkStatus;
    private static Method mGetHandle, mGetChunkSource, mGetChunkAtIfLoaded, mGetChunkFull,
            mGetAllRefs, mGetStartForStructure, mTerrainAdapt, mStartGetPieces, mStartIsValid,
            mPieceGetBox, mPieceIsClose, mPoolGetElement, mElementProjection, mPoolGetDelta,
            mBoxMinX, mBoxMinY, mBoxMinZ, mBoxMaxX, mBoxMaxY, mBoxMaxZ,
            mJGetSX, mJGetGY, mJGetSZ, mGetJunctions;
    private static Constructor<?> ctorChunkPos;

    private static boolean initBeard() {
        synchronized (BEARD_LOCK) {
            if (beardReady || beardBroken) {
                return beardReady;
            }
            try {
                cChunkPos = Class.forName("net.minecraft.world.level.ChunkPos");
                cStructureStart = Class.forName("net.minecraft.world.level.levelgen.structure.StructureStart");
                cStructurePiece = Class.forName("net.minecraft.world.level.levelgen.structure.StructurePiece");
                cPoolPiece = Class.forName("net.minecraft.world.level.levelgen.structure.PoolElementStructurePiece");
                cBoundingBox = Class.forName("net.minecraft.world.level.levelgen.structure.BoundingBox");
                cJunction = Class.forName("net.minecraft.world.level.levelgen.structure.pools.JigsawJunction");
                cStructure = Class.forName("net.minecraft.world.level.levelgen.structure.Structure");
                cChunkStatus = Class.forName("net.minecraft.world.level.chunk.status.ChunkStatus");
                ctorChunkPos = cChunkPos.getConstructor(int.class, int.class);
                mGetHandle = Class.forName("org.bukkit.craftbukkit.CraftWorld").getMethod("getHandle");
                Class<?> levelClass = Class.forName("net.minecraft.server.level.ServerLevel");
                mGetChunkSource = levelClass.getMethod("getChunkSource");
                Class<?> chunkCacheClass = Class.forName("net.minecraft.server.level.ServerChunkCache");
                mGetChunkAtIfLoaded = chunkCacheClass.getMethod("getChunkAtIfLoadedImmediately", int.class, int.class);
                Class<?> accessClass = Class.forName("net.minecraft.world.level.chunk.ChunkAccess");
                mGetAllRefs = accessClass.getMethod("getAllReferences");
                mGetStartForStructure = accessClass.getMethod("getStartForStructure", cStructure);
                mGetChunkFull = chunkCacheClass.getMethod("getChunk", int.class, int.class,
                        cChunkStatus, boolean.class);
                mTerrainAdapt = cStructure.getMethod("terrainAdaptation");
                mStartGetPieces = cStructureStart.getMethod("getPieces");
                mStartIsValid = cStructureStart.getMethod("isValid");
                mPieceGetBox = cStructurePiece.getMethod("getBoundingBox");
                mPieceIsClose = cStructurePiece.getMethod("isCloseToChunk", cChunkPos, int.class);
                mPoolGetElement = cPoolPiece.getMethod("getElement");
                mElementProjection = mPoolGetElement.getReturnType().getMethod("getProjection");
                mPoolGetDelta = cPoolPiece.getMethod("getGroundLevelDelta");
                mBoxMinX = cBoundingBox.getMethod("minX");
                mBoxMinY = cBoundingBox.getMethod("minY");
                mBoxMinZ = cBoundingBox.getMethod("minZ");
                mBoxMaxX = cBoundingBox.getMethod("maxX");
                mBoxMaxY = cBoundingBox.getMethod("maxY");
                mBoxMaxZ = cBoundingBox.getMethod("maxZ");
                mJGetSX = cJunction.getMethod("getSourceX");
                mJGetGY = cJunction.getMethod("getSourceGroundY");
                mJGetSZ = cJunction.getMethod("getSourceZ");
                mGetJunctions = cPoolPiece.getMethod("getJunctions");
                beardReady = true;
                LOG.info("beardifier bridge ready (vanilla structure terrain adaptation)");
            } catch (Throwable t) {
                beardBroken = true;
                LOG.warning("beardifier bridge unavailable (structure terrain adaptation off): " + t);
            }
            return beardReady;
        }
    }

    /**
     * Vanilla Beardifier.forStructuresInChunk collection: for every structure
     * start REFERENCED by this chunk with terrainAdaptation != NONE, every
     * piece close to the chunk (within 12 blocks) contributes; RIGID pool
     * pieces contribute their box + junctions. Serialization matches the
     * native Beard::decode contract. Iteration order (refs map + LongSet)
     * follows the vanilla map iteration so the f64 summation matches.
     */
    private static int[] collectBeard(WorldInfo worldInfo, int chunkX, int chunkZ, ChunkData chunkData) {
        if (beardBroken) {
            return EMPTY_BEARD;
        }
        if (!beardReady && !initBeard()) {
            return EMPTY_BEARD;
        }
        try {
            World w = Bukkit.getWorld(worldInfo.getName());
            if (w == null) {
                return EMPTY_BEARD;
            }
            Object nmsWorld = mGetHandle.invoke(w);
            Object chunkSource = mGetChunkSource.invoke(nmsWorld);
            // the generating ProtoChunk (LINKED CraftChunkData) carries the refs
            Object ref = weakChunkField.get(chunkData);
            if (!(ref instanceof WeakReference<?> wr)) {
                return EMPTY_BEARD;
            }
            Object proto = wr.get();
            if (proto == null) {
                return EMPTY_BEARD;
            }
            Object refsMap = mGetAllRefs.invoke(proto);
            if (!(refsMap instanceof java.util.Map<?, ?> map) || map.isEmpty()) {
                return EMPTY_BEARD;
            }
            Object chunkPos = ctorChunkPos.newInstance(chunkX, chunkZ);
            Object startsStatus = cChunkStatus.getField("STRUCTURE_STARTS").get(null);
            int[] buf = BEARD_BUF.get();
            int p = 2; // [0]=pieces header, [1]=junctions header
            int pieces = 0;
            int junctions = 0;
            for (java.util.Map.Entry<?, ?> e : map.entrySet()) {
                Object structure = e.getKey();
                Object adj = mTerrainAdapt.invoke(structure);
                if (adj == null || adj.toString().equals("NONE")) {
                    continue;
                }
                Object refsObj = e.getValue();
                if (!(refsObj instanceof Iterable<?> refIt)) {
                    continue;
                }
                for (Object refObj : refIt) {
                    long refPos = (Long) refObj;
                    int sx = (int) (refPos & 0xFFFFFFFFL);
                    int sz = (int) (refPos >> 32);
                    Object startChunk = mGetChunkAtIfLoaded.invoke(chunkSource, sx, sz);
                    // NEVER pass create=true to getChunk here (the old fallback
                    // did). Issuing chunk tickets from INSIDE the noise stage
                    // makes Paper generate dependency chunks out of forceload
                    // order, reshuffling the feature-decoration streams -> blob/
                    // kelp features land differently => ~5900 judge diffs vs a
                    // vanilla capture (measured: beard ON 6046, beard OFF 190).
                    // Per vanilla dependency semantics the 3x3 structure starts
                    // already exist at noise stage; a not-yet-loaded start
                    // (region border) is skipped below — strictly closer to
                    // vanilla than reordering the world.
                    if (startChunk == null) {
                        continue;
                    }
                    Object start = mGetStartForStructure.invoke(startChunk, structure);
                    if (start == null || !(boolean) mStartIsValid.invoke(start)) {
                        continue;
                    }
                    Object piecesList = mStartGetPieces.invoke(start);
                    if (!(piecesList instanceof java.util.List<?> pl)) {
                        continue;
                    }
                    for (Object piece : pl) {
                        if (!(boolean) mPieceIsClose.invoke(piece, chunkPos, 12)) {
                            continue;
                        }
                        Object box = mPieceGetBox.invoke(piece);
                        int groundDelta = 0;
                        Object junctionsList = null;
                        if (cPoolPiece.isInstance(piece)) {
                            Object element = mPoolGetElement.invoke(piece);
                            Object projection = mElementProjection.invoke(element);
                            if (projection == null || !projection.toString().equals("RIGID")) {
                                // non-rigid pool piece: only junctions contribute
                                junctionsList = mGetJunctions.invoke(piece);
                            } else {
                                groundDelta = (int) mPoolGetDelta.invoke(piece);
                            }
                        }
                        if (junctionsList == null && cPoolPiece.isInstance(piece)) {
                            junctionsList = mGetJunctions.invoke(piece);
                        }
                        // emit piece
                        if (p + 8 > buf.length) {
                            buf = java.util.Arrays.copyOf(buf, buf.length * 2);
                            BEARD_BUF.set(buf);
                        }
                        buf[p++] = (int) mBoxMinX.invoke(box);
                        buf[p++] = (int) mBoxMinY.invoke(box);
                        buf[p++] = (int) mBoxMinZ.invoke(box);
                        buf[p++] = (int) mBoxMaxX.invoke(box);
                        buf[p++] = (int) mBoxMaxY.invoke(box);
                        buf[p++] = (int) mBoxMaxZ.invoke(box);
                        buf[p++] = adjOrdinal(adj);
                        buf[p++] = groundDelta;
                        pieces++;
                        // junctions of this piece (vanilla: source within 12 of chunk)
                        if (junctionsList instanceof java.util.List<?> jl) {
                            int minBlockX = chunkX * 16;
                            int minBlockZ = chunkZ * 16;
                            for (Object j : jl) {
                                int jx = (int) mJGetSX.invoke(j);
                                int jz = (int) mJGetSZ.invoke(j);
                                if (jx > minBlockX - 12 && jz > minBlockZ - 12
                                        && jx < minBlockX + 15 + 12 && jz < minBlockZ + 15 + 12) {
                                    if (p + 3 > buf.length) {
                                        buf = java.util.Arrays.copyOf(buf, buf.length * 2);
                                        BEARD_BUF.set(buf);
                                    }
                                    buf[p++] = jx;
                                    buf[p++] = (int) mJGetGY.invoke(j);
                                    buf[p++] = jz;
                                    junctions++;
                                }
                            }
                        }
                    }
                }
            }
            if (pieces == 0 && junctions == 0) {
                return EMPTY_BEARD;
            }
            int[] out = new int[p];
            out[0] = pieces;
            out[1] = junctions;
            System.arraycopy(buf, 2, out, 2, p - 2);
            GenStats.BEARD_PIECES.addAndGet(pieces);
            GenStats.BEARD_CHUNKS.incrementAndGet();
            return out;
        } catch (Throwable t) {
            beardBroken = true;
            LOG.warning("beard collection failed, disabling: " + t);
            return EMPTY_BEARD;
        }
    }

    private static final int[] EMPTY_BEARD = new int[]{0, 0};

    private static int adjOrdinal(Object adj) {
        if (adj instanceof Enum<?> en) {
            return en.ordinal();
        }
        return 0;
    }

    // ------------------------------------------------------------------
    // NMS bridge: feed aquifer fluid-update marks into the ProtoChunk so
    // Paper's LevelChunk.postProcessGeneration runs the exact vanilla fluid
    // ticks (flowing water, obsidian where lava meets water, bubble columns).
    // Reflection only — no compile-time NMS dependency.
    // ------------------------------------------------------------------

    private static final Object BRIDGE_LOCK = new Object();
    private static volatile boolean bridgeReady;
    private static volatile boolean bridgeBroken;
    private static Field weakChunkField;
    private static Method markMethod;
    private static Constructor<?> blockPosCtor;
    private static final Logger LOG = Logger.getLogger("CrusstyGen");

    private static void initBridge(Object chunkData) {
        synchronized (BRIDGE_LOCK) {
            if (bridgeReady || bridgeBroken) {
                return;
            }
            try {
                weakChunkField = findField(chunkData.getClass(), "weakChunk");
                weakChunkField.setAccessible(true);
                Class<?> blockPos = Class.forName("net.minecraft.core.BlockPos");
                Class<?> protoChunk = Class.forName("net.minecraft.world.level.chunk.ProtoChunk");
                markMethod = protoChunk.getMethod("markPosForPostprocessing", blockPos);
                markMethod.setAccessible(true);
                blockPosCtor = blockPos.getConstructor(int.class, int.class, int.class);
                bridgeReady = true;
                LOG.info("NMS fluid-update marking bridge ready");
            } catch (Throwable t) {
                bridgeBroken = true;
                LOG.warning("NMS marking bridge unavailable (no fluid postprocessing): " + t);
            }
        }
    }

    private static Field findField(Class<?> c, String name) throws NoSuchFieldException {
        for (Class<?> k = c; k != null; k = k.getSuperclass()) {
            try {
                return k.getDeclaredField(name);
            } catch (NoSuchFieldException ignored) {
                // walk up
            }
        }
        throw new NoSuchFieldException(name);
    }

    /**
     * Marks the native-reported fluid update positions on the underlying
     * ProtoChunk (LINKED CraftChunkData). Silently no-ops when the data is
     * unlinked (OldCraftChunkData) or the bridge is unavailable.
     */
    private static void markFluidUpdates(ChunkData chunkData, long[] marks, int count) {
        if (bridgeBroken || count <= 0) {
            return;
        }
        if (!bridgeReady) {
            initBridge(chunkData);
            if (bridgeBroken || !bridgeReady) {
                return;
            }
        }
        try {
            Object ref = weakChunkField.get(chunkData);
            if (!(ref instanceof WeakReference<?> wr)) {
                GenStats.BRIDGE_NO_CHUNK.incrementAndGet();
                return; // unlinked path
            }
            Object chunk = wr.get();
            if (chunk == null) {
                GenStats.BRIDGE_NO_CHUNK.incrementAndGet();
                return;
            }
            GenStats.MARKS_SEEN.addAndGet(count);
            for (int i = 0; i < count; i++) {
                long p = marks[i];
                int x = (int) (p >> 38);
                int z = (int) (p << 26 >> 38);
                int y = (int) (p << 52 >> 52);
                Object bp = blockPosCtor.newInstance(x, y, z);
                markMethod.invoke(chunk, bp);
            }
            GenStats.MARKS_LANDED.addAndGet(count);
        } catch (Throwable t) {
            GenStats.BRIDGE_ERROR.incrementAndGet();
            bridgeBroken = true;
            LOG.warning("fluid-update marking failed, disabling bridge: " + t);
        }
    }

}
