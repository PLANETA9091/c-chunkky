import com.google.gson.JsonParser;
import com.mojang.serialization.JsonOps;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.Holder;
import net.minecraft.core.HolderGetter;
import net.minecraft.core.HolderLookup;
import net.minecraft.core.HolderOwner;
import net.minecraft.core.HolderSet;
import net.minecraft.resources.ResourceKey;
import net.minecraft.core.Registry;
import net.minecraft.resources.Identifier;
import net.minecraft.tags.TagKey;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.levelgen.NoiseRouter;
import net.minecraft.world.level.levelgen.RandomState;
import net.minecraft.world.level.levelgen.densityfunction.DensityBuffer;
import net.minecraft.world.level.levelgen.densityfunction.DensityFunction;
import net.minecraft.world.level.levelgen.densityfunction.DensitySampler;
import net.minecraft.world.level.levelgen.densityfunction.DensityVolume;
import net.minecraft.world.level.levelgen.densityfunction.SamplerContext;
import net.minecraft.world.level.levelgen.synth.NormalNoise;

import java.io.PrintWriter;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.HashMap;
import java.util.Map;
import java.util.Optional;
import java.util.Random;
import java.util.stream.Stream;

/**
 * 26.3 density-function ROUTER oracle v2.
 * Vanilla 26.3 DFs live in the datapack registry worldgen/density_function (55 JSONs).
 * We parse them with the REAL codecs (DensityFunction.CODEC / NormalNoise.CODEC) through
 * HolderLookup.Provider, then compile via RandomState exactly like the server and dump
 * point grids + full-chunk volume buffers (f32 bits) for the Rust df263 goldens.
 */
public class RouterOracle263 {
    static final long SEED = 3257840388504953787L;
    static final String DATA = "/home/z/my-project/c-faster/reference/vanilla263/data/minecraft/worldgen";

    // ---- minimal data-driven registry lookups (standalone references, late-bound) ----
    static final class MapLookup<T> implements HolderLookup.RegistryLookup<T> {
        final ResourceKey<? extends Registry<? extends T>> registryKey;
        final Map<ResourceKey<T>, Holder.Reference<T>> refs = new HashMap<>();
        final Map<ResourceKey<T>, T> values = new HashMap<>();

        MapLookup(ResourceKey<? extends Registry<? extends T>> registryKey) { this.registryKey = registryKey; }

        @SuppressWarnings("unchecked")
        Holder.Reference<T> stub(ResourceKey<T> key) {
            return refs.computeIfAbsent(key, k -> Holder.Reference.createStandAlone((HolderOwner<T>) this, k));
        }

        void bind(ResourceKey<T> key, T value) {
            values.put(key, value);
            Holder.Reference<T> ref = stub(key);
            try {
                var m = Holder.Reference.class.getDeclaredMethod("bindValue", Object.class);
                m.setAccessible(true);
                m.invoke(ref, value);
            } catch (Exception e) { throw new RuntimeException("bind " + key, e); }
        }

        @Override public Optional<T> getValueForCopying(ResourceKey<T> key) { return Optional.ofNullable(values.get(key)); }
        @Override public ResourceKey<? extends Registry<? extends T>> key() { return registryKey; }
        @Override public com.mojang.serialization.Lifecycle registryLifecycle() { return com.mojang.serialization.Lifecycle.stable(); }
        @Override public Optional<Holder.Reference<T>> get(ResourceKey<T> key) { return Optional.ofNullable(refs.get(key)); }
        @Override public Optional<HolderSet.Named<T>> get(TagKey<T> tag) { return Optional.empty(); }
        @Override public Stream<Holder.Reference<T>> listElements() { return refs.values().stream(); }
        @Override public Stream<HolderSet.Named<T>> listTags() { return Stream.empty(); }
    }

    interface Owner<T> extends HolderLookup.RegistryLookup<T>, net.minecraft.core.HolderOwner<T> {}

    @SuppressWarnings("unchecked")
    static <T> ResourceKey<T> rk(ResourceKey<? extends Registry<? extends T>> reg, String id) {
        return ResourceKey.create((ResourceKey<? extends Registry<T>>) (ResourceKey<?>) reg, Identifier.withDefaultNamespace(id));
    }

    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();

