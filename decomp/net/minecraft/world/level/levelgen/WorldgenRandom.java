/*
 * Decompiled with CFR 0.152.
 * 
 * Could not load the following classes:
 *  net.minecraft.util.RandomSource
 */
package net.minecraft.world.level.levelgen;

import net.minecraft.util.RandomSource;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.level.levelgen.PositionalRandomFactory;

public class WorldgenRandom
extends LegacyRandomSource {
    private final RandomSource randomSource;
    private int count;

    public WorldgenRandom(RandomSource randomSource) {
        super(0L);
        this.randomSource = randomSource;
    }

    public int getCount() {
        return this.count;
    }

    @Override
    public RandomSource fork() {
        return this.randomSource.fork();
    }

    @Override
    public PositionalRandomFactory forkPositional() {
        return this.randomSource.forkPositional();
    }

    @Override
    public int next(int bits) {
        int n;
        ++this.count;
        RandomSource randomSource = this.randomSource;
        if (randomSource instanceof LegacyRandomSource) {
            LegacyRandomSource legacyRandomSource = (LegacyRandomSource)randomSource;
            n = legacyRandomSource.next(bits);
        } else {
            n = (int)(this.randomSource.nextLong() >>> 64 - bits);
        }
        return n;
    }

    @Override
    public synchronized void setSeed(long seed) {
        if (this.randomSource != null) {
            this.randomSource.setSeed(seed);
        }
    }

    public long setDecorationSeed(long seed, int chunkX, int chunkZ) {
        this.setSeed(seed);
        long xScale = this.nextLong() | 1L;
        long zScale = this.nextLong() | 1L;
        long result = (long)chunkX * xScale + (long)chunkZ * zScale ^ seed;
        this.setSeed(result);
        return result;
    }

    public void setFeatureSeed(long seed, int index, int step) {
        long result = seed + (long)index + (long)(10000 * step);
        this.setSeed(result);
    }

    public void setLargeFeatureSeed(long seed, int chunkX, int chunkZ) {
        this.setSeed(seed);
        long xScale = this.nextLong();
        long zScale = this.nextLong();
        long result = (long)chunkX * xScale ^ (long)chunkZ * zScale ^ seed;
        this.setSeed(result);
    }

    public void setLargeFeatureWithSalt(long seed, int x, int z, int blend) {
        long result = (long)x * 341873128712L + (long)z * 132897987541L + seed + (long)blend;
        this.setSeed(result);
    }

    public static RandomSource seedSlimeChunk(int x, int z, long seed, long salt) {
        return RandomSource.createThreadLocalInstance((long)(seed + (long)(x * x * 4987142) + (long)(x * 5947611) + (long)(z * z) * 4392871L + (long)(z * 389711) ^ salt));
    }
}
