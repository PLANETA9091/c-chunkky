package net.minecraft.world.level.levelgen.feature.foliageplacers;

import com.mojang.serialization.Codec;
import com.mojang.serialization.MapCodec;
import com.mojang.serialization.codecs.RecordCodecBuilder;
import net.minecraft.core.BlockPos;
import net.minecraft.util.RandomSource;
import net.minecraft.util.valueproviders.IntProvider;
import net.minecraft.util.valueproviders.IntProviders;
import net.minecraft.world.level.WorldGenLevel;
import net.minecraft.world.level.levelgen.feature.TreeFeature;

public class CherryFoliagePlacer extends FoliagePlacer {
   public static final MapCodec<CherryFoliagePlacer> CODEC = RecordCodecBuilder.mapCodec(
      i -> foliagePlacerParts(i)
            .and(
               i.group(
                  IntProviders.codec(4, 16).fieldOf("height").forGetter(p -> p.height),
                  Codec.floatRange(0.0F, 1.0F).fieldOf("wide_bottom_layer_hole_chance").forGetter(p -> p.wideBottomLayerHoleChance),
                  Codec.floatRange(0.0F, 1.0F).fieldOf("corner_hole_chance").forGetter(p -> p.wideBottomLayerHoleChance),
                  Codec.floatRange(0.0F, 1.0F).fieldOf("hanging_leaves_chance").forGetter(p -> p.hangingLeavesChance),
                  Codec.floatRange(0.0F, 1.0F).fieldOf("hanging_leaves_extension_chance").forGetter(p -> p.hangingLeavesExtensionChance)
               )
            )
            .apply(i, CherryFoliagePlacer::new)
   );
   private final IntProvider height;
   private final float wideBottomLayerHoleChance;
   private final float cornerHoleChance;
   private final float hangingLeavesChance;
   private final float hangingLeavesExtensionChance;

   public CherryFoliagePlacer(
      IntProvider radius,
      IntProvider offset,
      IntProvider height,
      float wideBottomLayerHoleChance,
      float cornerHoleChance,
      float hangingLeavesChance,
      float hangingLeavesExtensionChance
   ) {
      super(radius, offset);
      this.height = height;
      this.wideBottomLayerHoleChance = wideBottomLayerHoleChance;
      this.cornerHoleChance = cornerHoleChance;
      this.hangingLeavesChance = hangingLeavesChance;
      this.hangingLeavesExtensionChance = hangingLeavesExtensionChance;
   }

   @Override
   protected FoliagePlacerType<?> type() {
      return FoliagePlacerType.CHERRY_FOLIAGE_PLACER;
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
      boolean doubleTrunk = foliageAttachment.doubleTrunk();
      BlockPos foliagePos = foliageAttachment.pos().above(offset);
      int currentRadius = leafRadius + foliageAttachment.radiusOffsetXZ() - 1;
      int foliageHeightWithOffset = foliageHeight + foliageAttachment.foliageHeightOffset();
      this.placeLeavesRow(level, foliageSetter, random, tree, foliagePos, currentRadius - 2, foliageHeightWithOffset - 3, doubleTrunk);
      this.placeLeavesRow(level, foliageSetter, random, tree, foliagePos, currentRadius - 1, foliageHeightWithOffset - 4, doubleTrunk);

      for (int y = foliageHeightWithOffset - 5; y >= 0; y--) {
         this.placeLeavesRow(level, foliageSetter, random, tree, foliagePos, currentRadius, y, doubleTrunk);
      }

      this.placeLeavesRowWithHangingLeavesBelow(
         level, foliageSetter, random, tree, foliagePos, currentRadius, -1, doubleTrunk, this.hangingLeavesChance, this.hangingLeavesExtensionChance
      );
      this.placeLeavesRowWithHangingLeavesBelow(
         level, foliageSetter, random, tree, foliagePos, currentRadius - 1, -2, doubleTrunk, this.hangingLeavesChance, this.hangingLeavesExtensionChance
      );
   }

   @Override
   public int foliageHeight(RandomSource random, int treeHeight, TreeFeature tree) {
      return this.height.sample(random);
   }

   @Override
   protected boolean shouldSkipLocation(RandomSource random, int dx, int y, int dz, int currentRadius, boolean doubleTrunk) {
      if (y == -1 && (dx == currentRadius || dz == currentRadius) && random.nextFloat() < this.wideBottomLayerHoleChance) {
         return true;
      } else {
         boolean corner = dx == currentRadius && dz == currentRadius;
         boolean wideLayer = currentRadius > 2;
         return wideLayer
            ? corner || dx + dz > currentRadius * 2 - 2 && random.nextFloat() < this.cornerHoleChance
            : corner && random.nextFloat() < this.cornerHoleChance;
      }
   }
}
