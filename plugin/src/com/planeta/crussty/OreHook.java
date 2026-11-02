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
    static Boolean place(String name, WorldGenLevel level, ChunkGenerator gen,
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
            if (lc[2] == 0 || lc[0] != cx || lc[1] != cz) {
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
