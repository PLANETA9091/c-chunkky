package com.planeta.crussty;

import java.util.List;

import com.mojang.serialization.MapCodec;

import net.minecraft.core.BlockPos;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.WorldGenLevel;
import net.minecraft.world.level.chunk.ChunkGenerator;
import net.minecraft.world.level.levelgen.feature.AbstractOreFeature;
import net.minecraft.world.level.levelgen.feature.BlockReplacement;
import net.minecraft.world.level.levelgen.feature.Feature;
import net.minecraft.world.level.levelgen.feature.OreFeature;

/**
 * cont-99 front 3 — ore hook replacement instance, swapped into the placed
 * features' Feature holders at world load (OreHook.swapFeatures, Unsafe
 * write on Holder.Reference.value). The vanilla decorate loop invokes
 * place() here AFTER the placement modifiers consumed their draws from the
 * shared WorldgenRandom; the expansion delegates to the native ore engine
 * (ChunkGenNative.oreRunConfigured) which CONTINUES that exact RNG stream
 * from the raw Xoroshiro128PlusPlus engine state.
 *
 * 26.3 worldgen shape: Feature is an interface, the config fields
 * (targetStates/size/discardChance) live on the feature instance itself
 * (AbstractOreFeature), and there is no ConfiguredFeature layer — the hook
 * binds the FEATURE registry name ("minecraft:ore_dirt") and keeps the
 * ORIGINAL instance for the vanilla fallback (every non-success path calls
 * vanilla.place(...) with the Java RNG untouched — bit-exact by
 * construction).
 */
public final class CrusstyOreFeature extends AbstractOreFeature {

    /** Feature registry name this instance replaced ("minecraft:ore_dirt")
     *  — the native registry lookup key (placed or configured). */
    public final String featureName;

    /** The original 26.3 config fields (the trace shadow needs size/discard
     *  without touching the original instance's private state). */
    public final int oreSize;
    public final float oreDiscard;

    /** The original registry-bound feature instance — the vanilla fallback
     *  path (super.place() is abstract here; the exact original instance
     *  carries the original config fields). */
    private final AbstractOreFeature vanilla;

    CrusstyOreFeature(String featureName, AbstractOreFeature vanilla) {
        super(List.<BlockReplacement>of(), 0, 0.0f);
        this.featureName = featureName;
        this.vanilla = vanilla;
        this.oreSize = vanilla.size();
        this.oreDiscard = vanilla.discardChanceOnAirExposure();
    }

    @Override
    public MapCodec<? extends AbstractOreFeature> codec() {
        return vanilla.codec();
    }

    @Override
    public boolean place(WorldGenLevel level, ChunkGenerator gen,
                         RandomSource random, BlockPos origin) {
        if (!OreHook.on()) {
            return vanilla.place(level, gen, random, origin);
        }
        Boolean r = OreHook.place(featureName, oreSize, oreDiscard, level, gen, random, origin);
        if (r == null) {
            return vanilla.place(level, gen, random, origin);
        }
        return r;
    }
}
