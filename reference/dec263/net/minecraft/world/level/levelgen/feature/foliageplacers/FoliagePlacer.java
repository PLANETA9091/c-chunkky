package net.minecraft.world.level.levelgen.feature.foliageplacers;

import com.mojang.datafixers.Products.P2;
import com.mojang.serialization.Codec;
import com.mojang.serialization.codecs.RecordCodecBuilder.Instance;
import com.mojang.serialization.codecs.RecordCodecBuilder.Mu;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.core.BlockPos.MutableBlockPos;
import net.minecraft.core.Direction.AxisDirection;
import net.minecraft.core.Direction.Plane;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.util.RandomSource;
import net.minecraft.util.valueproviders.IntProvider;
import net.minecraft.util.valueproviders.IntProviders;
import net.minecraft.world.level.WorldGenLevel;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.BlockStateProperties;
import net.minecraft.world.level.levelgen.feature.TreeFeature;
import net.minecraft.world.level.material.Fluids;

public abstract class FoliagePlacer {
   public static final Codec<FoliagePlacer> CODEC = BuiltInRegistries.FOLIAGE_PLACER_TYPE.byNameCodec().dispatch(FoliagePlacer::type, FoliagePlacerType::codec);
   protected final IntProvider radius;
   protected final IntProvider offset;

   protected static <P extends FoliagePlacer> P2<Mu<P>, IntProvider, IntProvider> foliagePlacerParts(Instance<P> instance) {
      return instance.group(
         IntProviders.codec(0, 16).fieldOf("radius").forGetter(p -> p.radius), IntProviders.codec(0, 16).fieldOf("offset").forGetter(p -> p.offset)
      );
   }

   public FoliagePlacer(IntProvider radius, IntProvider offset) {
      this.radius = radius;
      this.offset = offset;
   }

   protected abstract FoliagePlacerType<?> type();

   public void createFoliage(
      WorldGenLevel level,
      FoliagePlacer.FoliageSetter foliageSetter,
      RandomSource random,
      TreeFeature tree,
      int treeHeight,
      FoliagePlacer.FoliageAttachment foliageAttachment,
      int foliageHeight,
      int leafRadius
   ) {
      this.createFoliage(level, foliageSetter, random, tree, treeHeight, foliageAttachment, foliageHeight, leafRadius, this.offset(random));
   }

   protected abstract void createFoliage(
      WorldGenLevel var1,
      FoliagePlacer.FoliageSetter var2,
      RandomSource var3,
      TreeFeature var4,
      int var5,
      FoliagePlacer.FoliageAttachment var6,
      int var7,
      int var8,
      int var9
   );

   public abstract int foliageHeight(RandomSource var1, int var2, TreeFeature var3);

   public int foliageRadius(RandomSource random, int trunkHeight) {
      return this.radius.sample(random);
   }

   private int offset(RandomSource random) {
      return this.offset.sample(random);
   }

   protected abstract boolean shouldSkipLocation(RandomSource var1, int var2, int var3, int var4, int var5, boolean var6);

   protected boolean shouldSkipLocationSigned(RandomSource random, int dx, int y, int dz, int currentRadius, boolean doubleTrunk) {
      int minDx;
      int minDz;
      if (doubleTrunk) {
         minDx = Math.min(Math.abs(dx), Math.abs(dx - 1));
         minDz = Math.min(Math.abs(dz), Math.abs(dz - 1));
      } else {
         minDx = Math.abs(dx);
         minDz = Math.abs(dz);
      }

      return this.shouldSkipLocation(random, minDx, y, minDz, currentRadius, doubleTrunk);
   }

   protected void placeLeavesRow(
      WorldGenLevel level,
      FoliagePlacer.FoliageSetter foliageSetter,
      RandomSource random,
      TreeFeature tree,
      BlockPos origin,
      int currentRadius,
      int y,
      boolean doubleTrunk
   ) {
      int offset = doubleTrunk ? 1 : 0;
      MutableBlockPos pos = new MutableBlockPos();

      for (int dx = -currentRadius; dx <= currentRadius + offset; dx++) {
         for (int dz = -currentRadius; dz <= currentRadius + offset; dz++) {
            if (!this.shouldSkipLocationSigned(random, dx, y, dz, currentRadius, doubleTrunk)) {
               pos.setWithOffset(origin, dx, y, dz);
               tryPlaceLeaf(level, foliageSetter, random, tree, pos);
            }
         }
      }
   }

