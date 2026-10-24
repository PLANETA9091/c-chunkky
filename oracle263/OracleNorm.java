import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import net.minecraft.world.level.levelgen.synth.*;

public class OracleNorm {
    public static void main(String[] args) throws Exception {
        net.minecraft.SharedConstants.tryDetectVersion();
        net.minecraft.server.Bootstrap.bootStrap();
        long seed = 3257840388504953787L;
        NormalNoise nn = NormalNoise.builder().setBaseOctave(-7).setOctaveCount(4).setBaseAmplitude(1.7).build();
        var f = NormalNoise.class.getDeclaredField("normalizationFactor"); f.setAccessible(true);
        var oc = NormalNoise.class.getDeclaredField("octaves"); oc.setAccessible(true);
        System.out.println("factor " + f.getDouble(nn));
        Object[] octs = ((java.util.List<?>) oc.get(nn)).toArray();
        for (Object o : octs) {
            var oi = o.getClass();
            var idx = oi.getDeclaredField("octaveIndex"); idx.setAccessible(true);
            var freq = oi.getDeclaredField("frequency"); freq.setAccessible(true);
            var amp = oi.getDeclaredField("amplitude"); amp.setAccessible(true);
            System.out.println("oct " + idx.getInt(o) + " freq " + freq.getDouble(o) + " amp " + amp.getDouble(o));
        }
    }
}
