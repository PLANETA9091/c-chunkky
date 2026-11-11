/*
 * Decompiled with CFR 0.152.
 * 
 * Could not load the following classes:
 *  ca.spottedleaf.moonrise.patches.chunk_gen.noise.ChunkGenPerlinNoise
 *  ca.spottedleaf.moonrise.patches.chunk_gen.noise.NoiseGradients
 *  ca.spottedleaf.moonrise.patches.chunk_gen.noise.NoiseGradients$CoordCache
 *  com.google.common.annotations.VisibleForTesting
 *  net.minecraft.util.Interval
 *  net.minecraft.util.Mth
 *  net.minecraft.util.RandomSource
 *  net.minecraft.world.level.levelgen.densityfunction.DensityBuffer
 *  net.minecraft.world.level.levelgen.densityfunction.DensityVolume
 *  net.minecraft.world.level.levelgen.synth.GradientNoise
 *  net.minecraft.world.level.levelgen.synth.GradientNoise$Gradient
 *  net.minecraft.world.level.levelgen.synth.NoiseUtils
 */
package net.minecraft.world.level.levelgen.synth;

import ca.spottedleaf.moonrise.patches.chunk_gen.noise.ChunkGenPerlinNoise;
import ca.spottedleaf.moonrise.patches.chunk_gen.noise.NoiseGradients;
import com.google.common.annotations.VisibleForTesting;
import net.minecraft.util.Interval;
import net.minecraft.util.Mth;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.levelgen.densityfunction.DensityBuffer;
import net.minecraft.world.level.levelgen.densityfunction.DensityVolume;
import net.minecraft.world.level.levelgen.synth.GradientNoise;
import net.minecraft.world.level.levelgen.synth.NoiseUtils;

