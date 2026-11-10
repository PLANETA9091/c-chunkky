package com.planeta.crussty;

import org.bukkit.World;
import org.bukkit.generator.BlockPopulator;
import org.bukkit.generator.LimitedRegion;
import org.bukkit.generator.WorldInfo;

import java.util.ArrayList;
import java.util.List;
import java.util.Random;
import java.util.concurrent.atomic.AtomicLong;

/**
 * cont-136 CRUS_LIGHT_DIVERT: NMS light-divert populator (audit cont-134
 * "paper-26.3 light levers", researched against the decompiled 26.3 build-222
 * jar — /tmp/lightdivert/FINDINGS.md).
 *
 * ChunkLightTask on 26.3 takes a cheap branch when the chunk already claims
 * correct light:
 *     if (fromChunk.isLightCorrect() && persistedStatus.isOrAfter(LIGHT)) {
 *         lightEngine.forceLoadInChunk(...); lightEngine.checkChunkEdges(...);
 *     } else { ...full lightChunk (256-column sky seed + BFS)... }
 * A freshly generated chunk never satisfies the branch — so we set the two
 * flags from a BlockPopulator registered LAST (after DecoDriver and the
 * cont-92 marker): lightCorrect=true + persistedStatus=LIGHT.
 *
 * Ordering contract (FINDINGS §timing): ProtoChunk.setBlockState dereferences
 * the per-chunk lightEngine once status >= INITIALIZE_LIGHT; lightEngine is
 * null until INITIALIZE_LIGHT runs. This populator must therefore be the LAST
 * populator — every earlier populator's writes already happened, and Paper's
 * own later steps do not route block writes through this ProtoChunk.
 *
 * Runtime risk (kill-switched, default OFF): worldgen animals ignore light,
 * but persisted zero light means daylight-surface hostile spawns in played
 * worlds; light VALUES are not recomputed until an edge is touched.
 * Affects chunk METADATA only — block data is untouched (no judge parity risk).
 */
public final class LightDivert extends BlockPopulator {
    public static final String ENV = "CRUS_LIGHT_DIVERT";

    private static final AtomicLong DIVERTED = new AtomicLong();
    private static final AtomicLong SKIPPED_STATUS = new AtomicLong();
    private static final AtomicLong FAILED = new AtomicLong();
    private static volatile String firstFail = null;
    private static volatile String firstStatus = null;

    private LightDivert() {}

    /** Registers the divert populator LAST (called reflectively from
     *  CrusstyChunkGenerator.onWorldLoad when CRUS_LIGHT_DIVERT=1). */
    public static void setup(World w) {
        List<BlockPopulator> pops = new ArrayList<>(w.getPopulators());
        w.getPopulators().clear();
        w.getPopulators().addAll(pops);
        w.getPopulators().add(new LightDivert());
        java.util.logging.Logger.getLogger("CrusstyGen").info(
                "[CrusstyGen] light divert active — ChunkLightTask cheap branch forced (kill-switch: unset " + ENV + ")");
    }

    public static boolean enabled() {
        return "1".equals(System.getenv(ENV));
    }

    @Override
    public void populate(WorldInfo info, Random random, int cx, int cz,
                         LimitedRegion limited) {
        try {
            // CraftLimitedRegion.getHandle() -> WorldGenRegion of this decorate
            // pass; 2-arg getChunk hits the cache for the CENTER chunk.
            net.minecraft.world.level.chunk.ChunkAccess chunk =
                    ((org.bukkit.craftbukkit.generator.CraftLimitedRegion) limited)
                            .getHandle().getChunk(cx, cz);
            if (chunk instanceof net.minecraft.world.level.chunk.ProtoChunk pc) {
                net.minecraft.world.level.chunk.status.ChunkStatus st =
                        pc.getPersistedStatus();
                if (firstStatus == null) {
                    // cont-136 evidence: during decorate the persisted status is
                    // the LAST COMPLETED step (below FEATURES) — the original
                    // [FEATURES, LIGHT) guard diverted 0/1153 chunks; corrected
                    // to skip only chunks already at/after LIGHT (loaded chunks).
                    firstStatus = String.valueOf(st);
                }
                // Divert every fresh chunk: this populator runs exactly once,
                // inside the decorate (FEATURES) task of a newly generated
                // chunk. Anything already >= LIGHT is a loaded chunk — skip.
                if (!st.isOrAfter(net.minecraft.world.level.chunk.status.ChunkStatus.LIGHT)) {
                    pc.setLightCorrect(true);
                    pc.setPersistedStatus(net.minecraft.world.level.chunk.status.ChunkStatus.LIGHT);
                    DIVERTED.incrementAndGet();
                } else {
                    SKIPPED_STATUS.incrementAndGet();
                }
            }
        } catch (Throwable t) {
            // never fail a chunk because of the divert
            if (FAILED.incrementAndGet() == 1) {
                firstFail = t.toString();
            }
        }
    }

    /** crusgenstats tail line (reflectively fetched — API-compiled callers). */
    public static String statsLine() {
        StringBuilder sb = new StringBuilder("[lightdivert");
        if (!enabled()) {
            sb.append(" off (env not set)");
        }
        sb.append(" diverted=").append(DIVERTED.get())
          .append(" skipped_status=").append(SKIPPED_STATUS.get())
          .append(" fail=").append(FAILED.get());
        if (firstStatus != null) {
            sb.append(" first_status=").append(firstStatus);
        }
        if (firstFail != null) {
            sb.append(" first=").append(firstFail);
        }
        return sb.append(']').toString();
    }
}
