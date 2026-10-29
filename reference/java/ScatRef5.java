import net.minecraft.world.level.levelgen.WorldgenRandom;
import net.minecraft.world.level.levelgen.RandomSupport;

/** scattered oracle WITH the rule_test model in the live stream (vanilla
 *  order: spos offsets -> getBlockState -> random_block(stone,0.7) draw on
 *  stone / tag deepslate on y<0 -> write). Flat world: stone y>=0, deepslate
 *  y<0, no air; re-visits read the replaced ore (miss, no draw). */
public class ScatRef5 {
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
                java.util.Map<Long, String> world = new java.util.HashMap<>();
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
                    int sx = px + ax[0], sy = py + ax[1], sz = pz + ax[2];
                    sb.append(sx).append('/').append(sy).append('/').append(sz).append(' ');
                    long key = ((long) sx & 0x3FFFFFF) | (((long) sy & 0xFFF) << 26) | (((long) sz & 0x3FFFFFF) << 38);
                    String st = world.get(key);
                    if (st == null) st = sy < 0 ? "deepslate" : "stone";
                    if (st.equals("stone")) {
                        float rf = rng.nextFloat();
                        if (draws) System.out.printf("R gid=%d c=%d spos=%d rule=stone f=%08x hit=%s%n",
                            gid, p, i, Float.floatToIntBits(rf), rf < 0.7f);
                        if (rf < 0.7f) world.put(key, "copper_ore");
                    } else if (st.equals("deepslate")) {
                        world.put(key, "deepslate_copper_ore");
                    }
                }
                sb.append(']');
            }
            System.out.println("G " + gid + " |" + sb);
        }
    }
}
