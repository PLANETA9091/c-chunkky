import net.minecraft.world.level.levelgen.WorldgenRandom;
import net.minecraft.world.level.levelgen.RandomSupport;

/** scattered oracle over the REAL WorldgenRandom + a codec-free xoro clone. */
public class ScatRef3 {
    static final int COUNT = 4, SIZE = 7;

    public static void main(String[] args) {
        long seed = Long.parseLong(args[0]);
        int bx = Integer.parseInt(args[1]);
        int bz = Integer.parseInt(args[2]);
        int step = Integer.parseInt(args[3]);
        int gidLo = Integer.parseInt(args[4]);
        int gidHi = Integer.parseInt(args[5]);

        for (int gid = gidLo; gid <= gidHi; gid++) {
            WorldgenRandom rng = new WorldgenRandom(new net.minecraft.world.level.levelgen.XoroFree(seed));
            long deco = rng.setDecorationSeed(seed, bx, bz);
            rng.setFeatureSeed(deco, gid, step);
            StringBuilder sb = new StringBuilder();
            for (int p = 0; p < COUNT; p++) {
                int px = bx + rng.nextInt(16);
                int pz = bz + rng.nextInt(16);
                int py = rng.nextInt(41) - 16;
                int n = rng.nextInt(SIZE + 1);
                sb.append(" c").append(p).append("=(").append(px).append(',').append(py)
                  .append(',').append(pz).append(',').append(n).append(")[");
                for (int i = 0; i < n; i++) {
                    int mag = Math.min(i, 7);
                    int dx = Math.round((rng.nextFloat() - rng.nextFloat()) * (float) mag);
                    int dy = Math.round((rng.nextFloat() - rng.nextFloat()) * (float) mag);
                    int dz = Math.round((rng.nextFloat() - rng.nextFloat()) * (float) mag);
                    sb.append(px + dx).append('/').append(py + dy).append('/').append(pz + dz).append(' ');
                }
                sb.append(']');
            }
            System.out.println("G " + gid + " |" + sb);
        }
    }
}
