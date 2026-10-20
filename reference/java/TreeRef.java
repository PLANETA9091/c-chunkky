import java.lang.reflect.InvocationHandler;
import java.lang.reflect.Method;
import java.lang.reflect.Proxy;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.Optional;
import java.util.OptionalInt;
import java.util.function.Predicate;

import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.util.RandomSource;
import net.minecraft.util.valueproviders.UniformInt;
import net.minecraft.world.level.LevelAccessor;
import net.minecraft.world.level.LevelReader;
import net.minecraft.world.level.WorldGenLevel;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.Property;
import net.minecraft.world.level.levelgen.WorldgenRandom;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import net.minecraft.world.level.levelgen.feature.FeaturePlaceContext;
import net.minecraft.world.level.levelgen.feature.TreeFeature;
import net.minecraft.world.level.levelgen.feature.configurations.TreeConfiguration;
import net.minecraft.world.level.levelgen.feature.featuresize.TwoLayersFeatureSize;
import net.minecraft.world.level.levelgen.feature.foliageplacers.BlobFoliagePlacer;
import net.minecraft.world.level.levelgen.feature.foliageplacers.SpruceFoliagePlacer;
import net.minecraft.world.level.levelgen.feature.rootplacers.MangroveRootPlacer;
import net.minecraft.world.level.levelgen.feature.rootplacers.RootPlacer;
import net.minecraft.world.level.levelgen.feature.stateproviders.BlockStateProvider;
import net.minecraft.world.level.levelgen.feature.treedecorators.AlterGroundDecorator;
import net.minecraft.world.level.levelgen.feature.treedecorators.BeehiveDecorator;
import net.minecraft.world.level.levelgen.feature.treedecorators.CocoaDecorator;
import net.minecraft.world.level.levelgen.feature.treedecorators.LeaveVineDecorator;
import net.minecraft.world.level.levelgen.feature.treedecorators.TreeDecorator;
import net.minecraft.world.level.levelgen.feature.trunkplacers.StraightTrunkPlacer;
import net.minecraft.world.level.levelgen.feature.trunkplacers.ForkingTrunkPlacer;
import net.minecraft.world.level.levelgen.feature.trunkplacers.GiantTrunkPlacer;
import net.minecraft.world.level.levelgen.feature.trunkplacers.FancyTrunkPlacer;
import net.minecraft.world.level.levelgen.heightproviders.UniformHeight;
import net.minecraft.world.level.levelgen.placement.CaveSurface;

/**
 * Tree golden ground truth: runs the REAL vanilla TreeFeature (+ placers +
 * decorators) against a HashMap-backed WorldGenLevel proxy, printing every
 * setBlock as "x y z state". The native engine must reproduce this exactly
 * (incl. java.util.HashSet iteration order inside the decorators).
 */
public class TreeRef {
    static final Map<BlockPos, BlockState> WORLD = new HashMap<>();
    static final StringBuilder OUT = new StringBuilder();

    static String stateStr(BlockState st) {
        StringBuilder sb = new StringBuilder(
                net.minecraft.core.registries.BuiltInRegistries.BLOCK.getKey(st.getBlock()).toString());
        var props = st.getProperties();
        if (!props.isEmpty()) {
            sb.append('[');
            boolean first = true;
            for (Property<?> p : props) {
                if (!first) sb.append(',');
                first = false;
                sb.append(p.getName()).append('=').append(st.getValue(p).toString());
            }
            sb.append(']');
        }
        return sb.toString();
    }

