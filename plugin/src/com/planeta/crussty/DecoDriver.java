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
import net.minecraft.world.level.levelgen.RandomSupport;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import net.minecraft.world.level.levelgen.WorldgenRandom;
import net.minecraft.world.level.levelgen.feature.ConfiguredFeature;
import net.minecraft.world.level.levelgen.structure.BoundingBox;
import net.minecraft.world.level.levelgen.structure.Structure;
import net.minecraft.world.level.levelgen.structure.StructureStart;
import net.minecraft.world.level.StructureManager;
import net.minecraft.core.Registry;
import net.minecraft.world.level.levelgen.GenerationStep;
import net.minecraft.core.SectionPos;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.chunk.PalettedContainer;
import net.minecraft.world.level.chunk.Palette;
import net.minecraft.world.level.chunk.PalettedContainer.Data;
import net.minecraft.util.BitStorage;
import java.util.stream.Collectors;

import net.minecraft.world.level.levelgen.placement.PlacedFeature;
import net.minecraft.world.level.chunk.ChunkGenerator;
import net.minecraft.world.level.LevelAccessor;
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

    /** Per-thread reusable ops buffer (65536 ops x 6 i32). */
    private static final ThreadLocal<int[]> OPS_BUF = new ThreadLocal<>();
    /** Per-thread dropped-op coord buffer (x,y,z triples). */
    private static final ThreadLocal<int[]> DROP_BUF = new ThreadLocal<>();

    /** Delegate holder cache (parse+registry walk once per placed name).
     * Concurrent populate threads: CHM + Set are the safe scope. */
    private static final java.util.concurrent.ConcurrentHashMap<String, Holder.Reference<PlacedFeature>> DELEGATE_HOLDER =
            new java.util.concurrent.ConcurrentHashMap<>();
    private static final java.util.Set<String> DELEGATE_MISSING =
            java.util.concurrent.ConcurrentHashMap.newKeySet();

    private final World world;
    /** per-populate region handle (upcalls run on the populate thread). */
    private WorldGenRegion reg;
    /** NMS generator (hybrid delegate runs vanilla PlacedFeature with it). */
    private static volatile ChunkGenerator NMS_GEN;

    private DecoDriver(World world) {
        this.world = world;
    }

    /**
     * World-load handshake: store the native plan + biome features, register
     * the populator. Called from CrusstyChunkGenerator.setupDeco after
     * decoStates/decoPlan succeeded (ctx has a native deco engine).
     */
    public static void setup(World world, String[] states, String[][] plan,
                             Map<String, Set<String>> biomeFeatures, Set<String> possibleBiomes,
                             Object nmsGenRaw) {
        STATES = states;
        PLAN = plan;
        BIOME_FEATURES = biomeFeatures;
        POSSIBLE_BIOMES = possibleBiomes;
        // raw from the reflection handoff (CrusstyChunkGenerator has no mojmap)
        NMS_GEN = nmsGenRaw instanceof ChunkGenerator g ? g : null;
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
            long tw0 = System.nanoTime();
            runChunk(ctx, cx, cz, limited);
            DecoStats.recordChunk(System.nanoTime() - tw0);
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
        // chunk-scoped overlay + snapshot reset (Rust side) — must precede
        // the feature loop so the snapshot reflects the pre-deco region and
        // the overlay starts empty
        // BiomeManager.getBiome zoom seed (obfuscated world seed): the deco
        // biome queries zoom over 8 quart neighbors with this fiddle seed
        long zoomSeed;
        try {
            zoomSeed = net.minecraft.world.level.biome.BiomeManager
                    .obfuscateSeed(seed);
        } catch (Throwable t) {
            zoomSeed = 0;
        }
        ChunkGenNative.decoChunkBegin(ctx, cx, cz, reg.getMinSectionY(), zoomSeed);

        long tp0 = System.nanoTime();
        String[] biomes = regionBiomes(cx, cz);
        int[] runsBuf = new int[PLAN_TOTAL() * 2 + 16];
        int runs = ChunkGenNative.decoChunkPlan(ctx, biomes, runsBuf);
        DecoStats.PLAN_TOTAL.addAndGet(System.nanoTime() - tp0);
        if (first) {
            StringBuilder bs = new StringBuilder();
            for (String b : biomes) bs.append(b).append(' ');
            LOG.info("[deco-dbg] biomes(" + biomes.length + ")=" + bs + " runs=" + runs);
        }

        // Reused ops buffer (ThreadLocal): 1.5MB per chunk-deco allocated 332x
        // per pregen = 500MB GC churn. Same protocol: [n*6] ints, trailing
        // garbage ignored. Threads populate concurrently — TL is the safe scope.
        int[] ops = OPS_BUF.get();
        if (ops == null) {
            ops = new int[6 * 65536];
            OPS_BUF.set(ops);
        }
        int[] drops = DROP_BUF.get();
        if (drops == null) {
            drops = new int[3072];
            DROP_BUF.set(drops);
        }
        int totalOps = 0;
        long ts0 = System.nanoTime();
        runStructures(l, cx, cz);
        DecoStats.STRUCT_TOTAL.addAndGet(System.nanoTime() - ts0);
        long tr = 0;
        long ta = 0;
        for (int k = 0; k < runs; k++) {
            int step = runsBuf[k * 2];
            int gid = runsBuf[k * 2 + 1];
            if (step < 0) {
                break; // runs truncated (buffer too small) — logged below
            }
            DecoStats.FEATURES_TOTAL.incrementAndGet();
            long tr0 = System.nanoTime();
            int n = ChunkGenNative.decoRunFeature(ctx, l, -1L, ox, oy, oz, maxY,
                    step, gid, this, ops);
            tr += System.nanoTime() - tr0;
            if (n == -2) {
                // hybrid delegate: the placed feature's configured graph has an
                // unsupported node (tree, vegetation_patch, ...) — run it
                // ENTIRELY on the vanilla path with the same RNG slot.
                long td0 = System.nanoTime();
                delegateVanilla(PLAN[step][gid], step, gid, l, ox, oy, oz);
                // the vanilla feature wrote via the region — the native
                // snapshot cannot see those blocks: invalidate (lazy reload)
                ChunkGenNative.decoDelegateMark(ctx);
                DecoStats.DELEGATE_TOTAL.addAndGet(System.nanoTime() - td0);
                DecoStats.DELEGATE_COUNT.incrementAndGet();
                continue;
            }
            if (first && (n < 0 || k == 0)) {
                LOG.info("[deco-dbg] run k=" + k + " step=" + step + " gid=" + gid + " -> n=" + n);
            }
            if (n > 0) {
                totalOps += n;
            }
            long ta0 = System.nanoTime();
            int ndrop = 0;
            for (int i = 0; i < n; i++) {
                if (!applyOp(limited, ox, oz, ops, i * 6)) {
                    // dropped: (x,y,z) triple for the native overlay sync
                    if (ndrop * 3 + 2 >= drops.length) {
                        drops = java.util.Arrays.copyOf(drops, drops.length * 2);
                    }
                    drops[ndrop * 3] = ops[i * 6];
                    drops[ndrop * 3 + 1] = ops[i * 6 + 1];
                    drops[ndrop * 3 + 2] = ops[i * 6 + 2];
                    ndrop++;
                }
            }
            if (ndrop > 0) {
                ChunkGenNative.decoOpsDropped(ctx, drops, ndrop);
            }
            ta += System.nanoTime() - ta0;
        }
        DecoStats.RUN_TOTAL.addAndGet(tr);
        DecoStats.APPLY_TOTAL.addAndGet(ta);
        DecoStats.OPS_TOTAL.addAndGet(totalOps);
        if (first) {
            LOG.info("[deco-dbg] totalOps=" + totalOps);
        }
    }

    /**
     * Vanilla-lane execution of one placed feature (F3 hybrid delegate).
     * Exact vanilla protocol for a single feature slot:
     *   rng.setFeatureSeed(decoSeed, gid, step);
     *   placedFeature.placeWithBiomeCheck(reg, generator, rng, origin);
     * The RNG is fresh: setFeatureSeed fully defines the state (vanilla keeps
     * ONE shared WorldgenRandom, but every feature re-seeds before place).
     * NOTE: Paper featureSeeds override (config != -1) would need
     * rng.setDecorationSeed(configSeed, ox, oz) first — no pack here uses it;
     * the native path ignores it too (paperSeed == -1 always today).
     */
    private void delegateVanilla(String placedName, int step, int gid, long decoSeed,
                                 int ox, int oy, int oz) {
        if (NMS_GEN == null || reg == null) {
            return;
        }
        try {
            // Holder cache: registryAccess().lookupOrThrow(...).get(
            // ResourceLocation.parse(name)) parsed the name + walked the
            // registry PER DELEGATE (8206 tree features per pregen).
            Holder.Reference<PlacedFeature> holder = DELEGATE_HOLDER.get(placedName);
            if (holder == null) {
                if (DELEGATE_MISSING.contains(placedName)) {
                    return;
                }
                var holders = reg.registryAccess().lookupOrThrow(Registries.PLACED_FEATURE)
                        .get(ResourceLocation.parse(placedName));
                if (holders.isEmpty()) {
                    LOG.warning("deco delegate: placed feature missing in registry: " + placedName);
                    DELEGATE_MISSING.add(placedName);
                    return;
                }
                holder = holders.get();
                DELEGATE_HOLDER.put(placedName, holder);
            }
            // vanilla addVanillaDecorations wraps an XoroshiroRandomSource — the
            // wrapped source MUST be xoro: setFeatureSeed -> setSeed ->
            // upgradeSeedTo128bit (an LCG LegacyRandomSource here would run the
            // legacy next(31) path and shift every draw — the f1_hyb probe's
            // 9669-block tree divergence).
            WorldgenRandom rng = new WorldgenRandom(
                    new XoroshiroRandomSource(RandomSupport.generateUniqueSeed()));
            rng.setFeatureSeed(decoSeed, gid, step);
            holder.value().placeWithBiomeCheck(reg, NMS_GEN, rng, new BlockPos(ox, oy, oz));
        } catch (Throwable t) {
            LOG.warning("deco delegate failed for " + placedName + ": " + t);
        }
    }

    /**
     * Vanilla structures portion of addVanillaDecorations (shouldGenerateDecorations
     * = false kills structures too). Exact vanilla protocol: shared WorldgenRandom
     * over the deco seed, per step: setFeatureSeed(l, i1, i) with i1 the LOCAL
     * structure index (registry groupingBy order), startsForStructure ->
     * placeInChunk(level, sm, generator, rng, writableArea, pos).
     */
    private void runStructures(long decoSeed, int cx, int cz) {
        if (NMS_GEN == null || reg == null) {
            return;
        }
        // Fast gate: no structure references in the 3x3 region -> no starts ->
        // placeInChunk never runs. The srng draws are unobservable there (a
        // FRESH random per populate; only placeInChunk consumes it) — skip
        // the whole pass. Costs 3.44ms/chunk on structureless packs otherwise
        // (startsForStructure x ~90 structures x 10 steps).
        boolean anyRefs = false;
        for (int dz = -1; dz <= 1 && !anyRefs; dz++) {
            for (int dx = -1; dx <= 1; dx++) {
                if (reg.getChunk(cx + dx, cz + dz).hasAnyStructureReferences()) {
                    anyRefs = true;
                    break;
                }
            }
        }
        if (!anyRefs) {
            return;
        }
        try {
            // Cached step->structures grouping: structReg.stream().collect(
            // groupingBy) ran PER CHUNK (a full registry stream + sort per
            // chunk — 3.42ms of the deco wall). The registry is immutable at
            // runtime; compute once.
            Map<Integer, List<Structure>> map = STRUCT_BY_STEP;
            if (map == null) {
                Registry<Structure> structReg = reg.registryAccess()
                        .lookupOrThrow(Registries.STRUCTURE);
                map = structReg.stream()
                        .collect(Collectors.groupingBy(s -> s.step().ordinal()));
                STRUCT_BY_STEP = map;
                java.util.Map<Structure, String> names = new java.util.HashMap<>();
                for (net.minecraft.resources.ResourceKey<Structure> k : structReg.registryKeySet()) {
                    names.put(structReg.getValue(k), k.toString());
                }
                STRUCT_NAMES = names;
            }
            WorldgenRandom srng = new WorldgenRandom(
                    new XoroshiroRandomSource(RandomSupport.generateUniqueSeed()));
            SectionPos sp = SectionPos.of(new ChunkPos(cx, cz), reg.getMinSectionY());
            // Paper: WorldGenRegion lacks structureManager(); build one over
            // the region exactly like StructureManager.forWorldGenRegion does.
            StructureManager sm = reg.getLevel().structureManager().forWorldGenRegion(reg);
            if (!sm.shouldGenerateStructures()) {
                return;
            }
            BoundingBox writable = writableArea();
            int max = Math.max(GenerationStep.Decoration.values().length, PLAN.length);
            for (int i = 0; i < max; i++) {
                int i1 = 0;
                for (Structure structure : map.getOrDefault(i, List.of())) {
                    srng.setFeatureSeed(decoSeed, i1, i);
                    try {
                        reg.setCurrentlyGenerating(() -> structName(structure));
                    } catch (Throwable ignored) {
                        // diagnostics only
                    }
                    for (StructureStart ss : sm.startsForStructure(sp, structure)) {
                        ss.placeInChunk(reg, sm, NMS_GEN, srng, writable, new ChunkPos(cx, cz));
                    }
                    i1++;
                }
            }
        } catch (Throwable t) {
            LOG.warning("deco structures failed (chunk " + cx + "," + cz + "): " + t);
        }
    }

    private static volatile Map<Integer, List<Structure>> STRUCT_BY_STEP;
    private static volatile java.util.Map<Structure, String> STRUCT_NAMES = java.util.Map.of();

    private static String structName(Structure structure) {
        String n = STRUCT_NAMES.get(structure);
        return n != null ? n : structure.toString();
    }

    /** vanilla ChunkGenerator.getWritableArea: the 3x3 chunk region box. */
    private BoundingBox writableArea() {
        ChunkPos pos = reg.getCenter();
        int x = pos.getMinBlockX();
        int z = pos.getMinBlockZ();
        return new BoundingBox(x - 16, reg.getMinY(), z - 16,
                x + 31, reg.getMinY() + reg.getHeight() - 1, z + 31);
    }

    private int PLAN_TOTAL() {
        String[][] p = PLAN;
        int n = 0;
        for (String[] step : p) {
            n += step.length;
        }
        return n;
    }

    /** Apply one op; false = DROPPED (no block write happened) — the caller
     *  syncs the drop to the native overlay (a chunk-scoped overlay entry for
     *  a dropped op is a phantom that would serve stale answers all chunk). */
    private boolean applyOp(LimitedRegion limited,
                         int ox, int oz, int[] ops, int o) {
        int rx = ops[o];
        int y = ops[o + 1];
        int rz = ops[o + 2];
        int stateIdx = ops[o + 3];
        int flags = ops[o + 4];
        int aux = ops[o + 5];
        String[] states = STATES;
        if ((flags & 32) != 0) {
            // FLAG_POSTPROC: vanilla markAboveForPostProcessing (DiskFeature)
            // — chunk-level postprocessing mark, no block write.
            ChunkAccess ch = reg.getChunk((ox + rx) >> 4, (oz + rz) >> 4);
            ch.markPosForPostprocessing(new BlockPos(ox + rx, y, oz + rz));
            return false;
        }
        if (stateIdx < 0 || stateIdx >= states.length) {
            return false;
        }
        BlockState st = state(states[stateIdx]);
        if (st == null) {
            return false;
        }
        if ((flags & 2) != 0) { // FLAG_ROTATED: RotatedBlockProvider axis pick
            try {
                int axis = (flags >> 2) & 3;
                st = st.trySetValue(RotatedPillarBlock.AXIS, AXES[axis]);
            } catch (Throwable ignored) {
                // state without an axis property: trySetValue is a no-op anyway
            }
        }
        if ((flags & 2) == 0 && aux != 0) {
            // tree aux: bits 0-3 = leaf distance override (1..7), bit 4 =
            // waterlogged=true (vanilla LeavesBlock/roots fluid override) —
            // trySetValue is a no-op when the state lacks the property
            try {
                int dist = aux & 15;
                if (dist >= 1 && dist <= 7) {
                    st = st.trySetValue(net.minecraft.world.level.block.state.properties.BlockStateProperties.DISTANCE, dist);
                }
                if ((aux & 16) != 0) {
                    st = st.trySetValue(net.minecraft.world.level.block.state.properties.BlockStateProperties.WATERLOGGED, Boolean.TRUE);
                }
            } catch (Throwable ignored) {
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
                return false;
            }
            LevelChunkSection sec = ch.getSection(si);
            sec.setBlockState((ox + rx) & 15, y & 15, (oz + rz) & 15, st, false);
            return true;
        }
        reg.setBlock(pos, st, 2);
        if ((flags & 1) != 0) { // FLAG_TICK
            reg.scheduleTick(pos, st.getBlock(), 0);
        }
        return true;
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
        DecoStats.UP_HEIGHT.incrementAndGet();
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
        DecoStats.UP_STATE_IS.incrementAndGet();
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
        DecoStats.UP_FLUID_IS.incrementAndGet();
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
        DecoStats.UP_IS_AIR.incrementAndGet();
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

    /** Tree engine: FULL state string "block[k=v,...]" (props in definition
     *  order) — leaves distance/persistent reads, vine face recomputes. */
    public String stateFull(int x, int y, int z) {
        BlockState st = reg.getBlockState(new BlockPos(x, y, z));
        StringBuilder sb = new StringBuilder(
                BuiltInRegistries.BLOCK.getKey(st.getBlock()).toString());
        var props = st.getProperties();
        if (!props.isEmpty()) {
            sb.append('[');
            boolean first = true;
            for (var p : props) {
                if (!first) sb.append(',');
                first = false;
                sb.append(p.getName()).append('=').append(st.getValue(p).toString());
            }
            sb.append(']');
        }
        return sb.toString();
    }

    /** Tree PlaceOnGround decorator: BlockStateBase.isSolidRender at pos. */
    public boolean isSolidRender(int x, int y, int z) {
        BlockState st = reg.getBlockState(new BlockPos(x, y, z));
        return st.isSolidRender();
    }

    public boolean isReplaceable(int x, int y, int z) {
        DecoStats.UP_OTHER.incrementAndGet();
        return reg.getBlockState(new BlockPos(x, y, z)).is(BlockTags.REPLACEABLE);
    }

    public boolean isSolid(int x, int y, int z) {
        DecoStats.UP_OTHER.incrementAndGet();
        return reg.getBlockState(new BlockPos(x, y, z)).isSolid();
    }

    public boolean hasSturdyFace(int x, int y, int z, int dir) {
        DecoStats.UP_OTHER.incrementAndGet();
        return reg.getBlockState(new BlockPos(x, y, z))
                .isFaceSturdy(reg, new BlockPos(x, y, z), DIRS[dir]);
    }

    public boolean wouldSurvive(String state, int x, int y, int z) {
        DecoStats.UP_CAN_SURVIVE.incrementAndGet();
        BlockState st = state(state);
        return st != null && st.canSurvive(reg, new BlockPos(x, y, z));
    }

    public boolean biomeHasFeature(int x, int y, int z, String placed) {
        DecoStats.UP_OTHER.incrementAndGet();
        Holder<Biome> h = reg.getBiomeManager().getBiome(new BlockPos(x, y, z));
        String name = biomeKey(h.value());
        if (System.getenv("CRUS_BIOMECHK") != null
                && DecoStats.UP_OTHER.get() < 200000
                && (name.endsWith("test_a") || name.endsWith("test_b"))
                && DecoStats.BIOMECHK_LOG.incrementAndGet() <= 8) {
            LOG.info("[biomechk-java] (" + x + "," + y + "," + z + ") biome=" + name
                    + " minSecY=" + reg.getMinSectionY());
        }
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

    // ------------------------------------------------------------------
    // 3x3 region snapshot (perf pass — kills the per-query JNI upcalls)
    // ------------------------------------------------------------------

    /** Reflected PalettedContainer.Data.storage / .palette / section biomes.
     *  Data and section.states are public; Data's fields are private (moonrise
     *  layout) — cache the Fields once. */
    private static final java.lang.reflect.Field F_STORAGE;
    private static final java.lang.reflect.Field F_PALETTE;
    private static final java.lang.reflect.Field F_BIOMES;
    static {
        try {
            F_STORAGE = PalettedContainer.Data.class.getDeclaredField("storage");
            F_STORAGE.setAccessible(true);
            F_PALETTE = PalettedContainer.Data.class.getDeclaredField("palette");
            F_PALETTE.setAccessible(true);
            F_BIOMES = LevelChunkSection.class.getDeclaredField("biomes");
            F_BIOMES.setAccessible(true);
        } catch (Throwable t) {
            throw new ExceptionInInitializerError(t);
        }
    }

    /** Palette-entry flag bits (exact vanilla semantics, computed in Java).
     *  The native answers point queries from these — no string parsing. */
    private static int stateFlags(BlockState st) {
        int f = 0;
        if (st.isAir()) {
            f |= 1;
        }
        if (st.blocksMotion()) {
            f |= 2;
        }
        if (!st.getFluidState().isEmpty()) {
            f |= 4;
            if (st.getFluidState().is(net.minecraft.world.level.material.Fluids.WATER)) {
                f |= 1024;
            }
            if (st.getFluidState().is(net.minecraft.world.level.material.Fluids.LAVA)) {
                f |= 2048;
            }
        }
        if (st.is(BlockTags.LEAVES)) {
            f |= 8;
        }
        if (st.is(BlockTags.REPLACEABLE)) {
            f |= 16;
        }
        if (st.isSolid()) {
            f |= 32;
        }
        if (st.isSolidRender()) {
            f |= 64;
        }
        // sturdy faces: N(128) S(256) E(512) W(4096) UP(8192) DOWN(16384)
        if (st.isFaceSturdy(net.minecraft.world.level.EmptyBlockGetter.INSTANCE,
                net.minecraft.core.BlockPos.ZERO, Direction.NORTH)) {
            f |= 128;
        }
        if (st.isFaceSturdy(net.minecraft.world.level.EmptyBlockGetter.INSTANCE,
                net.minecraft.core.BlockPos.ZERO, Direction.SOUTH)) {
            f |= 256;
        }
        if (st.isFaceSturdy(net.minecraft.world.level.EmptyBlockGetter.INSTANCE,
                net.minecraft.core.BlockPos.ZERO, Direction.EAST)) {
            f |= 512;
        }
        if (st.isFaceSturdy(net.minecraft.world.level.EmptyBlockGetter.INSTANCE,
                net.minecraft.core.BlockPos.ZERO, Direction.WEST)) {
            f |= 4096;
        }
        if (st.isFaceSturdy(net.minecraft.world.level.EmptyBlockGetter.INSTANCE,
                net.minecraft.core.BlockPos.ZERO, Direction.UP)) {
            f |= 8192;
        }
        if (st.isFaceSturdy(net.minecraft.world.level.EmptyBlockGetter.INSTANCE,
                net.minecraft.core.BlockPos.ZERO, Direction.DOWN)) {
            f |= 16384;
        }
        return f;
    }

    /** One section snapshot as a little-endian byte payload:
     *  i32 secY, i32 blockBits, i32 palLen, palLen x {u16 nameIdx, u16 flags},
     *  i32 rawLen, i64 raw[rawLen], i32 biomeBits, i32 bioPalLen,
     *  bioPalLen x {u16 nameIdx}, i32 rawLen2, i64 raw2[rawLen2],
     *  i32 nNames, nNames x {u16 len, utf8}, i32 nBio, nBio x {u16 len, utf8}.
     *  nameIdx indexes the section-local block-name list (names of the block
     *  palette entries); biome palette entries index the biome-name list.
     *  bits==0 = single-value palette (raw empty, value = entry 0). */
    public byte[] decoSnapSection(int cx, int cz, int secY) {
        ChunkAccess ch = reg.getChunk(cx, cz);
        int si = secY - reg.getMinSectionY();
        java.nio.ByteBuffer buf = java.nio.ByteBuffer.allocate(8192)
                .order(java.nio.ByteOrder.LITTLE_ENDIAN);
        buf.putInt(secY);
        if (si < 0 || si >= ch.getSectionsCount()) {
            buf.putInt(-1);
            return java.util.Arrays.copyOf(buf.array(), buf.position());
        }
        LevelChunkSection sec = ch.getSection(si);
        ArrayList<String> names = new ArrayList<>();
        HashMap<String, Integer> nameIdx = new HashMap<>();
        // ---- blocks ----
        PalettedContainer<?> pc = sec.states;
        try {
            Data<?> d = (Data<?>) pc.data;
            BitStorage storage = (BitStorage) F_STORAGE.get(d);
            Palette<?> pal = (Palette<?>) F_PALETTE.get(d);
            int bits = storage.getBits();
            long[] raw = storage.getRaw();
            int n = pal.getSize();
            buf.putInt(bits);
            buf.putInt(n);
            for (int i = 0; i < n; i++) {
                BlockState st = (BlockState) pal.valueFor(i);
                String key = BuiltInRegistries.BLOCK.getKey(st.getBlock()).toString();
                Integer ix = nameIdx.get(key);
                if (ix == null) {
                    ix = names.size();
                    nameIdx.put(key, ix);
                    names.add(key);
                }
                buf.putShort((short) ix.intValue());
                buf.putShort((short) stateFlags(st));
            }
            buf.putInt(raw.length);
            for (long v : raw) {
                buf.putLong(v);
            }
            // ---- biomes (4x4x4 quart cells per section) ----
            PalettedContainer<?> bio = (PalettedContainer<?>) F_BIOMES.get(sec);
            Data<?> bd = (Data<?>) bio.data;
            BitStorage bs = (BitStorage) F_STORAGE.get(bd);
            Palette<?> bp = (Palette<?>) F_PALETTE.get(bd);
            int bbits = bs.getBits();
            long[] braw = bs.getRaw();
            int bn = bp.getSize();
            buf.putInt(bbits);
            buf.putInt(bn);
            ArrayList<String> bnames = new ArrayList<>();
            HashMap<String, Integer> bidx = new HashMap<>();
            for (int i = 0; i < bn; i++) {
                Object v = bp.valueFor(i);
                Holder<Biome> h;
                if (v instanceof Holder<?> hh) {
                    h = (Holder<Biome>) hh;
                } else {
                    @SuppressWarnings("unchecked")
                    Holder<Biome> h2 = (Holder<Biome>) v;
                    h = h2;
                }
                String key = biomeKey(h.value());
                Integer ix = bidx.get(key);
                if (ix == null) {
                    ix = bnames.size();
                    bidx.put(key, ix);
                    bnames.add(key);
                }
                buf.putShort((short) ix.intValue());
            }
            buf.putInt(braw.length);
            for (long v : braw) {
                buf.putLong(v);
            }
            buf.putInt(names.size());
            for (String s : names) {
                byte[] b = s.getBytes(java.nio.charset.StandardCharsets.UTF_8);
                buf.putShort((short) b.length);
                buf.put(b);
            }
            buf.putInt(bnames.size());
            for (String s : bnames) {
                byte[] b = s.getBytes(java.nio.charset.StandardCharsets.UTF_8);
                buf.putShort((short) b.length);
                buf.put(b);
            }
        } catch (Throwable t) {
            LOG.warning("deco snap: section " + cx + "," + cz + "," + secY + " failed: " + t);
            return null;
        }
        return java.util.Arrays.copyOf(buf.array(), buf.position());
    }
}
