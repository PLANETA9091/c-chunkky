package net.minecraft.world.level.levelgen.placement;

import com.mojang.serialization.Codec;
import com.mojang.serialization.MapCodec;
import java.util.function.Consumer;
import java.util.function.Function;
import net.minecraft.core.BlockPos;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.util.RandomSource;

public interface PlacementModifier {
   Codec<PlacementModifier> CODEC = BuiltInRegistries.PLACEMENT_MODIFIER_TYPE.byNameCodec().dispatch(PlacementModifier::codec, Function.identity());

   void modify(PlacementContext var1, RandomSource var2, BlockPos var3, Consumer<BlockPos> var4);

   MapCodec<? extends PlacementModifier> codec();
}
