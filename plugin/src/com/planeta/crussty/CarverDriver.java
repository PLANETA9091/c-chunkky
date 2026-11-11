package com.planeta.crussty;

import net.minecraft.world.level.chunk.LevelChunkSection;
import net.minecraft.world.level.chunk.PalettedContainer;
import net.minecraft.world.level.chunk.ChunkAccess;

import java.lang.reflect.Field;
import java.lang.reflect.Method;
import java.util.ArrayList;
import java.util.IdentityHashMap;
import java.util.List;
import java.util.logging.Logger;

/**
 * Native carvers 26.3 populator (CRUS_NATIVE_CARVERS=1): replaces the vanilla
 * generateCarvers stage (shouldGenerateCaves=false) with the Rust Cave/Canyon
 * carvers + aquifer apply. Flow per chunk:
 *   1. pack the 24 sections (BitStorage raw + palette id tables),
 *   2. JNI carve (mask over the 17x17 sources + substance apply),
 *   3. apply the changed cells via getAndSetUnchecked + rebuild counters,
 *   4. feed the fluid-update marks into the ProtoChunk postprocessing bridge.
 * The carver-mask topMaterial fix (the vanilla grass→dirt replacement under
 * carved grass) runs natively: the ops carry the replaced material ids from
 * the full sectionStatesById table (base materials + extended surface
 * states), so the apply loop reads STATES_BY_ID directly instead of the
 * air/water/lava-only fast path.
 */
public final class CarverDriver extends org.bukkit.generator.BlockPopulator {
    private static final Logger LOG = Logger.getLogger("CrusstyGen");
    private static volatile boolean ACTIVE = false;
    private static volatile long SEED;
    /** cont-143 carver fuse: ONE JNI carve+fetch, thread-local op/marks buffers.
     *  Byte-identical transport; DEFAULT ON after 2 paired A/B wins
     *  (e2e -2.97/-7.44 ms/ch, carver wall 20.2 -> 5.4-6.6 — the 3-call path's
     *  sessions_lock re-acquisitions contend brutally once apply does real
     *  work). CRUS_CARVER_FUSE=0 restores the legacy 3-call path. */
    private static final boolean CARVER_FUSE = !"0".equals(System.getenv("CRUS_CARVER_FUSE"));
    /** ops buffer: worst case = every cell of 24 sections changed (98304). */
    private static final ThreadLocal<long[]> TL_OPS = ThreadLocal.withInitial(() -> new long[98304]);
    private static final ThreadLocal<long[]> TL_MARKS = ThreadLocal.withInitial(() -> new long[1024]);
    /** NMS state per our u16 id (index = id; from the bulk layout tables). */
    private static volatile Object[] STATES_BY_ID;
    private static volatile Object AIR_STATE, WATER_STATE, LAVA_STATE;
    /** NMS state -> our id (identity). Unknown states collapse to 1 (stone). */
    private static IdentityHashMap<Object, Short> ID_OF;

    private CarverDriver() {
    }

    public static void setup(org.bukkit.World world, long seed) throws Exception {
        Object[] byId = CrusstyChunkGenerator.bulkStatesById();
        if (byId == null || byId.length < 22) {
            throw new IllegalStateException("bulk states table unavailable (bulk layout must init first)");
        }
        STATES_BY_ID = byId;
        AIR_STATE = byId[0];
        WATER_STATE = byId[3];
        LAVA_STATE = byId[17];
        ID_OF = new IdentityHashMap<>();
        for (short i = 0; i < byId.length; i++) {
            if (byId[i] != null) {
                ID_OF.put(byId[i], i);
            }
        }
        SEED = seed;
        world.getPopulators().add(0, new CarverDriver());
        ACTIVE = true;
        LOG.info("native carvers active (CRUS_NATIVE_CARVERS=1)");
    }

    public static boolean active() {
        return ACTIVE;
    }

