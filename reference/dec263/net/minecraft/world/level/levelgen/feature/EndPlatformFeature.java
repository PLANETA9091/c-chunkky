package net.minecraft.world.level.levelgen.feature;

import com.mojang.serialization.MapCodec;
import net.minecraft.core.BlockPos;
import net.minecraft.core.BlockPos.MutableBlockPos;
import net.minecraft.util.RandomSource;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.level.ServerLevelAccessor;
import net.minecraft.world.level.WorldGenLevel;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.chunk.ChunkGenerator;
import org.bukkit.World;
import org.bukkit.craftbukkit.util.BlockStateListPopulator;
import org.bukkit.event.world.PortalCreateEvent;
import org.bukkit.event.world.PortalCreateEvent.CreateReason;
import org.jspecify.annotations.Nullable;

public record EndPlatformFeature() implements Feature {
   public static final MapCodec<EndPlatformFeature> CODEC = MapCodec.unit(EndPlatformFeature::new);

   @Override
   public MapCodec<EndPlatformFeature> codec() {
      return CODEC;
   }

   @Override
   public boolean place(WorldGenLevel level, ChunkGenerator chunkGenerator, RandomSource random, BlockPos origin) {
      createEndPlatform(level, origin, false);
      return true;
   }

   public static void createEndPlatform(ServerLevelAccessor newLevel, BlockPos origin, boolean dropResources) {
      createEndPlatform(newLevel, origin, dropResources, null);
   }

   public static void createEndPlatform(ServerLevelAccessor newLevel, BlockPos origin, boolean dropResources, @Nullable Entity entity) {
      BlockStateListPopulator blockList = new BlockStateListPopulator(newLevel);
      MutableBlockPos pos = origin.mutable();

      for (int dz = -2; dz <= 2; dz++) {
         for (int dx = -2; dx <= 2; dx++) {
            for (int dy = -1; dy < 3; dy++) {
               BlockPos blockPos = pos.set(origin).move(dx, dy, dz);
               Block block = dy == -1 ? Blocks.OBSIDIAN : Blocks.AIR;
               if (!blockList.getBlockState(blockPos).is(block)) {
                  if (dropResources) {
                     blockList.destroyBlock(blockPos, true, null);
                  }

                  blockList.setBlock(blockPos, block.defaultBlockState(), 3);
               }
            }
         }
      }

      if (entity != null) {
         World bworld = newLevel.getLevel().getWorld();
         PortalCreateEvent portalEvent = new PortalCreateEvent(blockList.getSnapshotBlocks(), bworld, entity.getBukkitEntity(), CreateReason.END_PLATFORM);
         newLevel.getLevel().getCraftServer().getPluginManager().callEvent(portalEvent);
         if (portalEvent.isCancelled()) {
            return;
         }
      }

      if (dropResources) {
         blockList.placeBlocks(state -> newLevel.destroyBlock(state.getPosition(), true, null));
      } else {
         blockList.placeBlocks();
      }
   }
}
