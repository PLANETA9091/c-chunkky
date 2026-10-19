package com.planeta.crussty;

import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.core.Holder;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.core.registries.Registries;
import net.minecraft.resources.ResourceLocation;
import net.minecraft.server.level.WorldGenRegion;
import net.minecraft.tags.BlockTags;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.biome.Biome;
import net.minecraft.world.level.block.RotatedPillarBlock;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.Property;
import net.minecraft.world.level.chunk.ChunkAccess;
import net.minecraft.world.level.chunk.LevelChunkSection;
import net.minecraft.world.level.chunk.LevelChunkSection;
import net.minecraft.world.level.chunk.PalettedContainerRO;
import net.minecraft.world.level.levelgen.Heightmap;
import org.bukkit.World;
import org.bukkit.craftbukkit.generator.CraftLimitedRegion;
import org.bukkit.generator.LimitedRegion;
import org.bukkit.generator.BlockPopulator;

import java.util.ArrayList;
import java.util.HashMap;
import java.util.HashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.TreeSet;
import java.util.function.Consumer;

/**
 * Stage F1 native decoration driver (NMS companion, mojmap-mapped).
 *
 * Vanilla addVanillaDecorations replacement protocol:
 *   - shouldGenerateDecorations() = false on the generator (opt-in via
 *     CRUS_NATIVE_DECO=1): Paper skips addVanillaDecorations (structures AND
 *     features — see CustomChunkGenerator.applyBiomeDecoration) but still
 *     runs world populators, which live OUTSIDE that gate. This populator is
 *     the decoration entry point.
 *   - the master loop (steps x sorted feature indexes) is driven here; each
 *     feature is expanded natively (ChunkGenNative.decoRunFeature) with the
 *     exact vanilla seeding, level-dependent queries upcall back into
 *     {@link #height}/{@link #stateIs}/... over the 3x3 WorldGenRegion, and
 *     the returned ops are written with vanilla setBlock(pos, state, 2).
 *
 * Ordering contract: ops of feature k are applied BEFORE feature k+1 expands
 * (the level reads must see earlier writes) — that is why the loop stays in
 * Java across the per-feature JNI boundary.
 */
public final class DecoDriver extends BlockPopulator {

    /** decoStates table (index -> canonical state string). */
    private static volatile String[] STATES;
    /** featuresPerStep (FeatureSorter order, index within step = vanilla i3). */
    private static volatile String[][] PLAN;
    /** biome name -> union of placed feature refs (BiomeFilter source). */
    private static volatile Map<String, Set<String>> BIOME_FEATURES;
    /** possibleBiomes (native multi_noise source order — only names matter). */
    private static volatile Set<String> POSSIBLE_BIOMES;
    /** parse cache: state string -> BlockState. */
    private static final Map<String, BlockState> STATE_CACHE = new HashMap<>();

    private static volatile boolean ACTIVE = false;

    /** Heightmap.Types in the native HeightmapTy ordinal order
     *  (WorldSurfaceWg, WorldSurface, OceanFloorWg, OceanFloor,
     *   MotionBlocking, MotionBlockingNoLeaves). */
    private static final Heightmap.Types[] TYPES = {
            Heightmap.Types.WORLD_SURFACE_WG,
            Heightmap.Types.WORLD_SURFACE,
            Heightmap.Types.OCEAN_FLOOR_WG,
            Heightmap.Types.OCEAN_FLOOR,
            Heightmap.Types.MOTION_BLOCKING,
            Heightmap.Types.MOTION_BLOCKING_NO_LEAVES,
    };
    private static final Direction[] DIRS = new Direction[6];
    private static final Direction.Axis[] AXES = {
            Direction.Axis.X, Direction.Axis.Y, Direction.Axis.Z
    };

    static {
        for (int i = 0; i < 6; i++) {
            DIRS[i] = Direction.from3DDataValue(i);
        }
    }

    private static final java.util.logging.Logger LOG =
            java.util.logging.Logger.getLogger("CrusstyGen");

    private final World world;
    /** per-populate region handle (upcalls run on the populate thread). */
    private WorldGenRegion reg;

    private DecoDriver(World world) {
        this.world = world;
    }

    /**
     * World-load handshake: store the native plan + biome features, register
     * the populator. Called from CrusstyChunkGenerator.setupDeco after
     * decoStates/decoPlan succeeded (ctx has a native deco engine).
     */
    public static void setup(World world, String[] states, String[][] plan,
                             Map<String, Set<String>> biomeFeatures, Set<String> possibleBiomes) {
        STATES = states;
        PLAN = plan;
        BIOME_FEATURES = biomeFeatures;
        POSSIBLE_BIOMES = possibleBiomes;
        world.getPopulators().add(new DecoDriver(world));
        ACTIVE = true;
    }