        var noiseRegKey = net.minecraft.core.registries.Registries.NOISE;
        var dfRegKey = net.minecraft.core.registries.Registries.DENSITY_FUNCTION;

        MapLookup<NormalNoise> noises = new MapLookup<>(noiseRegKey);
        MapLookup<DensityFunction> dfs = new MapLookup<>(dfRegKey);

        // pre-stub every DF id found on disk (lazy recursion: refs bind after parse)
        try (var files = Files.walk(Path.of(DATA, "density_function"))) {
            files.filter(p -> p.toString().endsWith(".json")).forEach(p -> {
                String rel = Path.of(DATA, "density_function").relativize(p).toString()
                        .replace('\\', '/').replaceAll("\\.json$", "");
                dfs.stub(rk(dfRegKey, rel));
            });
        }
        // parse noises
        try (var files = Files.walk(Path.of(DATA, "noise"))) {
            files.filter(p -> p.toString().endsWith(".json")).forEach(p -> {
                String rel = Path.of(DATA, "noise").relativize(p).toString()
                        .replace('\\', '/').replaceAll("\\.json$", "");
                try {
                    var json = JsonParser.parseString(Files.readString(p));
                    var holder = NormalNoise.CODEC.parse(ops(dfs, noises), json).getOrThrow();
                    noises.bind(rk(noiseRegKey, rel), holder.value());
                } catch (Exception e) { throw new RuntimeException("noise " + rel, e); }
            });
        }
        System.out.println("noises parsed: " + noises.values.size());
        // parse DFs
        try (var files = Files.walk(Path.of(DATA, "density_function"))) {
            files.filter(p -> p.toString().endsWith(".json")).forEach(p -> {
                String rel = Path.of(DATA, "density_function").relativize(p).toString()
                        .replace('\\', '/').replaceAll("\\.json$", "");
                try {
                    var json = JsonParser.parseString(Files.readString(p));
                    var df = DensityFunction.CODEC.parse(ops(dfs, noises), json).getOrThrow();
                    dfs.bind(rk(dfRegKey, rel), df);
                } catch (Exception e) { throw new RuntimeException("df " + rel, e); }
            });
        }
        System.out.println("density functions parsed: " + dfs.values.size());

        NoiseRouter router = new NoiseRouter(
                dfs.values.get(rk(dfRegKey, "overworld/temperature")),
                dfs.values.get(rk(dfRegKey, "overworld/vegetation")),
                dfs.values.get(rk(dfRegKey, "overworld/continents")),
                dfs.values.get(rk(dfRegKey, "overworld/erosion")),
                dfs.values.get(rk(dfRegKey, "overworld/depth")),
                dfs.values.get(rk(dfRegKey, "overworld/ridges")),
                dfs.values.get(rk(dfRegKey, "overworld/chunk_surface_level")),
                dfs.values.get(rk(dfRegKey, "overworld/final_density")));
        RandomState rs = RandomState.create((HolderGetter<NormalNoise>) noises, SEED, false,
                Blocks.STONE.defaultBlockState(), 63, router);

        DensityFunction[] fns = router.finalDensity() == null ? new DensityFunction[0] : new DensityFunction[]{
                router.temperature(), router.vegetation(), router.continents(), router.erosion(), router.depth(),
                router.ridges(), dfs.values.get(rk(dfRegKey, "overworld/offset")),
                dfs.values.get(rk(dfRegKey, "overworld/factor")),
                dfs.values.get(rk(dfRegKey, "overworld/jaggedness")),
                dfs.values.get(rk(dfRegKey, "overworld/base_3d_noise")),
                dfs.values.get(rk(dfRegKey, "overworld/sloped_cheese")),
                dfs.values.get(rk(dfRegKey, "overworld/caves/noodle")),
                dfs.values.get(rk(dfRegKey, "overworld/caves/entrances")),
                dfs.values.get(rk(dfRegKey, "overworld/caves/spaghetti_2d")),
                dfs.values.get(rk(dfRegKey, "overworld/caves/pillars")),
                dfs.values.get(rk(dfRegKey, "overworld/caves/spaghetti_roughness_function")),
                router.chunkSurfaceLevel(), router.finalDensity()};
        String[] names = {"temperature", "vegetation", "continents", "erosion", "depth", "ridges",
                "offset", "factor", "jaggedness", "base_3d", "sloped_cheese",
                "noodle", "entrances", "spaghetti_2d", "pillars", "spaghetti_roughness",
                "chunk_surface_level", "final_density"};

