package com.planeta.crussty.oracle;

import net.minecraft.core.Holder;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.levelgen.Aquifer;
import net.minecraft.world.level.levelgen.DensityFunctions;
import net.minecraft.world.level.levelgen.NoiseChunk;
import net.minecraft.world.level.levelgen.NoiseGeneratorSettings;
import net.minecraft.world.level.levelgen.RandomState;
import org.bukkit.Bukkit;
import org.bukkit.craftbukkit.CraftServer;
import net.minecraft.world.level.levelgen.NoiseBasedChunkGenerator;
import org.bukkit.plugin.java.JavaPlugin;

import java.io.BufferedWriter;
import java.lang.reflect.Field;
import java.lang.reflect.Method;
import java.nio.file.Files;
import java.nio.file.Path;

/**
 * Runs the VANILLA NoiseChunk fill loop for a few chunks (empty beardifier,
 * matching the native takeover) and dumps every non-air block as
 * "x y z blockkey". Direct 1:1 comparison against the native fill.
 */
public final class OracleNoiseStagePlugin extends JavaPlugin {

    private static final int[] CHUNKS = {0, 1, -1};

    @Override
    public void onEnable() {
        try {
            run();
        } catch (Exception e) {
            throw new RuntimeException(e);
        }
        getLogger().info("OracleNoiseStage done");
    }

    private void run() throws Exception {
        CraftServer cs = (CraftServer) Bukkit.getServer();
        Object mcServer = cs.getServer();
        Object level = mcServer.getClass().getMethod("overworld").invoke(mcServer);
        Object chunkSource = level.getClass().getMethod("getChunkSource").invoke(level);
        RandomState state = (RandomState) chunkSource.getClass().getMethod("randomState").invoke(chunkSource);

        // NoiseBasedChunkGenerator: chunkMap.generator (CraftBukkit wraps the
        // vanilla generator in CustomChunkGenerator -> recurse to the delegate)
        Object chunkMap = null;
        for (Class<?> k = chunkSource.getClass(); k != null && chunkMap == null; k = k.getSuperclass()) {
            try {
                Field cf = k.getDeclaredField("chunkMap");
                cf.setAccessible(true);
                chunkMap = cf.get(chunkSource);
            } catch (NoSuchFieldException ignored) {
            }
        }
        if (chunkMap == null) throw new IllegalStateException("no chunkMap");
        Object generator = chunkMap.getClass().getDeclaredField("generator").get(chunkMap);
        Object nbcgObj = findInside(generator, "NoiseBasedChunkGenerator", 0);
        if (nbcgObj == null) throw new IllegalStateException("noise generator not found under "
                + generator.getClass().getName());
        NoiseBasedChunkGenerator nbcg = (NoiseBasedChunkGenerator) nbcgObj;
        Field settingsF = NoiseBasedChunkGenerator.class.getDeclaredField("settings");
        settingsF.setAccessible(true);
        @SuppressWarnings("unchecked")
        Holder<NoiseGeneratorSettings> settingsHolder = (Holder<NoiseGeneratorSettings>) settingsF.get(nbcg);
        // Holder.Reference caches the value at bind time — read the field
        // directly (value() would re-resolve through a registry we can't see)
        NoiseGeneratorSettings settings = null;
        for (Class<?> k = settingsHolder.getClass(); k != null && settings == null; k = k.getSuperclass()) {
            for (Field f : k.getDeclaredFields()) {
                if (f.getType() == NoiseGeneratorSettings.class
                        || f.getType().getSimpleName().equals("Object")) {
                    try {
                        f.setAccessible(true);
                        Object v = f.get(settingsHolder);
                        if (v instanceof NoiseGeneratorSettings ngs) {
                            settings = ngs;
                            break;
                        }
                    } catch (Exception ignored) {
                    }
                }
            }
        }
        if (settings == null) throw new IllegalStateException("settings value not in holder");
        getLogger().info("settings ok: seaLevel=" + settings.seaLevel()
                + " default=" + settings.defaultBlock());

        int seaLevel = settings.seaLevel();
        var defaultFluid = settings.defaultFluid();

        Aquifer.FluidPicker fluidPicker = (x, y, z) -> {
            var lava = new Aquifer.FluidStatus(-54, Blocks.LAVA.defaultBlockState());
            var water = new Aquifer.FluidStatus(seaLevel, defaultFluid);
            return y < Math.min(-54, seaLevel) ? lava : water;
        };

        Path dir = Path.of("oracle_noisestage");
        Files.createDirectories(dir);

        for (int cx : CHUNKS) {
            for (int cz : CHUNKS) {
                Object chunkAccess = level.getClass()
                        .getMethod("getChunk", int.class, int.class).invoke(level, cx, cz);
                Object beardifier = Class
                        .forName("net.minecraft.world.level.levelgen.DensityFunctions$BeardifierMarker")
                        .getDeclaredFields()[0].get(null); // enum INSTANCE
                Method forChunk = NoiseChunk.class.getMethod("forChunk",
                        Class.forName("net.minecraft.world.level.levelgen.ChunkAccess"),
                        RandomState.class,
                        Class.forName("net.minecraft.world.level.levelgen.DensityFunctions$BeardifierOrMarker"),
                        NoiseGeneratorSettings.class,
                        Aquifer.FluidPicker.class,
                        Class.forName("net.minecraft.world.level.levelgen.blending.Blender"));
                NoiseChunk nc = (NoiseChunk) forChunk.invoke(null,
                        chunkAccess, state, beardifier, settings, fluidPicker,
                        Class.forName("net.minecraft.world.level.levelgen.blending.Blender")
                                .getMethod("empty").invoke(null));

                Path out = dir.resolve(String.format("chunk_%d_%d.txt", cx, cz));
                try (BufferedWriter w = Files.newBufferedWriter(out)) {
                    fillLoop(w, nc, settings, cx, cz);
                }
                getLogger().info("noise stage dumped chunk " + cx + "," + cz);
            }
        }
    }

