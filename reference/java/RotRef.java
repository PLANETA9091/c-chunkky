import net.minecraft.world.level.levelgen.WorldgenRandom;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;

/**
 * Vanilla draw-order reference for the F1 probe's rotated simple_block:
 * placement [count=3(const), in_square, height_range(uniform abs -32..32)] +
 * simple_block(rotated_block_provider) per position.
 *
 * Stream per position: square x = nextInt(16), square z = nextInt(16),
 * height y = nextInt(65) - 32, axis = nextInt(3) (Axis.VALUES = X, Y, Z).
 */
public class RotRef {
    public static void main(String[] args) {
        long seed = Long.parseLong(args[0]);
        int ox = Integer.parseInt(args[1]);
        int oz = Integer.parseInt(args[2]);
        int gid = Integer.parseInt(args[3]);
        int step = Integer.parseInt(args[4]);
        int count = Integer.parseInt(args[5]);

        WorldgenRandom rng = new WorldgenRandom(new XoroshiroRandomSource(
                net.minecraft.world.level.levelgen.RandomSupport.upgradeSeedTo128bit(seed)));
        long deco = rng.setDecorationSeed(seed, ox, oz);
        rng.setFeatureSeed(deco, gid, step);
        StringBuilder sb = new StringBuilder();
        if (System.getenv("RAW") != null) {
            for (int i = 0; i < 8; i++) {
                sb.append("R ").append(rng.next(31)).append('\n');
            }
            System.out.print(sb);
            return;
        }
        for (int p = 0; p < count; p++) {
            int x = rng.nextInt(16);
            int z = rng.nextInt(16);
            int y = rng.nextInt(65) - 32;
            int axis = rng.nextInt(3); // 0=X 1=Y 2=Z
            sb.append("P ").append(x).append(' ').append(y).append(' ').append(z)
              .append(' ').append(axis).append('\n');
        }
        System.out.print(sb);
    }
}
