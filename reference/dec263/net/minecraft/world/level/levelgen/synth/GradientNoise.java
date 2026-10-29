package net.minecraft.world.level.levelgen.synth;

import net.minecraft.SharedConstants;
import net.minecraft.util.RandomSource;

public abstract class GradientNoise implements Noise {
   public static final GradientNoise.Gradient[] GRADIENT = new GradientNoise.Gradient[]{
      new GradientNoise.Gradient(1, 1, 0),
      new GradientNoise.Gradient(-1, 1, 0),
      new GradientNoise.Gradient(1, -1, 0),
      new GradientNoise.Gradient(-1, -1, 0),
      new GradientNoise.Gradient(1, 0, 1),
      new GradientNoise.Gradient(-1, 0, 1),
      new GradientNoise.Gradient(1, 0, -1),
      new GradientNoise.Gradient(-1, 0, -1),
      new GradientNoise.Gradient(0, 1, 1),
      new GradientNoise.Gradient(0, -1, 1),
      new GradientNoise.Gradient(0, 1, -1),
      new GradientNoise.Gradient(0, -1, -1),
      new GradientNoise.Gradient(1, 1, 0),
      new GradientNoise.Gradient(0, -1, 1),
      new GradientNoise.Gradient(-1, 1, 0),
      new GradientNoise.Gradient(0, -1, -1)
   };
   private static final int ROUND_OFF = 33554432;
   private static final double HALF_ROUND_OFF = Math.nextDown(1.6777216E7);
   protected final byte[] perms = new byte[256];
   protected final double offsetX;
   protected final double offsetY;
   protected final double offsetZ;

   protected GradientNoise(RandomSource random) {
      this(random, 256.0);
   }

   protected GradientNoise(RandomSource random, double noiseOffsetScale) {
      this.offsetX = random.nextDouble() * noiseOffsetScale;
      this.offsetY = random.nextDouble() * noiseOffsetScale;
      this.offsetZ = random.nextDouble() * noiseOffsetScale;

      for (int i = 0; i < 256; i++) {
         this.perms[i] = (byte)i;
      }

      for (int i = 0; i < 256; i++) {
         int offset = random.nextInt(256 - i);
         byte tmp = this.perms[i];
         this.perms[i] = this.perms[offset + i];
         this.perms[offset + i] = tmp;
      }
   }

   protected int permute(int x) {
      return this.perms[x & 0xFF] & 0xFF;
   }

   protected GradientNoise.Gradient permuteToGrad(int x) {
      return GRADIENT[this.permute(x) & 15];
   }

   protected static float gradDot(int hash, float x, float y, float z) {
      return GRADIENT[hash & 15].dot(x, y, z);
   }

   protected static double wrap(double x) {
      if (SharedConstants.DEBUG_ENABLE_FARLANDS) {
         return x;
      } else {
         return x >= -HALF_ROUND_OFF && x < HALF_ROUND_OFF ? x : x - Math.floor(x / 3.3554432E7 + 0.5) * 3.3554432E7;
      }
   }

   public static record Gradient(int x, int y, int z) {
      public double dot(double x, double y, double z) {
         return (double)this.x * x + (double)this.y * y + (double)this.z * z;
      }

      public float dot(float x, float y, float z) {
         return (float)this.x * x + (float)this.y * y + (float)this.z * z;
      }

      public float dotXz(float x, float z) {
         return (float)this.x * x + (float)this.z * z;
      }
   }
}
