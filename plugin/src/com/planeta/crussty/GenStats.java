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
    // native biome bulk fill (stage B-263): JNI fillBiomes wall time + count
    static final java.util.concurrent.atomic.AtomicLong BIOME_TOTAL = new java.util.concurrent.atomic.AtomicLong();
    static final java.util.concurrent.atomic.AtomicLong BIOME_N = new java.util.concurrent.atomic.AtomicLong();
    // thread CPU time (ThreadMXBean) — contention-immune; the honest metric
    // on a throttled sandbox where wall time stretches 6x under load.
    static final java.util.concurrent.atomic.AtomicLong BIOME_CPU = new java.util.concurrent.atomic.AtomicLong();
    private static volatile java.lang.management.ThreadMXBean THREAD_MX;

    static long threadCpuNano() {
        java.lang.management.ThreadMXBean mx = THREAD_MX;
        if (mx == null) {
            mx = java.lang.management.ManagementFactory.getThreadMXBean();
            THREAD_MX = mx;
        }
        return mx.isCurrentThreadCpuTimeSupported() ? mx.getCurrentThreadCpuTime() : 0L;
    }
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

    // ---- region-bounded honest window -------------------------------------
    // Counts ONLY chunks inside the timed rectangle — immune to spawn-backlog
    // pollution (observed: fills=314 for a 144-chunk window, 2026-10-28: the
    // spawn 13x13 throttles under forceload tickets and resumes INSIDE the
    // timed window; gamerule + drain cannot cancel pending tasks).
    // The span is server-side (first in-window fill start .. last in-window
    // fill end): no rcon latency, no 0.5s poll granularity.
    private static volatile int WIN_MIN_X, WIN_MIN_Z, WIN_N;
    private static final AtomicLong WIN_INSIDE = new AtomicLong();
    private static final AtomicLong WIN_TOTAL = new AtomicLong();
    // cont-142 margin-law probe: fills OUTSIDE the armed window are the
    // dependency-margin work (14x14 TERRAIN / 16x16 BIOMES / 32x32
    // STRUCTURE_STARTS drag) that consumes the same workers while the window
    // shows as "idle" wall — quantify fills + fill-CPU outside vs inside.
    private static final AtomicLong WIN_OUTSIDE = new AtomicLong();
    private static final AtomicLong WIN_OUTSIDE_NANOS = new AtomicLong();
    private static final AtomicLong WIN_NATIVE = new AtomicLong();
    private static final AtomicLong WIN_LAYOUT = new AtomicLong();
    private static final AtomicLong WIN_BEARD = new AtomicLong();
    private static final AtomicLong WIN_MARKS = new AtomicLong();
    private static final AtomicLong WIN_CARVER = new AtomicLong();
    private static volatile long WIN_T_FIRST, WIN_T_LAST;
    // cont-92 decorate-stage counters: the fill-span honest e2e closes at the
    // last in-window noise fill, but Paper's decorate phase (structures /
    // features / populators) for the later chunks extends BEYOND that span —
    // every window so far printed "deco: no chunks decorated yet". Bukkit
    // populators fire per chunk in BOTH deco modes (they live outside the
    // shouldGenerateDecorations gate), so the always-on DecoMarker populator
    // feeds one timestamp per chunk here; windowSnapshot() turns it into
    // deco_inside / deco_tail (last fill end -> last deco event, the tail the
    // fill-e2e misses) / e2e_deco (first fill start .. last deco event).
    private static final AtomicLong WIN_DECO_INSIDE = new AtomicLong();
    private static volatile long WIN_DECO_FIRST, WIN_DECO_LAST;
    // nanoTime when "crusgenstats window" armed the rect — precedes the
    // forceload command by one rcon round-trip; ramp = first in-window fill
    // start minus this = the scheduling/status-chain latency BEFORE the
    // first fill (satellites, ticket propagation, main-thread beat).
    private static volatile long WIN_T_ARMED;

    // pipeline-coverage probe ring: per-fill [start..end] intervals inside the
    // window, unioned by windowSnapshot() into the worker-busy fraction of the
    // span (cont-82: JFR showed Worker #0 busy 36.6% of the par-lane window —
    // this measures the same thing server-side, per lane, every run).
    private static final int WRING = 8192; // 48x48=2304 + big N=47=2209 fit; truncation past this would bias cover low on wide windows
    /** In-window fill START nanoTime (chronological slot = W_N order). */
    private static final long[] W_START = new long[WRING];
    /** In-window fill wall ns, same slot as W_START. */
    private static final long[] W_WALL = new long[WRING];
    private static final java.util.concurrent.atomic.AtomicLong W_N =
            new java.util.concurrent.atomic.AtomicLong();

    /** Arm the window: only chunks with x in [rx..rx+n], z in [rz..rz+n] are
     *  counted; zeroes the window counters. Global stats are untouched. */
    static void window(int rx, int rz, int n) {
        WIN_MIN_X = rx;
        WIN_MIN_Z = rz;
        WIN_N = n;
        WIN_INSIDE.set(0);
        WIN_TOTAL.set(0);
        WIN_OUTSIDE.set(0);
        WIN_OUTSIDE_NANOS.set(0);
        WIN_NATIVE.set(0);
        WIN_LAYOUT.set(0);
        WIN_BEARD.set(0);
        WIN_MARKS.set(0);
        WIN_CARVER.set(0);
        W_N.set(0);
        WIN_T_FIRST = 0;
        WIN_T_LAST = 0;
        WIN_T_ARMED = System.nanoTime();
        WIN_DECO_INSIDE.set(0);
        WIN_DECO_FIRST = 0;
        WIN_DECO_LAST = 0;
    }

    /** In-window fill count so far (fast poll for the harness). */
    static long winInside() {
        return WIN_INSIDE.get();
    }

    /** cont-92 decorate-phase marker: called once per chunk from the
     *  always-on DecoMarker populator (vanilla and native deco alike) —
     *  the only per-chunk plugin-visible decoration event. */
    static void noteDeco(int cx, int cz) {
        if (WIN_N == 0) {
            return;
        }
        if (cx < WIN_MIN_X || cx > WIN_MIN_X + WIN_N
                || cz < WIN_MIN_Z || cz > WIN_MIN_Z + WIN_N) {
            return;
        }
        long t = System.nanoTime();
        if (WIN_DECO_FIRST == 0) {
            WIN_DECO_FIRST = t;
        }
        WIN_DECO_LAST = t;
        WIN_DECO_INSIDE.incrementAndGet();
    }

    /** In-window decorated-chunk count so far (harness deco-drain poll). */
    static long winDecoInside() {
        return WIN_DECO_INSIDE.get();
    }

    /** Server-side window snapshot: span + per-chunk component breakdown
     *  restricted to EXACTLY the timed rectangle. */
    static String windowSnapshot() {
        if (WIN_N == 0) {
            return "window: never armed";
        }
        long n = WIN_INSIDE.get();
        if (n == 0) {
            return String.format("window[%d..%d,%d..%d] inside=0", WIN_MIN_X,
                    WIN_MIN_X + WIN_N, WIN_MIN_Z, WIN_MIN_Z + WIN_N);
        }
        double spanMs = (WIN_T_LAST - WIN_T_FIRST) / 1e6;
        double rampMs = (WIN_T_FIRST > 0 && WIN_T_ARMED > 0)
                ? (WIN_T_FIRST - WIN_T_ARMED) / 1e6 : 0.0;
        // pipeline coverage: union of per-fill [start..end] over the span = the
        // wall fraction where at least one fill runs; maxconc = peak parallel
        // fills. coverage << 100% = the span is scheduling latency — Paper
        // status-chain handoffs, not fill CPU — the lever is the pipeline,
        // not the generator internals.
        double busyMs = 0.0;
        long maxconc = 0;
        long wn = Math.min(W_N.get(), WRING);
        if (wn > 0) {
            // sweep line over [start..end] events; busy accumulates on
            // every level 1->0 transition (the union segment), peak = maxconc
            long[][] ev = new long[(int) wn * 2][2];
            for (int k = 0; k < wn; k++) {
                ev[2 * k][0] = W_START[k];
                ev[2 * k][1] = 1;
                ev[2 * k + 1][0] = W_START[k] + W_WALL[k];
                ev[2 * k + 1][1] = -1;
            }
            java.util.Arrays.sort(ev, (a, b) -> a[0] != b[0]
                    ? Long.compare(a[0], b[0]) : Long.compare(b[1], a[1]));
            long level = 0, segStart = 0, peak = 0;
            for (long[] e : ev) {
                if (level == 1 && e[1] == -1) {
                    busyMs += (e[0] - segStart) / 1e6;
                } else if (level == 0 && e[1] == 1) {
                    segStart = e[0];
                }
                level += e[1];
                peak = Math.max(peak, level);
            }
            maxconc = peak;
        }
        double cover = spanMs > 0 ? 100.0 * busyMs / spanMs : 0.0;
        double cnt = n;
        // cont-92 decorate tail: deco_tail = last in-window fill end -> last
        // in-window deco event (the async decorate/status-chain wall the fill
        // span misses); e2e_deco = first fill start .. last deco event over
        // the fill count = the full-pipeline honest number through decorate.
        double decoTailMs = (WIN_DECO_LAST > 0 && WIN_T_LAST > 0)
                ? (WIN_DECO_LAST - WIN_T_LAST) / 1e6 : 0.0;
        double e2eDeco = (WIN_DECO_LAST > 0 && WIN_T_FIRST > 0)
                ? (WIN_DECO_LAST - WIN_T_FIRST) / 1e6 / n : 0.0;
        return String.format(
                "window[%d..%d,%d..%d] inside=%d span=%.2fms ramp=%.0fms e2e=%.2fms/chunk"
                        + " gen=%.2f native=%.2f layout=%.2f beard=%.2f marks=%.2f carver=%.2f"
                        + " busy=%.1fms/chunk cover=%.1f%% maxconc=%d"
                        + " margin_fills=%d margin_fill_cpu=%.1fms/ch"
                        + " deco_inside=%d deco_tail=%.0fms e2e_deco=%.2f",
                WIN_MIN_X, WIN_MIN_X + WIN_N, WIN_MIN_Z, WIN_MIN_Z + WIN_N,
                n, spanMs, rampMs, spanMs / n,
                WIN_TOTAL.get() / cnt / 1e6,
                WIN_NATIVE.get() / cnt / 1e6,
                WIN_LAYOUT.get() / cnt / 1e6,
                WIN_BEARD.get() / cnt / 1e6,
                WIN_MARKS.get() / cnt / 1e6,
                WIN_CARVER.get() / cnt / 1e6,
                wn > 0 ? busyMs / wn : 0.0,
                cover,
                maxconc,
                WIN_OUTSIDE.get(),
                WIN_OUTSIDE_NANOS.get() / n / 1e6,
                WIN_DECO_INSIDE.get(),
                decoTailMs,
                e2eDeco);
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

    /** Carver stage (CarverDriver.populate): per-chunk wall + window slice.
     *  Population runs OUTSIDE generateNoise (POPULATE status) — without this
     *  counter the carver cost is invisible inside the honest window's
     *  span−gen residual. (cx, cz) attributes to the region rectangle. */
    static void recordCarver(long nanos, int cx, int cz) {
        if (WIN_N > 0 && cx >= WIN_MIN_X && cx <= WIN_MIN_X + WIN_N
                && cz >= WIN_MIN_Z && cz <= WIN_MIN_Z + WIN_N) {
            WIN_CARVER.addAndGet(nanos);
        }
    }

    static void record(long nanos, long nativeNs, long layoutNs, long beardNs,
                       long marksNs, int cx, int cz) {
        long i = COUNT.getAndIncrement();
        NANOS[(int) (i % RING)] = nanos;
        NATIVE_NANOS[(int) (i % RING)] = nativeNs;
        TOTAL.addAndGet(nanos);
        NATIVE_TOTAL.addAndGet(nativeNs);
        LAYOUT_TOTAL.addAndGet(layoutNs);
        BEARD_TOTAL.addAndGet(beardNs);
        MARKS_TOTAL.addAndGet(marksNs);
        if (WIN_N > 0 && cx >= WIN_MIN_X && cx <= WIN_MIN_X + WIN_N
                && cz >= WIN_MIN_Z && cz <= WIN_MIN_Z + WIN_N) {
            long tEnd = System.nanoTime();
            if (WIN_T_FIRST == 0) {
                WIN_T_FIRST = tEnd - nanos; // start of the first in-window fill
            }
            WIN_T_LAST = tEnd;
            WIN_INSIDE.incrementAndGet();
            WIN_TOTAL.addAndGet(nanos);
            WIN_NATIVE.addAndGet(nativeNs);
            WIN_LAYOUT.addAndGet(layoutNs);
            WIN_BEARD.addAndGet(beardNs);
            WIN_MARKS.addAndGet(marksNs);
            // pipeline-coverage probe: per-fill [start..end] intervals let the
            // snapshot union them into the worker-busy fraction of the span
            // (cont-82: par lane showed Worker #0 busy 36.6% of the window);
            // this measures the same thing server-side, per lane, every run.
            long wi = W_N.getAndIncrement();
            if (wi < WRING) {
                W_START[(int) wi] = tEnd - nanos;
                W_WALL[(int) wi] = nanos;
            }
        } else if (WIN_N > 0) {
            // cont-142: margin-law accounting — dependency-margin fill work
            // (outside the armed rect, dragged by the r8 STRUCTURE_STARTS /
            // r2 BIOMES / r1 TERRAIN read radii).
            WIN_OUTSIDE.incrementAndGet();
            WIN_OUTSIDE_NANOS.addAndGet(nanos);
        }
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
        String s = String.format(
                "chunks=%d  mean=%.1fus  min=%.1fus  p50=%.1fus  p99=%.1fus  max=%.1fus  [native=%.2fms nat_p50=%.2fms layout=%.2fms beard=%.2fms marks=%.2fms]  [lay setup=%.2f sec=%.2f hmap=%.2f verify=%.2f | uni=%.2f/%.1f lin=%.2f/%.1f glob=%.2f/%.1f pb=%.2f/%.2f]  [sections filled=%d mixed=%d air=%d v1fallback=%d bulk=%s]  swapfail=%s  marks_seen=%d marks_landed=%d bridge_no_chunk=%d bridge_err=%d beard_chunks=%d beard_pieces=%d",
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
                com.planeta.crussty.CrusstyChunkGenerator.swapFail,
                MARKS_SEEN.get(), MARKS_LANDED.get(), BRIDGE_NO_CHUNK.get(), BRIDGE_ERROR.get(),
                BEARD_CHUNKS.get(), BEARD_PIECES.get());
        s += String.format(" [biome_fill=%.2fms n=%d cpu=%.2fms]",
                BIOME_TOTAL.get() / (double) Math.max(1, BIOME_N.get()) / 1e6,
                BIOME_N.get(),
                BIOME_CPU.get() / (double) Math.max(1, BIOME_N.get()) / 1e6);
        return s;
    }

    static long count() {
        return COUNT.get();
    }
}
