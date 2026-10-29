import net.minecraft.world.level.levelgen.WorldgenRandom;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import net.minecraft.util.Mth;
import net.minecraft.util.RandomSource;

/**
 * OreFeature draw/candidate oracle with a STONE-filled mock level (no air,
 * OCEAN_FLOOR_WG height 200, world -64..320). Mirrors OreFeature.place +
 * doPlace exactly, printing each written block (W x y z) in order.
 * RNG: setDecorationSeed(seed, ox, oz) -> setFeatureSeed(deco, gid, step).
 */
public class OreRef {
    // mock level semantics shared with the Rust repro: every block is stone
    // (no air), targets tag-match, discard draws happen but air checks fail.
    static final int SIZE = 12;        // ore_gold size
    static final float DISCARD = 0.5f;

    public static void main(String[] args) {
        long seed = Long.parseLong(args[0]);
        int ox = Integer.parseInt(args[1]);
        int oz = Integer.parseInt(args[2]);
        int gid = Integer.parseInt(args[3]);
        int step = Integer.parseInt(args[4]);

        WorldgenRandom rng = new WorldgenRandom(new XoroshiroRandomSource(
                net.minecraft.world.level.levelgen.RandomSupport.upgradeSeedTo128bit(seed)));
        long deco = rng.setDecorationSeed(seed, ox, oz);
        rng.setFeatureSeed(deco, gid, step);

        // placement: [count=4, in_square, height_range(uniform abs -16..24)]
        // then the ore body per position (lazy depth-first pull).
        for (int p = 0; p < 4; p++) {
            int sx = ox + rng.nextInt(16);
            int sz = oz + rng.nextInt(16);
            int sy = rng.nextInt(41) - 16;
            // --- ore body at (sx, sy, sz) ---
            float f = rng.nextFloat() * (float) Math.PI;
            float f1 = (float) SIZE / 8.0F;
            int ceil = Mth.ceil(((float) SIZE / 16.0F * 2.0F + 1.0F) / 2.0F);
            double d = (double) sx + Math.sin((double) f) * (double) f1;
            double d1 = (double) sx - Math.sin((double) f) * (double) f1;
            double d2 = (double) sz + Math.cos((double) f) * (double) f1;
            double d3 = (double) sz - Math.cos((double) f) * (double) f1;
            double d4 = (double) (sy + rng.nextInt(3) - 2);
            double d5 = (double) (sy + rng.nextInt(3) - 2);
            System.out.printf("O f=%.9g sx=%d sy=%d sz=%d minx=%.17g maxx=%.17g miny=%.17g maxy=%.17g minz=%.17g maxz=%.17g%n", f, sx, sy, sz, d, d1, d2, d3, d4, d5);
            int i1 = sx - Mth.ceil(f1) - ceil;
            int i2 = sy - 2 - ceil;
            int i3 = sz - Mth.ceil(f1) - ceil;
            int i4 = 2 * (Mth.ceil(f1) + ceil);
            int i5 = 2 * (2 + ceil);
            doPlace(rng, d, d1, d2, d3, d4, d5, i1, i2, i3, i4, i5);
        }
    }

    static boolean doPlace(WorldgenRandom random, double minX, double maxX, double minZ,
                           double maxZ, double minY, double maxY,
                           int x, int y, int z, int width, int height) {
        java.util.BitSet bitSet = new java.util.BitSet(width * height * width);
        int i1 = SIZE;
        double[] doubles = new double[i1 * 4];
        for (int i2 = 0; i2 < i1; i2++) {
            float f = (float) i2 / (float) i1;
            double d = Mth.lerp((double) f, minX, maxX);
            double d1 = Mth.lerp((double) f, minY, maxY);
            double d2 = Mth.lerp((double) f, minZ, maxZ);
            double d3 = random.nextDouble() * (double) i1 / 16.0;
            double d4 = ((double) (Mth.sin((float) Math.PI * f) + 1.0F) * d3 + 1.0) / 2.0;
            doubles[i2 * 4 + 0] = d;
            doubles[i2 * 4 + 1] = d1;
            doubles[i2 * 4 + 2] = d2;
            doubles[i2 * 4 + 3] = d4;
            System.out.printf("N %d %.17g %.17g %.17g %.17g%n", i2, d, d1, d2, d4);
        }
        for (int i2 = 0; i2 < i1 - 1; i2++) {
            if (!(doubles[i2 * 4 + 3] <= 0.0)) {
                for (int i3 = i2 + 1; i3 < i1; i3++) {
                    if (!(doubles[i3 * 4 + 3] <= 0.0)) {
                        double d = doubles[i2 * 4 + 0] - doubles[i3 * 4 + 0];
                        double d1 = doubles[i2 * 4 + 1] - doubles[i3 * 4 + 1];
                        double d2 = doubles[i2 * 4 + 2] - doubles[i3 * 4 + 2];
                        double d3 = doubles[i2 * 4 + 3] - doubles[i3 * 4 + 3];
                        if (d3 * d3 > d * d + d1 * d1 + d2 * d2) {
                            if (d3 > 0.0) {
                                doubles[i3 * 4 + 3] = -1.0;
                            } else {
                                doubles[i2 * 4 + 3] = -1.0;
                            }
                        }
                    }
                }
            }
        }
        int wrote = 0;
        for (int i3x = 0; i3x < i1; i3x++) {
            double d = doubles[i3x * 4 + 3];
            if (!(d < 0.0)) {
                double d1 = doubles[i3x * 4 + 0];
                double d2 = doubles[i3x * 4 + 1];
                double d3 = doubles[i3x * 4 + 2];
                int max = Math.max(Mth.floor(d1 - d), x);
                int max1 = Math.max(Mth.floor(d2 - d), y);
                int max2 = Math.max(Mth.floor(d3 - d), z);
                int max3 = Math.max(Mth.floor(d1 + d), max);
                int max4 = Math.max(Mth.floor(d2 + d), max1);
                int max5 = Math.max(Mth.floor(d3 + d), max2);
                for (int i4 = max; i4 <= max3; i4++) {
                    double d5 = ((double) i4 + 0.5 - d1) / d;
                    if (d5 * d5 < 1.0) {
                        for (int i5 = max1; i5 <= max4; i5++) {
                            double d6 = ((double) i5 + 0.5 - d2) / d;
                            if (d5 * d5 + d6 * d6 < 1.0) {
                                for (int i6 = max2; i6 <= max5; i6++) {
                                    double d7 = ((double) i6 + 0.5 - d3) / d;
                                    if (d5 * d5 + d6 * d6 + d7 * d7 < 1.0 && !isOutside(i5)) {
                                        int i7 = i4 - x + (i5 - y) * width + (i6 - z) * width * height;
                                        if (!bitSet.get(i7)) {
                                            bitSet.set(i7);
                                            System.out.println("C " + i4 + " " + i5 + " " + i6);
                                            // canPlaceOre: tag test matches (no RNG),
                                            // then shouldSkipAirCheck draws; the mock
                                            // has NO air -> !isAdjacentToAir = true ->
                                            // the write happens regardless of the draw.
                                            if (shouldSkipAirCheck(random, DISCARD) || true) {
                                                System.out.println("W " + i4 + " " + i5 + " " + i6);
                                                wrote++;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        System.out.println("# wrote=" + wrote);
        return true;
    }

    static boolean isOutside(int y) {
        return y < -64 || y >= 320;
    }

    static boolean shouldSkipAirCheck(RandomSource random, float chance) {
        return chance <= 0.0F || !(chance >= 1.0F) && random.nextFloat() >= chance;
    }
}
