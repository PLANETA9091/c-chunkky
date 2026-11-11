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
    /** cont-143 non-vacuity counters: ops applied to sections, carve-declined
     *  chunks (native returned MIN_VALUE), failed-apply chunks. The carvers
     *  gate must see ops_applied > 0 to mean anything (cont-143 finding: the
     *  lane passed for 4 sessions while apply threw on every carved chunk). */
    static final java.util.concurrent.atomic.AtomicLong OPS_APPLIED = new java.util.concurrent.atomic.AtomicLong();
    static final java.util.concurrent.atomic.AtomicLong DECLINED = new java.util.concurrent.atomic.AtomicLong();
    static final java.util.concurrent.atomic.AtomicLong APPLY_FAILED = new java.util.concurrent.atomic.AtomicLong();

    static void reset() {
        ACQ.set(0); PACK.set(0); CARVE.set(0); OPS.set(0); APPLY.set(0); N.set(0);
        OPS_APPLIED.set(0); DECLINED.set(0); APPLY_FAILED.set(0);
    }

    static String snapshot() {
        long n = Math.max(1, N.get());
        // fuse flag re-read (CarverDriver is NMS-typed and absent from the
        // API-only compile pass; the env read is idempotent)
        boolean fuse;
        try {
            fuse = !"0".equals(System.getenv("CRUS_CARVER_FUSE"));
        } catch (Throwable tf) {
            fuse = false;
        }
        return String.format(
                "[carver-phases] n=%d acq=%.2f pack=%.2f carve=%.2f ops=%.2f apply=%.2f (ms/ch)"
                        + " opsApplied=%d declined=%d applyFailed=%d fuse=%s",
                N.get(), ACQ.get() / n / 1e6, PACK.get() / n / 1e6,
                CARVE.get() / n / 1e6, OPS.get() / n / 1e6, APPLY.get() / n / 1e6,
                OPS_APPLIED.get(), DECLINED.get(), APPLY_FAILED.get(), fuse);
    }

    private CarverStats() {
    }
}
