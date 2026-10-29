import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import net.minecraft.world.level.levelgen.RandomSupport;
import net.minecraft.world.level.levelgen.WorldgenRandom;

public class SlotRef {
    public static void main(String[] args) {
        long l = Long.parseLong(args[0]);
        int idx = Integer.parseInt(args[1]);
        int step = Integer.parseInt(args[2]);
        WorldgenRandom rng = new WorldgenRandom(new XoroshiroRandomSource(RandomSupport.generateUniqueSeed()));
        rng.setFeatureSeed(l, idx, step);
        StringBuilder sb = new StringBuilder();
        for (int i = 0; i < 4; i++) {
            sb.append(rng.nextInt(16)).append(i < 3 ? "," : "");
        }
        System.out.println("SLOT l=" + l + " idx=" + idx + " step=" + step + " sq4=" + sb);
    }
}
