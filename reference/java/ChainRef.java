// Bit-parity reference for native/src/features_placement.rs: LAZY depth-first flatMap chain.
// Chain: InSquare -> Count(UniformInt[-1..2]) -> RarityFilter(2) -> RandomOffset(xz U[-3..3], y U[0..1])
//        -> HeightRange(UniformHeight(abs(-32)..abs(64)))
// Replicates PlacedFeature.placeWithContext stream semantics 1:1 (lazy Streams).
public class ChainRef {
    static long GOLDEN = -7046029254386353131L, SILVER = 7640891576956012809L;
    static long lo, hi;
    static long mixStafford13(long seed) {
        seed = (seed ^ seed >>> 30) * -4658895280553007687L;
        seed = (seed ^ seed >>> 27) * -7723592293110705685L;
        return seed ^ seed >>> 31;
    }
    static void setSeed(long seed) {
        long l = seed ^ SILVER, l1 = l + GOLDEN;
        lo = mixStafford13(l); hi = mixStafford13(l1);
        if ((lo | hi) == 0L) { lo = GOLDEN; hi = SILVER; }
    }
    static long xnext() {
        long l = lo, l1 = hi;
        long l2 = Long.rotateLeft(l + l1, 17) + l;
        l1 ^= l; lo = Long.rotateLeft(l, 49) ^ l1 ^ l1 << 21; hi = Long.rotateLeft(l1, 28);
        return l2;
    }
    static int nextBits(int bits) { return (int)(xnext() >>> 64 - bits); }
    static long wrNextLong() { return ((long)nextBits(32) << 32) + nextBits(32); }
    static int nextInt(int bound) { // BitRandomSource default via next(31)
        if ((bound & bound - 1) == 0) return (int)((long)bound * (long)nextBits(31) >> 31);
        int i, r;
        do { i = nextBits(31); r = i % bound; } while (i - r + (bound - 1) < 0);
        return r;
    }
    static float nextFloat() { return (float)nextBits(24) * 5.9604645E-8F; }
    static int uniformInt(int a, int b) { return nextInt(b - a + 1) + a; } // UniformInt.sample
    static int randBetweenIncl(int a, int b) { return nextInt(b - a + 1) + a; } // Mth.randomBetweenInclusive

    record BP(int x, int y, int z) {}

    public static void main(String[] args) {
        long[][] cases = { {8675309L, 16, -32}, {1234567890L, -208, 304} };
        for (long[] c : cases) {
            setSeed(c[0]); long a = wrNextLong() | 1L, b = wrNextLong() | 1L;
            long deco = (long)c[1] * a + (long)c[2] * b ^ c[0];
            for (int[] origin : new int[][] {{0, 60, 0}, {-104, 70, 208}}) {
                setSeed(deco + 3L + 10000L * 2L); // setFeatureSeed(deco, 3, 2) — fresh per chain (vanilla: per feature index)
                StringBuilder sb = new StringBuilder();
                // lazy chain — same construction as vanilla placeWithContext
                java.util.stream.Stream<BP> s = java.util.stream.Stream.of(new BP(origin[0], origin[1], origin[2]));
                s = s.flatMap(p -> { int x = nextInt(16) + p.x(); int z = nextInt(16) + p.z(); return java.util.stream.Stream.of(new BP(x, p.y(), z)); });
                s = s.flatMap(p -> java.util.stream.IntStream.range(0, uniformInt(1, 4)).mapToObj(i -> p));
                s = s.flatMap(p -> nextFloat() < 1.0F / 2.0F ? java.util.stream.Stream.of(p) : java.util.stream.Stream.<BP>empty());
                s = s.flatMap(p -> { int x = p.x() + uniformInt(-3, 3); int y = p.y() + uniformInt(0, 1); int z = p.z() + uniformInt(-3, 3); return java.util.stream.Stream.of(new BP(x, y, z)); });
                s = s.flatMap(p -> java.util.stream.Stream.of(new BP(p.x(), randBetweenIncl(-32, 64), p.z())));
                s.forEach(q -> sb.append(q.x()).append(' ').append(q.y()).append(' ').append(q.z()).append(';'));
                System.out.println("CHAIN " + c[0] + " " + origin[0] + " " + origin[2] + " | " + sb);
            }
        }
    }
}
