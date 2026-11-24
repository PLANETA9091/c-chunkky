import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.levelgen.densityfunction.DensityVolume;
import net.minecraft.world.level.levelgen.densityfunction.DensitySamplerSet;
import net.minecraft.world.level.levelgen.densityfunction.SamplerContext;
import net.minecraft.world.level.levelgen.densityfunction.ScopedDensityBuffer;
import net.minecraft.util.context.ContextMap;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.HolderGetter;
import net.minecraft.world.level.levelgen.NoiseRouter;
import net.minecraft.world.level.levelgen.RandomState;
import net.minecraft.world.level.levelgen.Beardifier;
import net.minecraft.resources.Identifier;
import net.minecraft.world.level.block.state.BlockState;
import com.google.gson.JsonParser;
import net.minecraft.world.level.levelgen.synth.NormalNoise;
import net.minecraft.world.level.levelgen.densityfunction.DensityFunction;
import net.minecraft.world.level.levelgen.Aquifer;

import java.io.PrintWriter;

/** cont-151: real-vanilla aquifer substance decisions (density=0.0, the carve
 *  semantics) for a chunk's y-band — the A/B ground truth for the native
 *  Aquifer263. Reuses FillOracle263's registry plumbing. */
public class CarveSubstanceOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();

        int cx = Integer.parseInt(args[0]);
        int cz = Integer.parseInt(args[1]);
        int yLo = Integer.parseInt(args[2]);
        int yHi = Integer.parseInt(args[3]);

        var noiseRegKey = net.minecraft.core.registries.Registries.NOISE;
        var dfRegKey = net.minecraft.core.registries.Registries.DENSITY_FUNCTION;
        FillOracle263.MapLookup<NormalNoise> noises = new FillOracle263.MapLookup<>(noiseRegKey);
        FillOracle263.MapLookup<DensityFunction> dfs = new FillOracle263.MapLookup<>(dfRegKey);

        try (var files = java.nio.file.Files.walk(java.nio.file.Path.of(FillOracle263.DATA, "density_function"))) {
            files.filter(p -> p.toString().endsWith(".json")).forEach(p -> {
                String rel = java.nio.file.Path.of(FillOracle263.DATA, "density_function").relativize(p).toString()
                        .replace('\\', '/').replaceAll("\\.json$", "");
                dfs.stub(FillOracle263.rk(dfRegKey, rel));
            });
        }
        try (var files = java.nio.file.Files.walk(java.nio.file.Path.of(FillOracle263.DATA, "noise"))) {
            files.filter(p -> p.toString().endsWith(".json")).forEach(p -> {
                String rel = java.nio.file.Path.of(FillOracle263.DATA, "noise").relativize(p).toString()
                        .replace('\\', '/').replaceAll("\\.json$", "");
                try {
                    var json = com.google.gson.JsonParser.parseString(java.nio.file.Files.readString(p));
                    var holder = NormalNoise.CODEC.parse(FillOracle263.ops(dfs, noises), json).getOrThrow();
                    noises.bind(FillOracle263.rk(noiseRegKey, rel), holder.value());
                } catch (Exception e) { throw new RuntimeException("noise " + rel, e); }
            });
        }
        try (var files = java.nio.file.Files.walk(java.nio.file.Path.of(FillOracle263.DATA, "density_function"))) {
            files.filter(p -> p.toString().endsWith(".json")).forEach(p -> {
                String rel = java.nio.file.Path.of(FillOracle263.DATA, "density_function").relativize(p).toString()
                        .replace('\\', '/').replaceAll("\\.json$", "");
                try {
                    var json = com.google.gson.JsonParser.parseString(java.nio.file.Files.readString(p));
                    var df = DensityFunction.CODEC.parse(FillOracle263.ops(dfs, noises), json).getOrThrow();
                    dfs.bind(FillOracle263.rk(dfRegKey, rel), df);
                } catch (Exception e) { throw new RuntimeException("df " + rel, e); }
            });
        }

        NoiseRouter router = new NoiseRouter(
                dfs.values.get(FillOracle263.rk(dfRegKey, "overworld/temperature")),
                dfs.values.get(FillOracle263.rk(dfRegKey, "overworld/vegetation")),
                dfs.values.get(FillOracle263.rk(dfRegKey, "overworld/continents")),
                dfs.values.get(FillOracle263.rk(dfRegKey, "overworld/erosion")),
                dfs.values.get(FillOracle263.rk(dfRegKey, "overworld/depth")),
                dfs.values.get(FillOracle263.rk(dfRegKey, "overworld/ridges")),
                dfs.values.get(FillOracle263.rk(dfRegKey, "overworld/chunk_surface_level")),
                dfs.values.get(FillOracle263.rk(dfRegKey, "overworld/final_density")));
        RandomState rs = RandomState.create((HolderGetter<NormalNoise>) noises, FillOracle263.SEED, false,
                Blocks.STONE.defaultBlockState(), 63, router);

        var nsJson = com.google.gson.JsonParser.parseString(
                java.nio.file.Files.readString(java.nio.file.Path.of(FillOracle263.DATA, "noise_settings/overworld.json")));
        var aquifersJson = nsJson.getAsJsonObject().get("aquifers");
        Aquifer.Config aquiferConfig = Aquifer.Config.CODEC.parse(FillOracle263.ops(dfs, noises), aquifersJson).getOrThrow();

        DensityVolume vol = new DensityVolume(16, 384, 16, cx * 16, -64, cz * 16);
        ContextMap user = ContextMap.builder().set(Beardifier.CONTEXT_KEY, Beardifier.EMPTY).build();
        SamplerContext sctx = SamplerContext.builder().setUserFields(user).enableCaches().build();
        DensitySamplerSet samplers = rs.samplersWithContext(sctx);
        Aquifer aquifer = aquiferConfig.create(samplers,
                rs.getOrCreateRandomFactory(Identifier.withDefaultNamespace("aquifer")),
                vol, FillOracle263.FLUID_PICKER);

        StringBuilder sb = new StringBuilder(1 << 20);
        for (int lx = 0; lx < 16; lx++) {
            for (int lz = 0; lz < 16; lz++) {
                int wx = cx * 16 + lx;
                int wz = cz * 16 + lz;
                for (int wy = yLo; wy <= yHi; wy++) {
                    BlockState st = aquifer.computeSubstance(wx, wy, wz, 0.0);
                    String d;
                    if (st == null) d = "null";
                    else if (st.isAir()) d = "air";
                    else if (st.getBlock() == Blocks.WATER) d = "water";
                    else if (st.getBlock() == Blocks.LAVA) d = "lava";
                    else d = st.getBlock().toString();
                    sb.append(wx).append(' ').append(wy).append(' ').append(wz).append(' ').append(d).append('\n');
                }
            }
        }
        try (PrintWriter out = new PrintWriter("/home/z/my-project/c-faster/ab263_out_carvers7/vanilla_substance_" + cx + "_" + cz + ".txt")) {
            out.print(sb);
        }
        System.out.println("oracle done: " + ((yHi - yLo + 1) * 256) + " cells");
    }
}
