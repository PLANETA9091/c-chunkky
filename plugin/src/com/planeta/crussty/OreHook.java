package com.planeta.crussty;

import net.minecraft.core.BlockPos;
import net.minecraft.core.Holder;
import net.minecraft.core.registries.Registries;
import net.minecraft.resources.Identifier;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.WorldGenLevel;
import net.minecraft.world.level.biome.BiomeManager;
import net.minecraft.world.level.chunk.ChunkGenerator;
import net.minecraft.world.level.levelgen.Xoroshiro128PlusPlus;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import net.minecraft.world.level.levelgen.WorldgenRandom;
import net.minecraft.world.level.levelgen.feature.AbstractOreFeature;
import net.minecraft.world.level.levelgen.feature.Feature;
import net.minecraft.world.level.levelgen.placement.PlacedFeature;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.server.level.WorldGenRegion;

import java.lang.reflect.Field;
import java.util.concurrent.atomic.LongAdder;
import java.util.concurrent.locks.ReentrantLock;

/**
 * cont-99 front 3 — deco=0 ore takeover (the OreFeature port, surgical
 * form). The vanilla decorate loop keeps running in Java; the ONLY thing
 * replaced is the value inside the Feature holder of every ore placed
 * feature reachable from the native plan (Unsafe write on
 * Holder.Reference.value): vanilla OreFeature instances become
 * {@link CrusstyOreFeature} instances bound to the feature registry name
 * and the original instance (fallback). When the vanilla decorate loop
 * invokes {@code place()} on them, the placement modifiers have ALREADY
 * consumed their draws from the shared decorate WorldgenRandom — the hook
 * reads the RAW Xoroshiro128PlusPlus engine state (seedLo/seedHi of the
 * engine object) via reflection and hands it to the native ore expansion
 * (ChunkGenNative.oreRunConfigured), which CONTINUES that exact stream.
 *
 * Fallback discipline: EVERY non-success path (bridge missing, level not a
 * WorldGenRegion, RNG engine not Xoroshiro, ctx acquire failure, native
 * -1/-3) runs the ORIGINAL feature's vanilla place — the Java RNG is
 * untouched by the hook on every such path, so the fallback is bit-exact
 * vanilla by construction.
 *
 * 26.3 shape: no ConfiguredFeature layer — the hook binds the FEATURE
 * registry key ("minecraft:ore_dirt"); the native entry accepts the placed
 * name too (resolves placed -> configured from its own asset decode).
 *
 * Gating: CRUS_NATIVE_ORE=1 (CrusstyChunkGenerator.setupOre). Default off —
 * the prod default stays untouched until the A/B gate says otherwise.
 *
 * Scope note: single-world assumption carried over from DecoDriver (the
 * static tables are per-JVM; CI pregen worlds are single-world).
 */
public final class OreHook {

    private static final java.util.logging.Logger LOG =
            java.util.logging.Logger.getLogger("CrusstyGen");

    private static volatile boolean ON = false;
    private static volatile boolean SWAPPED = false;

    /** sun.misc.Unsafe (field write on the frozen holder). */
    private static final sun.misc.Unsafe UNSAFE;
    static {
        sun.misc.Unsafe u = null;
        try {
            Field uf = sun.misc.Unsafe.class.getDeclaredField("theUnsafe");
            uf.setAccessible(true);
            u = (sun.misc.Unsafe) uf.get(null);
        } catch (Throwable ignored) {
        }
        UNSAFE = u;
    }

    // --- reflection cache (re-read EVERY call: setSeed replaces the engine
    // object between features, so nothing may be cached but the Fields) ---
    private static volatile Field F_WGR_SOURCE;
    private static volatile Field F_XORO_ENGINE;
    private static volatile Field F_ENGINE_LO;
    private static volatile Field F_ENGINE_HI;

    /** Per-thread reusable ops buffer (same protocol as DecoDriver). */
    private static final ThreadLocal<int[]> OPS = new ThreadLocal<>();
    /** Per-thread last decoChunkBegin chunk: {cx, cz, valid}. */
    private static final ThreadLocal<long[]> LAST_CHUNK = new ThreadLocal<>();

    private static final LongAdder RUNS = new LongAdder();
    private static final LongAdder OPS_APPLIED = new LongAdder();
    private static final LongAdder FALLBACKS = new LongAdder();
    private static volatile long LAST_LOG_MS = 0;

