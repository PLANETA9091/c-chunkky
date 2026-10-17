// Bit-parity reference for c-faster native/src/features_rng.rs (WorldgenRandom decoration protocol).
// Formulas copied 1:1 from decompile 1.21.8: WorldgenRandom, XoroshiroRandomSource, RandomSupport.
public class WgrRef {
    static long GOLDEN = -7046029254386353131L, SILVER = 7640891576956012809L;
    static long lo, hi; // xoroshiro state

    static long mixStafford13(long seed) {
        seed = (seed ^ seed >>> 30) * -4658895280553007687L;
        seed = (seed ^ seed >>> 27) * -7723592293110705685L;
        return seed ^ seed >>> 31;
    }
    static void setSeed(long seed) {
        long l = seed ^ SILVER;
        long l1 = l + GOLDEN;
        lo = mixStafford13(l); hi = mixStafford13(l1);
        if ((lo | hi) == 0L) { lo = GOLDEN; hi = SILVER; }
    }
    static long xnext() {
        long l = lo, l1 = hi;
        long l2 = Long.rotateLeft(l + l1, 17) + l;
        l1 ^= l;
        lo = Long.rotateLeft(l, 49) ^ l1 ^ l1 << 21;
        hi = Long.rotateLeft(l1, 28);
        return l2;
    }
    // WorldgenRandom.next(bits) with XoroshiroRandomSource wrapped
    static int nextBits(int bits) { return (int)(xnext() >>> 64 - bits); }
    static long wrNextLong() { return ((long)nextBits(32) << 32) + nextBits(32); } // BitRandomSource default via overridden next()
    static long setDecorationSeed(long levelSeed, int minX, int minZ) {
        setSeed(levelSeed);
        long a = wrNextLong() | 1L, b = wrNextLong() | 1L;
        long s = (long)minX * a + (long)minZ * b ^ levelSeed;
        setSeed(s);
        return s;
    }
    static void setFeatureSeed(long deco, int index, int step) { setSeed(deco + (long)index + (long)(10000 * step)); }
    public static void main(String[] args) {
        long[][] decoCases = {
            {1234567890L, 0, 0}, {1234567890L, 16, -16}, {-987654321098L, -208, 304},
            {1L, 1024, -1024}, {8675309L, 3360, -1024}, {-7046029254386353131L, -5, 5}
        };
        for (long[] c : decoCases) {
            long s = setDecorationSeed(c[0], (int)c[1], (int)c[2]);
            StringBuilder sb = new StringBuilder();
            // after decoration seed: simulate 3 feature seeds (indices 7, 0, 4291 at steps 0..6) consuming mixed ops
            setFeatureSeed(s, 7, 6); sb.append(wrNextLong()).append(',').append(nextBits(31)).append(',');
            setFeatureSeed(s, 0, 1); sb.append(wrNextLong()).append(',').append(nextBits(16)).append(',');
            setFeatureSeed(s, 4291, 10); sb.append(wrNextLong());
            System.out.println("DECO " + c[0] + " " + c[1] + " " + c[2] + " -> " + s + " | " + sb);
        }
        // float quirks: WorldgenRandom next(bits) path with xoro semantics
        setDecorationSeed(8675309L, 16, -32);
        setFeatureSeed(8675309L, 3, 2);
        StringBuilder fs = new StringBuilder();
        for (int i = 0; i < 3; i++) fs.append(Float.floatToIntBits(wrFloat())).append(',');
        for (int i = 0; i < 3; i++) fs.append(Double.doubleToLongBits(wrDouble())).append(',');
        for (int i = 0; i < 3; i++) fs.append(wrNextIntBound(16)).append(',').append(wrNextIntBound(10)).append(',');
        System.out.println("FQUIRK " + fs);
    }
    // RandomSource.nextFloat default (BitRandomSource): (float)next(24) * 5.9604645E-8F
    static float wrFloat() { return (float)nextBits(24) * 5.9604645E-8F; }
    // RandomSource.nextDouble default (BitRandomSource): ((next(26)<<27)+next(27)) l2f * 1.110223E-16F -> f2d
    static double wrDouble() {
        int i = nextBits(26), i1 = nextBits(27);
        long l = ((long)i << 27) + i1;
        return (double)((float)l * 1.110223E-16F);
    }
    // BitRandomSource.nextInt(bound) default: pow2 shift / rejection mod, both via next(31)
    static int wrNextIntBound(int bound) {
        if ((bound & bound - 1) == 0) return (int)((long)bound * (long)nextBits(31) >> 31);
        int i, i1;
        do { i = nextBits(31); i1 = i % bound; } while (i - i1 + (bound - 1) < 0);
        return i1;
    }
}
