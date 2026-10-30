package com.planeta.crussty;

import net.minecraft.core.Holder;
import net.minecraft.core.Registry;
import net.minecraft.core.registries.Registries;
import net.minecraft.resources.Identifier;
import net.minecraft.world.level.biome.Biome;
import net.minecraft.world.level.biome.BiomeResolver;
import net.minecraft.world.level.biome.BiomeSource;
import net.minecraft.world.level.biome.Climate;
import net.minecraft.world.level.biome.MultiNoiseBiomeSource;

import java.lang.reflect.Field;
import java.util.Arrays;
import java.util.stream.Stream;

/**
 * Grid-backed BiomeSource: the chunk's biome quart grid comes from the NATIVE
 * engine (JNI fillBiomes — the same fill_biomes263 the hood/terrain paths
 * already use), so the BIOMES chunk status stops sampling the Java climate
 * splines + noise + RTree search (~1.5-2ms/chunk of Paper-side work that the
 * JFR window attributed to MultiNoiseBiomeSource.createResolverForChunk).
 *
 * Correctness: cont-53/54 proved the native point resolution bit-equal to the
 * preset MultiNoiseBiomeSource; this source makes the live pipeline agree BY
 * CONSTRUCTION (grid answers; vanilla RTree stays as the out-of-range
 * fallback).
 *
 * Serialization: codec() delegates to MultiNoiseBiomeSource.CODEC — a saved
 * level.dat round-trips as the vanilla preset source (bit-equal semantics);
 * onWorldLoad re-wraps on every boot, same as BiomeSwap today.
 *
 * Subclassing note: MultiNoiseBiomeSource has only a private ctor, so we
 * extend BiomeSource directly and compose the preset source (its protected
 * collectPossibleBiomes is NOT callable cross-instance; we rebuild the set
 * from the native name table, which is the same biome list by definition).
 *
 * Kill switch: CRUS_BIOME_GRID=0 in BiomeSwap keeps the plain preset source.
 */
public final class NativeBiomeSource extends BiomeSource {

    private final MultiNoiseBiomeSource vanilla;
    private final CrusstyChunkGenerator.CrusstyFiller filler;
    private final Holder<Biome>[] byIndex;
    private final int minQuartY;
    private final int sections;

    @SuppressWarnings("unchecked")
    private NativeBiomeSource(MultiNoiseBiomeSource vanilla,
                              CrusstyChunkGenerator.CrusstyFiller filler,
                              String[] names, int sections, int minQuartY,
                              Registry<Biome> biomeReg) {
        super();
        this.vanilla = vanilla;
        this.filler = filler;
        this.sections = sections;
        this.minQuartY = minQuartY;
        this.byIndex = new Holder[names.length];
        int misses = 0;
        for (int i = 0; i < names.length; i++) {
            Holder<Biome> h = null;
            try {
                h = biomeReg.get(Identifier.parse(names[i])).orElse(null);
            } catch (Throwable t) {
                // fall through to the miss path
            }
            if (h == null) {
                misses++;
                h = biomeReg.get(Identifier.withDefaultNamespace("plains")).orElse(null);
            }
            byIndex[i] = h;
        }
        if (misses > 0) {
            java.util.logging.Logger.getLogger("CrusstyGen").warning(
                    "NativeBiomeSource: " + misses + "/" + names.length
                            + " biome names unresolved (plains substituted)");
        }
    }

    /** Build over the level's biome registry; null on any failure (caller keeps vanilla). */
    public static NativeBiomeSource create(MultiNoiseBiomeSource vanilla,
                                           CrusstyChunkGenerator.CrusstyFiller filler,
                                           String[] names, int sections, int minQuartY,
                                           Object nmsWorld) {
        try {
            Object regAccess = nmsWorld.getClass().getMethod("registryAccess").invoke(nmsWorld);
            Object biomeRegistry = regAccess.getClass()
                    .getMethod("lookupOrThrow", net.minecraft.resources.ResourceKey.class)
                    .invoke(regAccess, Registries.BIOME);
            @SuppressWarnings("unchecked")
            Registry<Biome> biomeReg = (Registry<Biome>) biomeRegistry;
            return new NativeBiomeSource(vanilla, filler, names, sections, minQuartY, biomeReg);
        } catch (Throwable t) {
            java.util.logging.Logger.getLogger("CrusstyGen").warning(
                    "NativeBiomeSource create failed: " + t);
            return null;
        }
    }

    /** Saved level.dat round-trips as the vanilla preset source (bit-equal). */
    @Override
    @SuppressWarnings({"rawtypes", "unchecked"})
    protected com.mojang.serialization.MapCodec<? extends BiomeSource> codec() {
        return (com.mojang.serialization.MapCodec) MultiNoiseBiomeSource.CODEC;
    }

    /** The native name table IS the parameter list's biome set. */
    @Override
    protected Stream<Holder<Biome>> collectPossibleBiomes() {
        return Arrays.stream(byIndex).distinct();
    }

    @Override
    public BiomeResolver createResolver(Climate.Sampler sampler) {
        return new GridResolver(sampler);
    }

    @Override
    public BiomeResolver createResolverForChunk(Climate.Sampler sampler,
                                                int x0, int y0, int z0,
                                                int x1, int y1, int z1) {
        // bounds units are undocumented (quart vs block) — the resolver derives
        // the chunk lazily from the first query (qx>>2), unit-independent.
        return new GridResolver(sampler);
    }

    /** Per-chunk grid cache; one JNI fill (~0.15ms) then pure array lookups. */
    private final class GridResolver implements BiomeResolver {
        private final Climate.Sampler fallback;
        // volatile publication: grid is fully written before cxz lands
        private volatile long cxz = Long.MIN_VALUE;
        private volatile int[] grid;

        GridResolver(Climate.Sampler fallback) {
            this.fallback = fallback;
        }

        @Override
        public Holder<Biome> getNoiseBiome(int qx, int qy, int qz) {
            int cx = qx >> 2, cz = qz >> 2;
            long key = ((long) cx << 32) ^ (cz & 0xffffffffL);
            if (key != cxz) {
                synchronized (this) {
                    if (key != cxz) {
                        int[] g = new int[sections * 64];
                        // the filler closure pins its own pool ctx — the long
                        // param is unused by the production lambda
                        filler.fill(0L, cx, cz, g);
                        grid = g;
                        cxz = key;
                    }
                }
            }
            int s = (qy - minQuartY) >> 2;
            if (s < 0 || s >= sections) {
                // out of the world's quart range: vanilla RTree answers (same
                // semantics as today's pipeline)
                return vanilla.getNoiseBiome(fallback.sample(qx, qy, qz));
            }
            int idx = s * 64 + (qx & 3) * 16 + (qy & 3) * 4 + (qz & 3);
            Holder<Biome> h = byIndex[grid[idx]];
            return h != null ? h
                    : vanilla.getNoiseBiome(fallback.sample(qx, qy, qz));
        }
    }
}
