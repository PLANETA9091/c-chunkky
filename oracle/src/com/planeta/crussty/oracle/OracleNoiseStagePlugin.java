package com.planeta.crussty.oracle;

import net.minecraft.core.Holder;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import it.unimi.dsi.fastutil.objects.ObjectArrayList;
import net.minecraft.world.level.ChunkPos;
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