   protected final void placeLeavesRowWithHangingLeavesBelow(
      WorldGenLevel level,
      FoliagePlacer.FoliageSetter foliageSetter,
      RandomSource random,
      TreeFeature tree,
      BlockPos origin,
      int currentRadius,
      int y,
      boolean doubleTrunk,
      float hangingLeavesChance,
      float hangingLeavesExtensionChance
   ) {
      this.placeLeavesRow(level, foliageSetter, random, tree, origin, currentRadius, y, doubleTrunk);
      int offset = doubleTrunk ? 1 : 0;
      BlockPos logPos = origin.below();
      MutableBlockPos pos = new MutableBlockPos();

      for (Direction alongEdge : Plane.HORIZONTAL) {
         Direction toEdge = alongEdge.getClockWise();
         int offsetToEdge = toEdge.getAxisDirection() == AxisDirection.POSITIVE ? currentRadius + offset : currentRadius;
         pos.setWithOffset(origin, 0, y - 1, 0).move(toEdge, offsetToEdge).move(alongEdge, -currentRadius);
         int offsetAlongEdge = -currentRadius;

         while (offsetAlongEdge < currentRadius + offset) {
            boolean leavesAbove = foliageSetter.isSet(pos.move(Direction.UP));
            pos.move(Direction.DOWN);
            if (leavesAbove && tryPlaceExtension(level, foliageSetter, random, tree, hangingLeavesChance, logPos, pos)) {
               pos.move(Direction.DOWN);
               tryPlaceExtension(level, foliageSetter, random, tree, hangingLeavesExtensionChance, logPos, pos);
               pos.move(Direction.UP);
            }

            offsetAlongEdge++;
            pos.move(alongEdge);
         }
      }
   }

   private static boolean tryPlaceExtension(
      WorldGenLevel level, FoliagePlacer.FoliageSetter foliageSetter, RandomSource random, TreeFeature tree, float chance, BlockPos logPos, MutableBlockPos pos
   ) {
      return pos.distManhattan(logPos) < 7 && !(random.nextFloat() > chance) && tryPlaceLeaf(level, foliageSetter, random, tree, pos);
   }

   protected static boolean tryPlaceLeaf(WorldGenLevel level, FoliagePlacer.FoliageSetter foliageSetter, RandomSource random, TreeFeature tree, BlockPos pos) {
      boolean isPersistent = level.isStateAtPosition(pos, state -> state.getValueOrElse(BlockStateProperties.PERSISTENT, Boolean.valueOf(false)));
      if (!isPersistent && TreeFeature.validTreePos(level, pos)) {
         BlockState foliageState = tree.foliageProvider().value().getState(level, random, pos);
         if (foliageState.hasProperty(BlockStateProperties.WATERLOGGED)) {
            foliageState = foliageState.setValue(
               BlockStateProperties.WATERLOGGED, Boolean.valueOf(level.isFluidAtPosition(pos, fluidState -> fluidState.isSourceOfType(Fluids.WATER)))
            );
         }

         foliageSetter.set(pos, foliageState);
         return true;
      } else {
         return false;
      }
   }

   public static record FoliageAttachment(BlockPos pos, int radiusOffsetXZ, int foliageHeightOffset, int sizeX, int sizeZ) {
      public FoliageAttachment(BlockPos pos, int radiusOffsetXZ, boolean doubleTrunk) {
         int sizeXZ = doubleTrunk ? 2 : 1;
         this(pos, radiusOffsetXZ, sizeXZ, sizeXZ);
      }

      public FoliageAttachment(BlockPos pos, int radiusOffsetXZ, int sizeX, int sizeZ) {
         this(pos, radiusOffsetXZ, 0, sizeX, sizeZ);
      }

      public boolean doubleTrunk() {
         return this.sizeX == 2 && this.sizeZ == 2;
      }
   }

   public interface FoliageSetter {
      void set(BlockPos var1, BlockState var2);

      boolean isSet(BlockPos var1);
   }
}
