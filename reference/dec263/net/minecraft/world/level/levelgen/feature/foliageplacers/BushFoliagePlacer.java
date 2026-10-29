package net.minecraft.world.level.levelgen.feature.foliageplacers;

import com.mojang.serialization.MapCodec;
import com.mojang.serialization.codecs.RecordCodecBuilder;
import net.minecraft.util.RandomSource;
import net.minecraft.util.valueproviders.IntProvider;
import net.minecraft.world.level.WorldGenLevel;
import net.minecraft.world.level.levelgen.feature.TreeFeature;

public class BushFoliagePlacer extends BlobFoliagePlacer {
   public static final MapCodec<BushFoliagePlacer> CODEC = RecordCodecBuilder.mapCodec(i -> blobParts(i).apply(i, BushFoliagePlacer::new));

   public BushFoliagePlacer(IntProvider radius, IntProvider offset, int height) {
      super(radius, offset, height);
   }

   @Override
   protected FoliagePlacerType<?> type() {
      return FoliagePlacerType.BUSH_FOLIAGE_PLACER;
   }

   @Override
   protected void createFoliage(
      WorldGenLevel level,
      FoliagePlacer.FoliageSetter foliageSetter,
      RandomSource random,
      TreeFeature tree,
      int treeHeight,
      FoliagePlacer.FoliageAttachment foliageAttachment,
      int foliageHeight,
      int leafRadius,
      int offset
   ) {
      int foliageHeightWithOffset = foliageHeight + foliageAttachment.foliageHeightOffset();

      for (int yo = offset; yo >= offset - foliageHeightWithOffset; yo--) {
         int currentRadius = leafRadius + foliageAttachment.radiusOffsetXZ() - 1 - yo;
         this.placeLeavesRow(level, foliageSetter, random, tree, foliageAttachment.pos(), currentRadius, yo, foliageAttachment.doubleTrunk());
      }
   }

   @Override
   protected boolean shouldSkipLocation(RandomSource random, int dx, int y, int dz, int currentRadius, boolean doubleTrunk) {
      return dx == currentRadius && dz == currentRadius && random.nextInt(2) == 0;
   }
}
