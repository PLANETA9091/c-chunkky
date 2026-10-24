package net.minecraft.world.level.levelgen.synth;

import ca.spottedleaf.moonrise.patches.chunk_gen.noise.ChunkGenPerlinNoise;
import ca.spottedleaf.moonrise.patches.chunk_gen.noise.NoiseGradients;
import ca.spottedleaf.moonrise.patches.chunk_gen.noise.NoiseGradients.CoordCache;
import com.google.common.annotations.VisibleForTesting;
import net.minecraft.util.Interval;
import net.minecraft.util.Mth;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.levelgen.densityfunction.DensityBuffer;
import net.minecraft.world.level.levelgen.densityfunction.DensityVolume;

public class PerlinNoise extends GradientNoise implements ChunkGenPerlinNoise {
   public static final Interval RANGE = Interval.ofSymmetric(2.0F);
   public static final double STANDARD_DEVIATION = 0.2702247831245211;
   protected final char[] pAdj = new char[256];

   @Override
   public final char[] moonrise$getPAdj() {
      return this.pAdj;
   }

   private final int p(int x) {
      return this.perms[x & 0xFF] & 0xFF;
   }

   public PerlinNoise(RandomSource random) {
      super(random);

      for (int i = 0; i < 256; i++) {
         this.pAdj[i] = (char)(this.p(i) | this.p(i + 1) << 8);
      }
   }

   @Override
   public Interval range() {
      return RANGE;
   }

   @Override
   public float get(double x, double y) {
      return this.get(wrap(x), 0.0, wrap(y));
   }

   @Override
   public float get(double _x, double _y, double _z) {
      double x = wrap(_x) + this.offsetX;
      double y = wrap(_y) + this.offsetY;
      double z = wrap(_z) + this.offsetZ;
      int floorX = Mth.floor(x);
      int floorY = Mth.floor(y);
      int floorZ = Mth.floor(z);
      float relativeX = (float)(x - (double)floorX);
      float relativeY = (float)(y - (double)floorY);
      float relativeZ = (float)(z - (double)floorZ);
      return this.sampleAndLerp(floorX, floorY, floorZ, relativeX, relativeY, relativeZ, relativeY);
   }

   public float noiseWithDerivative(double _x, double _y, double _z, float[] derivativeOut) {
      double x = wrap(_x) + this.offsetX;
      double y = wrap(_y) + this.offsetY;
      double z = wrap(_z) + this.offsetZ;
      int floorX = Mth.floor(x);
      int floorY = Mth.floor(y);
      int floorZ = Mth.floor(z);
      float relativeX = (float)(x - (double)floorX);
      float relativeY = (float)(y - (double)floorY);
      float relativeZ = (float)(z - (double)floorZ);
      return this.sampleWithDerivative(floorX, floorY, floorZ, relativeX, relativeY, relativeZ, derivativeOut);
   }

   protected float sampleAndLerp(int x, int y, int z, float relativeX, float relativeY, float relativeZ, float originalRelativeY) {
      char[] p = this.pAdj;
      int x0 = p[x & 0xFF];
      int x1 = x0 >>> 8;
      int xy00 = p[x0 + y & 0xFF];
      int xy01 = xy00 >>> 8;
      int z000 = p[xy00 + z & 0xFF];
      int z001 = p[xy01 + z & 0xFF];
      float d000 = NoiseGradients.dot(z000 & 15, relativeX, relativeY, relativeZ);
      float d001 = NoiseGradients.dot(z000 >>> 8 & 15, relativeX, relativeY, relativeZ - 1.0F);
      float d010 = NoiseGradients.dot(z001 & 15, relativeX, relativeY - 1.0F, relativeZ);
      float d011 = NoiseGradients.dot(z001 >>> 8 & 15, relativeX, relativeY - 1.0F, relativeZ - 1.0F);
      int xy10 = p[x1 + y & 0xFF];
      int xy11 = xy10 >>> 8;
      int z100 = p[xy10 + z & 0xFF];
      int z101 = p[xy11 + z & 0xFF];
      float d100 = NoiseGradients.dot(z100 & 15, relativeX - 1.0F, relativeY, relativeZ);
      float d101 = NoiseGradients.dot(z100 >>> 8 & 15, relativeX - 1.0F, relativeY, relativeZ - 1.0F);
      float d110 = NoiseGradients.dot(z101 & 15, relativeX - 1.0F, relativeY - 1.0F, relativeZ);
      float d111 = NoiseGradients.dot(z101 >>> 8 & 15, relativeX - 1.0F, relativeY - 1.0F, relativeZ - 1.0F);
      return Mth.lerp3(Mth.smoothstep(relativeX), Mth.smoothstep(originalRelativeY), Mth.smoothstep(relativeZ), d000, d100, d010, d110, d001, d101, d011, d111);
   }