    private static Object registryKeyStatic() throws Exception {
        return Class.forName("net.minecraft.core.registries.Registries").getField("NOISE_SETTINGS").get(null);
    }

    private static Object elementKeyStatic() throws Exception {
        Class<?> resKey = Class.forName("net.minecraft.resources.ResourceKey");
        Class<?> resLoc = Class.forName("net.minecraft.resources.ResourceLocation");
        return resKey.getMethod("create", resKey, resLoc).invoke(null,
                registryKeyStatic(),
                resLoc.getMethod("withDefaultNamespace", String.class).invoke(null, "overworld"));
    }

    private static Object findInside(Object o, String simpleName, int depth) {
        if (o == null || depth > 6) return null;
        if (o.getClass().getName().endsWith("." + simpleName)) return o;
        for (Class<?> k = o.getClass(); k != null && k != Object.class; k = k.getSuperclass()) {
            for (Field f : k.getDeclaredFields()) {
                if (java.lang.reflect.Modifier.isStatic(f.getModifiers())) continue;
                String tn = f.getType().getName();
                if (!tn.startsWith("net.minecraft") && !tn.startsWith("org.bukkit")) continue;
                try {
                    f.setAccessible(true);
                    Object r = findInside(f.get(o), simpleName, depth + 1);
                    if (r != null) return r;
                } catch (Exception ignored) {
                }
            }
        }
        return null;
    }

    /** Decompiled doFill block loop (no sections/heightmaps — text dump). */
    private void fillLoop(BufferedWriter w, NoiseChunk noiseChunk, NoiseGeneratorSettings settings,
                          int cx, int cz) throws Exception {
        Method getInterpolatedState = NoiseChunk.class.getDeclaredMethod("getInterpolatedState");
        getInterpolatedState.setAccessible(true);
        Method cellWidth = NoiseChunk.class.getDeclaredMethod("cellWidth");
        cellWidth.setAccessible(true);
        Method cellHeight = NoiseChunk.class.getDeclaredMethod("cellHeight");
        cellHeight.setAccessible(true);
        int cw = (int) cellWidth.invoke(noiseChunk);
        int ch = (int) cellHeight.invoke(noiseChunk);

        BlockState air = Blocks.AIR.defaultBlockState();
        BlockState defaultBlock = settings.defaultBlock();

        int minBlockX = cx * 16;
        int minBlockZ = cz * 16;
        var noiseSettings = settings.noiseSettings();
        int minY = noiseSettings.minY();
        int height = noiseSettings.height();
        int minCellY = net.minecraft.util.Mth.floorDiv(minY, ch);
        int cellCountY = net.minecraft.util.Mth.floorDiv(height, ch);
        int i = 16 / cw;

        noiseChunk.initializeForFirstCellX();
        for (int i2 = 0; i2 < i; i2++) {
            noiseChunk.advanceCellX(i2);
            for (int i3 = 0; i3 < i; i3++) {
                for (int i5 = cellCountY - 1; i5 >= 0; i5--) {
                    noiseChunk.selectCellYZ(i5, i3);
                    for (int i6 = ch - 1; i6 >= 0; i6--) {
                        int i7 = (minCellY + i5) * ch + i6;
                        double d = (double) i6 / ch;
                        noiseChunk.updateForY(i7, d);
                        for (int i9 = 0; i9 < cw; i9++) {
                            int i10 = minBlockX + i2 * cw + i9;
                            double d1 = (double) i9 / cw;
                            noiseChunk.updateForX(i10, d1);
                            for (int i12 = 0; i12 < cw; i12++) {
                                int i13 = minBlockZ + i3 * cw + i12;
                                double d2 = (double) i12 / cw;
                                noiseChunk.updateForZ(i13, d2);
                                BlockState st = (BlockState) getInterpolatedState.invoke(noiseChunk);
                                if (st == null) {
                                    st = defaultBlock;
                                }
                                if (st != air) {
                                    w.write(i10 + " " + i7 + " " + i13 + " "
                                            + BuiltInRegistries.BLOCK.getKey(st.getBlock()) + "\n");
                                }
                            }
                        }
                    }
                }
            }
            noiseChunk.swapSlices();
        }
        noiseChunk.stopInterpolation();
    }
}