        // ---- hand-built spaghetti_2d internals probe (mirrors NoiseRouterData.spaghetti2D) ----
        Holder<NormalNoise> spa2d = noises.get(rk(noiseRegKey, "spaghetti_2d")).get();
        Holder<NormalNoise> spa2dMod = noises.get(rk(noiseRegKey, "spaghetti_2d_modulator")).get();
        Holder<NormalNoise> spa2dThick = noises.get(rk(noiseRegKey, "spaghetti_2d_thickness")).get();
        Holder<NormalNoise> spa2dElev = noises.get(rk(noiseRegKey, "spaghetti_2d_elevation")).get();
        net.minecraft.world.level.levelgen.densityfunction.DensityFunctions dfu = null;
        java.util.function.BiFunction<net.minecraft.world.level.levelgen.densityfunction.DensityFunction, Holder<NormalNoise>, net.minecraft.world.level.levelgen.densityfunction.DensityFunction> wrap2 =
                (input, noiseHolder) -> {
                    var D = net.minecraft.world.level.levelgen.densityfunction.DensityFunctions.class;
                    java.util.List<net.minecraft.world.level.levelgen.densityfunction.DensityFunction> branches = new java.util.ArrayList<>();
                    for (float rarity : new float[]{0.5F, 0.75F, 1.0F, 2.0F, 3.0F}) {
                        branches.add(net.minecraft.world.level.levelgen.densityfunction.DensityFunctions
                                .noise(noiseHolder, 1.0 / (double) rarity, 1.0 / (double) rarity).mul(rarity));
                    }
                    return net.minecraft.world.level.levelgen.densityfunction.DensityFunctions
                            .intervalSelect(input, it.unimi.dsi.fastutil.floats.FloatList.of(new float[]{-0.75F, -0.5F, 0.5F, 0.75F}), branches).abs();
                };
        var spa2DCave = wrap2.apply(net.minecraft.world.level.levelgen.densityfunction.DensityFunctions.noise(spa2dMod, 2.0, 1.0), spa2d);
        var elevMod = net.minecraft.world.level.levelgen.densityfunction.DensityFunctions.mappedNoise(spa2dElev, 0.0, (float) Math.floorDiv(-64, 8), 8.0F);
        var thickMod = net.minecraft.world.level.levelgen.densityfunction.DensityFunctions.mappedNoise(spa2dThick, 2.0, 1.0, -0.6F, -1.3F);
        var sloped = net.minecraft.world.level.levelgen.densityfunction.DensityFunctions.add(
                net.minecraft.world.level.levelgen.densityfunction.DensityFunctions.cache(elevMod),
                net.minecraft.world.level.levelgen.densityfunction.DensityFunctions.yClampedGradient(-64, 320, 8.0F, -40.0F)).abs();
        var layerRidged = net.minecraft.world.level.levelgen.densityfunction.DensityFunctions.add(sloped, thickMod).cube();
        var caveNoise = net.minecraft.world.level.levelgen.densityfunction.DensityFunctions.add(spa2DCave, thickMod.mul(0.083F));
        StringBuilder sb = new StringBuilder(1 << 22);
        long lcg = 88172645463325252L;
        for (int f = 0; f < fns.length; f++) {
            DensitySampler s = rs.getSampler(fns[f]);
            for (int i = 0; i < 24; i++) {
                lcg ^= lcg << 13; lcg ^= lcg >>> 7; lcg ^= lcg << 17;
                int x = (int) ((lcg >>> 33) % 96) - 16;
                lcg ^= lcg << 13; lcg ^= lcg >>> 7; lcg ^= lcg << 17;
                int z = (int) ((lcg >>> 33) % 96) - 16;
                int[] ys = {-60, -16, 32, 80, 128, 200, 256, 318};
                int y = ys[i % ys.length];
                float v = s.sampleValue(SamplerContext.EMPTY_UNCACHED, x, y, z);
                sb.append("pt ").append(names[f]).append(' ').append(x).append(' ').append(y).append(' ')
                  .append(z).append(' ').append(Integer.toHexString(Float.floatToIntBits(v))).append('\n');
            }
        }