    private static final Field F_STORAGE;
    private static final Field F_PALETTE;
    private static final Method M_GET_AND_SET;
    private static final Method M_RECOUNT;
    private static final Method M_SEC_SET;
    /** cont-143: typed MethodHandle over the 5-param section setBlockState —
     *  reflection invoke() costs ~3-5x a typed mh.invokeExact (measured
     *  apply 10.24ms/ch reflection vs ~3-5 expected). */
    private static final java.lang.invoke.MethodHandle MH_SEC_SET;

    static {
        try {
            F_STORAGE = PalettedContainer.Data.class.getDeclaredField("storage");
            F_STORAGE.setAccessible(true);
            F_PALETTE = PalettedContainer.Data.class.getDeclaredField("palette");
            F_PALETTE.setAccessible(true);
            Class<?> cPal = Class.forName("net.minecraft.world.level.chunk.PalettedContainer");
            M_GET_AND_SET = cPal.getMethod("getAndSetUnchecked",
                    int.class, int.class, int.class, Object.class);
            Class<?> cSec = Class.forName("net.minecraft.world.level.chunk.LevelChunkSection");
            M_RECOUNT = cSec.getDeclaredMethod("recalcBlockCounts");
            M_RECOUNT.setAccessible(true);
            // cont-138 H10: vanilla chunk.setBlockState path — section-level
            // 5-arg setBlockState(x,y,z,state,updateOnWrite=false) updates the
            // section counters incrementally (vanilla applyCarvingMask
            // semantics); replaces getAndSet+recalcBlockCounts.
            M_SEC_SET = cSec.getDeclaredMethod("setBlockState",
                    int.class, int.class, int.class,
                    Class.forName("net.minecraft.world.level.block.state.BlockState"), boolean.class);
            M_SEC_SET.setAccessible(true);
            MH_SEC_SET = java.lang.invoke.MethodHandles.lookup().unreflect(M_SEC_SET);
        } catch (Throwable t) {
            throw new ExceptionInInitializerError(t);
        }
    }