    /** cont-99d shadow-trace budget: the first N ore calls log the captured
     *  raw engine state + the EXACT vanilla place() prologue math computed
     *  in Java from that state (xoroshiro128++ continuation). The native's
     *  [vein@]/[draw] traces (CRUS_VEIN_TRACE=/CRUS_DRAW_TRACE=1 on the
     *  ab_on leg) print the same quantities — the artifact diff localizes
     *  the divergence to injection vs geometry vs node loop. */
    private static final java.util.concurrent.atomic.AtomicInteger TRACE_LEFT =
            new java.util.concurrent.atomic.AtomicInteger(2);
    /** Global audit budget across chunks (the per-chunk refill respects it). */
    private static final java.util.concurrent.atomic.AtomicInteger AUDIT_BUDGET =
            new java.util.concurrent.atomic.AtomicInteger(40);

    private OreHook() {
    }

    public static boolean on() {
        return ON;
    }

    /** World-load hook (CRUS_NATIVE_ORE=1): build the state tables, warm the
     *  reflection cache, swap the holder values reachable through the native
     *  plan. Any failure leaves the vanilla ores in place (ON stays false). */
    public static synchronized void setup(org.bukkit.World world, String[] states, String[][] plan) {
        if (ON || SWAPPED) {
            return;
        }
        try {
            if (UNSAFE == null) {
                throw new IllegalStateException("sun.misc.Unsafe unavailable");
            }
            if (plan == null) {
                throw new IllegalStateException("native plan is null (no deco engine)");
            }
            DecoDriver.setupOreTables(world, states);
            initReflection();
            int n = swapFeatures(plan);
            SWAPPED = true;
            ON = true;
            LOG.warning("ore hook ACTIVE: " + n + " ore features -> CrusstyOreFeature"
                    + " (native expansion inside vanilla decorate; CRUS_NATIVE_ORE=1)");
        } catch (Throwable t) {
            LOG.warning("ore hook setup failed (vanilla ores stay): " + t);
        }
    }

    private static void initReflection() throws Exception {
        if (F_WGR_SOURCE != null) {
            return;
        }
        Field src = WorldgenRandom.class.getDeclaredField("randomSource");
        src.setAccessible(true);
        Field eng = XoroshiroRandomSource.class.getDeclaredField("randomNumberGenerator");
        eng.setAccessible(true);
        Field lo = Xoroshiro128PlusPlus.class.getDeclaredField("seedLo");
        lo.setAccessible(true);
        Field hi = Xoroshiro128PlusPlus.class.getDeclaredField("seedHi");
        hi.setAccessible(true);
        F_WGR_SOURCE = src;
        F_XORO_ENGINE = eng;
        F_ENGINE_LO = lo;
        F_ENGINE_HI = hi;
    }

    /**
     * Raw engine state at place() entry: WorldgenRandom -> XoroshiroRandomSource
     * -> Xoroshiro128PlusPlus(seedLo, seedHi). null = not the decorate
     * xoroshiro engine (wrapped legacy, Paper random wrapper, ...) — the
     * caller falls back to vanilla (bit-exact).
     */
    static long[] engineState(RandomSource rs) {
        try {
            RandomSource src = rs;
            if (src instanceof WorldgenRandom wgr) {
                Object inner = F_WGR_SOURCE.get(wgr);
                if (!(inner instanceof RandomSource)) {
                    return null;
                }
                src = (RandomSource) inner;
            }
            if (!(src instanceof XoroshiroRandomSource xoro)) {
                return null;
            }
            Object eng = F_XORO_ENGINE.get(xoro);
            if (eng == null) {
                return null;
            }
            return new long[]{(long) F_ENGINE_LO.get(eng), (long) F_ENGINE_HI.get(eng)};
        } catch (Throwable t) {
            return null;
        }
    }