        // 1b) probe points for spaghetti_2d internals
        {
            var D = net.minecraft.world.level.levelgen.densityfunction.DensityFunctions.class;
            var slopedCh = dfs.values.get(rk(dfRegKey, "overworld/sloped_cheese"));
            var entrancesCh = dfs.values.get(rk(dfRegKey, "overworld/caves/entrances"));
            var spa2dCh = dfs.values.get(rk(dfRegKey, "overworld/caves/spaghetti_2d"));
            var roughCh = dfs.values.get(rk(dfRegKey, "overworld/caves/spaghetti_roughness_function"));
            var pillarsCh = dfs.values.get(rk(dfRegKey, "overworld/caves/pillars"));
            var noodleCh = dfs.values.get(rk(dfRegKey, "overworld/caves/noodle"));
            var caveLayer = noises.get(rk(noiseRegKey, "cave_layer")).get();
            var caveCheese = noises.get(rk(noiseRegKey, "cave_cheese")).get();
            var layerized = net.minecraft.world.level.levelgen.densityfunction.DensityFunctions.noise(caveLayer, 8.0, 1.0).square().mul(4.0F);
            var solidCheese = net.minecraft.world.level.levelgen.densityfunction.DensityFunctions.add(net.minecraft.world.level.levelgen.densityfunction.DensityFunctions.clamp(net.minecraft.world.level.levelgen.densityfunction.DensityFunctions.noise(caveCheese, 0.6666666666666666, 1.0).add(0.27F), -1.0F, 1.0F),
                    net.minecraft.world.level.levelgen.densityfunction.DensityFunctions.clamp(slopedCh.mul(-0.64F).add(1.5F), 0.0F, 0.5F));
            var baseCave = net.minecraft.world.level.levelgen.densityfunction.DensityFunctions.add(layerized, solidCheese);
            var subtractions = net.minecraft.world.level.levelgen.densityfunction.DensityFunctions.min(net.minecraft.world.level.levelgen.densityfunction.DensityFunctions.min(baseCave, entrancesCh), net.minecraft.world.level.levelgen.densityfunction.DensityFunctions.add(spa2dCh, roughCh));
            var pillarsChoice = net.minecraft.world.level.levelgen.densityfunction.DensityFunctions.rangeChoice(pillarsCh, -1000000.0F, 0.03F, net.minecraft.world.level.levelgen.densityfunction.DensityFunctions.constant(-1000000.0F), pillarsCh);
            var underground = net.minecraft.world.level.levelgen.densityfunction.DensityFunctions.max(subtractions, pillarsChoice);
            var surfaceWithEntrances = net.minecraft.world.level.levelgen.densityfunction.DensityFunctions.min(slopedCh, entrancesCh.mul(5.0F));
            var caves = net.minecraft.world.level.levelgen.densityfunction.DensityFunctions.rangeChoice(slopedCh, -1000000.0F, 1.5625F, surfaceWithEntrances, underground);
            // slide
            var topF = net.minecraft.world.level.levelgen.densityfunction.DensityFunctions.yClampedGradient(240, 256, 1.0F, 0.0F);
            var noiseValue = net.minecraft.world.level.levelgen.densityfunction.DensityFunctions.lerp(topF, net.minecraft.world.level.levelgen.densityfunction.DensityFunctions.constant(-0.078125F), caves);
            var botF = net.minecraft.world.level.levelgen.densityfunction.DensityFunctions.yClampedGradient(-64, -40, 0.0F, 1.0F);
            var slid = net.minecraft.world.level.levelgen.densityfunction.DensityFunctions.lerp(botF, net.minecraft.world.level.levelgen.densityfunction.DensityFunctions.constant(0.1171875F), noiseValue);
            var post = net.minecraft.world.level.levelgen.densityfunction.DensityFunctions.squeeze(net.minecraft.world.level.levelgen.densityfunction.DensityFunctions.interpolated(net.minecraft.world.level.levelgen.densityfunction.DensityFunctions.blendDensity(slid).mul(0.64F), 4, 8));
            var probes = new net.minecraft.world.level.levelgen.densityfunction.DensityFunction[]{
                    elevMod, thickMod, spa2DCave, sloped, layerRidged, caveNoise,
                    noodleCh, slopedCh, entrancesCh, spa2dCh, layerized, solidCheese, baseCave,
                    subtractions, pillarsChoice, underground, caves, slid, post};
            var pnames = new String[]{"elev", "thick", "wrap2d", "sloped", "layer_ridged", "cave_noise",
                    "noodle", "sloped_ch", "entrances_ch", "spa2d_ch", "layerized", "solid_cheese", "base_cave",
                    "subtractions", "pillars_choice", "underground", "caves", "slid", "post"};
            long lcg2 = 88172645463325252L;
            int[][] fixedPts = {{55, -16, 14}, {51, 32, 42}, {44, -16, 48}, {55, 32, 33}, {79, -60, 13}, {60, -16, -5}};
            for (int f = 0; f < probes.length; f++) {
                var s = rs.getSampler(probes[f]);
                for (int[] fp : fixedPts) {
                    float v = s.sampleValue(SamplerContext.EMPTY_UNCACHED, fp[0], fp[1], fp[2]);
                    sb.append("pt probe_").append(pnames[f]).append(' ').append(fp[0]).append(' ').append(fp[1]).append(' ')
                      .append(fp[2]).append(' ').append(Integer.toHexString(Float.floatToIntBits(v))).append('\n');
                }
                for (int i = 0; i < 24; i++) {
                    lcg2 ^= lcg2 << 13; lcg2 ^= lcg2 >>> 7; lcg2 ^= lcg2 << 17;
                    int x = (int) ((lcg2 >>> 33) % 96) - 16;
                    lcg2 ^= lcg2 << 13; lcg2 ^= lcg2 >>> 7; lcg2 ^= lcg2 << 17;
                    int z = (int) ((lcg2 >>> 33) % 96) - 16;
                    int[] ys = {-60, -16, 32, 80, 128, 200, 256, 318};
                    int y = ys[i % ys.length];
                    float v = s.sampleValue(SamplerContext.EMPTY_UNCACHED, x, y, z);
                    sb.append("pt probe_").append(pnames[f]).append(' ').append(x).append(' ').append(y).append(' ')
                      .append(z).append(' ').append(Integer.toHexString(Float.floatToIntBits(v))).append('\n');
                }
            }
        }

