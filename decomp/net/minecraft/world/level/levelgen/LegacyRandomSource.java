/*
 * Decompiled with CFR 0.152.
 * 
 * Could not load the following classes:
 *  net.minecraft.util.RandomSource
 *  net.minecraft.util.ThreadingDetector
 *  net.minecraft.world.level.levelgen.LegacyRandomSource$LegacyPositionalRandomFactory
 *  net.minecraft.world.level.levelgen.MarsagliaPolarGaussian
 */
package net.minecraft.world.level.levelgen;

import java.util.concurrent.atomic.AtomicLong;
import net.minecraft.util.RandomSource;
import net.minecraft.util.ThreadingDetector;
import net.minecraft.world.level.levelgen.BitRandomSource;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.level.levelgen.MarsagliaPolarGaussian;
import net.minecraft.world.level.levelgen.PositionalRandomFactory;

public class LegacyRandomSource
implements BitRandomSource {
    private static final int MODULUS_BITS = 48;
    private static final long MODULUS_MASK = 0xFFFFFFFFFFFFL;
    private static final long MULTIPLIER = 25214903917L;
    private static final long INCREMENT = 11L;
    private final AtomicLong seed = new AtomicLong();
    private final MarsagliaPolarGaussian gaussianSource = new MarsagliaPolarGaussian((RandomSource)this);

    public LegacyRandomSource(long seed) {
        this.setSeed(seed);
    }

    public RandomSource fork() {
        return new LegacyRandomSource(this.nextLong());
    }

    public PositionalRandomFactory forkPositional() {
        return new LegacyPositionalRandomFactory(this.nextLong());
    }

    public void setSeed(long seed) {
        if (!this.seed.compareAndSet(this.seed.get(), (seed ^ 0x5DEECE66DL) & 0xFFFFFFFFFFFFL)) {
            throw ThreadingDetector.makeThreadingException((String)"LegacyRandomSource", null);
        }
        this.gaussianSource.reset();
    }

    @Override
    public int next(int bits) {
        long newSeed;
        long oldSeed = this.seed.get();
        if (!this.seed.compareAndSet(oldSeed, newSeed = oldSeed * 25214903917L + 11L & 0xFFFFFFFFFFFFL)) {
            throw ThreadingDetector.makeThreadingException((String)"LegacyRandomSource", null);
        }
        return (int)(newSeed >> 48 - bits);
    }

    public double nextGaussian() {
        return this.gaussianSource.nextGaussian();
    }
}
