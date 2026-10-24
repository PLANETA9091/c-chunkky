package net.minecraft.world.level.levelgen.densityfunction;

public record CachingDensitySampler(int id, DensitySampler input) implements DensitySampler {
   @Override
   public void sampleVolume(SamplerContext context, DensityBuffer outputBuffer, DensityVolume volume) {
      context.sampleVolumeCached(this.id, this.input, outputBuffer, volume);
   }

   @Override
   public float sampleValue(SamplerContext context, int blockX, int blockY, int blockZ) {
      return context.sampleValueCached(this.id, this.input, blockX, blockY, blockZ);
   }
}
