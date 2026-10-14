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
    private static final long[] NATIVE_NANOS = new long[RING];
    private static final AtomicLong COUNT = new AtomicLong();
    private static final AtomicLong TOTAL = new AtomicLong();
    // phase breakdown of the measured window (native fill / java layout /
    // beard bridge / fluid-update marks)
    static final AtomicLong NATIVE_TOTAL = new AtomicLong();
    static final AtomicLong LAYOUT_TOTAL = new AtomicLong();
    static final AtomicLong BEARD_TOTAL = new AtomicLong();
    static final AtomicLong MARKS_TOTAL = new AtomicLong();
    // bulk section layout diagnostics
    static final AtomicLong BULK_FILLED = new AtomicLong();
    static final AtomicLong BULK_MIXED = new AtomicLong();
    static final AtomicLong BULK_AIR = new AtomicLong();
    static final AtomicLong BULK_BROKEN_COUNT = new AtomicLong();
    // fluid-update mark diagnostics
    static final AtomicLong MARKS_SEEN = new AtomicLong();
    static final AtomicLong MARKS_LANDED = new AtomicLong();
    // layout phase breakdown (setup reflection / section writes / heightmaps / self-verify)
    static final AtomicLong LAY_SETUP_TOTAL = new AtomicLong();
    static final AtomicLong LAY_SEC_TOTAL = new AtomicLong();
    static final AtomicLong LAY_HMAP_TOTAL = new AtomicLong();
    static final AtomicLong LAY_VER_TOTAL = new AtomicLong();
    // per-path section layout: 0 unused, 1 singleton, 2 linear, 3 global, 4 per-block
    static final AtomicLong[] LAY_PATH_NS = {
            new AtomicLong(), new AtomicLong(), new AtomicLong(),
            new AtomicLong(), new AtomicLong()};
    static final AtomicLong[] LAY_PATH_COUNT = {
            new AtomicLong(), new AtomicLong(), new AtomicLong(),
            new AtomicLong(), new AtomicLong()};
    static final AtomicLong BRIDGE_NO_CHUNK = new AtomicLong();
    static final AtomicLong BRIDGE_ERROR = new AtomicLong();
    // beardifier (structure terrain adaptation) diagnostics
    static final AtomicLong BEARD_PIECES = new AtomicLong();
    static final AtomicLong BEARD_CHUNKS = new AtomicLong();

    private GenStats() {
    }

    /** Zero the ring buffer + all counters: starts a fresh measurement window
     *  (used after the JIT-warmup pregen pass so the reported window is
     *  steady-state — the shared CI runner makes cold windows unreproducible). */
    static void reset() {
        COUNT.set(0);
        TOTAL.set(0);
        NATIVE_TOTAL.set(0);
        LAYOUT_TOTAL.set(0);
        BEARD_TOTAL.set(0);
        MARKS_TOTAL.set(0);
        BULK_FILLED.set(0);
        BULK_MIXED.set(0);
        BULK_AIR.set(0);
        BULK_BROKEN_COUNT.set(0);
        MARKS_SEEN.set(0);
        MARKS_LANDED.set(0);
        LAY_SETUP_TOTAL.set(0);
        LAY_SEC_TOTAL.set(0);
        LAY_HMAP_TOTAL.set(0);
        LAY_VER_TOTAL.set(0);
        for (AtomicLong a : LAY_PATH_NS) {
            a.set(0);
        }
        for (AtomicLong a : LAY_PATH_COUNT) {
            a.set(0);
        }
        BRIDGE_NO_CHUNK.set(0);
        BRIDGE_ERROR.set(0);
        BEARD_CHUNKS.set(0);
        BEARD_PIECES.set(0);
        java.util.Arrays.fill(NANOS, 0L);
        java.util.Arrays.fill(NATIVE_NANOS, 0L);
    }

    static void record(long nanos, long nativeNs, long layoutNs, long beardNs, long marksNs) {
        long i = COUNT.getAndIncrement();
        NANOS[(int) (i % RING)] = nanos;
        NATIVE_NANOS[(int) (i % RING)] = nativeNs;
        TOTAL.addAndGet(nanos);
        NATIVE_TOTAL.addAndGet(nativeNs);
        LAYOUT_TOTAL.addAndGet(layoutNs);
        BEARD_TOTAL.addAndGet(beardNs);
        MARKS_TOTAL.addAndGet(marksNs);
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
        long pmin = copy[0]; // noise-free estimate: additive noise (GC, desched,
                             // runner neighbors) only ever inflates a chunk time
        long[] ncopy = new long[(int) n];
        System.arraycopy(NATIVE_NANOS, 0, ncopy, 0, (int) n);
        java.util.Arrays.sort(ncopy);
        long natP50 = ncopy[(int) n / 2];
        double meanUs = (TOTAL.get() / Math.max(1, COUNT.get())) / 1000.0;
        long cnt = Math.max(1, COUNT.get());
        return String.format(
                "chunks=%d  mean=%.1fus  min=%.1fus  p50=%.1fus  p99=%.1fus  max=%.1fus  [native=%.2fms nat_p50=%.2fms layout=%.2fms beard=%.2fms marks=%.2fms]  [lay setup=%.2f sec=%.2f hmap=%.2f verify=%.2f | uni=%.2f/%.1f lin=%.2f/%.1f glob=%.2f/%.1f pb=%.2f/%.2f]  [sections filled=%d mixed=%d air=%d v1fallback=%d bulk=%s]  marks_seen=%d marks_landed=%d bridge_no_chunk=%d bridge_err=%d beard_chunks=%d beard_pieces=%d",
                COUNT.get(), meanUs,
                pmin / 1000.0, p50 / 1000.0, p99 / 1000.0, copy[(int) (n - 1)] / 1000.0,
                NATIVE_TOTAL.get() / (double) cnt / 1e6,
                natP50 / 1e6,
                LAYOUT_TOTAL.get() / (double) cnt / 1e6,
                BEARD_TOTAL.get() / (double) cnt / 1e6,
                MARKS_TOTAL.get() / (double) cnt / 1e6,
                LAY_SETUP_TOTAL.get() / (double) cnt / 1e6,
                LAY_SEC_TOTAL.get() / (double) cnt / 1e6,
                LAY_HMAP_TOTAL.get() / (double) cnt / 1e6,
                LAY_VER_TOTAL.get() / (double) cnt / 1e6,
                LAY_PATH_NS[1].get() / (double) cnt / 1e6,
                LAY_PATH_NS[2].get() / (double) cnt / 1e6,
                LAY_PATH_NS[3].get() / (double) cnt / 1e6,
                LAY_PATH_NS[4].get() / (double) cnt / 1e6,
                LAY_PATH_COUNT[1].get() / (double) cnt,
                LAY_PATH_COUNT[2].get() / (double) cnt,
                LAY_PATH_COUNT[3].get() / (double) cnt,
                LAY_PATH_COUNT[4].get() / (double) cnt,
                BULK_FILLED.get(), BULK_MIXED.get(), BULK_AIR.get(), BULK_BROKEN_COUNT.get(),
                com.planeta.crussty.CrusstyChunkGenerator.bulkFailReason == null ? "ok" : com.planeta.crussty.CrusstyChunkGenerator.bulkFailReason,
                MARKS_SEEN.get(), MARKS_LANDED.get(), BRIDGE_NO_CHUNK.get(), BRIDGE_ERROR.get(),
                BEARD_CHUNKS.get(), BEARD_PIECES.get());
    }

    static long count() {
        return COUNT.get();
    }
}