    /**
     * Unsafe swap of the {@code value} inside the Feature holder of every
     * ore placed feature reachable through the native plan's placed names.
     * Walking the plan (instead of blindly scanning the feature registry)
     * uses the proven registry idiom (DecoDriver delegate cache:
     * registryAccess -> lookupOrThrow(PLACED_FEATURE) -> get(Identifier))
     * AND guarantees every hooked feature is in the native registry (no -3
     * fallbacks). Runs once per JVM (registry is a frozen singleton;
     * CrusstyOreFeature instances are stateless beyond the bound name and
     * the original fallback instance).
     */
    private static int swapFeatures(String[][] plan) throws Exception {
        Field f = Holder.Reference.class.getDeclaredField("value");
        long off = UNSAFE.objectFieldOffset(f);
        var lookup = registryAccess().lookupOrThrow(Registries.PLACED_FEATURE);
        java.util.Set<Holder.Reference<Feature>> seen = new java.util.HashSet<>();
        int n = 0;
        for (String[] step : plan) {
            if (step == null) {
                continue;
            }
            for (String placedName : step) {
                if (placedName == null || placedName.isEmpty()) {
                    continue;
                }
                var holders = lookup.get(Identifier.parse(placedName));
                if (holders.isEmpty()) {
                    continue;
                }
                PlacedFeature pf = ((Holder.Reference<PlacedFeature>) holders.get()).value();
                Holder<Feature> h;
                try {
                    h = pf.feature();
                } catch (Throwable t) {
                    continue;
                }
                if (!(h instanceof Holder.Reference<Feature> ref)) {
                    continue; // direct/inline feature holder — not registry-swappable
                }
                if (!seen.add(ref)) {
                    continue;
                }
                if (!(ref.value() instanceof AbstractOreFeature orig)) {
                    continue;
                }
                String featName = ref.unwrapKey()
                        .map(k -> k.identifier().toString()).orElse(null);
                if (featName == null) {
                    continue;
                }
                UNSAFE.putObject(h, off, new CrusstyOreFeature(featName, orig));
                n++;
            }
        }
        return n;
    }

    /** RegistryAccess of the (single) pregen world — CraftWorld unwrap. */
    private static net.minecraft.core.RegistryAccess registryAccess() throws Exception {
        org.bukkit.World w = org.bukkit.Bukkit.getWorlds().get(0);
        Object handle = w.getClass().getMethod("getHandle").invoke(w);
        return ((ServerLevel) handle).registryAccess();
    }

