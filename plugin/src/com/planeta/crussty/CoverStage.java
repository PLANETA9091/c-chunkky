package com.planeta.crussty;

import org.bukkit.World;

import java.lang.reflect.Field;
import java.lang.reflect.Method;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.atomic.AtomicLong;
import java.util.logging.Logger;

/**
 * cont-142-E (Q6 cover lever): MARGIN STAGING — pre-stage the dependency
 * margin rings the moment the honest window is armed, at LOW priority, so
 * the STRUCTURE_STARTS/BIOMES wave that today gates the first in-window
 * TERRAIN fill (the "ramp" + dependency-wave idle) runs BEFORE/UNDER the
 * window's own work instead of serializing it.
 *
 * MECHANISM (decompile-verified, paper-26.3 build 222):
 * - ChunkTaskScheduler.beginChunkLoadForNonFullSync(int,int,ChunkStatus,
 *   Priority) is PUBLIC (javap) and schedules a gen target for a chunk
 *   WITHOUT adding a ticket: holder.raisePriority(prio) +
 *   upgradeGenTarget(status) + schedule(...) — the same code path the
 *   dependency walk itself uses (ChunkTaskScheduler L396-440). No ticket is
 *   added, so the chunk remains unload-eligible exactly like today's
 *   walk-dragged margin chunks; checkUnload reaps it after completion.
 * - The staged set is a STRICT SUBSET of the work the control run already
 *   performs: a fresh window walks its r8 STRUCTURE_STARTS ring (outside
 *   distance <= 8), distance-2 BIOMES (deps of ring-1 TERRAIN) and the
 *   distance-1 TERRAIN ring (deps of window FEATURES) — we queue exactly
 *   those chunks, only earlier and at LOW priority. ZERO extra fills.
 * - Priority.LOW means workers only pick staged work when no NORMAL
 *   (window) task is ready: staged margin work fills the parking gaps
 *   instead of queueing ahead of inside work. When a window chunk later
 *   blocks on a staged chunk, the walk's checkNeighbour raises the staged
 *   chunk's priority back (NewChunkHolder.raisePriority) — deadlock-free,
 *   just-in-time.
 *
 * WHAT IS DELIBERATELY NOT TOUCHED: FEATURES / LIGHT / SPAWN / FULL
 * targets, inside-window chunks (stage 0..1), executor/pool config, the
 * Rust library, server properties. Staged steps (EMPTY/STRUCTURE_STARTS/
 * BIOMES/TERRAIN) all have writeRadius 0 (ChunkPyramid GENERATION_PYRAMID,
 * ChunkPyramid.java:20) — they never write outside their own chunk, so
 * their relative order is content-irrelevant; every cross-chunk READ they
 * need is still enforced by ChunkStep requirements + checkNeighbour before
 * task creation. See docs/cover-REPORT.md for the full safety argument
 * and the A/B protocol (parity compare is MANDATORY before adoption).
 *
 * Kill-switch: CRUS_COVER_STAGE=0 (default, off) / 1 (SS+BIOMES rings) /
 * 2 (+TERRAIN ring). CRUS_COVER_PRIO overrides the staged priority
 * (default LOW; ablation knob). Every reflection failure permanently
 * disables the lever (vanilla timing stays) — mirrors RingsWarmup.
 */
public final class CoverStage {
    public static final String ENV_STAGE = "CRUS_COVER_STAGE";
    public static final String ENV_PRIO = "CRUS_COVER_PRIO";

    private static final Logger LOG = Logger.getLogger("CrusstyGen");

    // ring radii from the GENERATION_PYRAMID (ChunkPyramid.java:20):
    // TERRAIN/BIOMES/FEATURES all require STRUCTURE_STARTS at r8; FEATURES
    // requires TERRAIN r1; TERRAIN requires BIOMES r1.
    private static final int SS_RING = 8;      // STRUCTURE_STARTS drag
    private static final int BIOMES_RING = 2;  // BIOMES for ring-1 TERRAIN
    private static final int TERRAIN_RING = 1; // TERRAIN for window FEATURES

