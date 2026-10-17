package com.planeta.crussty.oracle;

import net.minecraft.core.Holder;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import it.unimi.dsi.fastutil.objects.ObjectArrayList;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.chunk.ChunkAccess;
import net.minecraft.world.level.StructureManager;
import net.minecraft.world.level.levelgen.Aquifer;
import net.minecraft.world.level.levelgen.Beardifier;
import net.minecraft.world.level.levelgen.DensityFunction;
import net.minecraft.world.level.levelgen.DensityFunctions;
import net.minecraft.world.level.levelgen.structure.StructurePiece;
import net.minecraft.world.level.levelgen.structure.StructureStart;
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
import java.util.ArrayList;
import java.util.List;

/**
 * Runs the VANILLA NoiseChunk fill loop for a few chunks (empty beardifier,
 * matching the native takeover) and dumps every non-air block as
 * "x y z blockkey". Direct 1:1 comparison against the native fill.
 *
 * ORACLE_MARKS=1 additionally dumps the aquifer fluid-update flags (vanilla
 * shouldScheduleFluidUpdate sampled per block) for every placed block with a
 * non-empty fluid state (or flag=true), plus the full aquifer status cache
 * (source locations + FluidStatus per grid cell) for cross-checking the
 * native port's status computation.
 */
public final class OracleNoiseStagePlugin extends JavaPlugin {

    private static final int[] CHUNKS = {0, 1, -1};
    private static final boolean MARKS = Boolean.getBoolean("oraclemarks");
    private static final boolean STATUSWALK = Boolean.getBoolean("oraclewalk");

    @Override
    public void onEnable() {
        // HARD GATE: this plugin evaluates vanilla density functions and
        // forces chunk generation at onEnable. If it ever boots WITHOUT the
        // explicit -Doraclemarks/-Doraclewalk flags (e.g. the jar left in
        // plugins/ when the judge server starts), that out-of-order
        // generation scrambles the world's decoration order — measured
        // ~5900 phantom judge diffs. Do nothing unless explicitly invoked.
        if (!MARKS && !STATUSWALK) {
            getLogger().info("OracleNoiseStage: no -Doraclemarks/-Doraclewalk — staying inert");
            return;
        }
        probeResolvedSettings();
        try {
            if (STATUSWALK) {
                statusWalk();
            } else {
                run();
            }
        } catch (Exception e) {
            throw new RuntimeException(e);
        }
        getLogger().info("OracleNoiseStage done");
    }

    /**
     * Prints the server-resolved worldgen settings for the overworld: the
     * noise router depth chain and the surface rule tree head — the ground
     * truth of WHICH pack's noise_settings/DF files actually won the
     * datapack layering (settles tectonic-vs-terralith empirically).
     */
    private void probeResolvedSettings() {
        try {
            CraftServer cs = (CraftServer) Bukkit.getServer();
            Object mcServer = cs.getServer();
            Object level = mcServer.getClass().getMethod("overworld").invoke(mcServer);
            Object chunkSource = level.getClass().getMethod("getChunkSource").invoke(level);
            Object gen = chunkSource.getClass().getMethod("getGenerator").invoke(chunkSource);
            Object settings = gen.getClass().getMethod("generatorSettings").invoke(gen);
            Object holder = settings instanceof Holder<?> h ? h : settings;
            Object value = holder.getClass().getMethod("value").invoke(holder);
            Object router = value.getClass().getMethod("noiseRouter").invoke(value);
            Object depth = router.getClass().getMethod("depth").invoke(router);
            Object rule = value.getClass().getMethod("surfaceRule").invoke(value);
            String ruleS = String.valueOf(rule);
            int terr = ruleS.split("terralith", -1).length - 1;
            int tect = ruleS.split("tectonic", -1).length - 1;
            getLogger().info("PROBE settings.class=" + value.getClass().getName());
            getLogger().info("PROBE seaLevel=" + value.getClass().getMethod("seaLevel").invoke(value));
            getLogger().info("PROBE ruleLen=" + ruleS.length() + " terralith_refs=" + terr + " tectonic_refs=" + tect);
            getLogger().info("PROBE depthDF=" + String.valueOf(depth).substring(0, Math.min(240, String.valueOf(depth).length())));
        } catch (Throwable t) {
            getLogger().info("PROBE failed: " + t);
        }
    }