public class PerlinNoise
extends GradientNoise
implements ChunkGenPerlinNoise {
    public static final Interval RANGE = Interval.ofSymmetric((float)2.0f);
    public static final double STANDARD_DEVIATION = 0.2702247831245211;
    protected final char[] pAdj = new char[256];

    public final char[] moonrise$getPAdj() {
        return this.pAdj;
    }

    private final int p(int x) {
        return this.perms[x & 0xFF] & 0xFF;
    }

    public PerlinNoise(RandomSource random) {
        super(random);
        for (int i = 0; i < 256; ++i) {
            this.pAdj[i] = (char)(this.p(i) | this.p(i + 1) << 8);
        }
    }

    public Interval range() {
        return RANGE;
    }

    public float get(double x, double y) {
        return this.get(PerlinNoise.wrap((double)x), 0.0, PerlinNoise.wrap((double)y));
    }

    public float get(double _x, double _y, double _z) {
        double x = PerlinNoise.wrap((double)_x) + this.offsetX;
        double y = PerlinNoise.wrap((double)_y) + this.offsetY;
        double z = PerlinNoise.wrap((double)_z) + this.offsetZ;
        int floorX = Mth.floor((double)x);
        int floorY = Mth.floor((double)y);
        int floorZ = Mth.floor((double)z);
        float relativeX = (float)(x - (double)floorX);
        float relativeY = (float)(y - (double)floorY);
        float relativeZ = (float)(z - (double)floorZ);
        return this.sampleAndLerp(floorX, floorY, floorZ, relativeX, relativeY, relativeZ, relativeY);
    }

    public float noiseWithDerivative(double _x, double _y, double _z, float[] derivativeOut) {
        double x = PerlinNoise.wrap((double)_x) + this.offsetX;
        double y = PerlinNoise.wrap((double)_y) + this.offsetY;
        double z = PerlinNoise.wrap((double)_z) + this.offsetZ;
        int floorX = Mth.floor((double)x);
        int floorY = Mth.floor((double)y);
        int floorZ = Mth.floor((double)z);
        float relativeX = (float)(x - (double)floorX);
        float relativeY = (float)(y - (double)floorY);
        float relativeZ = (float)(z - (double)floorZ);
        return this.sampleWithDerivative(floorX, floorY, floorZ, relativeX, relativeY, relativeZ, derivativeOut);
    }

    protected float sampleAndLerp(int x, int y, int z, float relativeX, float relativeY, float relativeZ, float originalRelativeY) {
        char[] p = this.pAdj;
        char x0 = p[x & 0xFF];
        int x1 = x0 >>> 8;
        char xy00 = p[x0 + y & 0xFF];
        int xy01 = xy00 >>> 8;
        char z000 = p[xy00 + z & 0xFF];
        char z001 = p[xy01 + z & 0xFF];
        float d000 = NoiseGradients.dot((int)(z000 & 0xF), (float)relativeX, (float)relativeY, (float)relativeZ);
        float d001 = NoiseGradients.dot((int)(z000 >>> 8 & 0xF), (float)relativeX, (float)relativeY, (float)(relativeZ - 1.0f));
        float d010 = NoiseGradients.dot((int)(z001 & 0xF), (float)relativeX, (float)(relativeY - 1.0f), (float)relativeZ);
        float d011 = NoiseGradients.dot((int)(z001 >>> 8 & 0xF), (float)relativeX, (float)(relativeY - 1.0f), (float)(relativeZ - 1.0f));
        char xy10 = p[x1 + y & 0xFF];
        int xy11 = xy10 >>> 8;
        char z100 = p[xy10 + z & 0xFF];
        char z101 = p[xy11 + z & 0xFF];
        float d100 = NoiseGradients.dot((int)(z100 & 0xF), (float)(relativeX - 1.0f), (float)relativeY, (float)relativeZ);
        float d101 = NoiseGradients.dot((int)(z100 >>> 8 & 0xF), (float)(relativeX - 1.0f), (float)relativeY, (float)(relativeZ - 1.0f));
        float d110 = NoiseGradients.dot((int)(z101 & 0xF), (float)(relativeX - 1.0f), (float)(relativeY - 1.0f), (float)relativeZ);
        float d111 = NoiseGradients.dot((int)(z101 >>> 8 & 0xF), (float)(relativeX - 1.0f), (float)(relativeY - 1.0f), (float)(relativeZ - 1.0f));
        float ret = Mth.lerp3((float)Mth.smoothstep((float)relativeX), (float)Mth.smoothstep((float)originalRelativeY), (float)Mth.smoothstep((float)relativeZ), (float)d000, (float)d100, (float)d010, (float)d110, (float)d001, (float)d101, (float)d011, (float)d111);
        return ret;
    }

    private float sampleAndLerpVanilla(int x, int y, int z, float relativeX, float relativeY, float relativeZ, float originalRelativeY) {
        int x0 = this.permute(x);
        int x1 = this.permute(x + 1);
        int xy00 = this.permute(x0 + y);
        int xy01 = this.permute(x0 + y + 1);
        int xy10 = this.permute(x1 + y);
        int xy11 = this.permute(x1 + y + 1);
        float d000 = PerlinNoise.gradDot((int)this.permute(xy00 + z), (float)relativeX, (float)relativeY, (float)relativeZ);
        float d100 = PerlinNoise.gradDot((int)this.permute(xy10 + z), (float)(relativeX - 1.0f), (float)relativeY, (float)relativeZ);
        float d010 = PerlinNoise.gradDot((int)this.permute(xy01 + z), (float)relativeX, (float)(relativeY - 1.0f), (float)relativeZ);
        float d110 = PerlinNoise.gradDot((int)this.permute(xy11 + z), (float)(relativeX - 1.0f), (float)(relativeY - 1.0f), (float)relativeZ);
        float d001 = PerlinNoise.gradDot((int)this.permute(xy00 + z + 1), (float)relativeX, (float)relativeY, (float)(relativeZ - 1.0f));
        float d101 = PerlinNoise.gradDot((int)this.permute(xy10 + z + 1), (float)(relativeX - 1.0f), (float)relativeY, (float)(relativeZ - 1.0f));
        float d011 = PerlinNoise.gradDot((int)this.permute(xy01 + z + 1), (float)relativeX, (float)(relativeY - 1.0f), (float)(relativeZ - 1.0f));
        float d111 = PerlinNoise.gradDot((int)this.permute(xy11 + z + 1), (float)(relativeX - 1.0f), (float)(relativeY - 1.0f), (float)(relativeZ - 1.0f));
        float xAlpha = Mth.smoothstep((float)relativeX);
        float yAlpha = Mth.smoothstep((float)originalRelativeY);
        float zAlpha = Mth.smoothstep((float)relativeZ);
        return Mth.lerp3((float)xAlpha, (float)yAlpha, (float)zAlpha, (float)d000, (float)d100, (float)d010, (float)d110, (float)d001, (float)d101, (float)d011, (float)d111);
    }

    public void addToVolume(DensityBuffer buffer, DensityVolume volume, double xzScale, double yScale, float amplitude) {
        float d000xz = 0.0f;
        float d100xz = 0.0f;
        float d010xz = 0.0f;
        float d110xz = 0.0f;
        float d001xz = 0.0f;
        float d101xz = 0.0f;
        float d011xz = 0.0f;
        float d111xz = 0.0f;
        float g000y = 0.0f;
        float g100y = 0.0f;
        float g010y = 0.0f;
        float g110y = 0.0f;
        float g001y = 0.0f;
        float g101y = 0.0f;
        float g011y = 0.0f;
        float g111y = 0.0f;
        int sX = volume.sizeX();
        int sY = volume.sizeY();
        int sZ = volume.sizeZ();
        NoiseGradients.CoordCache[] cacheX = NoiseGradients.getCoordCache((ThreadLocal)NoiseGradients.COORD_CACHE_X, (int)sX);
        NoiseGradients.CoordCache[] cacheY = NoiseGradients.getCoordCache((ThreadLocal)NoiseGradients.COORD_CACHE_Y, (int)sY);
        NoiseGradients.CoordCache[] cacheZ = NoiseGradients.getCoordCache((ThreadLocal)NoiseGradients.COORD_CACHE_Z, (int)sZ);
        double xv = volume.minBlockX();
        double xi = volume.stepBlockX();
        int x = 0;
        while (x < sX) {
            double val = PerlinNoise.wrap((double)(xv * xzScale)) + this.offsetX;
            double floorD = Math.floor(val);
            int floor = (int)floorD;
            float relative = (float)(val - floorD);
            float alpha = Mth.smoothstep((float)relative);
            NoiseGradients.CoordCache cx = cacheX[x];
            cx.floor = floor;
            cx.alpha = alpha;
            cx.relative = relative;
            ++x;
            xv += xi;
        }
        double yv = volume.minBlockY();
        double yi = volume.stepBlockY();
        int y = 0;
        while (y < sY) {
            double val = PerlinNoise.wrap((double)(yv * yScale)) + this.offsetY;
            double floorD = Math.floor(val);
            int floor = (int)floorD;
            float relative = (float)(val - floorD);
            float alpha = Mth.smoothstep((float)relative);
            NoiseGradients.CoordCache cy = cacheY[y];
            cy.floor = floor;
            cy.alpha = alpha;
            cy.relative = relative;
            ++y;
            yv += yi;
        }
        double zv = volume.minBlockZ();
        double zi = volume.stepBlockZ();
        int z = 0;
        while (z < sZ) {
            double val = PerlinNoise.wrap((double)(zv * xzScale)) + this.offsetZ;
            double floorD = Math.floor(val);
            int floor = (int)floorD;
            float relative = (float)(val - floorD);
            float alpha = Mth.smoothstep((float)relative);
            NoiseGradients.CoordCache cz = cacheZ[z];
            cz.floor = floor;
            cz.alpha = alpha;
            cz.relative = relative;
            ++z;
            zv += zi;
        }
        char[] p = this.pAdj;
        for (int indexX = 0; indexX < sX; ++indexX) {
            NoiseGradients.CoordCache cx = cacheX[indexX];
            int floorX = cx.floor;
            float alphaX = cx.alpha;
            float relativeX = cx.relative;
            char x0 = p[floorX & 0xFF];
            int x1 = x0 >>> 8;
            for (int indexZ = 0; indexZ < sZ; ++indexZ) {
                NoiseGradients.CoordCache cz = cacheZ[indexZ];
                int floorZ = cz.floor;
                float alphaZ = cz.alpha;
                float relativeZ = cz.relative;
                int lastFloorY = Integer.MIN_VALUE;
                int index = sY * (indexX + indexZ * sX);
                for (int indexY = 0; indexY < sY; ++indexY) {
                    NoiseGradients.CoordCache cy = cacheY[indexY];
                    int floorY = cy.floor;
                    float alphaY = cy.alpha;
                    float relativeY = cy.relative;
                    if (lastFloorY != floorY) {
                        lastFloorY = floorY;
                        char xy00 = p[x0 + floorY & 0xFF];
                        int xy01 = xy00 >>> 8;
                        char z000 = p[xy00 + floorZ & 0xFF];
                        char z010 = p[xy01 + floorZ & 0xFF];
                        int g000 = z000 & 0xF;
                        int g001 = z000 >>> 8 & 0xF;
                        d000xz = NoiseGradients.dotXz((int)g000, (float)relativeX, (float)relativeZ);
                        g000y = NoiseGradients.gradY((int)g000);
                        d001xz = NoiseGradients.dotXz((int)g001, (float)relativeX, (float)(relativeZ - 1.0f));
                        g001y = NoiseGradients.gradY((int)g001);
                        int g010 = z010 & 0xF;
                        int g011 = z010 >>> 8 & 0xF;
                        d010xz = NoiseGradients.dotXz((int)g010, (float)relativeX, (float)relativeZ);
                        g010y = NoiseGradients.gradY((int)g010);
                        d011xz = NoiseGradients.dotXz((int)g011, (float)relativeX, (float)(relativeZ - 1.0f));
                        g011y = NoiseGradients.gradY((int)g011);
                        char xy10 = p[x1 + floorY & 0xFF];
                        int xy11 = xy10 >>> 8;
                        char z100 = p[xy10 + floorZ & 0xFF];
                        char z110 = p[xy11 + floorZ & 0xFF];
                        int g100 = z100 & 0xF;
                        int g101 = z100 >>> 8 & 0xF;
                        d100xz = NoiseGradients.dotXz((int)g100, (float)(relativeX - 1.0f), (float)relativeZ);
                        g100y = NoiseGradients.gradY((int)g100);
                        d101xz = NoiseGradients.dotXz((int)g101, (float)(relativeX - 1.0f), (float)(relativeZ - 1.0f));
                        g101y = NoiseGradients.gradY((int)g101);
                        int g110 = z110 & 0xF;
                        int g111 = z110 >>> 8 & 0xF;
                        d110xz = NoiseGradients.dotXz((int)g110, (float)(relativeX - 1.0f), (float)relativeZ);
                        g110y = NoiseGradients.gradY((int)g110);
                        d111xz = NoiseGradients.dotXz((int)g111, (float)(relativeX - 1.0f), (float)(relativeZ - 1.0f));
                        g111y = NoiseGradients.gradY((int)g111);
                    }
                    int n = index++;
                    buffer.values[n] = buffer.values[n] + amplitude * Mth.lerp3((float)alphaX, (float)alphaY, (float)alphaZ, (float)(d000xz + g000y * relativeY), (float)(d100xz + g100y * relativeY), (float)(d010xz + g010y * (relativeY - 1.0f)), (float)(d110xz + g110y * (relativeY - 1.0f)), (float)(d001xz + g001y * relativeY), (float)(d101xz + g101y * relativeY), (float)(d011xz + g011y * (relativeY - 1.0f)), (float)(d111xz + g111y * (relativeY - 1.0f)));
                }
            }
        }
    }

    private void addToVolumeVanilla(DensityBuffer buffer, DensityVolume volume, double xzScale, double yScale, float amplitude) {
        float d000xz = 0.0f;
        float d100xz = 0.0f;
        float d010xz = 0.0f;
        float d110xz = 0.0f;
        float d001xz = 0.0f;
        float d101xz = 0.0f;
        float d011xz = 0.0f;
        float d111xz = 0.0f;
        float g000y = 0.0f;
        float g100y = 0.0f;
        float g010y = 0.0f;
        float g110y = 0.0f;
        float g001y = 0.0f;
        float g101y = 0.0f;
        float g011y = 0.0f;
        float g111y = 0.0f;
        int index = 0;
        for (int indexZ = 0; indexZ < volume.sizeZ(); ++indexZ) {
            double z = PerlinNoise.wrap((double)((double)volume.blockZ(indexZ) * xzScale)) + this.offsetZ;
            int floorZ = Mth.floor((double)z);
            float relativeZ = (float)(z - (double)floorZ);
            float alphaZ = Mth.smoothstep((float)relativeZ);
            for (int indexX = 0; indexX < volume.sizeX(); ++indexX) {
                double x = PerlinNoise.wrap((double)((double)volume.blockX(indexX) * xzScale)) + this.offsetX;
                int floorX = Mth.floor((double)x);
                float relativeX = (float)(x - (double)floorX);
                int x0 = this.permute(floorX);
                int x1 = this.permute(floorX + 1);
                float alphaX = Mth.smoothstep((float)relativeX);
                int lastFloorY = Integer.MIN_VALUE;
                for (int indexY = 0; indexY < volume.sizeY(); ++indexY) {
                    double y = PerlinNoise.wrap((double)((double)volume.blockY(indexY) * yScale)) + this.offsetY;
                    int floorY = Mth.floor((double)y);
                    float relativeY = (float)(y - (double)floorY);
                    float alphaY = Mth.smoothstep((float)relativeY);
                    if (lastFloorY != floorY) {
                        int xy00 = this.permute(x0 + floorY);
                        int xy01 = this.permute(x0 + floorY + 1);
                        int xy10 = this.permute(x1 + floorY);
                        int xy11 = this.permute(x1 + floorY + 1);
                        GradientNoise.Gradient g000 = this.permuteToGrad(xy00 + floorZ);
                        d000xz = g000.dotXz(relativeX, relativeZ);
                        g000y = g000.y();
                        GradientNoise.Gradient g100 = this.permuteToGrad(xy10 + floorZ);
                        d100xz = g100.dotXz(relativeX - 1.0f, relativeZ);
                        g100y = g100.y();
                        GradientNoise.Gradient g010 = this.permuteToGrad(xy01 + floorZ);
                        d010xz = g010.dotXz(relativeX, relativeZ);
                        g010y = g010.y();
                        GradientNoise.Gradient g110 = this.permuteToGrad(xy11 + floorZ);
                        d110xz = g110.dotXz(relativeX - 1.0f, relativeZ);
                        g110y = g110.y();
                        GradientNoise.Gradient g001 = this.permuteToGrad(xy00 + floorZ + 1);
                        d001xz = g001.dotXz(relativeX, relativeZ - 1.0f);
                        g001y = g001.y();
                        GradientNoise.Gradient g101 = this.permuteToGrad(xy10 + floorZ + 1);
                        d101xz = g101.dotXz(relativeX - 1.0f, relativeZ - 1.0f);
                        g101y = g101.y();
                        GradientNoise.Gradient g011 = this.permuteToGrad(xy01 + floorZ + 1);
                        d011xz = g011.dotXz(relativeX, relativeZ - 1.0f);
                        g011y = g011.y();
                        GradientNoise.Gradient g111 = this.permuteToGrad(xy11 + floorZ + 1);
                        d111xz = g111.dotXz(relativeX - 1.0f, relativeZ - 1.0f);
                        g111y = g111.y();
                        lastFloorY = floorY;
                    }
                    buffer.addTo(index, amplitude * Mth.lerp3((float)alphaX, (float)alphaY, (float)alphaZ, (float)(d000xz + g000y * relativeY), (float)(d100xz + g100y * relativeY), (float)(d010xz + g010y * (relativeY - 1.0f)), (float)(d110xz + g110y * (relativeY - 1.0f)), (float)(d001xz + g001y * relativeY), (float)(d101xz + g101y * relativeY), (float)(d011xz + g011y * (relativeY - 1.0f)), (float)(d111xz + g111y * (relativeY - 1.0f))));
                    ++index;
                }
            }
        }
    }

    private float sampleWithDerivative(int x, int y, int z, float xr, float yr, float zr, float[] derivativeOut) {
        int x0 = this.permute(x);
        int x1 = this.permute(x + 1);
        int xy00 = this.permute(x0 + y);
        int xy01 = this.permute(x0 + y + 1);
        int xy10 = this.permute(x1 + y);
        int xy11 = this.permute(x1 + y + 1);
        GradientNoise.Gradient g000 = this.permuteToGrad(xy00 + z);
        GradientNoise.Gradient g100 = this.permuteToGrad(xy10 + z);
        GradientNoise.Gradient g010 = this.permuteToGrad(xy01 + z);
        GradientNoise.Gradient g110 = this.permuteToGrad(xy11 + z);
        GradientNoise.Gradient g001 = this.permuteToGrad(xy00 + z + 1);
        GradientNoise.Gradient g101 = this.permuteToGrad(xy10 + z + 1);
        GradientNoise.Gradient g011 = this.permuteToGrad(xy01 + z + 1);
        GradientNoise.Gradient g111 = this.permuteToGrad(xy11 + z + 1);
        float d000 = g000.dot(xr, yr, zr);
        float d100 = g100.dot(xr - 1.0f, yr, zr);
        float d010 = g010.dot(xr, yr - 1.0f, zr);
        float d110 = g110.dot(xr - 1.0f, yr - 1.0f, zr);
        float d001 = g001.dot(xr, yr, zr - 1.0f);
        float d101 = g101.dot(xr - 1.0f, yr, zr - 1.0f);
        float d011 = g011.dot(xr, yr - 1.0f, zr - 1.0f);
        float d111 = g111.dot(xr - 1.0f, yr - 1.0f, zr - 1.0f);
        float xAlpha = Mth.smoothstep((float)xr);
        float yAlpha = Mth.smoothstep((float)yr);
        float zAlpha = Mth.smoothstep((float)zr);
        float d1x = Mth.lerp3((float)xAlpha, (float)yAlpha, (float)zAlpha, (float)g000.x(), (float)g100.x(), (float)g010.x(), (float)g110.x(), (float)g001.x(), (float)g101.x(), (float)g011.x(), (float)g111.x());
        float d1y = Mth.lerp3((float)xAlpha, (float)yAlpha, (float)zAlpha, (float)g000.y(), (float)g100.y(), (float)g010.y(), (float)g110.y(), (float)g001.y(), (float)g101.y(), (float)g011.y(), (float)g111.y());
        float d1z = Mth.lerp3((float)xAlpha, (float)yAlpha, (float)zAlpha, (float)g000.z(), (float)g100.z(), (float)g010.z(), (float)g110.z(), (float)g001.z(), (float)g101.z(), (float)g011.z(), (float)g111.z());
        float d2x = Mth.lerp2((float)yAlpha, (float)zAlpha, (float)(d100 - d000), (float)(d110 - d010), (float)(d101 - d001), (float)(d111 - d011));
        float d2y = Mth.lerp2((float)zAlpha, (float)xAlpha, (float)(d010 - d000), (float)(d011 - d001), (float)(d110 - d100), (float)(d111 - d101));
        float d2z = Mth.lerp2((float)xAlpha, (float)yAlpha, (float)(d001 - d000), (float)(d101 - d100), (float)(d011 - d010), (float)(d111 - d110));
        float xSD = Mth.smoothstepDerivative((float)xr);
        float ySD = Mth.smoothstepDerivative((float)yr);
        float zSD = Mth.smoothstepDerivative((float)zr);
        float dX = d1x + xSD * d2x;
        float dY = d1y + ySD * d2y;
        float dZ = d1z + zSD * d2z;
        derivativeOut[0] = derivativeOut[0] + dX;
        derivativeOut[1] = derivativeOut[1] + dY;
        derivativeOut[2] = derivativeOut[2] + dZ;
        return Mth.lerp3((float)xAlpha, (float)yAlpha, (float)zAlpha, (float)d000, (float)d100, (float)d010, (float)d110, (float)d001, (float)d101, (float)d011, (float)d111);
    }

    @VisibleForTesting
    public void parityConfigString(StringBuilder sb) {
        NoiseUtils.parityNoiseOctaveConfigString((StringBuilder)sb, (double)this.offsetX, (double)this.offsetY, (double)this.offsetZ, (byte[])this.perms);
    }
}