    /**
     * The hook body (called from CrusstyOreFeature.place). Returns TRUE when
     * the native expansion produced ops and they were applied; NULL = fall
     * back to the vanilla feature (Java RNG untouched — bit-exact).
     */
    static Boolean place(String name, int oreSize, float oreDiscard,
                         String[] targetTags, String[] targetStates,
                         WorldGenLevel level, ChunkGenerator gen,
                         RandomSource random, BlockPos origin) {
        DecoDriver drv = DecoDriver.oreBridge();
        if (drv == null || !ON) {
            return null;
        }
        if (!(level instanceof WorldGenRegion region)) {
            return fallback(name, "level");
        }
        long[] st = engineState(random);
        if (st == null) {
            return fallback(name, "rng");
        }
        boolean audit = TRACE_LEFT.getAndDecrement() > 0 && AUDIT_BUDGET.getAndDecrement() > 0;
        java.util.ArrayList<String> vanillaCells = audit ? new java.util.ArrayList<>() : null;
        if (audit) {
            // the shadow reads the LIVE region via the bridge — must run
            // BEFORE the native's ops are applied (pre-vein state, the same
            // view vanilla's own doPlace would read)
            drv.hookRegion(region);
            try {
                traceShadow(name, oreSize, oreDiscard, targetTags, targetStates,
                        origin, st, drv, region, vanillaCells);
            } catch (Throwable t) {
                LOG.warning("ore-shadow failed: " + t);
            } finally {
                drv.hookRegion(null);
            }
        }
        ReentrantLock[] lockBox = new ReentrantLock[1];
        long handle = CrusstyChunkGenerator.decoAcquire(lockBox);
        if (handle == 0) {
            return fallback(name, "ctx");
        }
        try {
            ChunkPos center = region.getCenter();
            int cx = center.x();
            int cz = center.z();
            int ox = cx << 4;
            int oz = cz << 4;
            long[] lc = LAST_CHUNK.get();
            if (lc == null) {
                lc = new long[3];
                LAST_CHUNK.set(lc);
            }
            boolean chunkChanged = lc[2] == 0 || lc[0] != cx || lc[1] != cz;
            if (chunkChanged) {
                // audit the first veins of EVERY chunk (the transition surface
                // is the unaudited drift suspect — cont-99f audited only the
                // first chunk, where the native proved cell-exact)
                if (AUDIT_BUDGET.get() > 0) {
                    TRACE_LEFT.set(2);
                }
                // chunk retarget (same clears as the native per-run reset +
                // the snapshot origin/section retarget + biome zoom seed)
                long zoom = 0;
                try {
                    zoom = BiomeManager.obfuscateSeed(region.getSeed());
                } catch (Throwable ignored) {
                }
                ChunkGenNative.decoChunkBegin(handle, cx, cz, region.getMinSectionY(), zoom);
                lc[0] = cx;
                lc[1] = cz;
                lc[2] = 1;
            }
            int[] ops = OPS.get();
            if (ops == null) {
                ops = new int[6 * 65536];
                OPS.set(ops);
            }
            drv.hookRegion(region);
            int n;
            try {
                n = ChunkGenNative.oreRunConfigured(handle, st[0], st[1],
                        ox, region.getMinY(), oz, region.getMaxY(),
                        origin.getX(), origin.getY(), origin.getZ(),
                        name, drv, ops);
            } finally {
                drv.hookRegion(null);
            }
            if (n < 0) {
                // native refused: -1 (env/bridge/run failure) or -3 (name not
                // in the native registry). Java RNG untouched — the vanilla
                // feature runs from the EXACT state the modifiers left.
                return fallback(name, "native" + n);
            }
            if (audit) {
                LOG.warning("[audit-ctx] c=" + cx + "," + cz + " h=" + handle);
                for (String vc : vanillaCells) {
                    LOG.warning("[audit-vanilla] " + vc);
                }
                for (int i = 0; i < n; i++) {
                    LOG.warning("[audit-native] " + (ops[i * 6] + ox) + " " + ops[i * 6 + 1]
                            + " " + (ops[i * 6 + 2] + oz) + " "
                            + DecoDriver.stateAt(ops[i * 6 + 3]));
                }
                LOG.warning("[audit-end] c=" + cx + "," + cz + " h=" + handle
                        + " native_ops=" + n + " vanilla_cells=" + vanillaCells.size());
            }
            int total = 0;
            for (int i = 0; i < n; i++) {
                if (DecoDriver.applyOpStatic(region, ox, oz, ops, i * 6)) {
                    total++;
                }
            }
            RUNS.increment();
            OPS_APPLIED.add(total);
            rateLog();
            return Boolean.TRUE;
        } finally {
            CrusstyChunkGenerator.decoRelease(lockBox);
        }
    }

    /** Rate-limited fallback note: first 8 with reasons, then silent counting
     *  (a fallback-heavy run means the hook is not covering — investigate). */
    private static Boolean fallback(String name, String why) {
        FALLBACKS.increment();
        if (FALLBACKS.sum() <= 8) {
            LOG.warning("ore hook fallback (" + why + ") feature=" + name
                    + " -> vanilla OreFeature (bit-exact)");
        }
        return null;
    }

    // --- xoroshiro128++ continuation (26.3 engine semantics, from the
    // Xoroshiro128PlusPlus.nextLong bytecode) + the BitRandomSource default
    // draw shapes — used ONLY by the shadow trace on a local state copy ---

    private static long draw(long[] s) {
        long l = s[0];
        long m = s[1];
        long r = Long.rotateLeft(l + m, 17) + l;
        m ^= l;
        s[0] = Long.rotateLeft(l, 49) ^ m ^ (m << 21);
        s[1] = Long.rotateLeft(m, 28);
        return r;
    }

    private static int nextBits(long[] s, int bits) {
        return (int) (draw(s) >>> (64 - bits));
    }

    /** BitRandomSource.nextInt(bound) default: next(31) + rejection. */
    private static int nextIntB(long[] s, int bound) {
        if ((bound & -bound) == bound) {
            return (int) ((bound * (long) nextBits(s, 31)) >> 31);
        }
        int i;
        int r;
        do {
            i = nextBits(s, 31);
            r = i % bound;
        } while (i - r + (bound - 1) < 0);
        return r;
    }

    private static float nextF32(long[] s) {
        return nextBits(s, 24) * 5.9604645E-8f;
    }

