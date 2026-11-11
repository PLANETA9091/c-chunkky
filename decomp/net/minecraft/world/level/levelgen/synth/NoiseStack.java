/*
 * Decompiled with CFR 0.152.
 * 
 * Could not load the following classes:
 *  com.google.common.annotations.VisibleForTesting
 *  net.minecraft.util.Interval
 *  net.minecraft.world.level.levelgen.densityfunction.DensityBuffer
 *  net.minecraft.world.level.levelgen.densityfunction.DensityVolume
 *  net.minecraft.world.level.levelgen.synth.Noise
 *  net.minecraft.world.level.levelgen.synth.NoiseStack$Builder
 *  net.minecraft.world.level.levelgen.synth.NoiseStack$Layer
 */
package net.minecraft.world.level.levelgen.synth;

import com.google.common.annotations.VisibleForTesting;
import net.minecraft.util.Interval;
import net.minecraft.world.level.levelgen.densityfunction.DensityBuffer;
import net.minecraft.world.level.levelgen.densityfunction.DensityVolume;
import net.minecraft.world.level.levelgen.synth.Noise;
import net.minecraft.world.level.levelgen.synth.NoiseStack;

public class NoiseStack
implements Noise {
    protected final Layer[] layers;
    private final Interval range;

    private NoiseStack(Layer[] layers) {
        this.layers = layers;
        Interval range = Interval.ofExact((float)0.0f);
        for (Layer layer : layers) {
            Interval layerRange = Interval.mul((Interval)layer.noise.range(), (Interval)Interval.ofExact((float)layer.amplitude));
            range = Interval.add((Interval)range, (Interval)layerRange);
        }
        this.range = range;
    }

    public static Builder builder() {
        return new Builder();
    }

    public Interval range() {
        return this.range;
    }

    public float get(double x, double y, double z) {
        float value = 0.0f;
        for (Layer layer : this.layers) {
            double frequency = layer.frequency;
            value += layer.amplitude * layer.noise.get(x * frequency, y * frequency, z * frequency);
        }
        return value;
    }

    public float get(double x, double y) {
        float value = 0.0f;
        for (Layer layer : this.layers) {
            double frequency = layer.frequency;
            value += layer.amplitude * layer.noise.get(x * frequency, y * frequency);
        }
        return value;
    }

    public void addToVolume(DensityBuffer buffer, DensityVolume volume, double xzScale, double yScale, float amplitude) {
        for (Layer layer : this.layers) {
            double frequency = layer.frequency;
            layer.noise.addToVolume(buffer, volume, xzScale * frequency, yScale * frequency, amplitude * layer.amplitude);
        }
    }

    @VisibleForTesting
    public Noise getLayer(int index) {
        return this.layers[index].noise;
    }
}
