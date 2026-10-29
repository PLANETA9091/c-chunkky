package net.minecraft.world.level.levelgen.densityfunction;

public interface DensitySampler {
   void sampleVolume(SamplerContext var1, DensityBuffer var2, DensityVolume var3);

   static void sampleVolumeNaive(SamplerContext context, DensityBuffer outputBuffer, DensityVolume volume, DensitySampler sampler) {
      int index = 0;

      for (int z = 0; z < volume.sizeZ(); z++) {
         int blockZ = volume.blockZ(z);

         for (int x = 0; x < volume.sizeX(); x++) {
            int blockX = volume.blockX(x);

            for (int y = 0; y < volume.sizeY(); y++) {
               int blockY = volume.blockY(y);
               outputBuffer.set(index++, sampler.sampleValue(context, blockX, blockY, blockZ));
            }
         }
      }
   }

   float sampleValue(SamplerContext var1, int var2, int var3, int var4);

   default DensitySampler.Bound bind(SamplerContext context) {
      return new DensitySampler.Bound(this, context);
   }

   public static record Bound(DensitySampler sampler, SamplerContext context) {
      public float sampleValue(int blockX, int blockY, int blockZ) {
         return this.sampler.sampleValue(this.context, blockX, blockY, blockZ);
      }

      public void sampleVolume(DensityBuffer outputBuffer, DensityVolume volume) {
         this.sampler.sampleVolume(this.context, outputBuffer, volume);
      }

      public ScopedDensityBuffer sampleVolume(DensityVolume volume) {
         ScopedDensityBuffer buffer = this.context.acquireBuffer(volume);
         this.sampleVolume(buffer, volume);
         return buffer;
      }
   }
}