    public static boolean active() {
        return ACTIVE;
    }

    // ------------------------------------------------------------------
    // Populator entry (the native decoration master loop)
    // ------------------------------------------------------------------

    @Override
    public void populate(org.bukkit.generator.WorldInfo info, java.util.Random random,
                         int cx, int cz, LimitedRegion limited) {
        if (!ACTIVE || STATES == null || PLAN == null) {
            return;
        }
        WorldGenRegion region;
        try {
            region = (WorldGenRegion) ((CraftLimitedRegion) limited).getHandle();
        } catch (Throwable t) {
            ACTIVE = false;
            LOG.warning("deco: region unwrap failed, native deco disabled: " + t);
            return;
        }
        this.reg = region;
        java.util.concurrent.locks.ReentrantLock[] lockBox =
                new java.util.concurrent.locks.ReentrantLock[1];
        long ctx = CrusstyChunkGenerator.decoAcquire(lockBox);
        if (ctx == 0) {
            return;
        }
        try {
            runChunk(ctx, cx, cz, limited);
        } catch (Throwable t) {
            LOG.warning("deco: chunk " + cx + "," + cz + " failed: " + t);
        } finally {
            CrusstyChunkGenerator.decoRelease(lockBox);
            this.reg = null;
        }
    }

    private static volatile boolean FIRST_LOGGED = false;

    private void runChunk(long ctx, int cx, int cz, LimitedRegion limited) {
        final boolean first = !FIRST_LOGGED;
        FIRST_LOGGED = true;
        int ox = cx << 4;
        int oz = cz << 4;
        int oy = reg.getMinY();
        int maxY = reg.getMaxY();
        long seed = world.getSeed();
        long l = ChunkGenNative.decoDecoSeed(seed, ox, oz);

        String[] biomes = regionBiomes(cx, cz);
        int[] runsBuf = new int[PLAN_TOTAL() * 2 + 16];
        int runs = ChunkGenNative.decoChunkPlan(ctx, biomes, runsBuf);
        if (first) {
            StringBuilder bs = new StringBuilder();
            for (String b : biomes) bs.append(b).append(' ');
            LOG.info("[deco-dbg] biomes(" + biomes.length + ")=" + bs + " runs=" + runs);
        }

        int[] ops = new int[5 * 65536];
        int totalOps = 0;
        for (int k = 0; k < runs; k++) {
            int step = runsBuf[k * 2];
            int gid = runsBuf[k * 2 + 1];
            if (step < 0) {
                break; // runs truncated (buffer too small) — logged below
            }
            int n = ChunkGenNative.decoRunFeature(ctx, l, -1L, ox, oy, oz, maxY,
                    step, gid, this, ops);
            if (first && (n < 0 || k == 0)) {
                LOG.info("[deco-dbg] run k=" + k + " step=" + step + " gid=" + gid + " -> n=" + n);
            }
            if (n > 0) {
                totalOps += n;
            }
            for (int i = 0; i < n; i++) {
                applyOp(limited, ox, oz, ops, i * 5);
            }
        }
        if (first) {
            LOG.info("[deco-dbg] totalOps=" + totalOps);
        }
    }

    private int PLAN_TOTAL() {
        String[][] p = PLAN;
        int n = 0;
        for (String[] step : p) {
            n += step.length;
        }
        return n;
    }

    private void applyOp(LimitedRegion limited,
                         int ox, int oz, int[] ops, int o) {
        int rx = ops[o];
        int y = ops[o + 1];
        int rz = ops[o + 2];
        int stateIdx = ops[o + 3];
        int flags = ops[o + 4];
        String[] states = STATES;
        if ((flags & 32) != 0) {
            // FLAG_POSTPROC: vanilla markAboveForPostProcessing (DiskFeature)
            // — chunk-level postprocessing mark, no block write.
            ChunkAccess ch = reg.getChunk((ox + rx) >> 4, (oz + rz) >> 4);
            ch.markPosForPostprocessing(new BlockPos(ox + rx, y, oz + rz));
            return;
        }
        if (stateIdx < 0 || stateIdx >= states.length) {
            return;
        }
        BlockState st = state(states[stateIdx]);
        if (st == null) {
            return;
        }
        if ((flags & 2) != 0) { // FLAG_ROTATED: RotatedBlockProvider axis pick
            try {
                int axis = (flags >> 2) & 3;
                st = st.trySetValue(RotatedPillarBlock.AXIS, AXES[axis]);
            } catch (Throwable ignored) {
                // state without an axis property: trySetValue is a no-op anyway
            }
        }
        BlockPos pos = new BlockPos(ox + rx, y, oz + rz);
        if ((flags & 16) != 0) {
            // FLAG_RAW: vanilla BulkSectionAccess write (OreFeature) — section-
            // level setBlockState(state, false): NO heightmap update, no
            // postprocessing marks, no block events.
            ChunkAccess ch = reg.getChunk((ox + rx) >> 4, (oz + rz) >> 4);
            int si = reg.getSectionIndex(y);
            if (si < 0 || si >= ch.getSectionsCount()) {
                return;
            }
            LevelChunkSection sec = ch.getSection(si);
            sec.setBlockState((ox + rx) & 15, y & 15, (oz + rz) & 15, st, false);
            return;
        }
        reg.setBlock(pos, st, 2);
        if ((flags & 1) != 0) { // FLAG_TICK
            reg.scheduleTick(pos, st.getBlock(), 0);
        }
    }

