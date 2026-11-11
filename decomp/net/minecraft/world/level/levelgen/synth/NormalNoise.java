/*
 * Decompiled with CFR 0.152.
 * 
 * Could not load the following classes:
 *  com.google.common.annotations.VisibleForTesting
 *  com.google.common.collect.ImmutableList
 *  com.google.common.collect.ImmutableList$Builder
 *  com.mojang.serialization.Codec
 *  it.unimi.dsi.fastutil.doubles.DoubleArrayList
 *  it.unimi.dsi.fastutil.doubles.DoubleList
 *  net.minecraft.core.Holder
 *  net.minecraft.core.registries.Registries
 *  net.minecraft.core.registries.codec.RegistryCodecs
 *  net.minecraft.resources.ResourceKey
 *  net.minecraft.util.Interval
 *  net.minecraft.util.Mth
 *  net.minecraft.util.RandomSource
 *  net.minecraft.world.level.levelgen.synth.LegacyFbmInitializer
 *  net.minecraft.world.level.levelgen.synth.NoiseStack
 *  net.minecraft.world.level.levelgen.synth.NoiseStack$Builder
 *  net.minecraft.world.level.levelgen.synth.NormalNoise$Builder
 *  net.minecraft.world.level.levelgen.synth.NormalNoise$Normalization
 *  net.minecraft.world.level.levelgen.synth.NormalNoise$OctaveInfo
 *  net.minecraft.world.level.levelgen.synth.NormalNoise$Parameters
 *  net.minecraft.world.level.levelgen.synth.PerlinNoise
 */
package net.minecraft.world.level.levelgen.synth;

import com.google.common.annotations.VisibleForTesting;
import com.google.common.collect.ImmutableList;
import com.mojang.serialization.Codec;
import it.unimi.dsi.fastutil.doubles.DoubleArrayList;
import it.unimi.dsi.fastutil.doubles.DoubleList;
import java.util.Collections;
import java.util.List;
import java.util.Locale;
import java.util.Optional;
import java.util.stream.IntStream;
import net.minecraft.core.Holder;
import net.minecraft.core.registries.Registries;
import net.minecraft.core.registries.codec.RegistryCodecs;
import net.minecraft.resources.ResourceKey;
import net.minecraft.util.Interval;
import net.minecraft.util.Mth;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.levelgen.PositionalRandomFactory;
import net.minecraft.world.level.levelgen.synth.LegacyFbmInitializer;
import net.minecraft.world.level.levelgen.synth.Noise;
import net.minecraft.world.level.levelgen.synth.NoiseStack;
import net.minecraft.world.level.levelgen.synth.NormalNoise;
import net.minecraft.world.level.levelgen.synth.PerlinNoise;

public final class NormalNoise {
    private static final double SAMPLE_COUNT = 2.0;
    private static final double INPUT_FACTOR = 1.0181268882175227;
    @VisibleForTesting
    public static final double TARGET_DEVIATION = 0.3333333333333333;
    private static final int MAX_OCTAVE_COUNT = 32;
    private static final double MAX_AMPLITUDE = 1000000.0;
    private static final double PERSISTENCE = 0.5;
    private static final double LACUNARITY = 2.0;
    public static final Codec<NormalNoise> DIRECT_CODEC = Parameters.CODEC.xmap(NormalNoise::new, n -> n.parameters);
    public static final Codec<Holder<NormalNoise>> CODEC = RegistryCodecs.holder((ResourceKey)Registries.NOISE, DIRECT_CODEC);
    private final Parameters parameters;
    private final List<OctaveInfo> octaves;
    private final double normalizationFactor;
    private final Interval range;

    private NormalNoise(Parameters parameters) {
        this.parameters = parameters;
        this.octaves = NormalNoise.buildOctaves(parameters.baseOctave, parameters.baseAmplitude, parameters.octaveCount, parameters.normalize != Normalization.DISABLED, parameters.amplitudeModifiers);
        double targetAmplitude = this.octaves.stream().mapToDouble(OctaveInfo::absAmplitude).sum();
        double normalizationFactor = NormalNoise.computeNormalizationFactor(targetAmplitude, this.octaves);
        if (parameters.normalize == Normalization.LEGACY && normalizationFactor != 0.0) {
            double parityNormalizationFactor = NormalNoise.computeParityNormalizationFactor(parameters.baseAmplitude, parameters.octaveCount, parameters.amplitudeModifiers);
            targetAmplitude *= parityNormalizationFactor / normalizationFactor;
            normalizationFactor = parityNormalizationFactor;
        }
        this.normalizationFactor = normalizationFactor;
        this.range = Interval.ofSymmetric((float)((float)(targetAmplitude * 0.3333333333333333 * 6.0)));
    }

