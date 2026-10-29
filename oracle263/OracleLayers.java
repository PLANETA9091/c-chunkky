import net.minecraft.util.RandomSource;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import net.minecraft.world.level.levelgen.synth.*;
import net.minecraft.world.level.levelgen.PositionalRandomFactory;

public class OracleLayers {
    public static void main(String[] args) throws Exception {
        net.minecraft.SharedConstants.tryDetectVersion();
        net.minecraft.server.Bootstrap.bootStrap();
        long seed = 3257840388504953787L;
        XoroshiroRandomSource r = new XoroshiroRandomSource(seed);
        PositionalRandomFactory f1 = r.forkPositional();
        PositionalRandomFactory f2 = r.forkPositional();
        for (int i = 0; i < 4; i++) {
            String s = "octave_" + (-7 + i);
            XoroshiroRandomSource a = (XoroshiroRandomSource) f1.fromHashOf(s);
            XoroshiroRandomSource b = (XoroshiroRandomSource) f2.fromHashOf(s);
            PerlinNoise pa = new PerlinNoise(a);
            PerlinNoise pb = new PerlinNoise(b);
            System.out.println("P " + (-7 + i) + " " + pa.get(0, 0, 0) + " " + pb.get(0, 0, 0));
        }
    }
}
