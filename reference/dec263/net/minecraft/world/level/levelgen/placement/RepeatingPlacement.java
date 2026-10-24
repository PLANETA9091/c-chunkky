package net.minecraft.world.level.levelgen.placement;

import com.mojang.serialization.MapCodec;
import java.util.function.Consumer;
import net.minecraft.core.BlockPos;
import net.minecraft.util.RandomSource;

public interface RepeatingPlacement extends PlacementModifier {
   int count(RandomSource var1, BlockPos var2);

   @Override
   default void modify(PlacementContext context, RandomSource random, BlockPos origin, Consumer<BlockPos> output) {
      int count = this.count(random, origin);

      for (int i = 0; i < count; i++) {
         output.accept(origin);
      }
   }

   @Override
   MapCodec<? extends RepeatingPlacement> codec();
}
