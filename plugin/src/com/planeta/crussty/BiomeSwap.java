package com.planeta.crussty;

import org.bukkit.World;
import org.bukkit.craftbukkit.CraftServer;

import java.lang.reflect.Field;
import java.lang.reflect.Method;

/**
 * Mojmap-compiled companion: swaps the world generator's biomeSource for the
 * native NativeBiomeSource. Requires the server jar at build time (the class
 * is only produced when plugin/build.sh finds it).
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
        // The biome registry lives on the level's registry access (1.21.8:
        // BIOME is not a BuiltInRegistries static field).
        Object regAccess = nmsWorld.getClass().getMethod("registryAccess").invoke(nmsWorld);
        Object biomeRegistry = regAccess.getClass()
                .getMethod("lookupOrThrow", net.minecraft.resources.ResourceKey.class)
                .invoke(regAccess, net.minecraft.core.registries.Registries.BIOME);
        @SuppressWarnings("unchecked")
        net.minecraft.core.Registry<net.minecraft.world.level.biome.Biome> biomeReg =
                (net.minecraft.core.Registry<net.minecraft.world.level.biome.Biome>) biomeRegistry;
        NativeBiomeSource nbs = new NativeBiomeSource(biomeReg,
                (ctx, cx, cz, out) -> filler.fill(ctx, cx, cz, out),
                names, sections, minQuartY);
        bf.set(gen, nbs);
    }

    private BiomeSwap() {
    }
}
