package com.planeta.crussty;

import net.minecraft.server.level.ServerChunkCache;
import net.minecraft.server.level.TicketType;
import net.minecraft.world.level.chunk.status.ChunkStatus;

import java.lang.reflect.Method;
import java.util.concurrent.CompletableFuture;
import java.util.logging.Logger;

/**
 * cont-138 EXPERIMENT: synchronous full-chunk generation path — bypass the
 * moonrise task-queue pipeline entirely.
 *
 * Evidence: the honest window is idle-dominated (~28-30 ms/ch of the ~60 ms
 * e2e wall) with maxconc=1 — the worker parks on chunk-future dependencies
 * (Notifier$Lock.waitUntilReady) between STATUS TASK submissions; the
 * task-queue latency is the pipeline, not the work. ServerChunkCache.getChunk
 * (x, z, ChunkStatus.FULL, load=true) falls back to syncLoad — the SAME
 * status chain executed INLINE on the calling thread (the spawn-position
 * finder path, seen in JFR). Identical generation work, bit-identical
 * chunks; only the delivery changes (no task-queue wakeups between statuses).
 *
 * Runs on the MAIN server thread (scheduled via Bukkit scheduler): syncLoad
 * is a main-thread API on Paper 26.3; the rcon command thread waits on a
 * future. The native fill still runs on its own 2-thread pool (the JNI fill
 * submits and the caller waits, exactly as in the populate path).
 */
public final class SyncGen {
    private static final Logger LOG = Logger.getLogger("CrusstyGen");

    private SyncGen() {
    }

    /**
     * Synchronously generate the n x n chunk rect [cx..cx+n, cz..cz+n] to
     * FULL on the main thread. Returns a timing report line (ms/chunk).
     */
    public static String gen(org.bukkit.World world, int cx, int cz, int n) throws Exception {
        // ASYNC fire-and-forget: the inline pass holds the MAIN thread for
        // the whole rect (minutes at 12x12) — the rcon reply would time out
        // and the caller cannot poll; the result lands in the server log
        // ([syncgen] line) instead. Returns "started" immediately.
        final int fn = n;
        org.bukkit.Bukkit.getScheduler().runTask(
                org.bukkit.Bukkit.getPluginManager().getPlugin("CrusstyGen"),
                () -> {
                    try {
                        run(world, cx, cz, fn);
                    } catch (Throwable t) {
                        LOG.warning("[syncgen] failed: " + t);
                    }
                });
        return "started rect[" + cx + ".." + (cx + fn) + "," + cz + ".." + (cz + fn)
                + "] on main thread — result in server log [syncgen]";
    }

    private static String run(org.bukkit.World world, int cx, int cz, int n) throws Exception {
        Object nmsWorld = world.getClass().getMethod("getHandle").invoke(world);
        ServerChunkCache chunkSource = (ServerChunkCache) nmsWorld
                .getClass().getMethod("getChunkSource").invoke(nmsWorld);
        Method getChunk = chunkSource.getClass().getMethod(
                "getChunk", int.class, int.class, ChunkStatus.class, boolean.class);
        long t0 = System.nanoTime();
        int ok = 0;
        long minNs = Long.MAX_VALUE, maxNs = 0;
        // v4: FORCED tickets at level 31 (FULL) held over the WHOLE rect for
        // the duration of the pass — syncLoad releases its implicit per-chunk
        // ticket on completion, which let previously generated neighbors get
        // demoted before chunk k's WorldGenRegion needed them ("Requested
        // chunk unavailable during world generation"). With FORCED tickets
        // every neighbor stays FULL while the inline pass runs. This is the
        // forceload ticket semantics + INLINE status chains.
        Method addT = chunkSource.getClass().getMethod("addTicketAtLevel",
                TicketType.class, net.minecraft.world.level.ChunkPos.class, int.class);
        Method remT = chunkSource.getClass().getMethod("removeTicketAtLevel",
                TicketType.class, net.minecraft.world.level.ChunkPos.class, int.class);
        int margin = 1;
        for (int dz = -margin; dz <= n + margin; dz++) {
            for (int dx = -margin; dx <= n + margin; dx++) {
                addT.invoke(chunkSource, TicketType.FORCED,
                        new net.minecraft.world.level.ChunkPos(cx + dx, cz + dz), 31);
            }
        }
        for (int dz = 0; dz <= n; dz++) {
            for (int dx = 0; dx <= n; dx++) {
                long c0 = System.nanoTime();
                Object ch = getChunk.invoke(chunkSource, cx + dx, cz + dz, ChunkStatus.FULL, true);
                long c1 = System.nanoTime();
                if (ch != null) {
                    ok++;
                }
                long ns = c1 - c0;
                if (ns < minNs) {
                    minNs = ns;
                }
                if (ns > maxNs) {
                    maxNs = ns;
                }
            }
        }
        for (int dz = -margin; dz <= n + margin; dz++) {
            for (int dx = -margin; dx <= n + margin; dx++) {
                remT.invoke(chunkSource, TicketType.FORCED,
                        new net.minecraft.world.level.ChunkPos(cx + dx, cz + dz), 31);
            }
        }
        long wall = (System.nanoTime() - t0) / 1_000_000L;
        int cnt = (n + 1) * (n + 1);
        double per = wall / (double) cnt;
        LOG.info(String.format("[syncgen] %s rect[%d..%d,%d..%d] ok=%d wall=%dms per-chunk=%.2fms min=%.2f max=%.2f",
                world.getName(), cx, cx + n, cz, cz + n, ok, wall, per,
                minNs / 1e6, maxNs / 1e6));
        return String.format("syncgen ok=%d wall=%dms per-chunk=%.2fms min=%.2fms max=%.2fms",
                ok, wall, per, minNs / 1e6, maxNs / 1e6);
    }
}
