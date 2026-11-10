package com.planeta.crussty;

import net.minecraft.core.Holder;
import net.minecraft.core.Registry;
import net.minecraft.core.registries.Registries;
import net.minecraft.resources.ResourceKey;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.chunk.ChunkGeneratorStructureState;
import net.minecraft.world.level.levelgen.structure.StructureSet;
import net.minecraft.world.level.levelgen.structure.placement.ConcentricRingsStructurePlacement;

import java.lang.reflect.Field;
import java.lang.reflect.Method;
import java.util.List;
import java.util.logging.Logger;

/**
 * cont-138: pre-warm the ConcentricRings structure ring positions at plugin
 * enable (boot) instead of lazily inside the measured window.
 *
 * JFR evidence (fill-only honest-window run): the Paper Common Worker parked
 * ~1.6 s (recording-wide) in ChunkGeneratorStructureState.getRingPositionsFor
 * — the ring futures are computed on the ForkJoin common pool the first time
 * a chunk references a rings structure set (villages / pillager outposts /
 * strongholds-style sets), sampling the MultiNoise biome source per candidate
 * position (Climate RTree) WHILE the window is measuring. Moving that work to
 * boot removes both the worker park AND the competing ForkJoin CPU from the
 * window.
 *
 * PARITY: we call getRingPositionsFor on the SERVER'S OWN ChunkGeneratorState
 * instance (ChunkMap.chunkGeneratorState) — the same instance and cached
 * future the generation path later reads. The computation is identical to
 * what vanilla would do lazily (same state, same biome source — created at
 * world init, BEFORE our biome-source swap), so the ring positions are
 * bit-identical; only the timing moves. Idempotent: repeated calls hit the
 * cached future.
 */
public final class RingsWarmup {
    private static final Logger LOG = Logger.getLogger("CrusstyGen");

    private RingsWarmup() {
    }

    public static void warm(org.bukkit.World world) {
        try {
            Object nmsWorld = world.getClass().getMethod("getHandle").invoke(world);
            Object chunkSource = nmsWorld.getClass().getMethod("getChunkSource").invoke(nmsWorld);
            Object chunkMap = null;
            for (Class<?> k = chunkSource.getClass(); k != null && chunkMap == null; k = k.getSuperclass()) {
                for (Field f : k.getDeclaredFields()) {
                    if (f.getType().getSimpleName().equals("ChunkMap")) {
                        f.setAccessible(true);
                        chunkMap = f.get(chunkSource);
                        break;
                    }
                }
            }
            if (chunkMap == null) {
                LOG.warning("[rings-warmup] ChunkMap field not found (skipped)");
                return;
            }
            Method genState = null;
            for (Class<?> k = chunkMap.getClass(); k != null && genState == null; k = k.getSuperclass()) {
                for (Method m : k.getDeclaredMethods()) {
                    if (m.getReturnType().getSimpleName().equals("ChunkGeneratorStructureState")
                            && m.getParameterCount() == 0) {
                        m.setAccessible(true);
                        genState = m;
                        break;
                    }
                }
            }
            if (genState == null) {
                LOG.warning("[rings-warmup] generatorState accessor not found (skipped)");
                return;
            }
            ChunkGeneratorStructureState state = (ChunkGeneratorStructureState) genState.invoke(chunkMap);
            Object regAccess = nmsWorld.getClass().getMethod("registryAccess").invoke(nmsWorld);
            @SuppressWarnings("unchecked")
            Registry<StructureSet> setReg = (Registry<StructureSet>) regAccess
                    .getClass()
                    .getMethod("lookupOrThrow", ResourceKey.class)
                    .invoke(regAccess, Registries.STRUCTURE_SET);
            long t0 = System.nanoTime();
            int warmed = 0;
            for (StructureSet ss : setReg.stream().toList()) {
                if (ss != null && ss.placement() instanceof ConcentricRingsStructurePlacement crp) {
                    List<ChunkPos> pos = state.getRingPositionsFor(crp);
                    warmed += pos == null ? 0 : pos.size();
                }
            }
            LOG.info("[rings-warmup] " + world.getName() + ": " + warmed
                    + " ring positions precomputed in "
                    + String.format("%.1f", (System.nanoTime() - t0) / 1e6) + " ms");
        } catch (Throwable t) {
            Throwable c = t;
            while (c.getCause() != null && c.getCause() != c) {
                c = c.getCause();
            }
            LOG.warning("[rings-warmup] failed (vanilla lazy timing stays): " + c);
        }
    }
}
