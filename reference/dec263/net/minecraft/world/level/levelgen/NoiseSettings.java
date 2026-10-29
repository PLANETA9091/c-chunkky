package net.minecraft.world.level.levelgen;

import com.mojang.serialization.Codec;
import com.mojang.serialization.DataResult;
import com.mojang.serialization.codecs.RecordCodecBuilder;
import java.util.function.Function;
import net.minecraft.world.level.LevelHeightAccessor;

public record NoiseSettings(int minY, int height) {
   public static final Codec<NoiseSettings> CODEC = RecordCodecBuilder.create(
         i -> i.group(
                  Codec.intRange(-2032, 2031).fieldOf("min_y").forGetter(NoiseSettings::minY),
                  Codec.intRange(0, 4064).fieldOf("height").forGetter(NoiseSettings::height)
               )
               .apply(i, NoiseSettings::new)
      )
      .comapFlatMap(NoiseSettings::guardY, Function.identity());
   static final NoiseSettings OVERWORLD_NOISE_SETTINGS = create(-64, 384);
   static final NoiseSettings NETHER_NOISE_SETTINGS = create(0, 128);
   static final NoiseSettings END_NOISE_SETTINGS = create(0, 128);
   static final NoiseSettings CAVES_NOISE_SETTINGS = create(-64, 192);
   static final NoiseSettings FLOATING_ISLANDS_NOISE_SETTINGS = create(0, 256);

   private static DataResult<NoiseSettings> guardY(NoiseSettings dimensionType) {
      if (dimensionType.minY() + dimensionType.height() > 2032) {
         return DataResult.error(() -> "min_y + height cannot be higher than: 2032");
      } else if (dimensionType.height() % 16 != 0) {
         return DataResult.error(() -> "height has to be a multiple of 16");
      } else {
         return dimensionType.minY() % 16 != 0 ? DataResult.error(() -> "min_y has to be a multiple of 16") : DataResult.success(dimensionType);
      }
   }

   public static NoiseSettings create(int minY, int height) {
      NoiseSettings noiseSettings = new NoiseSettings(minY, height);
      guardY(noiseSettings).error().ifPresent(error -> {
         throw new IllegalStateException(error.message());
      });
      return noiseSettings;
   }

   public NoiseSettings clampToHeightAccessor(LevelHeightAccessor heightAccessor) {
      int newMinY = Math.max(this.minY, heightAccessor.getMinY());
      int newHeight = Math.min(this.minY + this.height, heightAccessor.getMaxY() + 1) - newMinY;
      return new NoiseSettings(newMinY, newHeight);
   }
}
