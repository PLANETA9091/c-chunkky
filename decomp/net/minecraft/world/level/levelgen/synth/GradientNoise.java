/*
 * Decompiled with CFR 0.152.
 * 
 * Could not load the following classes:
 *  net.minecraft.SharedConstants
 *  net.minecraft.util.RandomSource
 *  net.minecraft.world.level.levelgen.synth.GradientNoise$Gradient
 *  net.minecraft.world.level.levelgen.synth.Noise
 */
package net.minecraft.world.level.levelgen.synth;

import net.minecraft.SharedConstants;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.levelgen.synth.GradientNoise;
import net.minecraft.world.level.levelgen.synth.Noise;

public abstract class GradientNoise
implements Noise {
    public static final Gradient[] GRADIENT = new Gradient[]{new Gradient(1, 1, 0), new Gradient(-1, 1, 0), new Gradient(1, -1, 0), new Gradient(-1, -1, 0), new Gradient(1, 0, 1), new Gradient(-1, 0, 1), new Gradient(1, 0, -1), new Gradient(-1, 0, -1), new Gradient(0, 1, 1), new Gradient(0, -1, 1), new Gradient(0, 1, -1), new Gradient(0, -1, -1), new Gradient(1, 1, 0), new Gradient(0, -1, 1), new Gradient(-1, 1, 0), new Gradient(0, -1, -1)};
    private static final int ROUND_OFF = 0x2000000;
    private static final double HALF_ROUND_OFF = Math.nextDown(1.6777216E7);
    protected final byte[] perms = new byte[256];
    protected final double offsetX;
    protected final double offsetY;
    protected final double offsetZ;

    protected GradientNoise(RandomSource random) {
        this(random, 256.0);
    }

    protected GradientNoise(RandomSource random, double noiseOffsetScale) {
        int i;
        this.offsetX = random.nextDouble() * noiseOffsetScale;
        this.offsetY = random.nextDouble() * noiseOffsetScale;
        this.offsetZ = random.nextDouble() * noiseOffsetScale;
        for (i = 0; i < 256; ++i) {
            this.perms[i] = (byte)i;
        }
        for (i = 0; i < 256; ++i) {
            int offset = random.nextInt(256 - i);
            byte tmp = this.perms[i];
            this.perms[i] = this.perms[offset + i];
            this.perms[offset + i] = tmp;
        }
    }

    protected int permute(int x) {
        return this.perms[x & 0xFF] & 0xFF;
    }

    protected Gradient permuteToGrad(int x) {
        return GRADIENT[this.permute(x) & 0xF];
    }

    protected static float gradDot(int hash, float x, float y, float z) {
        return GRADIENT[hash & 0xF].dot(x, y, z);
    }

    protected static double wrap(double x) {
        if (SharedConstants.DEBUG_ENABLE_FARLANDS) {
            return x;
        }
        return x >= -HALF_ROUND_OFF && x < HALF_ROUND_OFF ? x : x - Math.floor(x / 3.3554432E7 + 0.5) * 3.3554432E7;
    }
}
