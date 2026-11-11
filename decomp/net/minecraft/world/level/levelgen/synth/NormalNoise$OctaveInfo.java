/*
 * Decompiled with CFR 0.152.
 */
package net.minecraft.world.level.levelgen.synth;

private record NormalNoise.OctaveInfo(int octaveIndex, double frequency, double amplitude) {
    public String seed() {
        return "octave_" + this.octaveIndex;
    }

    public double absAmplitude() {
        return Math.abs(this.amplitude);
    }
}
