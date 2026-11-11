package com.planeta.crussty;

import org.bukkit.World;
import org.bukkit.craftbukkit.CraftServer;

import java.lang.reflect.Field;
import java.lang.reflect.Method;

/**
 * Mojmap-compiled companion: rewires the world generator's biomeSource to the
 * runtime-extracted multi_noise PRESET (a vanilla MultiNoiseBiomeSource built
 * from the registry's parameter list — the loaded datapacks define it). The
 * preset path is bit-exact-equal to the native point resolution (verified
 * cont-53/54), keeps level.dat vanilla-serializable, and leaves the native
 * value-add where it lives: the fillBiomes bulk grid (fill263, JNI).
 * Requires the server jar at build time (the class is only produced when
 * plugin/build.sh finds it).
 */
public final class BiomeSwap {

    public static void apply(World world, CrusstyChunkGenerator.CrusstyFiller filler, String[] names,
                             int sections, int minQuartY, long ctxId) throws Exception {
        Object nmsWorld = world.getClass().getMethod("getHandle").invoke(world);
        Object chunkSource = nmsWorld.getClass().getMethod("getChunkSource").invoke(nmsWorld);
        Object gen = chunkSource.getClass().getMethod("getGenerator").invoke(chunkSource);
        // The world's generator is Paper's CustomChunkGenerator (wrapping the
        // vanilla NoiseBasedChunkGenerator). createBiomes is NOT overridden by
        // CraftBukkit — the BASE ChunkGenerator.createBiomes runs with
        // this.biomeSource — swap THAT field on the top-level instance.
        Field bf = null;
        for (Class<?> k = gen.getClass(); k != null && bf == null; k = k.getSuperclass()) {
            for (Field f : k.getDeclaredFields()) {
                if (f.getName().equals("biomeSource") && !java.lang.reflect.Modifier.isStatic(f.getModifiers())) {
                    f.setAccessible(true);
                    bf = f;
                    break;
                }
            }
        }
        if (bf == null) {
            throw new IllegalStateException("generator.biomeSource field not found");
        }
        Object old = bf.get(gen);
        // sanity: only swap a MultiNoiseBiomeSource (data-driven pack source);
        // fixed/checkerboard sources stay vanilla.
        if (old != null && !old.getClass().getSimpleName().contains("MultiNoise")) {
            throw new IllegalStateException("biomeSource is "
                    + old.getClass().getSimpleName() + " — not swapping");
        }
        // The parameter-list registry lives on the level's registry access —
        // the entry reflects the LOADED datapacks (Terralith/Tectonic override
        // the overworld list; the vanilla builtin is the fallback).
        Object regAccess = nmsWorld.getClass().getMethod("registryAccess").invoke(nmsWorld);
        Object presetListReg = regAccess.getClass()
                .getMethod("lookupOrThrow", net.minecraft.resources.ResourceKey.class)
                .invoke(regAccess, net.minecraft.core.registries.Registries.MULTI_NOISE_BIOME_SOURCE_PARAMETER_LIST);
        @SuppressWarnings("unchecked")
        net.minecraft.core.Registry<net.minecraft.world.level.biome.MultiNoiseBiomeSourceParameterList> plReg =
                (net.minecraft.core.Registry<net.minecraft.world.level.biome.MultiNoiseBiomeSourceParameterList>) presetListReg;
        // 26.3 Registry: get(Identifier) -> Optional<Holder.Reference<T>>
        net.minecraft.core.Holder<net.minecraft.world.level.biome.MultiNoiseBiomeSourceParameterList> preset =
                plReg.get(net.minecraft.resources.Identifier.withDefaultNamespace("overworld"))
                        .orElseThrow(() -> new IllegalStateException(
                                "overworld multi_noise parameter list not registered"));
        Object nbs = net.minecraft.world.level.biome.MultiNoiseBiomeSource.createFromPreset(preset);
        // CRUS_BIOME_GRID=1: wrap the preset source with the grid-backed
        // NativeBiomeSource — the BIOMES status answers from the native JNI
        // quart grid (JFR cont-77: the vanilla createResolverForChunk climate
        // splines + RTree cost ~1.5-2ms/chunk on the gen worker). Kill switch
        // 0/unset keeps the plain preset source (cont-53/54 bit-equal anyway).
        if ("1".equals(System.getenv("CRUS_BIOME_GRID")) && names != null && names.length > 0 && sections > 0) {
            if (filler != null) {
                Object nativeSrc = com.planeta.crussty.NativeBiomeSource.create(
                        (net.minecraft.world.level.biome.MultiNoiseBiomeSource) nbs,
                        filler, names, sections, minQuartY, ctxId, nmsWorld);
                if (nativeSrc != null) {
                    nbs = nativeSrc;
                    java.util.logging.Logger.getLogger("CrusstyGen").info(
                            "world: grid-backed native biome source active ("
                                    + names.length + " biomes, sections=" + sections + ")");
                }
            }
        }
        bf.set(gen, nbs);

        // cont-142: structure-state reswap — ChunkGeneratorStructureState was
        // built (createForNormal) BEFORE this swap and holds the PRE-swap
        // vanilla MultiNoiseBiomeSource in its `biomeSource` field; its
        // generateRingPositions -> findBiomeHorizontal then runs the vanilla
        // climate search per ring candidate (JFR cont-142: Climate$RTree +
        // NoiseStack$Perlin ~5.8-8ms/ch across the r8 margin ring). Refresh
        // the field to the (grid-backed) native source: findBiomeHorizontal
        // is base-class polymorphic over createResolver -> GridResolver (one
        // JNI grid fill per touched chunk), and its RNG draw sequence
        // (random.nextInt(found+1) per match) is identical when the biome
        // answers are identical (native biome function is judge-verified
        // bit-exact). DEFAULT OFF: paired A/B (10500,10500 wiped, back-to-back,
        // same jar/.so): OFF 46.49/48.87 vs ON 51.68/57.69 e2e — REGRESSION
        // x2 (+5.2/+8.8; gen +1.0/+2.4): the ring search touches 9-16 chunks,
        // each triggering a full vertical JNI grid fill under the resolver
        // lock, contending with the fill pool's native contexts; the vanilla
        // caching climate sampler beats that shape. CRUS_STRUCT_RESWAP=1
        // opts back in (code kept for a y-sliced/batched JNI grid variant).
        if ("1".equals(System.getenv("CRUS_STRUCT_RESWAP"))) {
            try {
                Object chunkMap = null;
                for (Field f : chunkSource.getClass().getDeclaredFields()) {
                    if (f.getType().getSimpleName().equals("ChunkMap")) {
                        f.setAccessible(true);
                        chunkMap = f.get(chunkSource);
                        break;
                    }
                }
                if (chunkMap != null) {
                    Field stf = null;
                    for (Field f : chunkMap.getClass().getDeclaredFields()) {
                        if (f.getType().getSimpleName().equals("ChunkGeneratorStructureState")) {
                            stf = f;
                            break;
                        }
                    }
                    if (stf != null) {
                        stf.setAccessible(true);
                        final Object state = stf.get(chunkMap);
                        if (state != null) {
                            Field sbf = null;
                            for (Field f : state.getClass().getDeclaredFields()) {
                                if (f.getName().equals("biomeSource")) {
                                    sbf = f;
                                    break;
                                }
                            }
                            if (sbf != null) {
                                sbf.setAccessible(true);
                                Object cur = sbf.get(state);
                                if (cur != nbs) {
                                    Field uf = sun.misc.Unsafe.class.getDeclaredField("theUnsafe");
                                    uf.setAccessible(true);
                                    sun.misc.Unsafe U = (sun.misc.Unsafe) uf.get(null);
                                    long off = U.objectFieldOffset(sbf);
                                    U.putObject(state, off, nbs);
                                    java.util.logging.Logger.getLogger("CrusstyGen").info(
                                            "world: structure-state biomeSource refreshed to native grid source");
                                }
                            }
                        }
                    }
                }
            } catch (Throwable t) {
                java.util.logging.Logger.getLogger("CrusstyGen").warning(
                        "world: structure-state reswap failed (vanilla source stays): " + t);
            }
        }
    }

    private BiomeSwap() {
    }
}
