package net.minecraft.world.level.levelgen;

import net.minecraft.util.RandomSource;

/** Codec-free XoroshiroRandomSource clone (bit-identical stream) so the REAL
 *  WorldgenRandom classes can be exercised without the datafixerupper dep. */
public class XoroFree implements RandomSource {
    private Xoroshiro128PlusPlusFree rng;

    public XoroFree(long seed) {
        reset(seed);
    }

    static long mixStafford13(long seed) {
        seed = (seed ^ seed >>> 30) * -4658895280553007687L;
        seed = (seed ^ seed >>> 27) * -7723592293110705685L;
        return seed ^ seed >>> 31;
    }

    private void reset(long seed) {
        long l = seed ^ 7640891576956012809L;
        long l1 = l + -7046029254386353131L;
        long lo = mixStafford13(l), hi = mixStafford13(l1);
        if ((lo | hi) == 0L) { lo = -7046029254386353131L; hi = 7640891576956012809L; }
        this.rng = new Xoroshiro128PlusPlusFree(lo, hi);
    }

    @Override public RandomSource fork() { return new XoroFree(rng.nextLong()); }
    @Override public PositionalRandomFactory forkPositional() { return null; }
    @Override public void setSeed(long seed) { reset(seed); }
    public int next(int bits) { return (int)(rng.nextLong() >>> 64 - bits); }
    @Override public int nextInt() { return this.next(32); }
    @Override public int nextInt(int bound) {
        if ((bound & bound - 1) == 0) return (int)((long)bound * (long)this.next(31) >> 31);
        int i, i1;
        do { i = this.next(31); i1 = i % bound; } while (i - i1 + (bound - 1) < 0);
        return i1;
    }
    @Override public long nextLong() { return rng.nextLong(); }
    @Override public boolean nextBoolean() { return this.next(1) != 0; }
    @Override public float nextFloat() { return (float)this.next(24) * 5.9604645E-8F; }
    @Override public double nextDouble() {
        return (double)((float)(((long)this.next(26) << 27) + (long)this.next(27)) * 1.110223E-16F);
    }
    @Override public double nextGaussian() { return 0.0; }
}

class Xoroshiro128PlusPlusFree {
    private long seedLo, seedHi;
    Xoroshiro128PlusPlusFree(long seedLo, long seedHi) {
        this.seedLo = seedLo; this.seedHi = seedHi;
        if ((this.seedLo | this.seedHi) == 0L) { this.seedLo = -7046029254386353131L; this.seedHi = 7640891576956012809L; }
    }
    public long nextLong() {
        long l = this.seedLo, l1 = this.seedHi;
        long l2 = Long.rotateLeft(l + l1, 17) + l;
        l1 ^= l;
        this.seedLo = Long.rotateLeft(l, 49) ^ l1 ^ l1 << 21;
        this.seedHi = Long.rotateLeft(l1, 28);
        return l2;
    }
}