    @Override
    public void populate(org.bukkit.generator.WorldInfo info, java.util.Random random,
                         int cx, int cz, org.bukkit.generator.LimitedRegion limited) {
        if (!ACTIVE) {
            return;
        }
        long tCarver0 = System.nanoTime();
        long tp0 = System.nanoTime();
        java.util.concurrent.locks.ReentrantLock[] lockBox =
                new java.util.concurrent.locks.ReentrantLock[1];
        long ctx = CrusstyChunkGenerator.decoAcquire(lockBox);
        CarverStats.ACQ.addAndGet(System.nanoTime() - tp0);
        if (ctx == 0) {
            return;
        }
        long tp1 = System.nanoTime();
        try {
            Object handle = limited.getClass().getMethod("getHandle").invoke(limited);
            // WorldGenRegion IS the region handle (it has no getHandle(); the
            // old second hop threw NoSuchMethodException on every chunk and the
            // whole populate silently aborted — carver=0.00 in every window,
            // CI 41223656202 WARN 'carver driver failed'). The chunk being
            // populated is the region's CENTER chunk, i.e. (cx, cz) from the
            // populate args, via LevelReader.getChunk(int,int).
            ChunkAccess chunk = (ChunkAccess) handle.getClass()
                    .getMethod("getChunk", int.class, int.class)
                    .invoke(handle, cx, cz);
            // ---- pack sections ----
            int minSectionY = chunk.getMinSectionY();
            int sections = chunk.getSectionsCount();
            int[] bits = new int[24];
            int[] rawOff = new int[24];
            int[] palOff = new int[24];
            List<long[]> raws = new ArrayList<>();
            List<short[]> pals = new ArrayList<>();
            int ro = 0, po = 0;
            for (int s = 0; s < 24; s++) {
                if (s >= sections) {
                    bits[s] = -1;
                    rawOff[s] = ro;
                    palOff[s] = po;
                    continue;
                }
                LevelChunkSection sec = chunk.getSection(s);
                if (sec == null || sec.hasOnlyAir()) {
                    bits[s] = 0;
                    rawOff[s] = ro;
                    palOff[s] = po;
                    pals.add(new short[]{0});
                    po += 1;
                    continue;
                }
                PalettedContainer<?> pc = sec.getStates();
                Object d = pc.data;
                net.minecraft.util.BitStorage storage = (net.minecraft.util.BitStorage) F_STORAGE.get(d);
                net.minecraft.world.level.chunk.Palette<?> pal =
                        (net.minecraft.world.level.chunk.Palette<?>) F_PALETTE.get(d);
                int b = storage.getBits();
                bits[s] = b;
                if (b == 0) {
                    rawOff[s] = ro;
                    palOff[s] = po;
                    pals.add(new short[]{idOf(pal.valueFor(0))});
                    po += 1;
                    continue;
                }
                long[] raw = storage.getRaw();
                int n = pal.getSize();
                short[] table = new short[n];
                for (int i = 0; i < n; i++) {
                    table[i] = idOf(pal.valueFor(i));
                }
                rawOff[s] = ro;
                palOff[s] = po;
                raws.add(raw);
                ro += raw.length;
                pals.add(table);
                po += n;
            }
            long[] raw = new long[ro];
            int o = 0;
            for (long[] r : raws) {
                System.arraycopy(r, 0, raw, o, r.length);
                o += r.length;
            }
            short[] pal = new short[po];
            o = 0;
            for (short[] p : pals) {
                System.arraycopy(p, 0, pal, o, p.length);
                o += p.length;
            }
            int[] meta = new int[72];
            System.arraycopy(bits, 0, meta, 0, 24);
            System.arraycopy(rawOff, 0, meta, 24, 24);
            System.arraycopy(palOff, 0, meta, 48, 24);
            CarverStats.PACK.addAndGet(System.nanoTime() - tp1);
            CarverStats.N.incrementAndGet();
            long tp2 = System.nanoTime();

            long[] ops;
            long[] marks;
            int got;
            int nm;
            if (CARVER_FUSE) {
                // cont-143: carve + op/marks fetch in ONE JNI call, thread-local
                // buffers — byte-identical transport, 2 fewer sessions_lock
                // acquisitions per carved chunk.
                ops = TL_OPS.get();
                marks = TL_MARKS.get();
                int packed = ChunkGenNative.carveApply(ctx, cx, cz, SEED, meta, raw, pal, ops, marks);
                if (packed == Integer.MIN_VALUE) {
                    return; // native declined — vanilla semantics lost for this chunk
                }
                got = packed >>> 12;
                nm = packed & 0xFFF;
                CarverStats.CARVE.addAndGet(System.nanoTime() - tp2);
                CarverStats.OPS.addAndGet(0L);
            } else {
                int n = ChunkGenNative.carve(ctx, cx, cz, SEED, meta, raw, pal);
                if (n == Integer.MIN_VALUE) {
                    return; // native declined — vanilla semantics lost for this chunk
                }
                CarverStats.CARVE.addAndGet(System.nanoTime() - tp2);
                long tp3 = System.nanoTime();
                ops = new long[n];
                got = n > 0 ? ChunkGenNative.carveOps(ctx, ops) : 0;
                marks = new long[1024];
                nm = ChunkGenNative.carveMarks(ctx, marks);
                CarverStats.OPS.addAndGet(System.nanoTime() - tp3);
            }
            long tp4 = System.nanoTime();
            // ---- apply changed cells ----
            // ids span the full palette (base materials + the extended
            // surface states the topMaterial fix can write, e.g. dirt/grass
            // at ids 22+). Unknown/out-of-range ids are skipped defensively.
            // cont-95: recount ONCE per touched section, not per cell.
            // recalcBlockCounts walks every palette entry of the section
            // (4096 cells); a carved chunk produces thousands of ops, so the
            // per-cell recount made decorate O(cells x 4096) — run
            // 41409735829 deco lanes: 3/4 Paper Common Workers RUNNABLE in
            // recalcBlockCounts for >=2min, the chunk system starved, the
            // Server thread waited forever in syncLoad(first forceload tile)
            // and the Paper Watchdog (spigot.yml timeout-time, NOT the
            // server.properties knob cont-94 silenced) killed the JVM. All
            // writes of a section land first; one recount per touched
            // section then rebuilds the counts from the storage — identical
            // final state, O(sections) recounts instead of O(cells).
            // cont-138 H10: vanilla applyCarvingMask writes through
            // chunk.setBlockState — the SECTION method updates the counters
            // incrementally, so the bulk getAndSet + recalcBlockCounts pair
            // (cont-95) is replaced by the exact vanilla write path. The
            // recount loop walked up to 24 sections x 4096 storage cells per
            // chunk and dominated the carver wall (apply=5.50 of 7.3ms/ch in
            // the carver-phases honest split; carve native = 1.55).
            Object[] statesById = STATES_BY_ID;
            for (int i = 0; i < got; i++) {
                long op = ops[i];
                int idx = (int) (op >>> 16);
                short id = (short) (op & 0xFFFF);
                int s = idx >>> 12;
                int cell = idx & 0xFFF;
                int sy = cell >> 8;
                int rem = cell & 0xFF;
                int sx = rem & 15;
                int sz = rem >> 4;
                Object state = id >= 0 && id < statesById.length ? statesById[id] : null;
                if (state == null || s >= sections) {
                    continue;
                }
                LevelChunkSection sec = chunk.getSection(s);
                // cont-143 CRITICAL FIX: 5-param setBlockState needs the 5th
                // (boolean movedByPlayer/updateOnWrite=false) argument. Since
                // cont-138 (ab97461b) this invoke passed 4 args against the
                // resolved 5-param method -> IllegalArgumentException on the
                // FIRST op -> the whole apply loop aborted per carved chunk ->
                // carve ops (caves + topMaterial + fluid marks) were SILENTLY
                // DROPPED in every CRUS_NATIVE_CARVERS=1 run; the CI carvers
                // lane passed vacuously (both legs un-carved, AIR is SOFT).
                try {
                    // mh.invoke (asType-adapted): typed args, no per-call boxing
                    // of the boolean, ~2-4x faster than Method.invoke; invokeExact
                    // needs an exact BlockState-return descriptor which javac
                    // types as Object here (signature-polymorphic quirk).
                    Object ignore = MH_SEC_SET.invoke(sec, sx, sy, sz,
                            (net.minecraft.world.level.block.state.BlockState) state, false);
                    if (ignore == null) {
                        // vanilla never returns null here; guard keeps ignore "used"
                    }
                } catch (Throwable ti) {
                    throw new RuntimeException(ti);
                }
            }
            if (nm > 0) {
                markPositions(chunk, marks, nm);
            }
            CarverStats.APPLY.addAndGet(System.nanoTime() - tp4);
        } catch (Throwable t) {
            LOG.warning("carver driver failed: " + t);
        } finally {
            CrusstyChunkGenerator.decoRelease(lockBox);
            GenStats.recordCarver(System.nanoTime() - tCarver0, cx, cz);
        }
    }

