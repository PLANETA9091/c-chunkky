package com.planeta.crussty;

/** cont-138: carver phase timers — where does the honest-window carver wall
 * go (acquire vs section pack vs native carve vs ops/marks vs apply)? Pure
 * counters (no NMS) so the API-only compile pass can reference it. */
final class CarverStats {
    static final java.util.concurrent.atomic.AtomicLong ACQ = new java.util.concurrent.atomic.AtomicLong();
    static final java.util.concurrent.atomic.AtomicLong PACK = new java.util.concurrent.atomic.AtomicLong();
    static final java.util.concurrent.atomic.AtomicLong CARVE = new java.util.concurrent.atomic.AtomicLong();
    static final java.util.concurrent.atomic.AtomicLong OPS = new java.util.concurrent.atomic.AtomicLong();
    static final java.util.concurrent.atomic.AtomicLong APPLY = new java.util.concurrent.atomic.AtomicLong();
    static final java.util.concurrent.atomic.AtomicLong N = new java.util.concurrent.atomic.AtomicLong();

    static void reset() {
        ACQ.set(0); PACK.set(0); CARVE.set(0); OPS.set(0); APPLY.set(0); N.set(0);
    }

    static String snapshot() {
        long n = Math.max(1, N.get());
        return String.format(
                "[carver-phases] n=%d acq=%.2f pack=%.2f carve=%.2f ops=%.2f apply=%.2f (ms/ch)",
                N.get(), ACQ.get() / n / 1e6, PACK.get() / n / 1e6,
                CARVE.get() / n / 1e6, OPS.get() / n / 1e6, APPLY.get() / n / 1e6);
    }

    private CarverStats() {
    }
}