   private float sampleAndLerpVanilla(int x, int y, int z, float relativeX, float relativeY, float relativeZ, float originalRelativeY) {
      int x0 = this.permute(x);
      int x1 = this.permute(x + 1);
      int xy00 = this.permute(x0 + y);
      int xy01 = this.permute(x0 + y + 1);
      int xy10 = this.permute(x1 + y);
      int xy11 = this.permute(x1 + y + 1);
      float d000 = gradDot(this.permute(xy00 + z), relativeX, relativeY, relativeZ);
      float d100 = gradDot(this.permute(xy10 + z), relativeX - 1.0F, relativeY, relativeZ);
      float d010 = gradDot(this.permute(xy01 + z), relativeX, relativeY - 1.0F, relativeZ);
      float d110 = gradDot(this.permute(xy11 + z), relativeX - 1.0F, relativeY - 1.0F, relativeZ);
      float d001 = gradDot(this.permute(xy00 + z + 1), relativeX, relativeY, relativeZ - 1.0F);
      float d101 = gradDot(this.permute(xy10 + z + 1), relativeX - 1.0F, relativeY, relativeZ - 1.0F);
      float d011 = gradDot(this.permute(xy01 + z + 1), relativeX, relativeY - 1.0F, relativeZ - 1.0F);
      float d111 = gradDot(this.permute(xy11 + z + 1), relativeX - 1.0F, relativeY - 1.0F, relativeZ - 1.0F);
      float xAlpha = Mth.smoothstep(relativeX);
      float yAlpha = Mth.smoothstep(originalRelativeY);
      float zAlpha = Mth.smoothstep(relativeZ);
      return Mth.lerp3(xAlpha, yAlpha, zAlpha, d000, d100, d010, d110, d001, d101, d011, d111);
   }

