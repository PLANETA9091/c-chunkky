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

    /** world key: seed + resolved sea level (same seed, different datapacks -> different ctx). */
    private record WorldKey(long seed, int seaLevel) {
    }

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

        CtxPool(int n, long seed, int minY, int height, int sea) {
            this.ctxs = new long[n];
            this.locks = new ReentrantLock[n];
            for (int i = 0; i < n; i++) {
                long ctx = ChunkGenNative.newCtx(seed, minY, height, sea);
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
        WorldKey key = new WorldKey(seed, sea);
        return POOLS.computeIfAbsent(key, k -> new CtxPool(POOL_SIZE, seed, minY, height, sea));
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
        if (BEARD_ENABLED) {
            beard = collectBeard(worldInfo, chunkX, chunkZ, chunkData);
        }
        int nMarks;
        try {
            nMarks = ChunkGenNative.fillBeard(ctx, chunkX, chunkZ, out, marks, beard);
        } finally {
            lockBox[0].unlock();
        }
        layout(chunkData, minY, height, out);
        markFluidUpdates(chunkData, marks, nMarks);
        GenStats.record(System.nanoTime() - t0);
    }

    /** Straight per-block layout (v1); native-direct palette path is a later lever. */
    private static void layout(ChunkData chunkData, int minY, int height, short[] out) {
        Material[] mats = GenStats.MATERIALS;
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
                    if (startChunk == null) {
                        startChunk = mGetChunkFull.invoke(chunkSource, sx, sz, startsStatus, true);
                    }
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
