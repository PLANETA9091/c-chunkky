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

    private static final Map<WorldKey, Long> CTX2 = new ConcurrentHashMap<>();
    private static final Map<String, Integer> SEA_CACHE = new ConcurrentHashMap<>();

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

    static long ctxFor(WorldInfo worldInfo, long seed, int minY, int height) {
        int sea = resolveSeaLevel(worldInfo);
        WorldKey key = new WorldKey(seed, sea);
        return CTX2.computeIfAbsent(key, k -> {
            long ctx = ChunkGenNative.newCtx(seed, minY, height, sea);
            if (ctx == 0) {
                throw new IllegalStateException(
                        "crussty_chunkgen ctx_new failed for seed " + seed + " sea " + sea);
            }
            synchronized (CTX) {
                CTX.add(ctx);
            }
            return ctx;
        });
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

    static ReentrantLock lockFor(long ctx) {
        return LOCKS.computeIfAbsent(ctx, k -> new ReentrantLock());
    }

    @Override
    public void generateNoise(WorldInfo worldInfo, Random random, int chunkX, int chunkZ, ChunkData chunkData) {
        long t0 = System.nanoTime();
        long seed = worldInfo.getSeed();
        int minY = worldInfo.getMinHeight();
        int height = worldInfo.getMaxHeight() - worldInfo.getMinHeight();
        long ctx = ctxFor(worldInfo, seed, minY, height);
        short[] out = buf.get();
        long[] marks = MARK_BUF.get();
        int nMarks;
        ReentrantLock lock = lockFor(ctx);
        lock.lock();
        try {
            nMarks = ChunkGenNative.fillMarked(ctx, chunkX, chunkZ, out, marks);
        } finally {
            lock.unlock();
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
        for (Long ctx : CTX) {
            ChunkGenNative.freeCtx(ctx);
        }
        CTX.clear();
        LOCKS.clear();
        SEA_CACHE.clear();
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
                return; // unlinked path
            }
            Object chunk = wr.get();
            if (chunk == null) {
                return;
            }
            for (int i = 0; i < count; i++) {
                long p = marks[i];
                int x = (int) (p >> 38);
                int z = (int) (p << 26 >> 38);
                int y = (int) (p << 52 >> 52);
                Object bp = blockPosCtor.newInstance(x, y, z);
                markMethod.invoke(chunk, bp);
            }
        } catch (Throwable t) {
            bridgeBroken = true;
            LOG.warning("fluid-update marking failed, disabling bridge: " + t);
        }
    }

}
