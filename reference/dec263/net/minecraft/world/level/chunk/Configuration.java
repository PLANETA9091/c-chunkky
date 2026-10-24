package net.minecraft.world.level.chunk;

import ca.spottedleaf.moonrise.patches.fast_palette.FastHashMapPalette;
import java.util.List;

public interface Configuration {
   boolean alwaysRepack();

   int bitsInMemory();

   int bitsInStorage();

   <T> Palette<T> createPalette(Strategy<T> var1, List<T> var2);

   public static record Global(int bitsInMemory, int bitsInStorage) implements Configuration {
      @Override
      public boolean alwaysRepack() {
         return true;
      }

      @Override
      public <T> Palette<T> createPalette(Strategy<T> strategy, List<T> paletteEntries) {
         return strategy.globalPalette();
      }
   }

   public static record Simple(Palette.Factory factory, int bits) implements Configuration {
      @Override
      public boolean alwaysRepack() {
         return false;
      }

      @Override
      public <T> Palette<T> createPalette(Strategy<T> strategy, List<T> paletteEntries) {
         return FastHashMapPalette.create(this.bits, paletteEntries);
      }

      @Override
      public int bitsInMemory() {
         return this.bits;
      }

      @Override
      public int bitsInStorage() {
         return this.bits;
      }
   }
}