    private static volatile boolean ready;
    private static volatile boolean broken;
    private static final Object INIT_LOCK = new Object();

    // cached NMS handles (all reflective — no net.minecraft imports: the
    // concurrentutil/Priority + moonrise scheduler classes are not all on
    // the companion compile classpath, and reflection survives remapping)
    private static Method mGetHandle;            // CraftWorld -> ServerLevel
    private static Method mGetScheduler;         // ServerLevel.moonrise$getChunkTaskScheduler
    private static Method mBegin;                // ChunkTaskScheduler.beginChunkLoadForNonFullSync
    private static Object ST_SS, ST_BIOMES, ST_TERRAIN; // ChunkStatus constants
    private static Object PRIO;                  // Priority constant
    private static volatile org.bukkit.plugin.java.JavaPlugin PLUGIN;
    private static volatile org.bukkit.scheduler.BukkitTask TIMER;

    // active window staging state
    private static volatile int[] pendingX, pendingZ;
    private static volatile Object[] pendingStatus;
    private static volatile boolean[] pendingDone;
    private static volatile int attemptsLeft;
    private static volatile World stageWorld;

    private static final AtomicLong STAGED_SS = new AtomicLong();
    private static final AtomicLong STAGED_BIOMES = new AtomicLong();
    private static final AtomicLong STAGED_TERRAIN = new AtomicLong();
    private static final AtomicLong ATTEMPTS = new AtomicLong();
    private static volatile String lastStage = "never armed";

    private CoverStage() {
    }

    public static boolean enabled() {
        return stage() > 0;
    }

    private static int stage() {
        try {
            return Integer.parseInt(System.getenv(ENV_STAGE).trim());
        } catch (Throwable t) {
            return 0;
        }
    }

    private static String prioName() {
        String p = System.getenv(ENV_PRIO);
        return (p == null || p.isBlank()) ? "LOW" : p.trim();
    }

    /** Called reflectively from CrusstyGenPlugin right after GenStats.window
     *  arms the rect (window command). The forceload command lands one rcon
     *  round-trip later, so a 2-tick retry timer wins the race and stages as
     *  soon as the ticket pass has created the margin holders. */
    public static void onWindowArmed(org.bukkit.plugin.java.JavaPlugin plugin,
                                     World world, int rx, int rz, int n) {
        PLUGIN = plugin;
        cancelTimer();
        int st = stage();
        if (st <= 0 || world == null) {
            lastStage = "off (stage=" + st + ")";
            return;
        }
        if (!initHandles()) {
            lastStage = "broken (reflection init failed)";
            return;
        }
        // Build the target lists: window rect is [rx..rx+n] inclusive.
        // outside-distance d of chunk (x,z) = Chebyshev distance outside the
        // rect (0 when inside). Staged set = control's own margin work:
        //   stage 1: STRUCTURE_STARTS for outside-distance 1..8 — kills the
        //     SS wave that gates the first BIOMES/TERRAIN creations. SS has
        //     writeRadius 0 and its pre-completion does not reorder the
        //     BIOMES/TERRAIN queues (they stay walk-order in both variants).
        //   stage 2: + BIOMES for outside-distance 1..2 and TERRAIN for
        //     outside-distance 1 (the full margin the window drags). This
        //     pre-completes deps of edge-chunk FEATURES => edge deco can run
        //     earlier => DECO-ORDER PARITY A/B IS MANDATORY for stage 2.
        List<int[]> tx = new ArrayList<>();
        List<Object[]> ts = new ArrayList<>();
        int ssCount = 0, bioCount = 0, terCount = 0;
        for (int dz = -SS_RING; dz <= n + SS_RING; dz++) {
            for (int dx = -SS_RING; dx <= n + SS_RING; dx++) {
                // outside-distance of the candidate chunk
                int ox = dx < 0 ? -dx : Math.max(0, dx - n);
                int oz = dz < 0 ? -dz : Math.max(0, dz - n);
                int od = Math.max(ox, oz);
                if (od == 0 || od > SS_RING) {
                    continue; // interior (window's own tickets) or beyond r8 drag
                }
                Object status;
                if (st >= 2 && od <= TERRAIN_RING) {
                    status = ST_TERRAIN;
                    terCount++;
                } else if (st >= 2 && od <= BIOMES_RING) {
                    status = ST_BIOMES;
                    bioCount++;
                } else {
                    status = ST_SS;
                    ssCount++;
                }
                tx.add(new int[]{rx + dx, rz + dz});
                ts.add(new Object[]{status});
            }
        }
        int[] px = new int[tx.size()];
        int[] pz = new int[tx.size()];
        Object[] ps = new Object[tx.size()];
        boolean[] pd = new boolean[tx.size()];
        for (int i = 0; i < tx.size(); i++) {
            px[i] = tx.get(i)[0];
            pz[i] = tx.get(i)[1];
            ps[i] = ts.get(i)[0];
        }
        pendingX = px;
        pendingZ = pz;
        pendingStatus = ps;
        pendingDone = pd;
        stageWorld = world;
        attemptsLeft = 25;
        STAGED_SS.set(0);
        STAGED_BIOMES.set(0);
        STAGED_TERRAIN.set(0);
        ATTEMPTS.set(0);
        lastStage = "armed stage=" + st + " prio=" + prioName()
                + " targets=" + tx.size() + " (ss=" + ssCount
                + " biomes=" + bioCount + " terrain=" + terCount + ")";
        LOG.info("[cover-stage] " + lastStage);
        if (pd.length == 0) {
            return;
        }
        TIMER = plugin.getServer().getScheduler().runTaskTimer(plugin,
                CoverStage::attempt, 2L, 2L);
    }