    /** vanilla nextDouble: ((next(26) << 27) + next(27)) — quantized through
     *  f32 (the WgrRef golden-validated quirk the native mirror implements). */
    private static double nextF64(long[] s) {
        long l = ((long) nextBits(s, 26) << 27) + nextBits(s, 27);
        return (double) ((float) l * 1.1102230246251565e-16f);
    }

    /**
     * cont-99e FULL vanilla ore simulation in Java, from the captured raw
     * engine state: place() prologue (Math.sin/cos) + doPlace node cloud
     * (Mth.lerp + the REAL Mth.sin table + nextDouble segments) + sphere
     * elimination + cell loops + canPlaceOre evaluated through the bridge
     * reads (tagIs/isAir = the EXACT live-region view vanilla reads). Every
     * placed cell is logged as [audit-vanilla]; the native's ops are logged
     * as [audit-native] — the artifact diff is the cell-level verdict.
     */
    private static void traceShadow(String name, int size, float discard,
                                    String[] targetTags, String[] targetStates,
                                    BlockPos origin, long[] captured,
                                    DecoDriver drv, WorldGenRegion region,
                                    java.util.ArrayList<String> out) {
        long[] s = {captured[0], captured[1]};
        // --- place() prologue (26.3 bytecode-exact) ---
        float f = nextF32(s) * 3.1415927f;
        float f1 = (float) size / 8.0f;
        int j = net.minecraft.util.Mth.ceil(((float) size / 16.0f * 2.0f + 1.0f) / 2.0f);
        double minX = origin.getX() + Math.sin((double) f) * (double) f1;
        double maxX = origin.getX() - Math.sin((double) f) * (double) f1;
        double minZ = origin.getZ() + Math.cos((double) f) * (double) f1;
        double maxZ = origin.getZ() - Math.cos((double) f) * (double) f1;
        double minY = origin.getY() + nextIntB(s, 3) - 2;
        double maxY = origin.getY() + nextIntB(s, 3) - 2;
        int x0 = origin.getX() - net.minecraft.util.Mth.ceil(f1) - j;
        int y0 = origin.getY() - 2 - j;
        int z0 = origin.getZ() - net.minecraft.util.Mth.ceil(f1) - j;
        int w = 2 * (net.minecraft.util.Mth.ceil(f1) + j);
        int h = 2 * (2 + j);
        out.add("hdr " + name + " o=(" + origin.getX() + "," + origin.getY() + "," + origin.getZ()
                + ") size=" + size + " disc=" + discard + " st=(" + captured[0] + "," + captured[1]
                + ") f=" + f + " box=(" + x0 + "," + y0 + "," + z0 + ") w=" + w + " h=" + h);
        // --- the OF_WG gate: any column with y0 <= height passes ---
        boolean gate = false;
        outer:
        for (int gx = x0; gx <= x0 + w; gx++) {
            for (int gz = z0; gz <= z0 + w; gz++) {
                if (y0 <= drv.height(2, gx, gz)) {
                    gate = true;
                    break outer;
                }
            }
        }
        if (!gate) {
            out.add("gate=false");
            return;
        }
        // --- doPlace node cloud (Mth.lerp + the REAL Mth.sin table) ---
        double[] px = new double[size];
        double[] py = new double[size];
        double[] pz = new double[size];
        double[] pr = new double[size];
        for (int i = 0; i < size; i++) {
            float fr = (float) i / (float) size;
            px[i] = net.minecraft.util.Mth.lerp(fr, minX, maxX);
            py[i] = net.minecraft.util.Mth.lerp(fr, minY, maxY);
            pz[i] = net.minecraft.util.Mth.lerp(fr, minZ, maxZ);
            double d = nextF64(s) * (double) size / 16.0;
            float f2 = net.minecraft.util.Mth.sin((double) (3.1415927f * fr));
            pr[i] = ((double) (f2 + 1.0f) * d + 1.0) / 2.0;
        }
        // --- sphere elimination ---
        for (int i2 = 0; i2 < size - 1; i2++) {
            if (pr[i2] <= 0.0) continue;
            for (int i3 = i2 + 1; i3 < size; i3++) {
                if (pr[i3] <= 0.0) continue;
                double dd = pr[i2] - pr[i3];
                if (dd * dd > (px[i2] - px[i3]) * (px[i2] - px[i3])
                        + (py[i2] - py[i3]) * (py[i2] - py[i3])
                        + (pz[i2] - pz[i3]) * (pz[i2] - pz[i3])) {
                    if (dd > 0.0) pr[i3] = -1.0;
                    else pr[i2] = -1.0;
                }
            }
        }
        // --- cell loops (clamped-low bounds + vanilla BitSet-index dedup) ---
        java.util.HashSet<Long> tested = new java.util.HashSet<>();
        int ox = region.getCenter().x() << 4;
        int oz = region.getCenter().z() << 4;
        for (int i2 = 0; i2 < size; i2++) {
            double rad = pr[i2];
            if (rad < 0.0) continue;
            int bxLo = Math.max((int) Math.floor(px[i2] - rad), x0);
            int byLo = Math.max((int) Math.floor(py[i2] - rad), y0);
            int bzLo = Math.max((int) Math.floor(pz[i2] - rad), z0);
            int bxHi = Math.max((int) Math.floor(px[i2] + rad), bxLo);
            int byHi = Math.max((int) Math.floor(py[i2] + rad), byLo);
            int bzHi = Math.max((int) Math.floor(pz[i2] + rad), bzLo);
            for (int ix = bxLo; ix <= bxHi; ix++) {
                double dx = (ix + 0.5 - px[i2]) / rad;
                if (!(dx * dx < 1.0)) continue;
                for (int iy = byLo; iy <= byHi; iy++) {
                    double dy = (iy + 0.5 - py[i2]) / rad;
                    if (!(dx * dx + dy * dy < 1.0)) continue;
                    for (int iz = bzLo; iz <= bzHi; iz++) {
                        double dz = (iz + 0.5 - pz[i2]) / rad;
                        if (!(dx * dx + dy * dy + dz * dz < 1.0)) continue;
                        long bi = (long) (ix - x0) + (long) (iy - y0) * w + (long) (iz - z0) * w * h;
                        if (!tested.add(bi)) continue;
                        // ensureCanWrite: the 3x3 region write bounds
                        if (ix < ox - 16 || ix > ox + 47 || iz < oz - 16 || iz > oz + 47) {
                            out.add("vw " + ix + " " + iy + " " + iz + " out-of-region");
                            continue;
                        }
                        if (iy < region.getMinY() || iy > region.getMaxY()) {
                            out.add("vw " + ix + " " + iy + " " + iz + " outside-height");
                            continue;
                        }
                        // --- canPlaceOre (26.3) through the bridge reads ---
                        boolean placed = false;
                        for (int ti = 0; ti < targetTags.length && !placed; ti++) {
                            String tt = targetTags[ti];
                            boolean tagHit = tt.startsWith("minecraft:")
                                    ? drv.tagIs(ix, iy, iz, tt)
                                    : false;
                            if (!tagHit) {
                                out.add("vc " + ix + " " + iy + " " + iz + " t" + ti
                                        + " tag=false read=" + drv.stateFull(ix, iy, iz));
                                continue;
                            }
                            boolean skip;
                            if (discard <= 0.0f) {
                                skip = true;
                            } else if (discard >= 1.0f) {
                                skip = false;
                            } else {
                                skip = nextF32(s) < discard;
                            }
                            if (skip) {
                                placed = true;
                            } else {
                                boolean air = drv.isAir(ix + 1, iy, iz) || drv.isAir(ix - 1, iy, iz)
                                        || drv.isAir(ix, iy + 1, iz) || drv.isAir(ix, iy - 1, iz)
                                        || drv.isAir(ix, iy, iz + 1) || drv.isAir(ix, iy, iz - 1);
                                placed = !air;
                                if (air) {
                                    out.add("vc " + ix + " " + iy + " " + iz + " t" + ti
                                            + " air=true");
                                }
                            }
                            if (placed) {
                                out.add("vw " + ix + " " + iy + " " + iz + " t" + ti);
                            }
                        }
                    }
                }
            }
        }
    }

    private static void rateLog() {
        long r = RUNS.sum();
        if ((r & 4095) == 0) {
            long now = System.currentTimeMillis();
            if (now - LAST_LOG_MS > 20_000) {
                LAST_LOG_MS = now;
                LOG.warning("ore hook: runs=" + r + " ops=" + OPS_APPLIED.sum()
                        + " fallbacks=" + FALLBACKS.sum());
            }
        }
    }
}