   @Override
   public void addToVolume(DensityBuffer buffer, DensityVolume volume, double xzScale, double yScale, float amplitude) {
      float d000xz = 0.0F;
      float d100xz = 0.0F;
      float d010xz = 0.0F;
      float d110xz = 0.0F;
      float d001xz = 0.0F;
      float d101xz = 0.0F;
      float d011xz = 0.0F;
      float d111xz = 0.0F;
      float g000y = 0.0F;
      float g100y = 0.0F;
      float g010y = 0.0F;
      float g110y = 0.0F;
      float g001y = 0.0F;
      float g101y = 0.0F;
      float g011y = 0.0F;
      float g111y = 0.0F;
      int sX = volume.sizeX();
      int sY = volume.sizeY();
      int sZ = volume.sizeZ();
      CoordCache[] cacheX = NoiseGradients.getCoordCache(NoiseGradients.COORD_CACHE_X, sX);
      CoordCache[] cacheY = NoiseGradients.getCoordCache(NoiseGradients.COORD_CACHE_Y, sY);
      CoordCache[] cacheZ = NoiseGradients.getCoordCache(NoiseGradients.COORD_CACHE_Z, sZ);
      double xv = (double)volume.minBlockX();
      double xi = (double)volume.stepBlockX();

      for (int x = 0; x < sX; xv += xi) {
         double val = wrap(xv * xzScale) + this.offsetX;
         double floorD = Math.floor(val);
         int floor = (int)floorD;
         float relative = (float)(val - floorD);
         float alpha = Mth.smoothstep(relative);
         CoordCache cx = cacheX[x];
         cx.floor = floor;
         cx.alpha = alpha;
         cx.relative = relative;
         x++;
      }

      double yv = (double)volume.minBlockY();
      double yi = (double)volume.stepBlockY();

      for (int y = 0; y < sY; yv += yi) {
         double val = wrap(yv * yScale) + this.offsetY;
         double floorD = Math.floor(val);
         int floor = (int)floorD;
         float relative = (float)(val - floorD);
         float alpha = Mth.smoothstep(relative);
         CoordCache cy = cacheY[y];
         cy.floor = floor;
         cy.alpha = alpha;
         cy.relative = relative;
         y++;
      }

      double zv = (double)volume.minBlockZ();
      double zi = (double)volume.stepBlockZ();

      for (int z = 0; z < sZ; zv += zi) {
         double val = wrap(zv * xzScale) + this.offsetZ;
         double floorD = Math.floor(val);
         int floor = (int)floorD;
         float relative = (float)(val - floorD);
         float alpha = Mth.smoothstep(relative);
         CoordCache cz = cacheZ[z];
         cz.floor = floor;
         cz.alpha = alpha;
         cz.relative = relative;
         z++;
      }

      char[] p = this.pAdj;

      for (int indexX = 0; indexX < sX; indexX++) {
         CoordCache cx = cacheX[indexX];
         int floorX = cx.floor;
         float alphaX = cx.alpha;
         float relativeX = cx.relative;
         int x0 = p[floorX & 0xFF];
         int x1 = x0 >>> 8;

         for (int indexZ = 0; indexZ < sZ; indexZ++) {
            CoordCache cz = cacheZ[indexZ];
            int floorZ = cz.floor;
            float alphaZ = cz.alpha;
            float relativeZ = cz.relative;
            int lastFloorY = Integer.MIN_VALUE;
            int index = sY * (indexX + indexZ * sX);

            for (int indexY = 0; indexY < sY; indexY++) {
               CoordCache cy = cacheY[indexY];
               int floorY = cy.floor;
               float alphaY = cy.alpha;
               float relativeY = cy.relative;
               if (lastFloorY != floorY) {
                  lastFloorY = floorY;
                  int xy00 = p[x0 + floorY & 0xFF];
                  int xy01 = xy00 >>> 8;
                  int z000 = p[xy00 + floorZ & 0xFF];
                  int z010 = p[xy01 + floorZ & 0xFF];
                  int g000 = z000 & 15;
                  int g001 = z000 >>> 8 & 15;
                  d000xz = NoiseGradients.dotXz(g000, relativeX, relativeZ);
                  g000y = NoiseGradients.gradY(g000);
                  d001xz = NoiseGradients.dotXz(g001, relativeX, relativeZ - 1.0F);
                  g001y = NoiseGradients.gradY(g001);
                  int g010 = z010 & 15;
                  int g011 = z010 >>> 8 & 15;
                  d010xz = NoiseGradients.dotXz(g010, relativeX, relativeZ);
                  g010y = NoiseGradients.gradY(g010);
                  d011xz = NoiseGradients.dotXz(g011, relativeX, relativeZ - 1.0F);
                  g011y = NoiseGradients.gradY(g011);
                  int xy10 = p[x1 + floorY & 0xFF];
                  int xy11 = xy10 >>> 8;
                  int z100 = p[xy10 + floorZ & 0xFF];
                  int z110 = p[xy11 + floorZ & 0xFF];
                  int g100 = z100 & 15;
                  int g101 = z100 >>> 8 & 15;
                  d100xz = NoiseGradients.dotXz(g100, relativeX - 1.0F, relativeZ);
                  g100y = NoiseGradients.gradY(g100);
                  d101xz = NoiseGradients.dotXz(g101, relativeX - 1.0F, relativeZ - 1.0F);
                  g101y = NoiseGradients.gradY(g101);
                  int g110 = z110 & 15;
                  int g111 = z110 >>> 8 & 15;
                  d110xz = NoiseGradients.dotXz(g110, relativeX - 1.0F, relativeZ);
                  g110y = NoiseGradients.gradY(g110);
                  d111xz = NoiseGradients.dotXz(g111, relativeX - 1.0F, relativeZ - 1.0F);
                  g111y = NoiseGradients.gradY(g111);
               }

               int var10001 = index++;
               buffer.values[var10001] = buffer.values[var10001]
                  + amplitude
                     * Mth.lerp3(
                        alphaX,
                        alphaY,
                        alphaZ,
                        d000xz + g000y * relativeY,
                        d100xz + g100y * relativeY,
                        d010xz + g010y * (relativeY - 1.0F),
                        d110xz + g110y * (relativeY - 1.0F),
                        d001xz + g001y * relativeY,
                        d101xz + g101y * relativeY,
                        d011xz + g011y * (relativeY - 1.0F),
                        d111xz + g111y * (relativeY - 1.0F)
                     );
            }
         }
      }
   }

