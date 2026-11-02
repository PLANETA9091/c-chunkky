package com.planeta.crussty;

import net.minecraft.world.level.levelgen.feature.FeaturePlaceContext;
import net.minecraft.world.level.levelgen.feature.OreFeature;
import net.minecraft.world.level.levelgen.feature.configurations.OreConfiguration;

/**
 * cont-99 front 3 — OreFeature replacement swapped into the frozen
 * configured-feature registry entries at world load (OreHook.swapFeatures,
 * Unsafe final-field write). The vanilla decorate loop invokes place() here
 * AFTER the placement modifiers consumed their draws from the shared
 * WorldgenRandom; the expansion delegates to the native ore engine
 * (ChunkGenNative.oreRunConfigured) which CONTINUES that exact RNG stream
 * from the raw Xoroshiro128PlusPlus engine state.
 *
 * Every non-success path returns vanilla super.place() with the Java RNG
 * untouched — the fallback is bit-exact vanilla by construction.
 */
public final class CrusstyOreFeature extends OreFeature {

    /** Registry name of the configured feature this instance replaced
     *  ("minecraft:ore_dirt") — the native registry lookup key. */
    public final String configuredName;

    CrusstyOreFeature(String configuredName) {
        super(OreConfiguration.CODEC);
        this.configuredName = configuredName;
    }

    @Override
    public boolean place(FeaturePlaceContext<OreConfiguration> ctx) {
        if (!OreHook.on()) {
            return super.place(ctx);
        }
        Boolean r = OreHook.place(configuredName, ctx);
        return r != null && r;
    }
}
