// ScatteredOreFeature oracle (self-contained, WgrRef RNG core).
// Reproduces placed-feature pipeline [count=4 -> in_square -> height_range
// uniform abs -32..8 -> biome] + ScatteredOreFeature (nextInt(size+1),
// offsetTargetPos 6 nextFloat draws per spos) for a range of per-step
// feature indices (gid). Prints one line per gid:
//   G <gid> | c0=(x,y,z,n)[x/y/z ...] c1=... c2=... c3=...
public class ScatRef {
    static long GOLDEN = -7046029254386353131L, SILVER = 7640891576956012809L;
    static long lo, hi;
    static final int COUNT = 4, SIZE = 7;

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
    static float wrFloat() { return (float)nextBits(24) * 5.9604645E-8F; }
    static int wrNextIntBound(int bound) {
        if ((bound & bound - 1) == 0) return (int)((long)bound * (long)nextBits(31) >> 31);
        int i, i1;
        do { i = nextBits(31); i1 = i % bound; } while (i - i1 + (bound - 1) < 0);
        return i1;
    }

    public static void main(String[] args) {
        long seed = Long.parseLong(args[0]);
        int bx = Integer.parseInt(args[1]);
        int bz = Integer.parseInt(args[2]);
        int step = Integer.parseInt(args[3]);
        int gidLo = Integer.parseInt(args[4]);
        int gidHi = Integer.parseInt(args[5]);

        for (int gid = gidLo; gid <= gidHi; gid++) {
            WorldgenRandomless(seed, bx, bz, step, gid);
        }
    }

    static void WorldgenRandomless(long seed, int bx, int bz, int step, int gid) {
        setSeed(seed);
        long deco = setDecorationSeed(seed, bx, bz);
        setFeatureSeed(deco, gid, step);
        StringBuilder sb = new StringBuilder();
        for (int p = 0; p < COUNT; p++) {
            int px = bx + wrNextIntBound(16);
            int pz = bz + wrNextIntBound(16);
            int py = wrNextIntBound(41) - 16;
            int n = wrNextIntBound(SIZE + 1);
            sb.append(" c").append(p).append("=(").append(px).append(',').append(py)
              .append(',').append(pz).append(',').append(n).append(")[");
            for (int i = 0; i < n; i++) {
                int mag = Math.min(i, 7);
                int dx = Math.round((wrFloat() - wrFloat()) * (float) mag);
                int dy = Math.round((wrFloat() - wrFloat()) * (float) mag);
                int dz = Math.round((wrFloat() - wrFloat()) * (float) mag);
                sb.append(px + dx).append('/').append(py + dy).append('/').append(pz + dz).append(' ');
            }
            sb.append(']');
        }
        System.out.println("G " + gid + " |" + sb);
    }
}
