package net.minecraft.world.level.levelgen.densityfunction;

@FunctionalInterface
public interface DensitySamplerSet {
   DensitySampler.Bound get(DensityFunction var1);

   default float sampleValue(DensityFunction function, int blockX, int blockY, int blockZ) {
      return this.get(function).sampleValue(blockX, blockY, blockZ);
   }
}
