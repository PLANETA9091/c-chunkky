package net.minecraft.world.level.chunk;

import com.mojang.serialization.Codec;
import net.minecraft.core.Holder;
import net.minecraft.core.Registry;
import net.minecraft.core.RegistryAccess;
import net.minecraft.core.Holder.Reference;
import net.minecraft.core.registries.Registries;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.biome.Biome;
import net.minecraft.world.level.biome.Biomes;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import org.jspecify.annotations.Nullable;

public record PalettedContainerFactory(
   Strategy<BlockState> blockStatesStrategy,
   BlockState defaultBlockState,
   Codec<PalettedContainer<BlockState>> blockStatesContainerCodec,
   Strategy<Holder<Biome>> biomeStrategy,
   Holder<Biome> defaultBiome,
   Codec<PalettedContainerRO<Holder<Biome>>> biomeContainerCodec,
   Codec<PalettedContainer<Holder<Biome>>> biomeContainerRWCodec
) {
   public static PalettedContainerFactory create(RegistryAccess registries) {
      Strategy<BlockState> blockStateStrategy = Strategy.createForBlockStates(Block.BLOCK_STATE_REGISTRY);
      BlockState defaultBlockState = Blocks.AIR.defaultBlockState();
      Registry<Biome> biomes = registries.lookupOrThrow(Registries.BIOME);
      Strategy<Holder<Biome>> biomeStrategy = Strategy.createForBiomes(biomes.asHolderIdMap());
      Reference<Biome> defaultBiome = biomes.getOrThrow(Biomes.PLAINS);
      return new PalettedContainerFactory(
         blockStateStrategy,
         defaultBlockState,
         PalettedContainer.codecRW(BlockState.CODEC, blockStateStrategy, defaultBlockState, null),
         biomeStrategy,
         defaultBiome,
         PalettedContainer.codecRO(biomes.holderByNameCodec(), biomeStrategy, defaultBiome),
         PalettedContainer.codecRW(biomes.holderByNameCodec(), biomeStrategy, defaultBiome, null)
      );
   }

   public PalettedContainer<BlockState> createForBlockStates() {
      return new PalettedContainer<>(this.defaultBlockState, this.blockStatesStrategy, null);
   }

   public PalettedContainer<BlockState> createForBlockStates(@Nullable Level level, ChunkPos chunkPos, int chunkSectionY) {
      BlockState[] states = null;
      if (level != null && level.chunkPacketBlockController != null) {
         states = level.chunkPacketBlockController.getPresetBlockStates(level, chunkPos, chunkSectionY);
      }

      return new PalettedContainer<>(this.defaultBlockState, this.blockStatesStrategy, states);
   }

   public PalettedContainer<Holder<Biome>> createForBiomes() {
      return new PalettedContainer<>(this.defaultBiome, this.biomeStrategy, null);
   }
}
