import java.util.Random;

/** Reference: vanilla decoration seed + per-feature seed + count/in_square expansion
 *  for the probe flowers feature at chunk (100,100) — uses the golden-verified
 *  WgrRef RNG core (F1 goldens). */
public class FlowersRef {
    static long GOLDEN = -7046029254386353131L, SILVER = 7640891576956012809L;
    static long lo, hi;

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
    static int nextBits(int bits) { return (int)(xnext() >>> 64 - bits); }
    static long wrNextLong() { return ((long)nextBits(32) << 32) + nextBits(32); }
    static long setDecorationSeed(long levelSeed, int minX, int minZ) {
        setSeed(levelSeed);
        long a = wrNextLong() | 1L, b = wrNextLong() | 1L;
        long s = (long)minX * a + (long)minZ * b ^ levelSeed;
        setSeed(s);
        return s;
    }
    static void setFeatureSeed(long deco, int index, int step) { setSeed(deco + (long)index + (long)(10000 * step)); }
    static int nextInt(int bound) {
        if ((bound & -bound) == bound) return (int)((bound * (long)nextBits(31)) >> 31);
        int i, r;
        do { i = nextBits(31); r = i % bound; } while (i - r + (bound - 1) < 0);
        return r;
    }

    public static void main(String[] args) {
        long levelSeed = 3257840388504953787L;
        int cx = Integer.parseInt(args[0]);
        int cz = Integer.parseInt(args[1]);
        int ox = cx << 4, oz = cz << 4;
        long deco = setDecorationSeed(levelSeed, ox, oz);
        int idx = Integer.parseInt(args[2]);
        setFeatureSeed(deco, idx, 9);
        StringBuilder sb = new StringBuilder("idx" + idx + ":");
        int cnt = new int[]{6,5,3,2,2,2,4,4,4}[idx];
        for (int i = 0; i < cnt; i++) {
            int x = nextInt(16);
            int z = nextInt(16);
            if (idx == 6) {
                int dxc = nextInt(7) - 3;
                int dyc = nextInt(3);
                int dzc = nextInt(7) - 3;
                sb.append(" (").append(ox + x + dxc).append(",").append(63 + dyc).append(",").append(oz + z + dzc).append(')');
            } else {
                sb.append(" (").append(ox + x).append(",h,").append(oz + z).append(')');
                if (idx == 1) { nextInt(2); }            // weighted leaf draw
                if (idx == 2) { nextInt(3); }            // rotated leaf draw
                if (idx == 3) { if (nextBits(24) < 0.3f * (1<<24)) {} nextInt(0+1); } // rand_sel: nextFloat + maybe inner
                if (idx == 4) { nextInt(2); }            // srs selector draw
                if (idx == 5) { nextInt(2); }            // rbs bool draw
            }
        }
        System.out.println(sb);
        return;
    }
}