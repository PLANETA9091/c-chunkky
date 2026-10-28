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
                             int sections, int minQuartY) throws Exception {
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
        net.minecraft.core.Holder<net.minecraft.world.level.biome.MultiNoiseBiomeSourceParameterList> preset =
                plReg.getHolderOrThrow(net.minecraft.world.level.biome.MultiNoiseBiomeSourceParameterLists.OVERWORLD);
        Object nbs = net.minecraft.world.level.biome.MultiNoiseBiomeSource.createFromPreset(preset);
        bf.set(gen, nbs);
    }

    private BiomeSwap() {
    }
}
