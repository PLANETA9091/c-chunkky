import net.minecraft.world.level.levelgen.WorldgenRandom;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;

public class FlowerSolve2 {
    public static void main(String[] a) {
        long seed = Long.parseLong(a[0]);
        java.util.List<int[]> obs = new java.util.ArrayList<>();
        for (int i = 1; i + 3 < a.length; i += 4) {
            obs.add(new int[]{Integer.parseInt(a[i]), Integer.parseInt(a[i+1]), Integer.parseInt(a[i+2]), Integer.parseInt(a[i+3])});
        }
        for (int step = 0; step <= 10; step++) {
            for (int gid = 0; gid <= 15; gid++) {
                boolean all = true;
                for (int[] o : obs) {
                    int cx = o[0], cz = o[1];
                    WorldgenRandom rng = new WorldgenRandom(new XoroshiroRandomSource(
                            net.minecraft.world.level.levelgen.RandomSupport.upgradeSeedTo128bit(seed)));
                    long deco = rng.setDecorationSeed(seed, cx * 16, cz * 16);
                    rng.setFeatureSeed(deco, gid, step);
                    boolean hit = false;
                    for (int k = 0; k < 6 && !hit; k++) {
                        int x = cx * 16 + rng.nextInt(16);
                        int z = cz * 16 + rng.nextInt(16);
                        if (x == o[2] && z == o[3]) hit = true;
                    }
                    if (!hit) { all = false; break; }
                }
                if (all) System.out.println("MATCH step=" + step + " gid=" + gid);
            }
        }
        System.out.println("solvedone");
    }
}