    /**
     * Evaluates the server's REAL SurfaceRule on the noise-stage chunk at
     * (13,*,4) with the bytecode-verified walk semantics, logging per-y the
     * tryApply result + context fields. Ground truth for the native surface
     * engine divergence hunt (sand-vs-grass at the deep_lukewarm floor).
     */
    private void vanillaRuleProbe(Object chunkSource, ChunkAccess chunk, Class<?> accessClass) throws Exception {
        Class<?> surfaceRules = Class.forName("net.minecraft.world.level.levelgen.SurfaceRules");
        Class<?> ctxCls = Class.forName("net.minecraft.world.level.levelgen.SurfaceRules$Context");
        Class<?> ruleSrcCls = Class.forName("net.minecraft.world.level.levelgen.SurfaceRules$RuleSource");
        Class<?> ruleCls = Class.forName("net.minecraft.world.level.levelgen.SurfaceRules$SurfaceRule");

        RandomState state = (RandomState) chunkSource.getClass().getMethod("randomState").invoke(chunkSource);
        Object gen = chunkSource.getClass().getMethod("getGenerator").invoke(chunkSource);
        Object settingsHolder = gen.getClass().getMethod("generatorSettings").invoke(gen);
        Object settings = settingsHolder.getClass().getMethod("value").invoke(settingsHolder);
        Object surfaceSystem = state.surfaceSystem();

        // NoiseChunk for the above_preliminary / min-surface machinery
        Object beardifier = Class.forName(
                "net.minecraft.world.level.levelgen.DensityFunctions$BeardifierMarker")
                .getDeclaredFields()[0].get(null);
        Method forChunk = NoiseChunk.class.getMethod("forChunk",
                Class.forName("net.minecraft.world.level.chunk.ChunkAccess"),
                RandomState.class,
                Class.forName("net.minecraft.world.level.levelgen.DensityFunctions$BeardifierOrMarker"),
                NoiseGeneratorSettings.class,
                Aquifer.FluidPicker.class,
                Class.forName("net.minecraft.world.level.levelgen.blending.Blender"));
        int seaLevel = (Integer) settings.getClass().getMethod("seaLevel").invoke(settings);
        var defaultFluid = settings.getClass().getMethod("defaultFluid").invoke(settings);
        Aquifer.FluidPicker fluidPicker = (x, y, z) -> {
            var lava = new Aquifer.FluidStatus(-54, Blocks.LAVA.defaultBlockState());
            var water = new Aquifer.FluidStatus(seaLevel, Blocks.WATER.defaultBlockState());
            return y < Math.min(-54, seaLevel) ? lava : water;
        };
        Object nc = forChunk.invoke(null, chunk, state, beardifier, settings, fluidPicker,
                Class.forName("net.minecraft.world.level.levelgen.blending.Blender")
                        .getMethod("empty").invoke(null));

        Class<?> wgCtx = Class.forName("net.minecraft.world.level.levelgen.WorldGenerationContext");
        Object wgContext = wgCtx.getDeclaredConstructor(
                Class.forName("net.minecraft.world.level.chunk.ChunkGenerator"),
                Class.forName("net.minecraft.world.level.LevelHeightAccessor"))
                .newInstance(gen, chunk);
        java.util.function.Function<net.minecraft.core.BlockPos, net.minecraft.core.Holder<net.minecraft.world.level.biome.Biome>> biomeGetter =
                pos -> chunk.getNoiseBiome(pos.getX() >> 2, pos.getY() >> 2, pos.getZ() >> 2);
        Object registry = accessClass.getField("biomeRegistry").get(chunk);
        Object ctx = null;
        for (java.lang.reflect.Constructor<?> c : ctxCls.getDeclaredConstructors()) {
            if (c.getParameterCount() == 7) {
                c.setAccessible(true);
                try {
                    ctx = c.newInstance(surfaceSystem, state, chunk, nc, biomeGetter, registry, wgContext);
                    break;
                } catch (IllegalArgumentException iae) {
                    getLogger().info("VRULE ctor arg mismatch: " + iae + " types: ");
                    for (Class<?> pc : c.getParameterTypes()) getLogger().info("VRULE   param " + pc);
                }
            }
        }
        if (ctx == null) { getLogger().info("VRULE: no 7-arg Context ctor worked"); return; }

        Object ruleSource = settings.getClass().getMethod("surfaceRule").invoke(settings);
        java.util.function.Function<Object, Object> rf = (java.util.function.Function<Object, Object>) ruleSource;
        Object rule = rf.apply(ctx);
        getLogger().info("VRULE rule class=" + rule.getClass().getName());

        Method updateXZ = ctxCls.getDeclaredMethod("updateXZ", int.class, int.class);
        updateXZ.setAccessible(true);
        Method updateY = ctxCls.getDeclaredMethod("updateY", int.class, int.class, int.class, int.class, int.class, int.class);
        updateY.setAccessible(true);
        Method tryApply = ruleCls.getMethod("tryApply", int.class, int.class, int.class);
        Method getBlockState = accessClass.getMethod("getBlockState", net.minecraft.core.BlockPos.class);
        Method isAir = net.minecraft.world.level.block.state.BlockState.class.getMethod("isAir");
        Method getFluidState = net.minecraft.world.level.block.state.BlockState.class
                .getMethod("getFluidState");
        Method fluidEmpty = net.minecraft.world.level.material.FluidState.class.getMethod("isEmpty");
        Object defaultBlock = settings.getClass().getMethod("defaultBlock").invoke(settings);

        StringBuilder log = new StringBuilder();
        int x = 13, z = 4;
        updateXZ.invoke(ctx, x, z);
        int depth = 0, wh = Integer.MIN_VALUE, stoneBottom = Integer.MAX_VALUE;
        // deep-walk probe: replicate the vanilla surface column walk for an
        // arbitrary column and log context + rule results per block. Activated
        // with -Doracleprobe.col=x:z (chunk = the walked noise-status chunk).
        String probeCol = System.getProperty("oracleprobe.col", "");
        if (!probeCol.isBlank()) {
            String[] pp = probeCol.split(":");
            int px = Integer.parseInt(pp[0].trim());
            int pz = Integer.parseInt(pp[1].trim());
            updateXZ.invoke(ctx, px, pz);
            int pd = 0, pwh = Integer.MIN_VALUE, psb = Integer.MAX_VALUE;
            for (int y = chunk.getHeight() + chunk.getMinY() - 1; y >= chunk.getMinY(); y--) {
                var pos = new net.minecraft.core.BlockPos(px, y, pz);
                var b = getBlockState.invoke(chunk, pos);
                boolean air = (Boolean) isAir.invoke(b);
                boolean isFluid = !((Boolean) fluidEmpty.invoke(getFluidState.invoke(b)));
                if (air) {
                    pd = 0; pwh = Integer.MIN_VALUE;
                    getLogger().info(String.format("VWALK %d %d %d AIR", px, y, pz));
                    continue;
                }
                if (isFluid) {
                    if (pwh == Integer.MIN_VALUE) pwh = y + 1;
                    getLogger().info(String.format("VWALK %d %d %d FLUID wh=%d", px, y, pz, pwh));
                    continue;
                }
                if (psb >= y) {
                    psb = -32512;
                    for (int y2 = y - 1; y2 >= chunk.getMinY() - 1; y2--) {
                        if (y2 < chunk.getMinY()) { psb = y2 + 1; break; }
                        var b2 = getBlockState.invoke(chunk, new net.minecraft.core.BlockPos(px, y2, pz));
                        boolean air2 = (Boolean) isAir.invoke(b2);
                        boolean fe2 = (Boolean) fluidEmpty.invoke(getFluidState.invoke(b2));
                        if (air2 || !fe2) { psb = y2 + 1; break; }
                    }
                }
                pd++;
                int psdb = y - psb + 1;
                updateY.invoke(ctx, pd, psdb, pwh, px, y, pz);
                String st = null;
                if (b == defaultBlock || b.equals(defaultBlock)) {
                    Object r = tryApply.invoke(rule, px, y, pz);
                    if (r != null) st = String.valueOf(r);
                }
                getLogger().info(String.format("VWALK %d %d %d %s d=%d wh=%d sdb=%d state=%s",
                        px, y, pz, b, pd, pwh, psdb, st));
            }
            getLogger().info("VWALK done");
            return;
        }
        for (int y = 62; y >= 10; y--) {
            var pos = new net.minecraft.core.BlockPos(x, y, z);
            var b = getBlockState.invoke(chunk, pos);
            boolean air = (Boolean) isAir.invoke(b);
            boolean isFluid = !((Boolean) fluidEmpty.invoke(getFluidState.invoke(b)));
            String applied = null;
            if (air) {
                depth = 0; wh = Integer.MIN_VALUE;
            } else if (isFluid) {
                if (wh == Integer.MIN_VALUE) wh = y + 1;
            } else {
                if (stoneBottom >= y) {
                    stoneBottom = -32512;
                    for (int y2 = y - 1; y2 >= chunk.getMinY() - 1; y2--) {
                        if (y2 < chunk.getMinY()) { stoneBottom = y2 + 1; break; }
                        var b2 = getBlockState.invoke(chunk, new net.minecraft.core.BlockPos(x, y2, z));
                        boolean air2 = (Boolean) isAir.invoke(b2);
                        boolean fe2 = (Boolean) fluidEmpty.invoke(getFluidState.invoke(b2));
                        if (air2 || !fe2) { stoneBottom = y2 + 1; break; }
                    }
                }
                depth++;
                int sdb = y - stoneBottom + 1;
                updateY.invoke(ctx, depth, sdb, wh, x, y, z);
                if (b == defaultBlock || b.equals(defaultBlock)) {
                    Object st = tryApply.invoke(rule, x, y, z);
                    if (st != null) applied = String.valueOf(st);
                }
                if (y == 25) {
                    // dump the vanilla tree's matching path at this position
                    try {
                        StringBuilder p = new StringBuilder();
                        Object node = rule;
                        boolean first = true;
                        while (true) {
                            String cn = node.getClass().getSimpleName();
                            if (cn.equals("SequenceRule")) {
                                Field rf2 = node.getClass().getDeclaredField("rules");
                                rf2.setAccessible(true);
                                java.util.List<?> kids = (java.util.List<?>) rf2.get(node);
                                int mi = -1;
                                for (int i = 0; i < kids.size(); i++) {
                                    Object kid = kids.get(i);
                                    Method tam = kid.getClass().getMethod("tryApply", int.class, int.class, int.class);
                                    tam.setAccessible(true);
                                    Object r = tam.invoke(kid, x, y, z);
                                    if (r != null) { mi = i; break; }
                                }
                                p.append(first ? "seq" : "/seq").append('[').append(mi).append(']');
                                first = false;
                                if (mi < 0) break;
                                node = kids.get(mi);
                            } else if (cn.equals("TestRule")) {
                                Field cf2 = node.getClass().getDeclaredField("condition");
                                cf2.setAccessible(true);
                                Object cond = cf2.get(node);
                                Method tm = cond.getClass().getMethod("test");
                                tm.setAccessible(true);
                                boolean ct = (Boolean) tm.invoke(cond);
                                p.append("/cond{").append(ct).append("}");
                                Field ff2 = node.getClass().getDeclaredField("followup");
                                ff2.setAccessible(true);
                                node = ff2.get(node);
                            } else {
                                p.append("/").append(cn).append("=").append(node);
                                break;
                            }
                        }
                        getLogger().info("VRULE PATH y=25: " + p);
                    } catch (Throwable t2) {
                        getLogger().info("VRULE path failed: " + t2);
                    }
                }
                if (y <= 30) {
                    Object biome = "?";
                    try {
                        Field biomeF = ctxCls.getDeclaredField("biome");
                        biomeF.setAccessible(true);
                        Object sup = biomeF.get(ctx);
                        if (sup instanceof java.util.function.Supplier<?> s2) biome = s2.get();
                    } catch (Throwable ignored) {
                    }
                    log.append(String.format("y=%d b=%s depth=%d sdb=%d wh=%d biome=%s applied=%s; ",
                            y, String.valueOf(b).replace("Block{", "").replace("}", ""),
                            depth, sdb, wh, String.valueOf(biome),
                            applied == null ? "-" : applied));
                }
            }
        }
        getLogger().info("VRULE walk: " + log);
    }
    /**
     * Walks requested chunks through the vanilla generation statuses
     * (NOISE -> SURFACE -> CARVERS -> FEATURES -> FULL) and dumps the
     * ProtoChunk postProcessing lists at each status — the ground truth of
     * which stage marked which position. Marks are cumulative.
     */
    private void statusWalk() throws Exception {
        CraftServer cs = (CraftServer) Bukkit.getServer();
        Object mcServer = cs.getServer();
        Object level = mcServer.getClass().getMethod("overworld").invoke(mcServer);
        Object chunkSource = level.getClass().getMethod("getChunkSource").invoke(level);
        Class<?> scc = chunkSource.getClass();
        while (scc != null && scc != Object.class && scc.getSimpleName().isEmpty() == false
                && !scc.getName().endsWith("ServerChunkCache")) {
            scc = scc.getSuperclass();
        }
        Method getChunk = scc.getMethod("getChunk",
                int.class, int.class, Class.forName("net.minecraft.world.level.chunk.status.ChunkStatus"), boolean.class);
        Class<?> statusClass = Class.forName("net.minecraft.world.level.chunk.status.ChunkStatus");
        Object noise = statusClass.getField("NOISE").get(null);
        Object surface = statusClass.getField("SURFACE").get(null);
        Object carvers = statusClass.getField("CARVERS").get(null);
        Object features = statusClass.getField("FEATURES").get(null);
        Object full = statusClass.getField("FULL").get(null);

        Class<?> accessClass = Class.forName("net.minecraft.world.level.chunk.ChunkAccess");
        Method getPostProcessing = accessClass.getMethod("getPostProcessing");

        Path dir = Path.of("oracle_statuswalk");
        Files.createDirectories(dir);

        String spec = System.getProperty("oraclemarks.chunklist", "");
        List<int[]> chunkList = new ArrayList<>();
        if (!spec.isBlank()) {
            for (String pair : spec.split(",")) {
                String[] ab = pair.split(":");
                chunkList.add(new int[]{Integer.parseInt(ab[0].trim()), Integer.parseInt(ab[1].trim())});
            }
        }
        if (chunkList.isEmpty()) {
            chunkList.add(new int[]{3, -2});
        }

        Object[][] statuses = {{noise, "noise"}, {surface, "surface"}, {carvers, "carvers"}, {features, "features"}, {full, "full"}};
        for (int[] cc : chunkList) {
            int cx = cc[0], cz = cc[1];
            Path out = dir.resolve(String.format("walk_%d_%d.txt", cx, cz));
            try (BufferedWriter w = Files.newBufferedWriter(out)) {
                for (Object[] st : statuses) {
                    Object status = st[0];
                    Object chunk = getChunk.invoke(chunkSource, cx, cz, status, true);
                    if (st[1].equals("noise")) {
                        try {
                            Class<?> hmTypes = Class.forName(
                                    "net.minecraft.world.level.levelgen.Heightmap$Types");
                            Object wsWg = hmTypes.getField("WORLD_SURFACE_WG").get(null);
                            Object ofWg = hmTypes.getField("OCEAN_FLOOR_WG").get(null);
                            Method getHm = accessClass.getMethod("getHeight", hmTypes, int.class, int.class);
                            Object h1 = getHm.invoke(chunk, wsWg, 13, 4);
                            Object h2 = getHm.invoke(chunk, ofWg, 13, 4);
                            getLogger().info(String.format(
                                    "HMTRACE (%d,%d) WORLD_SURFACE_WG=%s OCEAN_FLOOR_WG=%s",
                                    cx, cz, h1, h2));
                            Object st13_62_4 = accessClass.getMethod("getBlockState",
                                    Class.forName("net.minecraft.core.BlockPos")).invoke(chunk,
                                    Class.forName("net.minecraft.core.BlockPos")
                                            .getMethod("containing", int.class, int.class, int.class)
                                            .invoke(null, 13, 62, 4));
                            getLogger().info("HMTRACE block(13,62,4)=" + st13_62_4);
                        } catch (Throwable t) {
                            getLogger().info("HMTRACE failed: " + t);
                        }
                    }
                    if (st[1].equals("noise")) {
                        try {
                            vanillaRuleProbe(chunkSource, (ChunkAccess) chunk, accessClass);
                        } catch (Throwable t) {
                            getLogger().info("VRULE failed: " + t);
                        }
                    }
                    Object[] lists = (Object[]) getPostProcessing.invoke(chunk);
                    int count = 0;
                    StringBuilder sb = new StringBuilder();
                    int minSection = -4; // overworld minY=-64 -> sections -4..7
                    for (int k = 0; k < lists.length; k++) {
                        Object list = lists[k];
                        if (list == null) continue;
                        List<?> shorts = (List<?>) list;
                        for (Object o : shorts) {
                            short packed = (Short) o;
                            // vanilla packOffsetCoordinates: x | (y&15)<<4 | (z&15)<<8
                            int x = packed & 15;
                            int yl = (packed >> 4) & 15;
                            int z = (packed >> 8) & 15;
                            int y = (minSection + k) * 16 + yl;
                            sb.append((cx * 16 + x)).append(' ').append(y).append(' ').append((cz * 16 + z)).append('\n');
                            count++;
                        }
                    }
                    w.write("STATUS " + st[1] + " marks=" + count + "\n");
                    w.write(sb.toString());
                    getLogger().info("walk " + cx + "," + cz + " status=" + st[1] + " cumulative_marks=" + count);

                    // At SURFACE status: dump every non-air block of the full
                    // column range as "x y z name[prop=v,...]" (props sorted) —
                    // the ground truth for the native surface engine (sfcmp).
                    if (st[1].equals("surface")) {
                        Class<?> stateClass2 = Class.forName(
                                "net.minecraft.world.level.block.state.BlockState");
                        Class<?> blockPosClass2 = Class.forName(
                                "net.minecraft.core.BlockPos$MutableBlockPos");
                        Object blockPos2 = blockPosClass2.getDeclaredConstructor().newInstance();
                        Method getBlockState2 = accessClass.getMethod("getBlockState",
                                Class.forName("net.minecraft.core.BlockPos"));
                        Method mIsAir2 = stateClass2.getMethod("isAir");
                        Method mGetBlock2 = stateClass2.getMethod("getBlock");
                        Object blockRegistry2 = Class.forName(
                                "net.minecraft.core.registries.BuiltInRegistries")
                                .getField("BLOCK").get(null);
                        Method mKey2 = blockRegistry2.getClass()
                                .getMethod("getKey", Object.class);
                        Method mGetValues2 = stateClass2.getMethod("getValues");
                        Class<?> cProperty2 = Class.forName(
                                "net.minecraft.world.level.block.state.properties.Property");
                        Method mPropName2 = cProperty2.getMethod("getName", Comparable.class);
                        StringBuilder surf = new StringBuilder();
                        for (int y = -64; y < 320; y++) {
                            for (int lz = 0; lz < 16; lz++) {
                                for (int lx = 0; lx < 16; lx++) {
                                    blockPosClass2.getMethod("set", int.class, int.class, int.class)
                                            .invoke(blockPos2, cx * 16 + lx, y, cz * 16 + lz);
                                    Object state = getBlockState2.invoke(chunk, blockPos2);
                                    if ((Boolean) mIsAir2.invoke(state)) continue;
                                    Object block = mGetBlock2.invoke(state);
                                    Object rl = mKey2.invoke(blockRegistry2, block);
                                    String name = rl.toString();
                                    java.util.Map<?, ?> values = (java.util.Map<?, ?>) mGetValues2.invoke(state);
                                    if (!values.isEmpty()) {
                                        java.util.TreeMap<String, String> kv = new java.util.TreeMap<>();
                                        for (java.util.Map.Entry<?, ?> e : values.entrySet()) {
                                            Object pn = e.getKey().getClass()
                                                    .getMethod("getName").invoke(e.getKey());
                                            kv.put(pn.toString(), mPropName2.invoke(e.getKey(), e.getValue()).toString());
                                        }
                                        StringBuilder pb = new StringBuilder("[");
                                        boolean first = true;
                                        for (java.util.Map.Entry<String, String> e : kv.entrySet()) {
                                            if (!first) pb.append(',');
                                            pb.append(e.getKey()).append('=').append(e.getValue());
                                            first = false;
                                        }
                                        pb.append(']');
                                        name = name + pb;
                                    }
                                    surf.append(cx * 16 + lx).append(' ').append(y).append(' ')
                                            .append(cz * 16 + lz).append(' ').append(name).append('\n');
                                }
                            }
                        }
                        w.write("SURFACEBLOCKS\n");
                        w.write(surf.toString());
                        getLogger().info("walk " + cx + "," + cz + " surface blocks dumped: " + surf.length());
                    }

                    // At NOISE status: dump the noise-stage blocks of the deep
                    // region (y -64..0) for direct comparison with the native fill.
                    if (st[1].equals("noise")) {
                        Method getBlockState = accessClass.getMethod("getBlockState",
                                Class.forName("net.minecraft.core.BlockPos"));
                        Class<?> blockPosClass = Class.forName("net.minecraft.core.BlockPos$MutableBlockPos");
                        Object blockPos = blockPosClass.getDeclaredConstructor().newInstance();
                        Class<?> stateClass = Class.forName("net.minecraft.world.level.block.state.BlockState");
                        Class<?> fluidStateClass = Class.forName("net.minecraft.world.level.material.FluidState");
                        Method getFluidState = stateClass.getMethod("getFluidState");
                        Method isEmpty = fluidStateClass.getMethod("isEmpty");
                        Method stateToString = stateClass.getMethod("toString");

                        StringBuilder bb = new StringBuilder();
                        for (int y = -64; y < 0; y++) {
                            for (int lz = 0; lz < 16; lz++) {
                                for (int lx = 0; lx < 16; lx++) {
                                    blockPosClass.getMethod("set", int.class, int.class, int.class)
                                            .invoke(blockPos, cx * 16 + lx, y, cz * 16 + lz);
                                    Object state = getBlockState.invoke(chunk, blockPos);
                                    Object fluid = getFluidState.invoke(state);
                                    boolean fluidEmpty = (boolean) isEmpty.invoke(fluid);
                                    if (fluidEmpty) continue;
                                    String s0 = (String) stateToString.invoke(state);
                                    String fname = s0.contains("minecraft:lava") ? "lava"
                                            : s0.contains("minecraft:water") ? "water" : "other";
                                    bb.append(cx * 16 + lx).append(' ').append(y).append(' ').append(cz * 16 + lz)
                                            .append(' ').append(fname).append('\n');
                                }
                            }
                        }
                        w.write("NOISEFLUIDS\n");
                        w.write(bb.toString());
                        getLogger().info("walk " + cx + "," + cz + " noise fluids dumped");

                        // Ground-truth beardifier: vanilla collection loop +
                        // real Beardifier.compute values on a grid, so the
                        // native port can be verified bit-for-bit locally.
                        StructureManager sm = ((net.minecraft.server.level.ServerLevel) level).structureManager();
                        ChunkPos cp = new ChunkPos(cx, cz);
                        ObjectArrayList<Beardifier.Rigid> rigids = new ObjectArrayList<>();
                        ObjectArrayList<net.minecraft.world.level.levelgen.structure.pools.JigsawJunction> junctions = new ObjectArrayList<>();
                        int minBlockX = cx * 16, minBlockZ = cz * 16;
                        for (StructureStart start : sm.startsForStructure(cp, s -> s.terrainAdaptation() != net.minecraft.world.level.levelgen.structure.TerrainAdjustment.NONE)) {
                            var adj = start.getStructure().terrainAdaptation();
                            for (StructurePiece piece : start.getPieces()) {
                                if (!piece.isCloseToChunk(cp, 12)) continue;
                                if (piece instanceof net.minecraft.world.level.levelgen.structure.PoolElementStructurePiece pp) {
                                    if (pp.getElement().getProjection() == net.minecraft.world.level.levelgen.structure.pools.StructureTemplatePool.Projection.RIGID) {
                                        rigids.add(new Beardifier.Rigid(pp.getBoundingBox(), adj, pp.getGroundLevelDelta()));
                                    }
                                    for (var j : pp.getJunctions()) {
                                        if (j.getSourceX() > minBlockX - 12 && j.getSourceZ() > minBlockZ - 12
                                                && j.getSourceX() < minBlockX + 15 + 12 && j.getSourceZ() < minBlockZ + 15 + 12) {
                                            junctions.add(j);
                                        }
                                    }
                                } else {
                                    rigids.add(new Beardifier.Rigid(piece.getBoundingBox(), adj, 0));
                                }
                            }
                        }
                        w.write("BEARDPIECES " + rigids.size() + " " + junctions.size() + "\n");
                        for (var r : rigids) {
                            var b = r.box();
                            w.write("BP " + b.minX() + " " + b.minY() + " " + b.minZ() + " "
                                    + b.maxX() + " " + b.maxY() + " " + b.maxZ() + " "
                                    + r.terrainAdjustment().name() + " " + r.groundLevelDelta() + "\n");
                        }
                        for (var j : junctions) {
                            w.write("BJ " + j.getSourceX() + " " + j.getSourceGroundY() + " " + j.getSourceZ() + "\n");
                        }
                        Beardifier beard = new Beardifier(rigids.iterator(), junctions.iterator());
                        StringBuilder bv = new StringBuilder();
                        for (int y = -64; y < 64; y += 1) {
                            for (int lz = 0; lz < 16; lz += 3) {
                                for (int lx = 0; lx < 16; lx += 3) {
                                    double v = beard.compute(new net.minecraft.world.level.levelgen.DensityFunction.SinglePointContext(cx * 16 + lx, y, cz * 16 + lz));
                                    bv.append(cx * 16 + lx).append(' ').append(y).append(' ').append(cz * 16 + lz).append(' ')
                                            .append(java.lang.Long.toHexString(java.lang.Double.doubleToLongBits(v))).append('\n');
                                }
                            }
                        }
                        w.write("BEARDVALS\n");
                        w.write(bv.toString());
                        getLogger().info("walk " + cx + "," + cz + " beard dumped: " + rigids.size() + " pieces " + junctions.size() + " junctions");

                        // Inspect the REAL NoiseChunk cached on the ProtoChunk
                        // (created by the pipeline's own doFill) and re-run the
                        // fill loop on it — isolates pipeline-vs-synthetic setup
                        // differences (blender, beardifier, geometry).
                        Field ncField = accessClass.getDeclaredField("noiseChunk");
                        ncField.setAccessible(true);
                        Object realNc = ncField.get(chunk);
                        if (realNc instanceof NoiseChunk realChunk) {
                            w.write("NCINFO blender=" + blenderInfo(realChunk)
                                    + " beardifier=" + BeardifierOrMarkerName(realChunk)
                                    + " firstCellX=" + intField(realChunk, "firstCellX")
                                    + " firstCellZ=" + intField(realChunk, "firstCellZ")
                                    + " cellCountXZ=" + intField(realChunk, "cellCountXZ")
                                    + " cellCountY=" + intField(realChunk, "cellCountY")
                                    + " cellNoiseMinY=" + intField(realChunk, "cellNoiseMinY")
                                    + " firstNoiseX=" + intField(realChunk, "firstNoiseX")
                                    + " firstNoiseZ=" + intField(realChunk, "firstNoiseZ")
                                    + " noiseSizeXZ=" + intField(realChunk, "noiseSizeXZ")
                                    + " cellWidth=" + intField(realChunk, "cellWidth")
                                    + " cellHeight=" + intField(realChunk, "cellHeight")
                                    + "\n");
                        // re-run the loop on the REAL noise chunk; dump flags
                        var aqu = realChunk.aquifer();
                        var shouldSchedM = aqu.getClass().getMethod("shouldScheduleFluidUpdate");
                        Method getIs2 = NoiseChunk.class.getDeclaredMethod("getInterpolatedState");
                        getIs2.setAccessible(true);
                        Class<?> stateClass2 = Class.forName("net.minecraft.world.level.block.state.BlockState");
                        Class<?> fluidStateClass2 = Class.forName("net.minecraft.world.level.material.FluidState");
                        Method getFluidState2 = stateClass2.getMethod("getFluidState");
                        Method isEmpty2 = fluidStateClass2.getMethod("isEmpty");
                        BlockState air2 = Blocks.AIR.defaultBlockState();
                        int cwR = (int) intField(realChunk, "cellWidth");
                        int chR = (int) intField(realChunk, "cellHeight");
                        int minCellY = (int) intField(realChunk, "cellNoiseMinY");
                        int cellCountY = (int) intField(realChunk, "cellCountY");
                        int minBlockXr = (int) intField(realChunk, "firstCellX") * cwR;
                        int minBlockZr = (int) intField(realChunk, "firstCellZ") * cwR;
                        StringBuilder rr = new StringBuilder();
                        int rerunMarks = 0;
                        realChunk.initializeForFirstCellX();
                        for (int i2 = 0; i2 < 16 / cwR; i2++) {
                            realChunk.advanceCellX(i2);
                            for (int i3 = 0; i3 < 16 / cwR; i3++) {
                                for (int i5 = cellCountY - 1; i5 >= 0; i5--) {
                                    realChunk.selectCellYZ(i5, i3);
                                    for (int i6 = chR - 1; i6 >= 0; i6--) {
                                        int yR = (minCellY + i5) * chR + i6;
                                        realChunk.updateForY(yR, (double) i6 / chR);
                                        for (int i9 = 0; i9 < cwR; i9++) {
                                            int xR = minBlockXr + i2 * cwR + i9;
                                            realChunk.updateForX(xR, (double) i9 / cwR);
                                            for (int i12 = 0; i12 < cwR; i12++) {
                                                int zR = minBlockZr + i3 * cwR + i12;
                                                realChunk.updateForZ(zR, (double) i12 / cwR);
                                                Object st2 = getIs2.invoke(realChunk);
                                                boolean flag2 = (boolean) shouldSchedM.invoke(aqu);
                                                if (st2 == null) continue;
                                                boolean fluid2 = !(boolean) isEmpty2.invoke(getFluidState2.invoke(st2));
                                                if (st2 != air2 && fluid2 && flag2) {
                                                    rr.append(xR).append(' ').append(yR).append(' ').append(zR).append('\n');
                                                    rerunMarks++;
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            realChunk.swapSlices();
                        }
                        realChunk.stopInterpolation();
                        w.write("NCRERUN marks=" + rerunMarks + "\n");
                        w.write(rr.toString());
                        getLogger().info("walk " + cx + "," + cz + " nc rerun marks=" + rerunMarks);
                        }
                    }
                }
            }
        }
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
        // NoiseBasedChunkGenerator lives under the chunkMap's generator field
        // (CraftBukkit wraps the vanilla generator in CustomChunkGenerator);
        // Paper 1.21.8 may not declare "generator" on the exact chunkMap class,
        // so search recursively instead of a fixed field name.
        Object nbcgObj = findInside(chunkMap, "NoiseBasedChunkGenerator", 0);
        if (nbcgObj == null) throw new IllegalStateException("noise generator not found under chunkMap");
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

        // DF probe mode: -Doracledfprobe="x,y,z;x,y,z;..." evaluates every
        // noise_router density function at the given block positions and
        // writes dfprobe.txt ("x y z field <bits> <decimal>").
        String dfSpec = System.getProperty("oracledfprobe", "");
        if (!dfSpec.isBlank()) {
            Field nrF = RandomState.class.getDeclaredField("router");
            nrF.setAccessible(true);
            Object router = nrF.get(state);
            Class<?> noiseRouterCls = Class.forName("net.minecraft.world.level.levelgen.NoiseRouter");
            String[] fields = {"barrierNoise", "fluidLevelFloodednessNoise", "fluidLevelSpreadNoise",
                    "lavaNoise", "temperature", "vegetation", "continents", "erosion", "depth",
                    "ridges", "initialDensityWithoutJaggedness", "finalDensity",
                    "veinToggle", "veinRidged", "veinGap"};
            java.util.Map<String, Object> fns = new java.util.HashMap<>();
            for (String f : fields) {
                try {
                    fns.put(f, noiseRouterCls.getMethod(f).invoke(router));
                } catch (Exception e) {
                    getLogger().warning("router field missing: " + f + " (" + e + ")");
                }
            }
            Class<?> spc = Class.forName(
                    "net.minecraft.world.level.levelgen.DensityFunction$SinglePointContext");
            java.lang.reflect.Constructor<?> ctor = spc.getConstructors()[0];
            Class<?> dfIface = Class.forName("net.minecraft.world.level.levelgen.DensityFunction");
            Class<?> fcCtx = Class.forName(
                    "net.minecraft.world.level.levelgen.DensityFunction$FunctionContext");
            java.lang.reflect.Method sampleM = dfIface.getMethod("compute", fcCtx);
            try (BufferedWriter wp = Files.newBufferedWriter(dir.resolve("dfprobe.txt"))) {
                for (String part : dfSpec.split(";")) {
                    String[] c = part.trim().split(",");
                    int px = Integer.parseInt(c[0].trim());
                    int py = Integer.parseInt(c[1].trim());
                    int pz = Integer.parseInt(c[2].trim());
                    Object ctx = ctor.newInstance(px, py, pz);
                    for (java.util.Map.Entry<String, Object> e : fns.entrySet()) {
                        Object df = e.getValue();
                        double v = (double) sampleM.invoke(df, ctx);
                        wp.write(px + " " + py + " " + pz + " " + e.getKey()
                                + " " + Double.doubleToRawLongBits(v) + " " + v + "\n");
                    }
                }
            }
            getLogger().info("dfprobe done: " + dfSpec);
            return;
        }

        // chunk list: ORACLE_MARKS mode uses -Doraclemarks.chunklist="cx:cz,cx:cz,..."
        List<int[]> chunkList = new ArrayList<>();
        if (MARKS) {
            String spec = System.getProperty("oraclemarks.chunklist", "");
            if (!spec.isBlank()) {
                for (String pair : spec.split(",")) {
                    String[] ab = pair.split(":");
                    chunkList.add(new int[]{Integer.parseInt(ab[0].trim()), Integer.parseInt(ab[1].trim())});
                }
            }
        }
        if (chunkList.isEmpty()) {
            for (int cx : CHUNKS) {
                for (int cz : CHUNKS) {
                    chunkList.add(new int[]{cx, cz});
                }
            }
        }

        for (int[] cc : chunkList) {
            int cx = cc[0], cz = cc[1];
            Object chunkAccess = level.getClass()
                    .getMethod("getChunk", int.class, int.class).invoke(level, cx, cz);
                Object beardifier = Class
                        .forName("net.minecraft.world.level.levelgen.DensityFunctions$BeardifierMarker")
                        .getDeclaredFields()[0].get(null); // enum INSTANCE
                Method forChunk = NoiseChunk.class.getMethod("forChunk",
                        Class.forName("net.minecraft.world.level.chunk.ChunkAccess"),
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
                Path outM = dir.resolve(String.format("chunk_%d_%d_marks.txt", cx, cz));
                Path outFd = dir.resolve(String.format("fd_%d_%d.txt", cx, cz));
                boolean fdMode = Boolean.getBoolean("oraclefd");
                DensityFunction finalDensity = null;
                if (fdMode) {
                    Field nrF = RandomState.class.getDeclaredField("router");
                    nrF.setAccessible(true);
                    Object router = nrF.get(state);
                    finalDensity = (DensityFunction) Class.forName(
                            "net.minecraft.world.level.levelgen.NoiseRouter")
                            .getMethod("finalDensity").invoke(router);
                }
                try (BufferedWriter w = Files.newBufferedWriter(out);
                     BufferedWriter wm = MARKS ? Files.newBufferedWriter(outM) : null;
                     BufferedWriter wf = fdMode ? Files.newBufferedWriter(outFd) : null) {
                    fillLoop(w, wm, nc, settings, cx, cz, finalDensity, wf);
                }
                if (Boolean.getBoolean("oraclecells")) {
                    Path outC = dir.resolve(String.format("cells_%d_%d.txt", cx, cz));
                    try (BufferedWriter wc = Files.newBufferedWriter(outC)) {
                        dumpCells(nc, wc);
                    }
                }
                if (Boolean.getBoolean("oraclebiomes")) {
                    Path outB = dir.resolve(String.format("biomes_%d_%d.txt", cx, cz));
                    try (BufferedWriter wb = Files.newBufferedWriter(outB)) {
                        dumpBiomes((ChunkAccess) chunkAccess, wb, cx, cz);
                    }
                }
                try (BufferedWriter wi = Files.newBufferedWriter(dir.resolve(String.format("ncinfo_%d_%d.txt", cx, cz)))) {
                    wi.write("NCINFO blender=" + blenderInfo(nc)
                            + " beardifier=" + BeardifierOrMarkerName(nc)
                            + " firstCellX=" + intField(nc, "firstCellX")
                            + " firstCellZ=" + intField(nc, "firstCellZ")
                            + " cellCountXZ=" + intField(nc, "cellCountXZ")
                            + " cellCountY=" + intField(nc, "cellCountY")
                            + " cellNoiseMinY=" + intField(nc, "cellNoiseMinY")
                            + " firstNoiseX=" + intField(nc, "firstNoiseX")
                            + " firstNoiseZ=" + intField(nc, "firstNoiseZ")
                            + " noiseSizeXZ=" + intField(nc, "noiseSizeXZ")
                            + " cellWidth=" + intField(nc, "cellWidth")
                            + " cellHeight=" + intField(nc, "cellHeight")
                            + "\n");
                }
                getLogger().info("noise stage dumped chunk " + cx + "," + cz);
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

    private static String BeardifierOrMarkerName(NoiseChunk nc) throws Exception {
        Field f = NoiseChunk.class.getDeclaredField("beardifier");
        f.setAccessible(true);
        return f.get(nc).getClass().getSimpleName();
    }

    private static int intField(Object o, String name) throws Exception {
        Field f = o.getClass().getDeclaredField(name);
        f.setAccessible(true);
        return f.getInt(o);
    }

    private static String blenderInfo(NoiseChunk nc) throws Exception {
        Field f = NoiseChunk.class.getDeclaredField("blender");
        f.setAccessible(true);
        Object b = f.get(nc);
        int h = 0, d = 0;
        for (Field bf : b.getClass().getDeclaredFields()) {
            if (bf.getType().getSimpleName().equals("Long2ObjectOpenHashMap")) {
                bf.setAccessible(true);
                Object m = bf.get(b);
                int size = (int) m.getClass().getMethod("size").invoke(m);
                if (h == 0) h = size; else d = size;
            }
        }
        return b.getClass().getSimpleName() + " hmap=" + h + " dmap=" + d;
    }

    /** Dump the chunk's biome container per quart in the vanilla
     *  fillBiomesFromNoise iteration order (sections asc, x, y, z):
     *  "BIOME <qx> <qy> <qz> <registry-name>". */
    private void dumpBiomes(ChunkAccess chunk, BufferedWriter wb, int cx, int cz) throws Exception {
        Class<?> accessClass = chunk.getClass();
        while (accessClass != null && !accessClass.getSimpleName().equals("ChunkAccess")) {
            accessClass = accessClass.getSuperclass();
        }
        if (accessClass == null) throw new IllegalStateException("ChunkAccess not found");

        // sections carry their own biome PalettedContainer (4x4x4 quarts)
        Object[] sectionsArr = (Object[]) accessClass.getMethod("getSections").invoke(chunk);
        Class<?> sectionCls = sectionsArr.getClass().getComponentType();
        Method getBiomesM = sectionCls.getMethod("getBiomes");

        int quartX0 = cx * 4;
        int quartZ0 = cz * 4;
        // getMinY lives on LevelHeightAccessor — search the hierarchy
        Method minBuildM = null;
        for (Class<?> k = chunk.getClass(); k != null && minBuildM == null; k = k.getSuperclass()) {
            try {
                minBuildM = k.getMethod("getMinY");
            } catch (NoSuchMethodException ignored) {
            }
        }
        if (minBuildM == null) throw new IllegalStateException("getMinY not found");
        int minQuartY = net.minecraft.core.QuartPos.fromBlock((Integer) minBuildM.invoke(chunk));
        for (int sy = 0; sy < sectionsArr.length; sy++) {
            Object container = getBiomesM.invoke(sectionsArr[sy]);
            Method get = null;
            for (Class<?> k = container.getClass(); k != null && get == null; k = k.getSuperclass()) {
                for (Method m : k.getDeclaredMethods()) {
                    if (m.getName().equals("get") && m.getParameterCount() == 3
                            && m.getParameterTypes()[0] == int.class) {
                        m.setAccessible(true);
                        get = m;
                        break;
                    }
                }
            }
            if (get == null) throw new IllegalStateException("PalettedContainer.get(x,y,z) not found");
            int qy0 = minQuartY + sy * 4;
            for (int x = 0; x < 4; x++) {
                for (int y = 0; y < 4; y++) {
                    for (int z = 0; z < 4; z++) {
                        Object holder = get.invoke(container, x, y, z);
                        String name = "unknown";
                        if (holder instanceof net.minecraft.core.Holder<?> h) {
                            name = h.unwrapKey().map(k -> k.location().toString())
                                    .orElse("none");
                        }
                        wb.write("BIOME " + (quartX0 + x) + " " + (qy0 + y) + " "
                                + (quartZ0 + z) + " " + name + "\n");
                    }
                }
            }
        }
    }

    /** Decompiled doFill block loop (no sections/heightmaps — text dump). */
    /** Re-drive the vanilla loop and snapshot every interpolator's slice0/slice1
     *  after each advanceCellX — the full 5 x-plane corner grids per slot.
     *  Format: "SLICE <interpIdx> <xPlane> <z> <y> <bits>" per corner value
     *  (xPlane = the plane this slice array currently holds), plus CELLINFO
     *  header with the interpolator count and per-interp wrapped class name. */
    private void dumpCells(NoiseChunk nc, BufferedWriter wc) throws Exception {
        Field interpsF = NoiseChunk.class.getDeclaredField("interpolators");
        interpsF.setAccessible(true);
        List<?> interps = (List<?>) interpsF.get(nc);
        Class<?> interpCls = Class.forName("net.minecraft.world.level.levelgen.NoiseChunk$NoiseInterpolator");
        Field s0F = interpCls.getDeclaredField("slice0");
        Field s1F = interpCls.getDeclaredField("slice1");
        s0F.setAccessible(true);
        s1F.setAccessible(true);
        Method cellWidth = NoiseChunk.class.getDeclaredMethod("cellWidth");
        cellWidth.setAccessible(true);
        Method cellHeight = NoiseChunk.class.getDeclaredMethod("cellHeight");
        cellHeight.setAccessible(true);
        int cw = (int) cellWidth.invoke(nc);
        int ch = (int) cellHeight.invoke(nc);
        int minCellY = (int) intField(nc, "cellNoiseMinY");
        int cellCountY = (int) intField(nc, "cellCountY");
        int firstCellX = (int) intField(nc, "firstCellX");
        int firstCellZ = (int) intField(nc, "firstCellZ");
        wc.write("CELLS interps=" + interps.size() + " cw=" + cw + " ch=" + ch
                + " minCellY=" + minCellY + " cellCountY=" + cellCountY
                + " firstCellX=" + firstCellX + " firstCellZ=" + firstCellZ + "\n");
        Method stopInterp = NoiseChunk.class.getDeclaredMethod("stopInterpolation");
        stopInterp.setAccessible(true);
        Field interpFlagF = NoiseChunk.class.getDeclaredField("interpolating");
        interpFlagF.setAccessible(true);
        if (interpFlagF.getBoolean(nc)) {
            stopInterp.invoke(nc);
        }
        nc.initializeForFirstCellX();
        int n = 16 / cw;
        for (int i2 = 0; i2 < n; i2++) {
            nc.advanceCellX(i2);
            // slice0 holds plane (firstCellX+i2), slice1 holds plane (firstCellX+i2+1)
            for (int j = 0; j < interps.size(); j++) {
                Object ip = interps.get(j);
                double[][] s0 = (double[][]) s0F.get(ip);
                double[][] s1 = (double[][]) s1F.get(ip);
                int plane0 = firstCellX + i2;
                int plane1 = firstCellX + i2 + 1;
                for (int z = 0; z < s0.length; z++) {
                    for (int y = 0; y < s0[z].length; y++) {
                        int cornerZ = (firstCellZ + z) * cw;
                        int cornerY = (y + minCellY) * ch;
                        wc.write("SLICE " + j + " " + plane0 + " " + cornerZ + " " + cornerY
                                + " " + Double.doubleToRawLongBits(s0[z][y]) + "\n");
                        wc.write("SLICE " + j + " " + plane1 + " " + cornerZ + " " + cornerY
                                + " " + Double.doubleToRawLongBits(s1[z][y]) + "\n");
                    }
                }
            }
            for (int i3 = 0; i3 < n; i3++) {
                for (int i5 = cellCountY - 1; i5 >= 0; i5--) {
                    nc.selectCellYZ(i5, i3);
                    for (int i6 = ch - 1; i6 >= 0; i6--) {
                        int yR = (minCellY + i5) * ch + i6;
                        nc.updateForY(yR, (double) i6 / ch);
                        for (int i9 = 0; i9 < cw; i9++) {
                            int xR = firstCellX * cw + i2 * cw + i9;
                            nc.updateForX(xR, (double) i9 / cw);
                            for (int i12 = 0; i12 < cw; i12++) {
                                nc.updateForZ(firstCellZ * cw + i3 * cw + i12, (double) i12 / cw);
                            }
                        }
                    }
                }
            }
            nc.swapSlices();
        }
        nc.stopInterpolation();
    }

    private void fillLoop(BufferedWriter w, BufferedWriter wm, NoiseChunk noiseChunk,
                          NoiseGeneratorSettings settings, int cx, int cz,
                          DensityFunction finalDensity, BufferedWriter wf) throws Exception {
        Method getInterpolatedState = NoiseChunk.class.getDeclaredMethod("getInterpolatedState");
        getInterpolatedState.setAccessible(true);
        Method cellWidth = NoiseChunk.class.getDeclaredMethod("cellWidth");
        cellWidth.setAccessible(true);
        Method cellHeight = NoiseChunk.class.getDeclaredMethod("cellHeight");
        cellHeight.setAccessible(true);
        Method aquiferM = NoiseChunk.class.getDeclaredMethod("aquifer");
        aquiferM.setAccessible(true);
        Object aquifer = aquiferM.invoke(noiseChunk);
        Method shouldSched = aquifer.getClass().getMethod("shouldScheduleFluidUpdate");
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
                                boolean flag = (boolean) shouldSched.invoke(aquifer);
                                if (finalDensity != null && wf != null) {
                                    // extra compute is cache-served (same ctx,
                                    // same interpolationCounter) -> identical to
                                    // the value getInterpolatedState just used
                                    double fdv = finalDensity.compute(noiseChunk);
                                    wf.write(i10 + " " + i7 + " " + i13 + " "
                                            + Double.doubleToRawLongBits(fdv) + " " + fdv + "\n");
                                }
                                if (st == null) {
                                    st = defaultBlock;
                                }
                                if (st != air) {
                                    w.write(i10 + " " + i7 + " " + i13 + " "
                                            + BuiltInRegistries.BLOCK.getKey(st.getBlock()) + "\n");
                                    if (wm != null && (!st.getFluidState().isEmpty() || flag)) {
                                        wm.write(i10 + " " + i7 + " " + i13 + " "
                                                + BuiltInRegistries.BLOCK.getKey(st.getBlock())
                                                + " " + flag + "\n");
                                    }
                                }
                            }
                        }
                    }
                }
            }
            noiseChunk.swapSlices();
        }
        noiseChunk.stopInterpolation();

        // Full aquifer status cache dump (source locations + FluidStatus).
        if (wm != null) {
            dumpAquiferCache(wm, noiseChunk);
        }
    }

    /** Dumps NoiseBasedAquifer.aquiferLocationCache + aquiferCache via reflection. */
    private void dumpAquiferCache(BufferedWriter wm, NoiseChunk noiseChunk) throws Exception {
        Object aquifer = NoiseChunk.class.getDeclaredMethod("aquifer").invoke(noiseChunk);
        Class<?> k = aquifer.getClass();
        Field locF = null, cacheF = null, mgxF = null, mgyF = null, mgzF = null, gsxF = null, gszF = null;
        while (k != null && k != Object.class) {
            for (Field f : k.getDeclaredFields()) {
                if (f.getType() == long[].class) locF = f;
                if (f.getType().getName().endsWith("Aquifer$FluidStatus[]")) cacheF = f;
                String n = f.getName();
                if (n.equals("minGridX")) mgxF = f;
                if (n.equals("minGridY")) mgyF = f;
                if (n.equals("minGridZ")) mgzF = f;
                if (n.equals("gridSizeX")) gsxF = f;
                if (n.equals("gridSizeZ")) gszF = f;
            }
            k = k.getSuperclass();
        }
        if (locF == null || cacheF == null) {
            wm.write("AQ no-aquifer-fields\n");
            return;
        }
        locF.setAccessible(true); cacheF.setAccessible(true);
        long[] locs = (long[]) locF.get(aquifer);
        Object[] stats = (Object[]) cacheF.get(aquifer);
        int mgx = mgxF != null ? mgxF.getInt(aquifer) : 0;
        int mgy = mgyF != null ? mgyF.getInt(aquifer) : 0;
        int mgz = mgzF != null ? mgzF.getInt(aquifer) : 0;
        int gsx = gsxF != null ? gsxF.getInt(aquifer) : 3;
        int gsz = gszF != null ? gszF.getInt(aquifer) : 3;
        Class<?> fsClass = Class.forName("net.minecraft.world.level.levelgen.Aquifer$FluidStatus");
        Method lvlM = fsClass.getMethod("fluidLevel");
        Method typM = fsClass.getMethod("fluidType");
        wm.write("AQ minGrid " + mgx + " " + mgy + " " + mgz + " size " + gsx + " " + gsz + "\n");
        for (int gy = 0; gy < (stats.length / (gsz * gsx)); gy++) {
            for (int gz = 0; gz < gsz; gz++) {
                for (int gx = 0; gx < gsx; gx++) {
                    int idx = (gy * gsz + gz) * gsx + gx;
                    long p = locs[idx];
                    Object st = stats[idx];
                    if (p == Long.MAX_VALUE || st == null) continue;
                    int px = (int) (p >> 38);
                    int pz = (int) ((p << 26) >> 38);
                    int py = (int) ((p << 52) >> 52);
                    int lvl = (int) lvlM.invoke(st);
                    Object type = typM.invoke(st);
                    wm.write("AQ " + idx + " src " + px + " " + py + " " + pz
                            + " level " + lvl + " "
                            + BuiltInRegistries.BLOCK.getKey(((BlockState) type).getBlock()) + "\n");
                }
            }
        }
    }
}
