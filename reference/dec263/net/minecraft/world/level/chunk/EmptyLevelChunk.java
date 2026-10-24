package net.minecraft.world.level.chunk;

import ca.spottedleaf.moonrise.patches.starlight.chunk.StarlightChunk;
import ca.spottedleaf.moonrise.patches.starlight.light.SWMRNibbleArray;
import ca.spottedleaf.moonrise.patches.starlight.light.StarLightEngine;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Holder;
import net.minecraft.server.level.FullChunkStatus;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.biome.Biome;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.Block.UpdateFlags;
import net.minecraft.world.level.block.entity.BlockEntity;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.material.FluidState;
import net.minecraft.world.level.material.Fluids;
import org.jspecify.annotations.Nullable;

public class EmptyLevelChunk extends LevelChunk implements StarlightChunk {
   private final Holder<Biome> biome;

   public EmptyLevelChunk(Level level, ChunkPos pos, Holder<Biome> biome) {
      super(level, pos);
      this.biome = biome;
   }

   @Override
   public SWMRNibbleArray[] starlight$getBlockNibbles() {
      return StarLightEngine.getFilledEmptyLight(this.getLevel());
   }

   @Override
   public void starlight$setBlockNibbles(SWMRNibbleArray[] nibbles) {
   }

   @Override
   public SWMRNibbleArray[] starlight$getSkyNibbles() {
      return StarLightEngine.getFilledEmptyLight(this.getLevel());
   }

   @Override
   public void starlight$setSkyNibbles(SWMRNibbleArray[] nibbles) {
   }

   @Override
   public boolean[] starlight$getSkyEmptinessMap() {
      return null;
   }

   @Override
   public void starlight$setSkyEmptinessMap(boolean[] emptinessMap) {
   }

   @Override
   public boolean[] starlight$getBlockEmptinessMap() {
      return null;
   }

   @Override
   public void starlight$setBlockEmptinessMap(boolean[] emptinessMap) {
   }

   @Override
   public BlockState getBlockState(BlockPos pos) {
      return Blocks.VOID_AIR.defaultBlockState();
   }

   @Override
   public BlockState getBlockState(int x, int y, int z) {
      return Blocks.VOID_AIR.defaultBlockState();
   }

   @Nullable
   @Override
   public BlockState setBlockState(BlockPos pos, BlockState state, @UpdateFlags int flags) {
      return null;
   }

   @Override
   public FluidState getFluidState(BlockPos pos) {
      return Fluids.EMPTY.defaultFluidState();
   }

   @Override
   public int getLightEmission(BlockPos pos) {
      return 0;
   }

   @Nullable
   @Override
   public BlockEntity getBlockEntity(BlockPos pos, LevelChunk.EntityCreationType creationType) {
      return null;
   }

   @Override
   public void addAndRegisterBlockEntity(BlockEntity blockEntity) {
   }

   @Override
   public void setBlockEntity(BlockEntity blockEntity) {
   }

   @Override
   public void removeBlockEntity(BlockPos pos) {
   }

   @Override
   public boolean isEmpty() {
      return true;
   }

   @Override
   public boolean isYSpaceEmpty(int yStartInclusive, int yEndInclusive) {
      return true;
   }

   @Override
   public FullChunkStatus getFullStatus() {
      return FullChunkStatus.FULL;
   }

   @Override
   public Holder<Biome> getNoiseBiome(int quartX, int quartY, int quartZ) {
      return this.biome;
   }
}
