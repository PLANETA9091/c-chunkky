import net.minecraft.util.RandomSource;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import net.minecraft.world.level.levelgen.synth.PerlinNoise;
import net.minecraft.world.level.levelgen.synth.NormalNoise;
import net.minecraft.world.level.levelgen.synth.SmearedPerlinNoise;
import net.minecraft.world.level.levelgen.synth.BlendedNoise;
import net.minecraft.world.level.levelgen.synth.Noise;

import java.io.PrintWriter;
import java.util.ArrayList;
import java.util.List;

/**
 * 26.3 noise oracle: dumps golden noise values for the Rust n263 port.
 * Compile against server263/versions/26.3/paper-26.3.jar (mojmap), run with Java 25.
 * Deterministic; writes reference/oracle263_noise.txt.
 */
public class NoiseOracle263 {
    public static void main(String[] args) throws Exception {
        net.minecraft.SharedConstants.tryDetectVersion();
        net.minecraft.server.Bootstrap.bootStrap();
        StringBuilder sb = new StringBuilder();
        long seed = 3257840388504953787L;

        // 1) PerlinNoise directly (octave-independent)
        XoroshiroRandomSource r = new XoroshiroRandomSource(seed);
        PerlinNoise perlin = new PerlinNoise(r);
        for (double[] c : new double[][]{{0,0,0},{1.25,2.5,3.75},{-7.5,63.25,1023.75},{0.001,-0.001,0.002},{12345.5,-6789.25,314.125}}) {
            sb.append("perlin ").append(c[0]).append(' ').append(c[1]).append(' ').append(c[2])
              .append(' ').append(perlin.get(c[0], c[1], c[2])).append('\n');
        }

        // 2) NormalNoise via Parameters (base_octave=-7, count=4, base_amplitude=1.7)
        XoroshiroRandomSource r2 = new XoroshiroRandomSource(seed);
        Noise nn = NormalNoise.builder()
                .setBaseOctave(-7).setOctaveCount(4).setBaseAmplitude(1.7).build().create(r2);
        for (double[] c : new double[][]{{0,0,0},{10.5,20.25,30.125},{-100.75,0.5,4096.125}}) {
            sb.append("normal ").append(c[0]).append(' ').append(c[1]).append(' ').append(c[2])
              .append(' ').append(nn.get(c[0], c[1], c[2])).append('\n');
        }

        // 3) NormalNoise parity-style (createParity, legacy normalization)
        XoroshiroRandomSource r3 = new XoroshiroRandomSource(seed);
        NormalNoise nnp = NormalNoise.createParity(-14, new double[]{1.0,1.0,1.0,1.0,1.0,1.0,1.0,1.0,1.0,1.0,1.0,1.0,1.0,1.0,1.0,1.0});
        Noise created3 = nnp.create(r3);
        for (double[] c : new double[][]{{0,0,0},{5.5,-5.5,55.5}}) {
            sb.append("normalparity ").append(c[0]).append(' ').append(c[1]).append(' ').append(c[2])
              .append(' ').append(created3.get(c[0], c[1], c[2])).append('\n');
        }

        // 4) SmearedPerlinNoise (BlendedNoise component)
        XoroshiroRandomSource r4 = new XoroshiroRandomSource(seed);
        SmearedPerlinNoise smeared = new SmearedPerlinNoise(r4, 4.277575000000001);
        for (double[] c : new double[][]{{0,0,0},{3.5,-0.5,9.25},{1.0,0.3,2.0}}) {
            sb.append("smeared ").append(c[0]).append(' ').append(c[1]).append(' ').append(c[2])
              .append(' ').append(smeared.get(c[0], c[1], c[2])).append('\n');
        }

        // 5) BlendedNoise (terrain seed): use the record's createFbmSet via compileSampler path
        XoroshiroRandomSource r5 = new XoroshiroRandomSource(seed);
        BlendedNoise blended = new BlendedNoise(1.0, 1.0, 80.0, 160.0, 1.0);
        // sample through compileSampler against a trivial context is complex;
        // instead replicate evaluation pieces: createFbmSet with the SAME random
        // and evaluate min/max/main stacks at scaled coords.
        BlendedNoise.FbmSet fbms = blended.createFbmSet(r5);
        double xzM = 684.412, yM = 684.412;
        for (double[] c : new double[][]{{0.5,0.25,0.75},{-3.25,1.5,2.75}}) {
            double x = c[0], y = c[1], z = c[2];
            float min = fbms.minLimitNoise().get(x * xzM, y * yM, z);
            float max = fbms.maxLimitNoise().get(x * xzM, y * yM, z);
            float main = fbms.mainNoise().get(x * xzM / 80.0, y * yM / 160.0, z);
            sb.append("blended ").append(x).append(' ').append(y).append(' ').append(z)
              .append(' ').append(min).append(' ').append(max).append(' ').append(main).append('\n');
        }

        // 6) chained forkPositional semantics: forks consume two nextLongs each
        XoroshiroRandomSource r6 = new XoroshiroRandomSource(seed);
        var f1 = r6.forkPositional();
        var f2 = r6.forkPositional();
        XoroshiroRandomSource a1 = (XoroshiroRandomSource) f1.fromHashOf("octave_-7");
        XoroshiroRandomSource b1 = (XoroshiroRandomSource) f2.fromHashOf("octave_-7");
        sb.append("forkA first ").append(a1.nextLong()).append('\n');
        sb.append("forkB first ").append(b1.nextLong()).append('\n');

        try (PrintWriter out = new PrintWriter("/home/z/my-project/c-faster/reference/oracle263_noise.txt")) {
            out.print(sb);
        }
        System.out.println("oracle done: " + sb.length() + " chars");
    }
}