    /** State-string parser: "ns:block[k=v,k2=v2]" -> BlockState (cached). */
    private static BlockState state(String s) {
        BlockState cached = STATE_CACHE.get(s);
        if (cached != null) {
            return cached;
        }
        try {
            String name = s;
            String props = null;
            int b = s.indexOf('[');
            if (b >= 0 && s.endsWith("]")) {
                name = s.substring(0, b);
                props = s.substring(b + 1, s.length() - 1);
            }
            final String fname = name;
            ResourceLocation rl = ResourceLocation.parse(fname);
            BlockState st = BuiltInRegistries.BLOCK.get(rl)
                    .orElseThrow(() -> new IllegalStateException("unknown block " + fname))
                    .value()
                    .defaultBlockState();
            if (props != null && !props.isEmpty()) {
                for (String kv : props.split(",")) {
                    int eq = kv.indexOf('=');
                    if (eq <= 0) {
                        continue;
                    }
                    String pk = kv.substring(0, eq);
                    String pv = kv.substring(eq + 1);
                    for (Property<?> p : st.getProperties()) {
                        if (p.getName().equals(pk)) {
                            st = setValue(st, p, pv);
                            break;
                        }
                    }
                }
            }
            STATE_CACHE.put(s, st);
            return st;
        } catch (Throwable t) {
            LOG.warning("deco: state parse failed '" + s + "': " + t);
            STATE_CACHE.put(s, null);
            return null;
        }
    }

    @SuppressWarnings({"unchecked", "rawtypes"})
    private static BlockState setValue(BlockState st, Property p, String v) {
        return st.setValue(p, (Comparable) p.getValue(v).orElse(null));
    }

    // ------------------------------------------------------------------
    // 3x3 biome union (vanilla: section palettes of the 9 chunks, deduped,
    // filtered to possibleBiomes)
    // ------------------------------------------------------------------

    private String[] regionBiomes(int cx, int cz) {
        Set<String> possible = POSSIBLE_BIOMES;
        Set<String> out = new TreeSet<>();
        List<Holder<Biome>> sink = new ArrayList<>(8);
        for (int dx = -1; dx <= 1; dx++) {
            for (int dz = -1; dz <= 1; dz++) {
                ChunkAccess ch = reg.getChunk(cx + dx, cz + dz);
                for (LevelChunkSection sec : ch.getSections()) {
                    PalettedContainerRO<Holder<Biome>> biomes = sec.getBiomes();
                    sink.clear();
                    biomes.getAll(sink::add);
                    for (Holder<Biome> h : sink) {
                        String name = biomeKey(h.value());
                        if (possible == null || possible.contains(name)) {
                            out.add(name);
                        }
                    }
                }
            }
        }
        return out.toArray(new String[0]);
    }

    /** 1.21.8: the biome registry is data-driven-only — resolve keys through
     *  the region's registry access. */
    private String biomeKey(Biome biome) {
        return reg.registryAccess().lookupOrThrow(Registries.BIOME).getKey(biome).toString();
    }

    // ------------------------------------------------------------------
    // Native upcalls (DecoLevelBridge ABI — see native/src/features_jni.rs)
    // ------------------------------------------------------------------

    public int height(int ty, int x, int z) {
        // Paper 1.21.8 (vantrace agent proof): ChunkStatus CARVERS/FEATURES
        // register FINAL_HEIGHTMAPS for setBlock updates, so the WG maps
        // (WORLD_SURFACE_WG/OCEAN_FLOOR_WG) are FROZEN at their pre-decoration
        // (surface) values — decoration writes do NOT reach them (vanilla
        // trace: WS_WG(1616,1750)=102 after a cornflower@102 whose update
        // r=true hit only the FINAL WORLD_SURFACE map). Read the chunk's
        // heightmap data instead of scanning the live blocks.
        if (ty == 0 || ty == 2) {
            ChunkAccess ch = reg.getChunk(x >> 4, z >> 4);
            return ch.getHeight(TYPES[ty], x & 15, z & 15) + 1;
        }
        // The non-WG types are FINAL maps (live): vanilla WorldGenRegion
        // recomputes them from the actual blocks with the fluid-aware
        // predicates. The region's own writes of PREVIOUS features are
        // flushed, so a column scan matches vanilla's live FINAL maps.
        // (The current feature's own writes are overlayed in Rust.)
        return scanHeight(TYPES[ty], x, z);
    }

