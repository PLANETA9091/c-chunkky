package net.minecraft.world.level.levelgen.feature.stateproviders;

import com.mojang.serialization.Codec;
import com.mojang.serialization.MapCodec;
import com.mojang.serialization.codecs.RecordCodecBuilder;
import java.util.Collection;
import java.util.Optional;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Holder;
import net.minecraft.util.RandomSource;
import net.minecraft.util.valueproviders.IntProvider;
import net.minecraft.util.valueproviders.IntProviders;
import net.minecraft.world.level.LevelAccessor;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.IntegerProperty;
import net.minecraft.world.level.block.state.properties.Property;
import org.jspecify.annotations.Nullable;

public class RandomizedIntStateProvider implements BlockStateProvider {
   public static final MapCodec<RandomizedIntStateProvider> CODEC = RecordCodecBuilder.mapCodec(
      i -> i.group(
               BlockStateProvider.CODEC.fieldOf("source").forGetter(c -> c.source),
               Codec.STRING.fieldOf("property").forGetter(c -> c.propertyName),
               IntProviders.CODEC.fieldOf("values").forGetter(c -> c.values)
            )
            .apply(i, RandomizedIntStateProvider::new)
   );
   private final Holder<BlockStateProvider> source;
   private final String propertyName;
   @Nullable
   private IntegerProperty property;
   private final IntProvider values;

   public RandomizedIntStateProvider(BlockStateProvider source, IntegerProperty property, IntProvider values) {
      this.source = Holder.direct(source);
      this.property = property;
      this.propertyName = property.getName();
      this.values = values;
      Collection<Integer> possibleValues = property.getPossibleValues();

      for (int i = values.minInclusive(); i <= values.maxInclusive(); i++) {
         if (!possibleValues.contains(i)) {
            throw new IllegalArgumentException("Property value out of range: " + property.getName() + ": " + i);
         }
      }
   }

   public RandomizedIntStateProvider(Holder<BlockStateProvider> source, String propertyName, IntProvider values) {
      this.source = source;
      this.propertyName = propertyName;
      this.values = values;
   }

   @Override
   public MapCodec<RandomizedIntStateProvider> codec() {
      return CODEC;
   }

   @Override
   public BlockState getState(LevelAccessor level, RandomSource random, BlockPos pos) {
      BlockState unmodifiedState = this.source.value().getState(level, random, pos);
      if (this.property == null || !unmodifiedState.hasProperty(this.property)) {
         IntegerProperty property = findProperty(unmodifiedState, this.propertyName);
         if (property == null) {
            return unmodifiedState;
         }

         this.property = property;
      }

      return unmodifiedState.setValue(this.property, Integer.valueOf(this.values.sample(random)));
   }

   @Nullable
   private static IntegerProperty findProperty(BlockState source, String propertyName) {
      Collection<Property<?>> properties = source.getProperties();
      Optional<IntegerProperty> found = properties.stream()
         .filter(p -> p.getName().equals(propertyName))
         .filter(p -> p instanceof IntegerProperty)
         .map(p -> (IntegerProperty)p)
         .findAny();
      return found.orElse(null);
   }
}
