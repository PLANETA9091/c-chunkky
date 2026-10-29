package net.minecraft.world.level.levelgen.structure.placement;

import com.mojang.serialization.Codec;
import com.mojang.serialization.MapCodec;
import io.papermc.paper.annotation.DoNotUse;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Vec3i;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.resources.ResourceKey;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.chunk.ChunkGeneratorStructureState;
import net.minecraft.world.level.levelgen.structure.StructureSet;
import org.jspecify.annotations.Nullable;

public interface StructurePlacement {
   Codec<StructurePlacement> CODEC = BuiltInRegistries.STRUCTURE_PLACEMENT.byNameCodec().dispatch(StructurePlacement::codec, c -> c);

   @Deprecated
   @DoNotUse
   default boolean isStructureChunk(ChunkGeneratorStructureState state, int sourceX, int sourceZ) {
      return this.isStructureChunk(state, sourceX, sourceZ, null);
   }

   boolean isStructureChunk(ChunkGeneratorStructureState var1, int var2, int var3, @Nullable ResourceKey<StructureSet> var4);

   default boolean applyAdditionalChunkRestrictions(int sourceX, int sourceZ, long levelSeed, @Nullable Integer saltOverride) {
      return true;
   }

   default BlockPos getLocatePos(ChunkPos chunkPos) {
      return new BlockPos(chunkPos.getMinBlockX(), 0, chunkPos.getMinBlockZ()).offset(this.locateOffset());
   }

   default Vec3i locateOffset() {
      return Vec3i.ZERO;
   }

   MapCodec<? extends StructurePlacement> codec();
}
