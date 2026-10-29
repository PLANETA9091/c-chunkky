import com.google.gson.JsonParser;
import com.mojang.serialization.JsonOps;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.Holder;
import net.minecraft.core.HolderGetter;
import net.minecraft.core.HolderLookup;
import net.minecraft.core.HolderOwner;
import net.minecraft.core.HolderSet;
import net.minecraft.core.BlockPos;
import net.minecraft.resources.ResourceKey;
import net.minecraft.core.Registry;
import net.minecraft.resources.Identifier;
import net.minecraft.tags.TagKey;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.levelgen.NoiseRouter;
import net.minecraft.world.level.levelgen.RandomState;
import net.minecraft.world.level.levelgen.Aquifer;
import net.minecraft.world.level.levelgen.Beardifier;
import net.minecraft.world.level.levelgen.densityfunction.DensityBuffer;
import net.minecraft.world.level.levelgen.densityfunction.DensityFunction;
import net.minecraft.world.level.levelgen.densityfunction.DensitySampler;
import net.minecraft.world.level.levelgen.densityfunction.DensitySamplerSet;
import net.minecraft.world.level.levelgen.densityfunction.DensityVolume;
import net.minecraft.world.level.levelgen.densityfunction.ScopedDensityBuffer;
import net.minecraft.world.level.levelgen.densityfunction.SamplerContext;
import net.minecraft.world.level.levelgen.synth.NormalNoise;
import net.minecraft.util.context.ContextMap;

import java.io.PrintWriter;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.HashMap;
import java.util.Map;
import java.util.Optional;
import java.util.stream.Stream;

/**
 * 26.3 doFill oracle — replicates NoiseBasedChunkGenerator.doFill exactly:
 * NoiseChunk semantics (cached SamplerContext + Beardifier user field),
 * final_density volume fill, aquifer computeSubstance, stone/water/lava
 * mapping, fluid-update marks. Dumps "fill cx cz idx id" (non-air cells,
 * volume index order) + "mark cx cz packedPos" for the Rust fill263 golden.
 */
public class FillOracle263 {
    static final long SEED = 3257840388504953787L;
    static final String DATA = "/home/z/my-project/c-faster/reference/vanilla263/data/minecraft/worldgen";

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

    static com.mojang.serialization.DynamicOps<com.google.gson.JsonElement> ops(MapLookup<DensityFunction> dfs, MapLookup<NormalNoise> noises) {
        HolderLookup.Provider provider = HolderLookup.Provider.create(Stream.of(dfs, noises));
        return provider.createSerializationContext(JsonOps.INSTANCE);
    }

    // createFluidPicker (NoiseBasedChunkGenerator 26.3): lava(-54) / sea(63, water)
    static final Aquifer.FluidPicker FLUID_PICKER = (x, y, z) -> {
        if (y < Math.min(-54, 63)) {
            return new Aquifer.FluidStatus(-54, Blocks.LAVA.defaultBlockState());
        }
        return new Aquifer.FluidStatus(63, Blocks.WATER.defaultBlockState());
    };

    static int blockId(BlockState st) {
        if (st.isAir()) return 0;
        if (st.getBlock() == Blocks.WATER) return 3;
        if (st.getBlock() == Blocks.LAVA) return 17;
        return 1; // default block: stone
    }

    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();

        var noiseRegKey = net.minecraft.core.registries.Registries.NOISE;
        var dfRegKey = net.minecraft.core.registries.Registries.DENSITY_FUNCTION;

        MapLookup<NormalNoise> noises = new MapLookup<>(noiseRegKey);
        MapLookup<DensityFunction> dfs = new MapLookup<>(dfRegKey);

        try (var files = Files.walk(Path.of(DATA, "density_function"))) {
            files.filter(p -> p.toString().endsWith(".json")).forEach(p -> {
                String rel = Path.of(DATA, "density_function").relativize(p).toString()
                        .replace('\\', '/').replaceAll("\\.json$", "");
                dfs.stub(rk(dfRegKey, rel));
            });
        }
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
        System.out.println("registries parsed: " + noises.values.size() + " noises, " + dfs.values.size() + " dfs");

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

        // aquifers section from noise_settings/overworld.json (real codecs)
        var nsJson = JsonParser.parseString(Files.readString(Path.of(DATA, "noise_settings/overworld.json")));
        var aquifersJson = nsJson.getAsJsonObject().get("aquifers");
        Aquifer.Config aquiferConfig = Aquifer.Config.CODEC.parse(ops(dfs, noises), aquifersJson).getOrThrow();
        System.out.println("aquifer config parsed");

        int[][] chunks = {{0, 0}, {1, -2}, {3, 1}, {-2, 3}};
        long written = 0;
        long marks = 0;
        StringBuilder sb = new StringBuilder(1 << 26);
        for (int[] c : chunks) {
            int cx = c[0], cz = c[1];
            DensityVolume vol = new DensityVolume(16, 384, 16, cx * 16, -64, cz * 16);
            ContextMap user = ContextMap.builder().set(Beardifier.CONTEXT_KEY, Beardifier.EMPTY).build();
            SamplerContext sctx = SamplerContext.builder().setUserFields(user).enableCaches().build();
            DensitySamplerSet samplers = rs.samplersWithContext(sctx);
            Aquifer aquifer = aquiferConfig.create(samplers,
                    rs.getOrCreateRandomFactory(Identifier.withDefaultNamespace("aquifer")),
                    vol, FLUID_PICKER);
            DensitySampler.Bound finalDensity = samplers.get(router.finalDensity());
            try (ScopedDensityBuffer densityBuffer = finalDensity.sampleVolume(vol)) {
                for (int z = 0; z < vol.sizeZ(); z++) {
                    int blockZ = vol.blockZ(z);
                    for (int x = 0; x < vol.sizeX(); x++) {
                        int blockX = vol.blockX(x);
                        for (int y = vol.sizeY() - 1; y >= 0; y--) {
                            int blockY = vol.blockY(y);
                            float density = densityBuffer.get(vol.indexUnchecked(x, y, z));
                            BlockState st = aquifer.computeSubstance(blockX, blockY, blockZ, (double) density);
                            if (st == null) {
                                st = Blocks.STONE.defaultBlockState();
                            }
                            if (st.isAir()) {
                                continue;
                            }
                            sb.append("fill ").append(cx).append(' ').append(cz).append(' ')
                              .append(vol.indexUnchecked(x, y, z)).append(' ').append(blockId(st)).append('\n');
                            written++;
                            if (aquifer.shouldScheduleFluidUpdate() && !st.getFluidState().isEmpty()) {
                                sb.append("mark ").append(cx).append(' ').append(cz).append(' ')
                                  .append(BlockPos.asLong(blockX, blockY, blockZ)).append('\n');
                                marks++;
                            }
                        }
                    }
                }
            }
        }

        try (PrintWriter out = new PrintWriter("/home/z/my-project/c-faster/reference/oracle263_fill.txt")) {
            out.print(sb);
        }
        System.out.println("fill oracle done: " + written + " written cells, " + marks + " marks, " + sb.length() + " chars");
    }
}
