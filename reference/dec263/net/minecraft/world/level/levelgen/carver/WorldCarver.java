package net.minecraft.world.level.levelgen.carver;

import com.mojang.serialization.Codec;
import com.mojang.serialization.MapCodec;
import java.util.function.Function;
import net.minecraft.core.Holder;
import net.minecraft.core.HolderSet;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.core.registries.Registries;
import net.minecraft.core.registries.codec.RegistryCodecs;
import net.minecraft.util.Mth;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.chunk.CarverOutput;
import net.minecraft.world.level.levelgen.WorldGenerationContext;

public interface WorldCarver {
   Codec<WorldCarver> DIRECT_CODEC = BuiltInRegistries.CARVER_TYPE.byNameCodec().dispatch(WorldCarver::codec, Function.identity());
   Codec<Holder<WorldCarver>> CODEC = RegistryCodecs.holder(Registries.CARVER, DIRECT_CODEC);
   Codec<HolderSet<WorldCarver>> LIST_CODEC = RegistryCodecs.holderSet(Registries.CARVER, DIRECT_CODEC);

   default int getRange() {
      return 4;
   }

   static void carveEllipsoid(
      ChunkPos chunkPos,
      double x,
      double y,
      double z,
      double horizontalRadius,
      double verticalRadius,
      CarverOutput output,
      WorldCarver.CarveSkipChecker skipChecker
   ) {
      double centerX = (double)chunkPos.getMiddleBlockX();
      double centerZ = (double)chunkPos.getMiddleBlockZ();
      double maxDelta = 16.0 + horizontalRadius * 2.0;
      if (!(Math.abs(x - centerX) > maxDelta) && !(Math.abs(z - centerZ) > maxDelta)) {
         int chunkMinX = chunkPos.getMinBlockX();
         int chunkMinZ = chunkPos.getMinBlockZ();
         int minXIndex = Math.max(Mth.floor(x - horizontalRadius) - chunkMinX - 1, 0);
         int maxXIndex = Math.min(Mth.floor(x + horizontalRadius) - chunkMinX, 15);
         int minY = Math.max(Mth.floor(y - verticalRadius) - 1, output.minY());
         int maxY = Math.min(Mth.floor(y + verticalRadius) + 1, output.maxY());
         int minZIndex = Math.max(Mth.floor(z - horizontalRadius) - chunkMinZ - 1, 0);
         int maxZIndex = Math.min(Mth.floor(z + horizontalRadius) - chunkMinZ, 15);

         for (int xIndex = minXIndex; xIndex <= maxXIndex; xIndex++) {
            int worldX = chunkPos.getBlockX(xIndex);
            double xd = ((double)worldX + 0.5 - x) / horizontalRadius;

            for (int zIndex = minZIndex; zIndex <= maxZIndex; zIndex++) {
               int worldZ = chunkPos.getBlockZ(zIndex);
               double zd = ((double)worldZ + 0.5 - z) / horizontalRadius;
               if (!(xd * xd + zd * zd >= 1.0)) {
                  for (int worldY = maxY; worldY > minY; worldY--) {
                     double yd = ((double)worldY - 0.5 - y) / verticalRadius;
                     if (!skipChecker.shouldSkip(xd, yd, zd, worldY)) {
                        output.carve(xIndex, worldY, zIndex);
                     }
                  }
               }
            }
         }
      }
   }

   boolean carve(WorldGenerationContext var1, RandomSource var2, ChunkPos var3, ChunkPos var4, CarverOutput var5);

   boolean isStartChunk(RandomSource var1);

   MapCodec<? extends WorldCarver> codec();

   static boolean canReach(ChunkPos chunkPos, double x, double z, int currentStep, int totalSteps, float thickness) {
      double xMid = (double)chunkPos.getMiddleBlockX();
      double zMid = (double)chunkPos.getMiddleBlockZ();
      double xd = x - xMid;
      double zd = z - zMid;
      double remaining = (double)(totalSteps - currentStep);
      double rr = (double)(thickness + 2.0F + 16.0F);
      return xd * xd + zd * zd - remaining * remaining <= rr * rr;
   }

   public interface CarveSkipChecker {
      boolean shouldSkip(double var1, double var3, double var5, int var7);
   }
}
