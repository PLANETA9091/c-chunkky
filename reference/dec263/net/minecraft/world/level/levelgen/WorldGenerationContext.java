package net.minecraft.world.level.levelgen;

import net.minecraft.world.level.Level;
import net.minecraft.world.level.LevelAccessor;
import net.minecraft.world.level.LevelHeightAccessor;
import net.minecraft.world.level.WorldGenLevel;
import net.minecraft.world.level.chunk.ChunkGenerator;
import org.jspecify.annotations.Nullable;

public class WorldGenerationContext {
   private final int minY;
   private final int height;
   private final int seaLevel;
   @Nullable
   private final Level level;

   public static WorldGenerationContext of(LevelAccessor level) {
      return level instanceof WorldGenLevel worldGenLevel
         ? new WorldGenerationContext(worldGenLevel.getLevel().getChunkSource().getGenerator(), level)
         : new WorldGenerationContext(level);
   }

   public WorldGenerationContext(ChunkGenerator generator, LevelHeightAccessor heightAccessor) {
      this(generator, heightAccessor, null);
   }

   public WorldGenerationContext(ChunkGenerator generator, LevelHeightAccessor heightAccessor, @Nullable Level level) {
      this.level = level;
      this.minY = Math.max(heightAccessor.getMinY(), generator.getMinY());
      this.height = Math.min(heightAccessor.getHeight(), generator.getGenDepth());
      this.seaLevel = generator.getSeaLevel();
   }

   private WorldGenerationContext(LevelAccessor level) {
      this.minY = level.getMinY();
      this.height = level.getHeight();
      this.seaLevel = level.getSeaLevel();
      this.level = null;
   }

   public int getMinGenY() {
      return this.minY;
   }

   public int getGenDepth() {
      return this.height;
   }

   public int seaLevel() {
      return this.seaLevel;
   }

   public Level level() {
      if (this.level == null) {
         throw new NullPointerException("WorldGenerationContext was initialized without a Level, but WorldGenerationContext#level was called");
      } else {
         return this.level;
      }
   }
}