    /** One timer tick: stage every still-pending chunk. Idempotent —
     *  beginChunkLoadForNonFullSync returns true both when the chunk is
     *  already at/after the target status and when the gen target was set
     *  (upgradeGenTarget is a no-op for existing higher targets). */
    private static void attempt() {
        final int[] px = pendingX;
        final int[] pz = pendingZ;
        final Object[] ps = pendingStatus;
        final boolean[] pd = pendingDone;
        final World w = stageWorld;
        if (px == null || pz == null || pd == null || w == null) {
            cancelTimer();
            return;
        }
        if (attemptsLeft-- <= 0) {
            LOG.info("[cover-stage] give-up after 25 ticks: " + statsLine());
            cancelTimer();
            return;
        }
        Object scheduler = schedulerFor(w);
        if (scheduler == null) {
            return; // retry next tick (forceload not landed yet)
        }
        ATTEMPTS.incrementAndGet();
        int left = 0;
        try {
            for (int i = 0; i < px.length; i++) {
                if (pd[i]) {
                    continue;
                }
                Object r = mBegin.invoke(scheduler, px[i], pz[i], ps[i], PRIO);
                boolean ok = r instanceof Boolean b && b;
                if (ok) {
                    pd[i] = true;
                    if (ps[i] == ST_TERRAIN) {
                        STAGED_TERRAIN.incrementAndGet();
                    } else if (ps[i] == ST_BIOMES) {
                        STAGED_BIOMES.incrementAndGet();
                    } else {
                        STAGED_SS.incrementAndGet();
                    }
                } else {
                    left++;
                }
            }
        } catch (Throwable t) {
            broken = true;
            cancelTimer();
            LOG.warning("[cover-stage] staging invoke failed, lever disabled: " + root(t));
            return;
        }
        if (left == 0) {
            lastStage = "staged prio=" + prioName() + " ss=" + STAGED_SS.get()
                    + " biomes=" + STAGED_BIOMES.get()
                    + " terrain=" + STAGED_TERRAIN.get()
                    + " attempts=" + ATTEMPTS.get();
            LOG.info("[cover-stage] " + lastStage);
            cancelTimer();
        }
    }