    static WorldGenLevel mockLevel() {
        Class<?>[] ifaces = findIfaces();
        return (WorldGenLevel) Proxy.newProxyInstance(TreeRef.class.getClassLoader(), ifaces,
                new InvocationHandler() {
                    @Override
                    public Object invoke(Object proxy, Method m, Object[] a) {
                        switch (m.getName()) {
                            case "getBlockState": {
                                BlockPos p = (BlockPos) a[0];
                                BlockState st = WORLD.get(p);
                                return st != null ? st : Blocks.AIR.defaultBlockState();
                            }
                            case "setBlock": {
                                BlockPos p = (BlockPos) a[0];
                                BlockState st = (BlockState) a[1];
                                WORLD.put(new BlockPos(p.getX(), p.getY(), p.getZ()), st);
                                OUT.append("W ").append(p.getX()).append(' ').append(p.getY())
                                        .append(' ').append(p.getZ()).append(' ')
                                        .append(stateStr(st)).append('\n');
                                return true;
                            }
                            case "isStateAtPosition": {
                                BlockPos p = (BlockPos) a[0];
                                @SuppressWarnings("unchecked")
                                Predicate<BlockState> pred = (Predicate<BlockState>) a[1];
                                BlockState st = WORLD.get(p);
                                return pred.test(st != null ? st : Blocks.AIR.defaultBlockState());
                            }
                            case "isFluidAtPosition": {
                                return false; // dry mock world
                            }
                            case "isAir": {
                                BlockPos p = (BlockPos) a[0];
                                BlockState st = WORLD.get(p);
                                return st == null || st.isAir();
                            }
                            case "getMinY":
                            case "getMinBuildHeight":
                                return -64;
                            case "getMaxY":
                                return 319;
                            case "getMaxBuildHeight":
                            case "getHeight":
                                return 320;
                            case "getHeightmapPos": {
                                // MOTION_BLOCKING_NO_LEAVES scan over the mock
                                BlockPos p = (BlockPos) a[1];
                                for (int y = 318; y >= -64; y--) {
                                    BlockState st = WORLD.get(new BlockPos(p.getX(), y, p.getZ()));
                                    boolean hit = st != null && !st.isAir();
                                    if (hit) return new BlockPos(p.getX(), y + 1, p.getZ());
                                }
                                return new BlockPos(p.getX(), -64, p.getZ());
                            }
                            case "ensureCanWrite":
                                return true;
                            case "getRandom":
                                return RANDOM;
                            case "getBlockEntity":
                                return java.util.Optional.empty();
                            case "getFluidState":
                                return net.minecraft.world.level.material.Fluids.EMPTY.defaultFluidState();
                            case "hashCode":
                                return System.identityHashCode(proxy);
                            case "equals":
                                return proxy == a[0];
                            case "toString":
                                return "TreeRefMock";
                            default:
                                Class<?> rt = m.getReturnType();
                                if (rt == boolean.class) return false;
                                if (rt == int.class) return 0;
                                if (rt == long.class) return 0L;
                                if (rt == float.class) return 0f;
                                if (rt == double.class) return 0d;
                                return null;
                        }
                    }
                });
    }

    static Class<?>[] findIfaces() {
        // collect the full WorldGenLevel interface closure
        java.util.LinkedHashSet<Class<?>> set = new java.util.LinkedHashSet<>();
        java.util.ArrayDeque<Class<?>> q = new java.util.ArrayDeque<>();
        q.add(WorldGenLevel.class);
        while (!q.isEmpty()) {
            Class<?> c = q.poll();
            if (!c.isInterface()) continue;
            if (set.add(c)) {
                for (Class<?> i : c.getInterfaces()) q.add(i);
            }
        }
        return set.toArray(new Class<?>[0]);
    }

    static WorldgenRandom RANDOM;

    static void run(String tag, TreeConfiguration cfg, int seed, int ox, int oy, int oz) {
        WORLD.clear();
        RANDOM = new WorldgenRandom(new XoroshiroRandomSource(0L));
        RANDOM.setSeed(seed);
        WorldGenLevel level = mockLevel();
        FeaturePlaceContext<TreeConfiguration> ctx = new FeaturePlaceContext<TreeConfiguration>(
                Optional.empty(), level, null, RANDOM, new BlockPos(ox, oy, oz), cfg);
        boolean ok = new TreeFeature(TreeConfiguration.CODEC).place(ctx);
        OUT.append("R ").append(tag).append(" ok=").append(ok).append('\n');
    }

