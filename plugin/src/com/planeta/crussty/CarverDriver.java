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
 * KNOWN GAP (documented): the vanilla grass→dirt topMaterial replacement
 * under carved grass is deferred to the MaterialSystem port (vanilla keeps
 * the dirt when the optional is empty — the common case).
 */
public final class CarverDriver extends org.bukkit.generator.BlockPopulator {
    private static final Logger LOG = Logger.getLogger("CrusstyGen");
    private static volatile boolean ACTIVE = false;
    private static volatile long SEED;
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
        java.util.concurrent.locks.ReentrantLock[] lockBox =
                new java.util.concurrent.locks.ReentrantLock[1];
        long ctx = CrusstyChunkGenerator.decoAcquire(lockBox);
        if (ctx == 0) {
            return;
        }
        try {
            Object handle = limited.getClass().getMethod("getHandle").invoke(limited);
            ChunkAccess chunk = (ChunkAccess) handle.getClass().getMethod("getHandle").invoke(handle);
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

            int n = ChunkGenNative.carve(ctx, cx, cz, SEED, meta, raw, pal);
            if (n == Integer.MIN_VALUE) {
                return; // native declined — vanilla semantics lost for this chunk
            }
            long[] ops = new long[n];
            int got = n > 0 ? ChunkGenNative.carveOps(ctx, ops) : 0;
            long[] marks = new long[1024];
            int nm = ChunkGenNative.carveMarks(ctx, marks);
            // ---- apply changed cells ----
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
                Object state = switch (id) {
                    case 0 -> AIR_STATE;
                    case 3 -> WATER_STATE;
                    case 17 -> LAVA_STATE;
                    default -> null;
                };
                if (state == null || s >= sections) {
                    continue;
                }
                LevelChunkSection sec = chunk.getSection(s);
                PalettedContainer<?> pc = sec.getStates();
                M_GET_AND_SET.invoke(pc, sx, sy, sz, state);
                M_RECOUNT.invoke(sec);
            }
            if (nm > 0) {
                markPositions(chunk, marks, nm);
            }
        } catch (Throwable t) {
            LOG.warning("carver driver failed: " + t);
        } finally {
            CrusstyChunkGenerator.decoRelease(lockBox);
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
