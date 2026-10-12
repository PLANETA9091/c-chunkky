package com.planeta.crussty;

import org.bukkit.Material;

import java.util.concurrent.atomic.AtomicLong;

/**
 * Per-chunk generation stats for the native noise stage (generateNoise only).
 * Ring buffer of the last N stage durations + material lookup table shared by
 * the generator. Thread-safe: generateNoise may be called from Paper worker
 * threads concurrently.
 */
public final class GenStats {

    /** native block id -> Material (block_registry.txt order, ids 0..21). */
    static final Material[] MATERIALS = {
            Material.AIR,            // 0
            Material.STONE,          // 1
            Material.BEDROCK,        // 2
            Material.WATER,          // 3
            Material.GRASS_BLOCK,    // 4
            Material.DIRT,           // 5
            Material.DEEPSLATE,      // 6
            Material.SAND,           // 7
            Material.GRAVEL,         // 8
            Material.GRANITE,        // 9
            Material.DIORITE,        // 10
            Material.ANDESITE,       // 11
            Material.TUFF,           // 12
            Material.SANDSTONE,      // 13
            Material.CLAY,           // 14
            Material.POWDER_SNOW,    // 15
            Material.ICE,            // 16
            Material.LAVA,           // 17
            Material.COPPER_ORE,     // 18
            Material.RAW_COPPER_BLOCK, // 19
            Material.DEEPSLATE_IRON_ORE, // 20
            Material.RAW_IRON_BLOCK, // 21
    };

    private static final int RING = 4096;
    private static final long[] NANOS = new long[RING];
    private static final AtomicLong COUNT = new AtomicLong();
    private static final AtomicLong TOTAL = new AtomicLong();
    // fluid-update mark diagnostics
    static final AtomicLong MARKS_SEEN = new AtomicLong();
    static final AtomicLong MARKS_LANDED = new AtomicLong();
    static final AtomicLong BRIDGE_NO_CHUNK = new AtomicLong();
    static final AtomicLong BRIDGE_ERROR = new AtomicLong();
    // beardifier (structure terrain adaptation) diagnostics
    static final AtomicLong BEARD_PIECES = new AtomicLong();
    static final AtomicLong BEARD_CHUNKS = new AtomicLong();

    private GenStats() {
    }

    static void record(long nanos) {
        long i = COUNT.getAndIncrement();
        NANOS[(int) (i % RING)] = nanos;
        TOTAL.addAndGet(nanos);
    }

    /** Human-readable stats over the ring buffer. */
    static String snapshot() {
        long n = Math.min(COUNT.get(), RING);
        if (n == 0) {
            return "no chunks generated yet";
        }
        long[] copy = new long[(int) n];
        System.arraycopy(NANOS, 0, copy, 0, (int) n);
        java.util.Arrays.sort(copy);
        long p50 = copy[(int) (n / 2)];
        long p99 = copy[(int) Math.min(n - 1, (long) (n * 0.99))];
        double meanUs = (TOTAL.get() / Math.max(1, COUNT.get())) / 1000.0;
        return String.format(
                "chunks=%d  mean=%.1fus  p50=%.1fus  p99=%.1fus  max=%.1fus  marks_seen=%d marks_landed=%d bridge_no_chunk=%d bridge_err=%d beard_chunks=%d beard_pieces=%d",
                COUNT.get(), meanUs,
                p50 / 1000.0, p99 / 1000.0, copy[(int) (n - 1)] / 1000.0,
                MARKS_SEEN.get(), MARKS_LANDED.get(), BRIDGE_NO_CHUNK.get(), BRIDGE_ERROR.get(),
                BEARD_CHUNKS.get(), BEARD_PIECES.get());
    }

    static long count() {
        return COUNT.get();
    }
}
