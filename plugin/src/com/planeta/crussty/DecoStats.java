package com.planeta.crussty;

import java.util.concurrent.atomic.AtomicLong;

/**
 * Per-chunk decoration stats for the native deco driver (DecoDriver.populate).
 * Mirrors GenStats: ring buffer of the last N chunk wall durations + phase
 * breakdown counters. Thread-safe — populate runs on Paper worker threads.
 *
 * Phase split of one chunk wall:
 *   plan   — regionBiomes (3x3 union) + decoChunkPlan JNI
 *   struct — runStructures (vanilla placeInChunk over the region)
 *   run    — decoRunFeature JNI wall (native expansion + ALL level upcalls)
 *   apply  — the Java op-apply loop (applyOp -> reg.setBlock)
 *   upcalls counted per method (they live INSIDE the run phase).
 * CRUS_DECO_NOUPCALL=1 dummies the upcalls (native-only floor measurement —
 * deco output is garbage, timings are honest).
 */
public final class DecoStats {

    private static final int RING = 4096;
    private static final long[] NANOS = new long[RING];
    private static final AtomicLong COUNT = new AtomicLong();
    private static final AtomicLong TOTAL = new AtomicLong();

    static final AtomicLong PLAN_TOTAL = new AtomicLong();
    static final AtomicLong STRUCT_TOTAL = new AtomicLong();
    static final AtomicLong RUN_TOTAL = new AtomicLong();
    static final AtomicLong APPLY_TOTAL = new AtomicLong();
    static final AtomicLong OPS_TOTAL = new AtomicLong();
    static final AtomicLong FEATURES_TOTAL = new AtomicLong();
    static final AtomicLong DELEGATE_TOTAL = new AtomicLong();
    static final AtomicLong DELEGATE_COUNT = new AtomicLong();

    // upcall counters (Java side; they measure the JNI traffic, not the time)
    static final AtomicLong UP_HEIGHT = new AtomicLong();
    static final AtomicLong UP_STATE_IS = new AtomicLong();
    static final AtomicLong UP_FLUID_IS = new AtomicLong();
    static final AtomicLong UP_IS_AIR = new AtomicLong();
    static final AtomicLong UP_CAN_SURVIVE = new AtomicLong();
    static final AtomicLong UP_OTHER = new AtomicLong();
    static final AtomicLong BIOMECHK_LOG = new AtomicLong();

    private DecoStats() {
    }

    static void reset() {
        COUNT.set(0);
        TOTAL.set(0);
        PLAN_TOTAL.set(0);
        STRUCT_TOTAL.set(0);
        RUN_TOTAL.set(0);
        APPLY_TOTAL.set(0);
        OPS_TOTAL.set(0);
        FEATURES_TOTAL.set(0);
        DELEGATE_TOTAL.set(0);
        DELEGATE_COUNT.set(0);
        UP_HEIGHT.set(0);
        UP_STATE_IS.set(0);
        UP_FLUID_IS.set(0);
        UP_IS_AIR.set(0);
        UP_CAN_SURVIVE.set(0);
        UP_OTHER.set(0);
        BIOMECHK_LOG.set(0);
        java.util.Arrays.fill(NANOS, 0L);
    }

    static void recordChunk(long nanos) {
        long i = COUNT.getAndIncrement();
        NANOS[(int) (i % RING)] = nanos;
        TOTAL.addAndGet(nanos);
    }

    /** Human-readable stats over the ring buffer (crusgenstats tail). */
    static String snapshot() {
        long n = Math.min(COUNT.get(), RING);
        if (n == 0) {
            return "deco: no chunks decorated yet";
        }
        long[] copy = new long[(int) n];
        System.arraycopy(NANOS, 0, copy, 0, (int) n);
        java.util.Arrays.sort(copy);
        long pmin = copy[0];
        long p50 = copy[(int) (n / 2)];
        long p99 = copy[(int) Math.min(n - 1, (long) (n * 0.99))];
        long cnt = Math.max(1, COUNT.get());
        long upcalls = UP_HEIGHT.get() + UP_STATE_IS.get() + UP_FLUID_IS.get()
                + UP_IS_AIR.get() + UP_CAN_SURVIVE.get() + UP_OTHER.get();
        return String.format(
                "native deco stage: chunks=%d  mean=%.1fus  min=%.1fus  p50=%.1fus  p99=%.1fus  max=%.1fus"
                        + "  [plan=%.2fms struct=%.2fms run=%.2fms apply=%.2fms]"
                        + "  ops=%d features=%d delegate=%d"
                        + "  upcalls=%d (h=%d stateis=%d fluidis=%d isair=%d survive=%d other=%d)",
                COUNT.get(),
                (TOTAL.get() / (double) cnt) / 1000.0,
                pmin / 1000.0, p50 / 1000.0, p99 / 1000.0,
                copy[(int) (n - 1)] / 1000.0,
                PLAN_TOTAL.get() / (double) cnt / 1e6,
                STRUCT_TOTAL.get() / (double) cnt / 1e6,
                RUN_TOTAL.get() / (double) cnt / 1e6,
                APPLY_TOTAL.get() / (double) cnt / 1e6,
                OPS_TOTAL.get(), FEATURES_TOTAL.get(),
                DELEGATE_COUNT.get(),
                upcalls, UP_HEIGHT.get(), UP_STATE_IS.get(), UP_FLUID_IS.get(),
                UP_IS_AIR.get(), UP_CAN_SURVIVE.get(), UP_OTHER.get());
    }
}