    public static void main(String[] args) {
        net.minecraft.SharedConstants.tryDetectVersion();
        net.minecraft.server.Bootstrap.bootStrap();
        int seed = Integer.parseInt(args[0]);
        Block oakLog = Blocks.OAK_LOG;
        Block oakLeaves = Blocks.OAK_LEAVES;

        // straight oak + leave_vine (HashSet order ground truth)
        run("straight_vine", new TreeConfiguration.TreeConfigurationBuilder(
                BlockStateProvider.simple(oakLog),
                new StraightTrunkPlacer(4, 2, 1),
                BlockStateProvider.simple(oakLeaves),
                new BlobFoliagePlacer(UniformInt.of(2, 3), UniformInt.of(0, 2), 3),
                new TwoLayersFeatureSize(1, 0, 1))
                .decorators(List.of(new LeaveVineDecorator(0.25f)))
                .build(), seed, 8, 64, 8);

        // straight oak + beehive + alter_ground (pale moss style ground writes
        // need a dirt-support mock: pre-place grass under the origin)
        WORLD.put(new BlockPos(8, 63, 8), Blocks.GRASS_BLOCK.defaultBlockState());
        run("straight_beehive", new TreeConfiguration.TreeConfigurationBuilder(
                BlockStateProvider.simple(oakLog),
                new StraightTrunkPlacer(4, 2, 1),
                BlockStateProvider.simple(oakLeaves),
                new BlobFoliagePlacer(UniformInt.of(2, 3), UniformInt.of(0, 2), 3),
                new TwoLayersFeatureSize(1, 0, 1))
                .decorators(List.of(new BeehiveDecorator(0.9f), new AlterGroundDecorator(
                        BlockStateProvider.simple(Blocks.PODZOL))))
                .build(), seed + 1, 8, 64, 8);

        // forking oak + cocoa (jungle-ish)
        WORLD.put(new BlockPos(8, 63, 8), Blocks.GRASS_BLOCK.defaultBlockState());
        run("forking", new TreeConfiguration.TreeConfigurationBuilder(
                BlockStateProvider.simple(oakLog),
                new ForkingTrunkPlacer(4, 2, 1),
                BlockStateProvider.simple(oakLeaves),
                new BlobFoliagePlacer(UniformInt.of(2, 3), UniformInt.of(0, 2), 3),
                new TwoLayersFeatureSize(1, 0, 1))
                .decorators(List.of(new CocoaDecorator(0.9f)))
                .build(), seed + 2, 8, 64, 8);

        // fancy oak (limbs) + leave_vine
        WORLD.put(new BlockPos(8, 63, 8), Blocks.GRASS_BLOCK.defaultBlockState());
        run("fancy", new TreeConfiguration.TreeConfigurationBuilder(
                BlockStateProvider.simple(oakLog),
                new FancyTrunkPlacer(4, 11, 0),
                BlockStateProvider.simple(oakLeaves),
                new net.minecraft.world.level.levelgen.feature.foliageplacers.FancyFoliagePlacer(
                        UniformInt.of(2, 3), UniformInt.of(0, 2), 4),
                new TwoLayersFeatureSize(0, 0, 0, java.util.OptionalInt.of(4)))
                .decorators(List.of(new LeaveVineDecorator(0.25f)))
                .build(), seed + 3, 8, 64, 8);

        // giant spruce-ish (2x2 trunk)
        WORLD.put(new BlockPos(8, 63, 8), Blocks.DIRT.defaultBlockState());
        run("giant", new TreeConfiguration.TreeConfigurationBuilder(
                BlockStateProvider.simple(Blocks.SPRUCE_LOG),
                new GiantTrunkPlacer(10, 2, 1),
                BlockStateProvider.simple(Blocks.SPRUCE_LEAVES),
                new net.minecraft.world.level.levelgen.feature.foliageplacers.MegaPineFoliagePlacer(
                        UniformInt.of(2, 3), UniformInt.of(0, 2), UniformInt.of(5, 7)),
                new TwoLayersFeatureSize(1, 1, 2))
                .build(), seed + 4, 8, 64, 8);

        System.out.print(OUT);
    }
}