    private static List<OctaveInfo> buildOctaves(int baseOctave, double baseAmplitude, int octaveCount, boolean normalize, DoubleList amplitudeModifiers) {
        double frequency = Math.pow(2.0, baseOctave);
        double amplitude = baseAmplitude;
        if (normalize) {
            amplitude *= Math.pow(0.5, -(octaveCount - 1)) / (Math.pow(0.5, -octaveCount) - 1.0);
        }
        ImmutableList.Builder octaves = ImmutableList.builderWithExpectedSize((int)octaveCount);
        for (int i = 0; i < octaveCount; ++i) {
            double amplitudeModifier = NormalNoise.getAmplitudeModifier(amplitudeModifiers, i);
            if (amplitudeModifier != 0.0) {
                double modifiedAmplitude = amplitude * amplitudeModifier;
                octaves.add((Object)new OctaveInfo(baseOctave + i, frequency, modifiedAmplitude));
            }
            frequency *= 2.0;
            amplitude *= 0.5;
        }
        return octaves.build();
    }

    private static double getAmplitudeModifier(DoubleList amplitudeModifiers, int index) {
        return amplitudeModifiers.isEmpty() ? 1.0 : amplitudeModifiers.getDouble(index);
    }

    private static double computeNormalizationFactor(double targetAmplitude, List<OctaveInfo> octaves) {
        double inputDeviation = NormalNoise.estimateDeviation(octaves);
        if (inputDeviation == 0.0) {
            return 0.0;
        }
        double inputSumDeviation = inputDeviation * Math.sqrt(2.0);
        double targetDeviation = targetAmplitude * 0.3333333333333333;
        return targetDeviation / inputSumDeviation;
    }

    private static double estimateDeviation(List<OctaveInfo> octaves) {
        double variance = 0.0;
        for (OctaveInfo octave : octaves) {
            double layerDeviation = 0.2702247831245211 * octave.absAmplitude();
            variance += Mth.square((double)layerDeviation);
        }
        return Math.sqrt(variance);
    }

    public static Builder builder() {
        return new Builder();
    }

    public static NormalNoise createParity(int firstOctave, DoubleList amplitudes) {
        if (amplitudes.isEmpty()) {
            throw new IllegalArgumentException("Need at least 1 amplitude");
        }
        Builder parameters = NormalNoise.builder();
        parameters.setBaseOctave(firstOctave);
        parameters.setOctaveCount(amplitudes.size());
        parameters.setBaseAmplitude(NormalNoise.computeParityBaseAmplitude(parameters.baseOctave, amplitudes));
        for (int i = 0; i < amplitudes.size(); ++i) {
            double modifier = amplitudes.getDouble(i);
            if (modifier == 1.0) continue;
            parameters.setAmplitudeModifier(i, modifier);
        }
        return parameters.build();
    }

    public static NormalNoise createParity(int firstOctave, double ... amplitudes) {
        return NormalNoise.createParity(firstOctave, DoubleList.of((double[])amplitudes));
    }

    private static double computeParityBaseAmplitude(int baseOctave, DoubleList amplitudes) {
        List<OctaveInfo> octaves = NormalNoise.buildOctaves(baseOctave, 1.0, amplitudes.size(), true, amplitudes);
        double targetAmplitude = octaves.stream().mapToDouble(OctaveInfo::absAmplitude).sum();
        double newNormalizationFactor = NormalNoise.computeNormalizationFactor(targetAmplitude, octaves);
        if (newNormalizationFactor == 0.0) {
            return 1.0;
        }
        double oldNormalizationFactor = NormalNoise.computeParityNormalizationFactor(1.0, amplitudes.size(), amplitudes);
        return oldNormalizationFactor / newNormalizationFactor;
    }

