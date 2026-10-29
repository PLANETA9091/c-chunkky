import java.util.Random;

/** Reference: patch_ref draw chain (chunk 100,100, step=10, gid=2).
 *  patch placed [count2, in_square, height_range(60..90), biome]
 *  -> patch cfg tries=8 xz=4 y=1 (6 draws/try)
 *  -> inner placed rotated_nb [count3, in_square, height_range(-32..32)]
 *  -> inner cfg simple_block rotated bone_block (nextInt(3) per leaf). */
public class PatchRef {
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
        int ox = (Integer.parseInt(args[0])) << 4, oz = (Integer.parseInt(args[1])) << 4;
        long deco = setDecorationSeed(levelSeed, ox, oz);
        setFeatureSeed(deco, 2, 10);
        StringBuilder sb = new StringBuilder("bones:");
        for (int p = 0; p < 2; p++) {           // count 2 patch origins
            int px = ox + nextInt(16);          // in_square
            int pz = oz + nextInt(16);
            int py = 60 + nextInt(31);          // height_range 60..90
            for (int t = 0; t < 8; t++) {       // tries
                int dx = nextInt(5) - nextInt(5);
                int dy = nextInt(2) - nextInt(2);
                int dz = nextInt(5) - nextInt(5);
                if (p == 0 && t < 2) System.out.println("try p" + p + "t" + t + ": offs=(" + dx + "," + dy + "," + dz + ")");
                int tx = px + dx, ty = py + dy, tz = pz + dz;
                for (int c = 0; c < 3; c++) {   // inner count 3
                    int bx = tx + nextInt(16);
                    int bz = tz + nextInt(16);
                    int by = -32 + nextInt(65); // height_range -32..32
                    int axis = nextInt(3);
                    sb.append(" (").append(bx).append(",").append(by).append(",").append(bz)
                      .append(",a").append(axis).append(')');
                }
            }
        }
        System.out.println(sb);
    }
}