        int[][] chunks = {{0, 0}, {1, -2}};
        long volCount = 0;
        for (int f = 0; f < fns.length; f++) {
            DensitySampler s = rs.getSampler(fns[f]);
            for (int[] c : chunks) {
                DensityVolume vol = new DensityVolume(16, 384, 16, c[0] * 16, -64, c[1] * 16);
                SamplerContext ctx = SamplerContext.builder().enableCaches().build();
                DensityBuffer buf = DensityBuffer.createUnpooled(vol.size());
                s.sampleVolume(ctx, buf, vol);
                for (int i = 0; i < vol.size(); i++) {
                    sb.append("vol ").append(names[f]).append(' ').append(c[0]).append(' ').append(c[1])
                      .append(' ').append(i).append(' ')
                      .append(Integer.toHexString(Float.floatToIntBits(buf.get(i)))).append('\n');
                    volCount++;
                }
            }
        }

        try (PrintWriter out = new PrintWriter("/home/z/my-project/c-faster/reference/oracle263_df.txt")) {
            out.print(sb);
        }
        System.out.println("router oracle done: " + sb.length() + " chars, "
                + (fns.length * 24) + " points, " + volCount + " volume samples");
    }

    static com.mojang.serialization.DynamicOps<com.google.gson.JsonElement> ops(MapLookup<DensityFunction> dfs, MapLookup<NormalNoise> noises) {
        HolderLookup.Provider provider = HolderLookup.Provider.create(Stream.of(dfs, noises));
        return provider.createSerializationContext(JsonOps.INSTANCE);
    }
}