    private static void cancelTimer() {
        org.bukkit.scheduler.BukkitTask t = TIMER;
        TIMER = null;
        if (t != null) {
            try {
                t.cancel();
            } catch (Throwable ignored) {
            }
        }
    }

    /** crussyncgen-free diagnostics: appended to the crusgenstats tail by
     *  CrusstyGenPlugin (reflective, like LightDivert.statsLine). */
    public static String statsLine() {
        StringBuilder sb = new StringBuilder("[cover-stage stage=").append(stage());
        if (broken) {
            sb.append(" BROKEN");
        }
        sb.append(' ').append(lastStage);
        return sb.append(']').toString();
    }

    /** crusgenstats reset / window re-arm hygiene. */
    public static void reset() {
        cancelTimer();
        pendingX = null;
        pendingZ = null;
        pendingStatus = null;
        pendingDone = null;
        stageWorld = null;
        lastStage = "reset";
    }

    // ------------------------------------------------------------------
    // reflection plumbing (one-time, fail-permanent like RingsWarmup)
    // ------------------------------------------------------------------

    private static boolean initHandles() {
        if (ready) {
            return true;
        }
        if (broken) {
            return false;
        }
        synchronized (INIT_LOCK) {
            if (ready) {
                return true;
            }
            if (broken) {
                return false;
            }
            try {
                Class<?> craftWorld = Class.forName("org.bukkit.craftbukkit.CraftWorld");
                mGetHandle = craftWorld.getMethod("getHandle");
                Class<?> serverLevel = mGetHandle.getReturnType();
                Method sched = null;
                for (Method m : serverLevel.getMethods()) {
                    if (m.getName().equals("moonrise$getChunkTaskScheduler")
                            && m.getParameterCount() == 0) {
                        sched = m;
                        break;
                    }
                }
                if (sched == null) {
                    throw new NoSuchMethodException(
                            "ServerLevel.moonrise$getChunkTaskScheduler not found");
                }
                sched.setAccessible(true);
                mGetScheduler = sched;
                Class<?> schedClass = Class.forName(
                        "ca.spottedleaf.moonrise.patches.chunk_system.scheduling.ChunkTaskScheduler");
                Class<?> statusClass = Class.forName(
                        "net.minecraft.world.level.chunk.status.ChunkStatus");
                Class<?> prioClass = Class.forName(
                        "ca.spottedleaf.concurrentutil.util.Priority");
                mBegin = schedClass.getMethod("beginChunkLoadForNonFullSync",
                        int.class, int.class, statusClass, prioClass);
                mBegin.setAccessible(true);
                ST_SS = statusClass.getField("STRUCTURE_STARTS").get(null);
                ST_BIOMES = statusClass.getField("BIOMES").get(null);
                ST_TERRAIN = statusClass.getField("TERRAIN").get(null);
                @SuppressWarnings({"unchecked", "rawtypes"})
                Object p = Enum.valueOf((Class<? extends Enum>) prioClass, prioName());
                PRIO = p;
                ready = true;
                LOG.info("[cover-stage] bridge ready (prio=" + prioName() + ")");
            } catch (Throwable t) {
                broken = true;
                LOG.warning("[cover-stage] bridge unavailable (vanilla timing stays): "
                        + root(t));
            }
            return ready;
        }
    }

    private static Object schedulerFor(World w) {
        try {
            Object nms = mGetHandle.invoke(w);
            return mGetScheduler.invoke(nms);
        } catch (Throwable t) {
            broken = true;
            LOG.warning("[cover-stage] scheduler lookup failed, lever disabled: " + root(t));
            return null;
        }
    }

    private static Throwable root(Throwable t) {
        Throwable c = t;
        while (c.getCause() != null && c.getCause() != c) {
            c = c.getCause();
        }
        return c;
    }
}
