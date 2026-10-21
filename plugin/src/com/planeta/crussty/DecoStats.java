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
    // wouldSurvive histogram (CRUS_SURVIVE_HIST=1): state -> {count, trueCount}
    static final java.util.concurrent.ConcurrentHashMap<String, long[]> SURVIVE_HIST
            = new java.util.concurrent.ConcurrentHashMap<>();
    // below-block name -> count (the environment the rules must classify)
    static final java.util.concurrent.ConcurrentHashMap<String, long[]> SURVIVE_BELOW_HIST
            = new java.util.concurrent.ConcurrentHashMap<>();

    static void surviveSample(String state, boolean ok, String below) {
        SURVIVE_HIST.computeIfAbsent(state, k -> new long[2])[0]++;
        if (ok) {
            SURVIVE_HIST.get(state)[1]++;
        }
        SURVIVE_BELOW_HIST.computeIfAbsent(below, k -> new long[2])[0]++;
    }
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
    /** plan-phase split: the regionBiomes Java wall (palette walk) */
    static final AtomicLong PLAN_BIOMES = new AtomicLong();

    // upcall counters (Java side; they measure the JNI traffic, not the time)
    static final AtomicLong UP_HEIGHT = new AtomicLong();
    /** height upcalls by ty (WG=0/2, FINAL=1/3/4/5) — the native-split decision data */
    static final AtomicLong[] UP_H_TY = new AtomicLong[] {
            new AtomicLong(), new AtomicLong(), new AtomicLong(),
            new AtomicLong(), new AtomicLong(), new AtomicLong()
    };
    /** region-snapshot section loads (decoSnapSection JNI) + nanos + HM handshakes */
    static final AtomicLong SNAP_SECTIONS = new AtomicLong();
    static final AtomicLong SNAP_SECTION_NS = new AtomicLong();
    static final AtomicLong SNAP_HM = new AtomicLong();
    /** individual sections built across snap calls (batch = up to 3 each) */
    static final AtomicLong SNAP_SECTION_BLOCKS = new AtomicLong();
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
        PLAN_BIOMES.set(0);
        UP_HEIGHT.set(0);
        for (AtomicLong a : UP_H_TY) {
            a.set(0);
        }
        SNAP_SECTIONS.set(0);
        SNAP_SECTION_NS.set(0);
        SNAP_HM.set(0);
        SNAP_SECTION_BLOCKS.set(0);
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
                UP_IS_AIR.get(), UP_CAN_SURVIVE.get(), UP_OTHER.get())
                .concat(String.format(" snap-sec=%d/%.2fms blocks=%d snap-hm=%d planbio=%.2fms",
                SNAP_SECTIONS.get(), SNAP_SECTION_NS.get() / 1e6,
                SNAP_SECTION_BLOCKS.get(), SNAP_HM.get(),
                PLAN_BIOMES.get() / 1e6));
    }

    /** wouldSurvive histogram tail (top 12 each), CRUS_SURVIVE_HIST=1. */
    static String surviveHist() {
        if (SURVIVE_HIST.isEmpty()) {
            return "";
        }
        StringBuilder sb = new StringBuilder(" | [survive-hist]");
        SURVIVE_HIST.entrySet().stream()
                .sorted((a, b) -> Long.compare(b.getValue()[0], a.getValue()[0]))
                .limit(12)
                .forEach(e -> sb.append(' ').append(e.getKey()).append('=')
                        .append(e.getValue()[0]).append('/').append(e.getValue()[1]));
        sb.append(" | [survive-below]");
        SURVIVE_BELOW_HIST.entrySet().stream()
                .sorted((a, b) -> Long.compare(b.getValue()[0], a.getValue()[0]))
                .limit(16)
                .forEach(e -> sb.append(' ').append(e.getKey()).append('=').append(e.getValue()[0]));
        return sb.toString();
    }

    /** height ty histogram tail. */
    static String hTyHist() {
        long total = 0;
        for (AtomicLong a : UP_H_TY) {
            total += a.get();
        }
        if (total == 0) {
            return "";
        }
        StringBuilder sb = new StringBuilder(" | [h-ty]");
        for (int i = 0; i < UP_H_TY.length; i++) {
            sb.append(i == 0 ? "" : " ").append(i).append('=').append(UP_H_TY[i].get());
        }
        return sb.toString();
    }
}
