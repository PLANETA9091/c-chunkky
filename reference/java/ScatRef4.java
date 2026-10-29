import net.minecraft.world.level.levelgen.WorldgenRandom;
import net.minecraft.world.level.levelgen.RandomSupport;

/** scattered oracle printing EVERY draw (f1/f2 bits) + spos, for exact diff
 *  against the Rust scat_probe. Same chunk/gid/step inputs. */
public class ScatRef4 {
    static final int COUNT = 4, SIZE = 7;

    public static void main(String[] args) {
        long seed = Long.parseLong(System.getProperty("scat.seed", "3257840388504953787"));
        int bx = Integer.parseInt(System.getProperty("scat.bx", "1584"));
        int bz = Integer.parseInt(System.getProperty("scat.bz", "1568"));
        int step = Integer.parseInt(System.getProperty("scat.step", "6"));
        int yLo = Integer.parseInt(System.getProperty("scat.ylo", "-16"));
        boolean draws = System.getProperty("scat.draws") != null;
        int gidLo = Integer.parseInt(System.getProperty("scat.gidlo", "0"));
        int gidHi = Integer.parseInt(System.getProperty("scat.gidhi", "5"));

        for (int gid = gidLo; gid <= gidHi; gid++) {
            WorldgenRandom rng = new WorldgenRandom(new net.minecraft.world.level.levelgen.XoroFree(seed));
            long deco = rng.setDecorationSeed(seed, bx, bz);
            rng.setFeatureSeed(deco, gid, step);
            StringBuilder sb = new StringBuilder();
            for (int p = 0; p < COUNT; p++) {
                int px = bx + rng.nextInt(16);
                int pz = bz + rng.nextInt(16);
                int py = rng.nextInt(41) + yLo;
                int n = rng.nextInt(SIZE + 1);
                sb.append(" c").append(p).append("=(").append(px).append(',').append(py)
                  .append(',').append(pz).append(',').append(n).append(")[");
                for (int i = 0; i < n; i++) {
                    int mag = Math.min(i, 7);
                    int[] ax = new int[3];
                    String[] names = {"x", "y", "z"};
                    for (int a = 0; a < 3; a++) {
                        float f1 = rng.nextFloat(), f2 = rng.nextFloat();
                        float v = (f1 - f2) * (float) mag;
                        ax[a] = Math.round(v);
                        if (draws) {
                            System.out.printf("D gid=%d c=%d spos=%d axis=%s f1=%08x f2=%08x v=%.9f -> %d%n",
                                gid, p, i, names[a], Float.floatToIntBits(f1), Float.floatToIntBits(f2), v, ax[a]);
                        }
                    }
                    sb.append(px + ax[0]).append('/').append(py + ax[1]).append('/').append(pz + ax[2]).append(' ');
                }
                sb.append(']');
                // rule model pass (after offsets): world lives in a map
                java.util.Map<String, String> world = new java.util.HashMap<>();
                {
                    java.util.List<int[]> ps = new java.util.ArrayList<>();
                    java.util.List<String> ws = new java.util.ArrayList<>();
                    java.util.RandomSource rr = new net.minecraft.world.level.levelgen.XoroFree(0); // unused
                }
            }
            System.out.println("G " + gid + " |" + sb);
        }
    }
}
