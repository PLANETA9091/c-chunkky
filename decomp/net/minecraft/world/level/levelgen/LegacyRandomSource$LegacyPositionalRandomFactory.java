/*
 * Decompiled with CFR 0.152.
 * 
 * Could not load the following classes:
 *  com.google.common.annotations.VisibleForTesting
 *  net.minecraft.util.Mth
 *  net.minecraft.util.RandomSource
 */
package net.minecraft.world.level.levelgen;

import com.google.common.annotations.VisibleForTesting;
import net.minecraft.util.Mth;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.level.levelgen.PositionalRandomFactory;

public static class LegacyRandomSource.LegacyPositionalRandomFactory
implements PositionalRandomFactory {
    private final long seed;

    public LegacyRandomSource.LegacyPositionalRandomFactory(long seed) {
        this.seed = seed;
    }

    @Override
    public RandomSource at(int x, int y, int z) {
        long positionalSeed = Mth.getSeed((int)x, (int)y, (int)z);
        long randomSeed = positionalSeed ^ this.seed;
        return new LegacyRandomSource(randomSeed);
    }

    @Override
    public RandomSource fromHashOf(String name) {
        int positionalSeed = name.hashCode();
        return new LegacyRandomSource((long)positionalSeed ^ this.seed);
    }

    @Override
    public RandomSource fromSeed(long seed) {
        return new LegacyRandomSource(seed);
    }

    @Override
    @VisibleForTesting
    public void parityConfigString(StringBuilder sb) {
        sb.append("LegacyPositionalRandomFactory{").append(this.seed).append("}");
    }
}
