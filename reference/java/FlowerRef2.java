import net.minecraft.world.level.levelgen.WorldgenRandom;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;

public class FlowerRef2 {
    public static void main(String[] a) {
        long seed = Long.parseLong(a[0]);
        int ox = Integer.parseInt(a[1]), oz = Integer.parseInt(a[2]);
        int gid = Integer.parseInt(a[3]), step = Integer.parseInt(a[4]);
        WorldgenRandom rng = new WorldgenRandom(new XoroshiroRandomSource(
                net.minecraft.world.level.levelgen.RandomSupport.upgradeSeedTo128bit(seed)));
        long deco = rng.setDecorationSeed(seed, ox, oz);
        rng.setFeatureSeed(deco, gid, step);
        for (int i = 0; i < 6; i++) {
            int x = ox + rng.nextInt(16);
            int z = oz + rng.nextInt(16);
            System.out.println("P " + x + " " + z);
        }
    }
}
