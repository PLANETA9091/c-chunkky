package com.planeta.crussty;

import net.minecraft.core.Holder;
import net.minecraft.core.Registry;
import net.minecraft.core.registries.Registries;
import net.minecraft.resources.Identifier;
import net.minecraft.resources.ResourceKey;
import net.minecraft.world.level.biome.Biome;
import net.minecraft.world.level.biome.BiomeResolver;
import net.minecraft.world.level.biome.BiomeSource;
import net.minecraft.world.level.biome.Climate;

import com.mojang.serialization.MapCodec;

import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;
import java.util.stream.Stream;

/**
 * NMS biome source backed by the native multi_noise resolution (bit-exact
 * port of vanilla Climate.RTree + the climate sampler, warm-start included).
 * Replaces the pack's Java MultiNoiseBiomeSource on the world generator so
 * the BIOMES chunk status runs the native path (the vanilla per-quart RTree
 * search + climate DF evaluation move into Rust).
 *
 * Compiled against the mojmap paper jar (NOT paper-api) — plugin/build.sh
 * includes it only when that jar is present (bootstrap ran first).
 */
public final class NativeBiomeSource extends BiomeSource {

    /** Bulk-fill callback: fills out[0..sections*64] with biome indexes for
     *  one chunk in the vanilla fillBiomesFromNoise order; returns the count
     *  or a negative value on failure. Implemented by CrusstyChunkGenerator
     *  (acquires a pool ctx + JNI fillBiomes). */
    public interface BulkFiller {
        int fill(long ctx, int cx, int cz, int[] out);
    }

    private final BulkFiller filler;
    private final Holder<Biome>[] byIndex;
    private final int sections;
    private final int minQuartY;
    private final Map<Long, int[]> chunkCache = new ConcurrentHashMap<>();

    @SuppressWarnings("unchecked")
    public NativeBiomeSource(Registry<Biome> biomeRegistry, BulkFiller filler, String[] names,
                             int sections, int minQuartY) {
        this.filler = filler;
        this.sections = sections;
        this.minQuartY = minQuartY;
        this.byIndex = new Holder[names.length];
        Map<Holder<Biome>, Boolean> set = new LinkedHashMap<>();
        // The registry's OWN Holder.Reference instances — the biome
        // PalettedContainer / surface rules / spawn logic expect exactly these
        // (Holder.is(ResourceKey) needs the wrapped key; a Holder.direct
        // wrapper would break every biome-condition check).
        for (int i = 0; i < names.length; i++) {
            Identifier id = Identifier.parse(names[i]);
            java.util.Optional<Holder.Reference<Biome>> h = biomeRegistry.get(id);
            if (h.isEmpty()) {
                throw new IllegalStateException("biome not registered: " + names[i]);
            }
            this.byIndex[i] = h.get();
            set.put(h.get(), Boolean.TRUE);
        }
        // possibleBiomes() is a lazy Supplier in the base class — force the
        // value at construction (cheap: the set is small).
        // (collectPossibleBiomes below feeds it.)
        this.possible = java.util.Set.copyOf(set.keySet());
    }

    /** Precomputed possible-biome set (base-class Supplier is lazy; we have
     *  the list at construction time). */
    private final java.util.Set<Holder<Biome>> possible;

    @Override
    protected Stream<Holder<Biome>> collectPossibleBiomes() {
        return possible.stream();
    }

    @Override
    public java.util.Set<Holder<Biome>> possibleBiomes() {
        return possible;
    }

    @Override
    protected MapCodec<? extends BiomeSource> codec() {
        throw new UnsupportedOperationException("native biome source is not serializable");
    }

    /** 26.3: BiomeSource hands out BiomeResolver views of itself; the native
     *  path ignores the climate sampler (the climate DFs run in Rust). */
    @Override
    public BiomeResolver createResolver(Climate.Sampler sampler) {
        return (qx, qy, qz) -> this.biomeAt(qx, qy, qz);
    }

    private Holder<Biome> biomeAt(int qx, int qy, int qz) {
        long chunkKey = (long) (qx >> 2) & 0x3FFFFFFL | (((long) (qz >> 2) & 0x3FFFFFFL) << 26);
        int[] ids = chunkCache.get(chunkKey);
        if (ids == null) {
            ids = chunkCache.computeIfAbsent(chunkKey, k -> {
                int cx = qx >> 2;
                int cz = qz >> 2;
                int[] a = new int[sections * 64];
                int n = filler.fill(0L, cx, cz, a);
                if (n < 0) {
                    throw new IllegalStateException("native fillBiomes failed");
                }
                return a;
            });
        }
        // quart -> (section, x, y, z) within the chunk
        int lx = qx & 3;
        int lz = qz & 3;
        int relY = qy - minQuartY;
        // clamp out-of-range y (structure queries may probe beyond the
        // generation height; vanilla computes those directly — the clamped
        // value only affects structures already gated on in-range checks)
        if (relY < 0) relY = 0;
        int maxY = sections * 4 - 1;
        if (relY > maxY) relY = maxY;
        int s = relY >> 2;
        int y = relY & 3;
        int idx = ids[s * 64 + lx * 16 + y * 4 + lz];
        return byIndex[idx];
    }

    /** Unique biome count (diagnostics). */
    public int biomeCount() {
        return byIndex.length;
    }

    /** The list view (diagnostics). */
    public List<Holder<Biome>> biomeList() {
        return List.of(byIndex);
    }
}