   private void addToVolumeVanilla(DensityBuffer buffer, DensityVolume volume, double xzScale, double yScale, float amplitude) {
      float d000xz = 0.0F;
      float d100xz = 0.0F;
      float d010xz = 0.0F;
      float d110xz = 0.0F;
      float d001xz = 0.0F;
      float d101xz = 0.0F;
      float d011xz = 0.0F;
      float d111xz = 0.0F;
      float g000y = 0.0F;
      float g100y = 0.0F;
      float g010y = 0.0F;
      float g110y = 0.0F;
      float g001y = 0.0F;
      float g101y = 0.0F;
      float g011y = 0.0F;
      float g111y = 0.0F;
      int index = 0;

      for (int indexZ = 0; indexZ < volume.sizeZ(); indexZ++) {
         double z = wrap((double)volume.blockZ(indexZ) * xzScale) + this.offsetZ;
         int floorZ = Mth.floor(z);
         float relativeZ = (float)(z - (double)floorZ);
         float alphaZ = Mth.smoothstep(relativeZ);

         for (int indexX = 0; indexX < volume.sizeX(); indexX++) {
            double x = wrap((double)volume.blockX(indexX) * xzScale) + this.offsetX;
            int floorX = Mth.floor(x);
            float relativeX = (float)(x - (double)floorX);
            int x0 = this.permute(floorX);
            int x1 = this.permute(floorX + 1);
            float alphaX = Mth.smoothstep(relativeX);
            int lastFloorY = Integer.MIN_VALUE;

            for (int indexY = 0; indexY < volume.sizeY(); indexY++) {
               double y = wrap((double)volume.blockY(indexY) * yScale) + this.offsetY;
               int floorY = Mth.floor(y);
               float relativeY = (float)(y - (double)floorY);
               float alphaY = Mth.smoothstep(relativeY);
               if (lastFloorY != floorY) {
                  int xy00 = this.permute(x0 + floorY);
                  int xy01 = this.permute(x0 + floorY + 1);
                  int xy10 = this.permute(x1 + floorY);
                  int xy11 = this.permute(x1 + floorY + 1);
                  GradientNoise.Gradient g000 = this.permuteToGrad(xy00 + floorZ);
                  d000xz = g000.dotXz(relativeX, relativeZ);
                  g000y = (float)g000.y();
                  GradientNoise.Gradient g100 = this.permuteToGrad(xy10 + floorZ);
                  d100xz = g100.dotXz(relativeX - 1.0F, relativeZ);
                  g100y = (float)g100.y();
                  GradientNoise.Gradient g010 = this.permuteToGrad(xy01 + floorZ);
                  d010xz = g010.dotXz(relativeX, relativeZ);
                  g010y = (float)g010.y();
                  GradientNoise.Gradient g110 = this.permuteToGrad(xy11 + floorZ);
                  d110xz = g110.dotXz(relativeX - 1.0F, relativeZ);
                  g110y = (float)g110.y();
                  GradientNoise.Gradient g001 = this.permuteToGrad(xy00 + floorZ + 1);
                  d001xz = g001.dotXz(relativeX, relativeZ - 1.0F);
                  g001y = (float)g001.y();
                  GradientNoise.Gradient g101 = this.permuteToGrad(xy10 + floorZ + 1);
                  d101xz = g101.dotXz(relativeX - 1.0F, relativeZ - 1.0F);
                  g101y = (float)g101.y();
                  GradientNoise.Gradient g011 = this.permuteToGrad(xy01 + floorZ + 1);
                  d011xz = g011.dotXz(relativeX, relativeZ - 1.0F);
                  g011y = (float)g011.y();
                  GradientNoise.Gradient g111 = this.permuteToGrad(xy11 + floorZ + 1);
                  d111xz = g111.dotXz(relativeX - 1.0F, relativeZ - 1.0F);
                  g111y = (float)g111.y();
                  lastFloorY = floorY;
               }

               buffer.addTo(
                  index,
                  amplitude
                     * Mth.lerp3(
                        alphaX,
                        alphaY,
                        alphaZ,
                        d000xz + g000y * relativeY,
                        d100xz + g100y * relativeY,
                        d010xz + g010y * (relativeY - 1.0F),
                        d110xz + g110y * (relativeY - 1.0F),
                        d001xz + g001y * relativeY,
                        d101xz + g101y * relativeY,
                        d011xz + g011y * (relativeY - 1.0F),
                        d111xz + g111y * (relativeY - 1.0F)
                     )
               );
               index++;
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
      float d100 = g100.dot(xr - 1.0F, yr, zr);
      float d010 = g010.dot(xr, yr - 1.0F, zr);
      float d110 = g110.dot(xr - 1.0F, yr - 1.0F, zr);
      float d001 = g001.dot(xr, yr, zr - 1.0F);
      float d101 = g101.dot(xr - 1.0F, yr, zr - 1.0F);
      float d011 = g011.dot(xr, yr - 1.0F, zr - 1.0F);
      float d111 = g111.dot(xr - 1.0F, yr - 1.0F, zr - 1.0F);
      float xAlpha = Mth.smoothstep(xr);
      float yAlpha = Mth.smoothstep(yr);
      float zAlpha = Mth.smoothstep(zr);
      float d1x = Mth.lerp3(
         xAlpha,
         yAlpha,
         zAlpha,
         (float)g000.x(),
         (float)g100.x(),
         (float)g010.x(),
         (float)g110.x(),
         (float)g001.x(),
         (float)g101.x(),
         (float)g011.x(),
         (float)g111.x()
      );
      float d1y = Mth.lerp3(
         xAlpha,
         yAlpha,
         zAlpha,
         (float)g000.y(),
         (float)g100.y(),
         (float)g010.y(),
         (float)g110.y(),
         (float)g001.y(),
         (float)g101.y(),
         (float)g011.y(),
         (float)g111.y()
      );
      float d1z = Mth.lerp3(
         xAlpha,
         yAlpha,
         zAlpha,
         (float)g000.z(),
         (float)g100.z(),
         (float)g010.z(),
         (float)g110.z(),
         (float)g001.z(),
         (float)g101.z(),
         (float)g011.z(),
         (float)g111.z()
      );
      float d2x = Mth.lerp2(yAlpha, zAlpha, d100 - d000, d110 - d010, d101 - d001, d111 - d011);
      float d2y = Mth.lerp2(zAlpha, xAlpha, d010 - d000, d011 - d001, d110 - d100, d111 - d101);
      float d2z = Mth.lerp2(xAlpha, yAlpha, d001 - d000, d101 - d100, d011 - d010, d111 - d110);
      float xSD = Mth.smoothstepDerivative(xr);
      float ySD = Mth.smoothstepDerivative(yr);
      float zSD = Mth.smoothstepDerivative(zr);
      float dX = d1x + xSD * d2x;
      float dY = d1y + ySD * d2y;
      float dZ = d1z + zSD * d2z;
      derivativeOut[0] += dX;
      derivativeOut[1] += dY;
      derivativeOut[2] += dZ;
      return Mth.lerp3(xAlpha, yAlpha, zAlpha, d000, d100, d010, d110, d001, d101, d011, d111);
   }

   @VisibleForTesting
   public void parityConfigString(StringBuilder sb) {
      NoiseUtils.parityNoiseOctaveConfigString(sb, this.offsetX, this.offsetY, this.offsetZ, this.perms);
   }
}