    private int scanHeight(Heightmap.Types t, int x, int z) {
        int top = reg.getMaxY() - 1;
        int bottom = reg.getMinY();
        for (int y = top; y >= bottom; y--) {
            BlockState st = reg.getBlockState(new BlockPos(x, y, z));
            boolean hit = switch (t) {
                case WORLD_SURFACE, WORLD_SURFACE_WG -> !st.isAir();
                case OCEAN_FLOOR, MOTION_BLOCKING -> st.blocksMotion() || !st.getFluidState().isEmpty();
                case OCEAN_FLOOR_WG -> st.blocksMotion();
                case MOTION_BLOCKING_NO_LEAVES -> st.blocksMotion() && !st.is(BlockTags.LEAVES)
                        || !st.getFluidState().isEmpty();
            };
            if (hit) {
                return y + 1;
            }
        }
        return bottom;
    }

    public boolean stateIs(int x, int y, int z, String[] names) {
        BlockState st = reg.getBlockState(new BlockPos(x, y, z));
        String key = BuiltInRegistries.BLOCK.getKey(st.getBlock()).toString();
        for (String n : names) {
            if (key.equals(n)) {
                return true;
            }
        }
        return false;
    }

    public boolean fluidIs(int x, int y, int z, String[] names) {
        BlockState st = reg.getBlockState(new BlockPos(x, y, z));
        String key = BuiltInRegistries.FLUID.getKey(st.getFluidState().getType()).toString();
        for (String n : names) {
            if (key.equals(n)) {
                return true;
            }
        }
        return false;
    }

    /** OreFeature isAdjacentToAir: state.isAir() at pos. */
    public boolean isAir(int x, int y, int z) {
        return reg.getBlockState(new BlockPos(x, y, z)).isAir();
    }

    /** OverlayLevel: solidity of a block NAME (state string) written earlier
     *  in the same feature run — cached by the native side. */
    public boolean nameIsSolid(String name) {
        BlockState st = state(name);
        return st != null && st.isSolid();
    }

    /** RuleTest.tag_match: state.is(TagKey.create(BLOCK, tag)). */
    public boolean tagIs(int x, int y, int z, String tag) {
        BlockState st = reg.getBlockState(new BlockPos(x, y, z));
        return st.is(net.minecraft.tags.TagKey.create(net.minecraft.core.registries.Registries.BLOCK,
                ResourceLocation.parse(tag)));
    }

    /** Debug upcall: block registry key at pos (draw-trace diagnostics). */
    public String stateName(int x, int y, int z) {
        BlockState st = reg.getBlockState(new BlockPos(x, y, z));
        return BuiltInRegistries.BLOCK.getKey(st.getBlock()).toString();
    }

    public boolean isReplaceable(int x, int y, int z) {
        return reg.getBlockState(new BlockPos(x, y, z)).is(BlockTags.REPLACEABLE);
    }

    public boolean isSolid(int x, int y, int z) {
        return reg.getBlockState(new BlockPos(x, y, z)).isSolid();
    }

    public boolean hasSturdyFace(int x, int y, int z, int dir) {
        return reg.getBlockState(new BlockPos(x, y, z))
                .isFaceSturdy(reg, new BlockPos(x, y, z), DIRS[dir]);
    }

    public boolean wouldSurvive(String state, int x, int y, int z) {
        BlockState st = state(state);
        return st != null && st.canSurvive(reg, new BlockPos(x, y, z));
    }

    public boolean biomeHasFeature(int x, int y, int z, String placed) {
        Holder<Biome> h = reg.getBiomeManager().getBiome(new BlockPos(x, y, z));
        String name = biomeKey(h.value());
        if ((x == 1615 && z == 1612) || (x == 1612 && z == 1665) || (x == 1612 && z == 1673)) {
            LOG.info("[f1trace] biomeHasFeature " + placed + " (" + x + "," + y + "," + z + ") biome=" + name);
        }
        Map<String, Set<String>> bf = BIOME_FEATURES;
        if (bf == null) {
            return false;
        }
        Set<String> feats = bf.get(name);
        return feats != null && feats.contains(placed);
    }

    public boolean outsideBuildHeight(int y) {
        return y < reg.getMinY() || y >= reg.getMaxY();
    }
}
