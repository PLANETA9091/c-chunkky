package net.minecraft.world.level.levelgen.feature.foliageplacers;

import com.mojang.serialization.MapCodec;
import com.mojang.serialization.codecs.RecordCodecBuilder;
import net.minecraft.util.RandomSource;
import net.minecraft.util.valueproviders.IntProvider;
import net.minecraft.util.valueproviders.IntProviders;
import net.minecraft.world.level.WorldGenLevel;
import net.minecraft.world.level.levelgen.feature.TreeFeature;

public class PineFoliagePlacer extends FoliagePlacer {
   public static final MapCodec<PineFoliagePlacer> CODEC = RecordCodecBuilder.mapCodec(
      i -> foliagePlacerParts(i).and(IntProviders.codec(0, 24).fieldOf("height").forGetter(p -> p.height)).apply(i, PineFoliagePlacer::new)
   );
   private final IntProvider height;

   public PineFoliagePlacer(IntProvider radius, IntProvider offset, IntProvider height) {
      super(radius, offset);
      this.height = height;
   }

   @Override
   protected FoliagePlacerType<?> type() {
      return FoliagePlacerType.PINE_FOLIAGE_PLACER;
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
      int currentRadius = 0;
      int foliageHeightWithOffset = foliageHeight + foliageAttachment.foliageHeightOffset();

      for (int yo = offset; yo >= offset - foliageHeightWithOffset; yo--) {
         this.placeLeavesRow(level, foliageSetter, random, tree, foliageAttachment.pos(), currentRadius, yo, foliageAttachment.doubleTrunk());
         if (currentRadius >= 1 && yo == offset - foliageHeightWithOffset + 1) {
            currentRadius--;
         } else if (currentRadius < leafRadius + foliageAttachment.radiusOffsetXZ()) {
            currentRadius++;
         }
      }
   }

   @Override
   public int foliageRadius(RandomSource random, int trunkHeight) {
      return super.foliageRadius(random, trunkHeight) + random.nextInt(Math.max(trunkHeight + 1, 1));
   }

   @Override
   public int foliageHeight(RandomSource random, int treeHeight, TreeFeature tree) {
      return this.height.sample(random);
   }

   @Override
   protected boolean shouldSkipLocation(RandomSource random, int dx, int y, int dz, int currentRadius, boolean doubleTrunk) {
      return dx == currentRadius && dz == currentRadius && currentRadius > 0;
   }
}
