package net.minecraft.world.level.levelgen.feature;

import com.mojang.serialization.MapCodec;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.core.Holder;
import net.minecraft.core.BlockPos.MutableBlockPos;
import net.minecraft.tags.BlockTags;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.LevelAccessor;
import net.minecraft.world.level.WorldGenLevel;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.ChunkGenerator;
import net.minecraft.world.level.levelgen.blockpredicates.BlockPredicate;
import net.minecraft.world.level.levelgen.feature.stateproviders.BlockStateProvider;

public interface AbstractHugeMushroomFeature extends Feature {
   int MIN_MUSHROOM_HEIGHT = 4;

   Holder<BlockStateProvider> capProvider();

   Holder<BlockStateProvider> stemProvider();

   int foliageRadius();

   BlockPredicate canPlaceOn();

   @Override
   MapCodec<? extends AbstractHugeMushroomFeature> codec();

   default void placeTrunk(WorldGenLevel level, RandomSource random, BlockPos origin, int treeHeight, MutableBlockPos blockPos) {
      for (int dy = 0; dy < treeHeight; dy++) {
         blockPos.set(origin).move(Direction.UP, dy);
         this.placeMushroomBlock(level, blockPos, this.stemProvider().value().getState(level, random, origin));
      }
   }

   default void placeMushroomBlock(LevelAccessor level, MutableBlockPos blockPos, BlockState newState) {
      BlockState currentState = level.getBlockState(blockPos);
      if (currentState.isAir() || currentState.is(BlockTags.REPLACEABLE_BY_MUSHROOMS)) {
         this.setBlock(level, blockPos, newState);
      }
   }

   default int getTreeHeight(RandomSource random) {
      int treeHeight = random.nextInt(3) + 4;
      if (random.nextInt(12) == 0) {
         treeHeight *= 2;
      }

      return treeHeight;
   }

   default boolean isValidPosition(WorldGenLevel level, BlockPos origin, int treeHeight, MutableBlockPos blockPos) {
      int y = origin.getY();
      if (y >= level.getMinY() + 1 && y + treeHeight + 1 <= level.getMaxY()) {
         if (!this.canPlaceOn().test(level, origin.below())) {
            return false;
         } else {
            for (int dy = 0; dy <= treeHeight; dy++) {
               int radius = this.getTreeRadiusForHeight(-1, -1, this.foliageRadius(), dy);

               for (int dx = -radius; dx <= radius; dx++) {
                  for (int dz = -radius; dz <= radius; dz++) {
                     BlockState state = level.getBlockState(blockPos.setWithOffset(origin, dx, dy, dz));
                     if (!state.isAir() && !state.is(BlockTags.LEAVES)) {
                        return false;
                     }
                  }
               }
            }

            return true;
         }
      } else {
         return false;
      }
   }

   @Override
   default boolean place(WorldGenLevel level, ChunkGenerator chunkGenerator, RandomSource random, BlockPos origin) {
      int treeHeight = this.getTreeHeight(random);
      MutableBlockPos blockPos = new MutableBlockPos();
      if (!this.isValidPosition(level, origin, treeHeight, blockPos)) {
         return false;
      } else {
         this.makeCap(level, random, origin, treeHeight, blockPos);
         this.placeTrunk(level, random, origin, treeHeight, blockPos);
         return true;
      }
   }

   int getTreeRadiusForHeight(int var1, int var2, int var3, int var4);

   void makeCap(WorldGenLevel var1, RandomSource var2, BlockPos var3, int var4, MutableBlockPos var5);
}
