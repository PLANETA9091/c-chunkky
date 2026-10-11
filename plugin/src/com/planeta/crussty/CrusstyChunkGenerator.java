package com.planeta.crussty;

import org.bukkit.Material;
import org.bukkit.generator.ChunkGenerator;
import org.bukkit.generator.WorldInfo;

import java.util.Map;
import java.util.Random;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.locks.ReentrantLock;

/**
 * Takes over ONLY the vanilla noise stage: one native call fills the whole
 * chunk (16*16*height u16 block ids), then we lay them into ChunkData.
 * Vanilla continues on top: surface rules, carvers, features, lighting —
 * preserving block-for-block parity with the vanilla reference dumps.
 */
public final class CrusstyChunkGenerator extends ChunkGenerator {

    /** world seed -> native ctx pointer. */
    private static final Map<Long, Long> CTX = new ConcurrentHashMap<>();
    /** serializes native fill per ctx (native scratch is ctx-bound for now). */
    private static final Map<Long, ReentrantLock> LOCKS = new ConcurrentHashMap<>();

    private static final int ARRAY_SIZE = 16 * 16 * 384; // 98304

    private final ThreadLocal<short[]> buf = ThreadLocal.withInitial(() -> new short[ARRAY_SIZE]);

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

    static long ctxFor(long seed, int minY, int height) {
        return CTX.computeIfAbsent(seed, s -> {
            long ctx = ChunkGenNative.newCtx(s, minY, height);
            if (ctx == 0) {
                throw new IllegalStateException("crussty_chunkgen ctx_new failed for seed " + s);
            }
            return ctx;
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
        long ctx = ctxFor(seed, minY, height);
        short[] out = buf.get();
        ReentrantLock lock = lockFor(ctx);
        lock.lock();
        try {
            ChunkGenNative.fill(ctx, chunkX, chunkZ, out);
        } finally {
            lock.unlock();
        }
        layout(chunkData, minY, height, out);
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
        for (Long ctx : CTX.values()) {
            ChunkGenNative.freeCtx(ctx);
        }
        CTX.clear();
        LOCKS.clear();
    }

}
