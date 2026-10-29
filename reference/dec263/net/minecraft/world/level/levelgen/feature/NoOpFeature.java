package net.minecraft.world.level.levelgen.feature;

import com.mojang.serialization.MapCodec;
import net.minecraft.core.BlockPos;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.WorldGenLevel;
import net.minecraft.world.level.chunk.ChunkGenerator;

public record NoOpFeature() implements Feature {
   public static final MapCodec<NoOpFeature> CODEC = MapCodec.unit(NoOpFeature::new);

   @Override
   public MapCodec<NoOpFeature> codec() {
      return CODEC;
   }

   @Override
   public boolean place(WorldGenLevel level, ChunkGenerator chunkGenerator, RandomSource random, BlockPos origin) {
      return true;
   }
}
