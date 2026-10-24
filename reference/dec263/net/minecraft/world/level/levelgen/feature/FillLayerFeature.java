package net.minecraft.world.level.levelgen.feature;

import com.mojang.serialization.Codec;
import com.mojang.serialization.MapCodec;
import com.mojang.serialization.codecs.RecordCodecBuilder;
import net.minecraft.core.BlockPos;
import net.minecraft.core.BlockPos.MutableBlockPos;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.WorldGenLevel;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.ChunkGenerator;

public record FillLayerFeature(int height, BlockState state) implements Feature {
   public static final MapCodec<FillLayerFeature> CODEC = RecordCodecBuilder.mapCodec(
      i -> i.group(
               Codec.intRange(0, 4064).fieldOf("height").forGetter(FillLayerFeature::height),
               BlockState.CODEC.fieldOf("state").forGetter(FillLayerFeature::state)
            )
            .apply(i, FillLayerFeature::new)
   );

   @Override
   public MapCodec<FillLayerFeature> codec() {
      return CODEC;
   }

   @Override
   public boolean place(WorldGenLevel level, ChunkGenerator chunkGenerator, RandomSource random, BlockPos origin) {
      MutableBlockPos pos = new MutableBlockPos();

      for (int dx = 0; dx < 16; dx++) {
         for (int dz = 0; dz < 16; dz++) {
            int x = origin.getX() + dx;
            int z = origin.getZ() + dz;
            int y = level.getMinY() + this.height;
            pos.set(x, y, z);
            if (level.getBlockState(pos).isAir()) {
               level.setBlock(pos, this.state, 2);
            }
         }
      }

      return true;
   }
}