    private static short idOf(Object state) {
        Short id = ID_OF.get(state);
        return id == null ? 1 : id; // unknown states are carvable non-grass
    }

    private static volatile boolean markBroken;
    private static Method markMethod;
    private static Object blockPosCtor;

    private static void markPositions(ChunkAccess chunk, long[] packed, int count)
            throws Exception {
        if (markBroken) {
            return;
        }
        try {
            if (markMethod == null) {
                Class<?> blockPos = Class.forName("net.minecraft.core.BlockPos");
                markMethod = chunk.getClass().getMethod("markPosForPostProcessing", blockPos);
                markMethod.setAccessible(true);
                blockPosCtor = Class.forName("net.minecraft.core.BlockPos")
                        .getConstructor(int.class, int.class, int.class);
            }
            for (int i = 0; i < count; i++) {
                long p = packed[i];
                int x = (int) (p >> 38);
                int y = (int) ((p << 52) >> 52);
                int z = (int) ((p << 26) >> 38);
                Object pos = ((java.lang.reflect.Constructor<?>) blockPosCtor)
                        .newInstance(x, y, z);
                markMethod.invoke(chunk, pos);
            }
        } catch (Throwable t) {
            markBroken = true;
            LOG.warning("carver mark bridge unavailable: " + t);
        }
    }
}