    private static double computeParityNormalizationFactor(double baseAmplitude, int octaveCount, DoubleList amplitudeModifiers) {
        int minOctave = Integer.MAX_VALUE;
        int maxOctave = Integer.MIN_VALUE;
        for (int i = 0; i < octaveCount; ++i) {
            double modifier = NormalNoise.getAmplitudeModifier(amplitudeModifiers, i);
            if (modifier == 0.0) continue;
            minOctave = Math.min(minOctave, i);
            maxOctave = Math.max(maxOctave, i);
        }
        return baseAmplitude * 0.5 * 0.3333333333333333 / NormalNoise.parityExpectedDeviation(maxOctave - minOctave);
    }

    private static double parityExpectedDeviation(int octaveSpan) {
        return 0.1 * (1.0 + 1.0 / (double)(octaveSpan + 1));
    }

    public Noise create(RandomSource random) {
        PositionalRandomFactory firstRandom = random.forkPositional();
        PositionalRandomFactory secondRandom = random.forkPositional();
        NoiseStack.Builder stack = NoiseStack.builder();
        for (OctaveInfo octave : this.octaves) {
            String octaveSeed = octave.seed();
            PerlinNoise firstNoise = new PerlinNoise(firstRandom.fromHashOf(octaveSeed));
            PerlinNoise secondNoise = new PerlinNoise(secondRandom.fromHashOf(octaveSeed));
            float valueFactor = (float)(this.normalizationFactor * octave.amplitude);
            stack.add((Noise)firstNoise, octave.frequency, valueFactor);
            stack.add((Noise)secondNoise, octave.frequency * 1.0181268882175227, valueFactor);
        }
        return stack.build();
    }

    @Deprecated
    public Noise createForLegacyNetherBiome(RandomSource random) {
        DoubleList amplitudes = this.parameters.amplitudeModifiers();
        if (amplitudes.isEmpty()) {
            amplitudes = new DoubleArrayList(Collections.nCopies(this.parameters.octaveCount(), 1.0));
        }
        NoiseStack first = LegacyFbmInitializer.createForLegacyNetherBiome((RandomSource)random, (int)this.parameters.baseOctave(), (DoubleList)amplitudes);
        NoiseStack second = LegacyFbmInitializer.createForLegacyNetherBiome((RandomSource)random, (int)this.parameters.baseOctave(), (DoubleList)amplitudes);
        float valueFactor = (float)(this.normalizationFactor * this.parameters.baseAmplitude());
        return NoiseStack.builder().addStack(first, 1.0, valueFactor).addStack(second, 1.0181268882175227, valueFactor).build();
    }

    public Interval range() {
        return this.range;
    }

    /*
     * Enabled force condition propagation
     * Lifted jumps to return sites
     */
    public boolean equals(Object obj) {
        if (obj == this) return true;
        if (!(obj instanceof NormalNoise)) return false;
        NormalNoise noise = (NormalNoise)obj;
        if (!this.parameters.equals((Object)noise.parameters)) return false;
        return true;
    }

    public int hashCode() {
        return this.parameters.hashCode();
    }

    public String toString() {
        return "NormalNoise[parameters=" + String.valueOf(this.parameters) + "]";
    }

    @VisibleForTesting
    public void parityConfigString(StringBuilder sb, RandomSource random) {
        sb.append("NormalNoise {");
        sb.append("first: ");
        this.perlinParityConfigString(sb, random);
        sb.append(", second: ");
        this.perlinParityConfigString(sb, random);
        sb.append("}");
    }

    @VisibleForTesting
    private void perlinParityConfigString(StringBuilder sb, RandomSource random) {
        sb.append("PerlinNoise{");
        List<String> amplitudeStrings = IntStream.range(0, this.parameters.octaveCount).mapToDouble(ix -> NormalNoise.getAmplitudeModifier(this.parameters.amplitudeModifiers, ix)).mapToObj(d -> String.format(Locale.ROOT, "%.2f", d)).toList();
        sb.append("first octave: ").append(this.parameters.baseOctave).append(", amplitudes: ").append(amplitudeStrings).append(", noise levels: [");
        PositionalRandomFactory positional = random.forkPositional();
        for (int i = 0; i < this.parameters.octaveCount; ++i) {
            sb.append(i).append(": ");
            int octaveIndex = this.parameters.baseOctave + i;
            Optional<OctaveInfo> octave = this.octaves.stream().filter(info -> info.octaveIndex == octaveIndex).findFirst();
            if (octave.isPresent()) {
                new PerlinNoise(positional.fromHashOf(octave.get().seed())).parityConfigString(sb);
            } else {
                sb.append("null");
            }
            sb.append(", ");
        }
        sb.append("]");
        sb.append("}");
    }
}
