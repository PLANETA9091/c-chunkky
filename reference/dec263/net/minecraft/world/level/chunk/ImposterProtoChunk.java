package net.minecraft.world.level.chunk;

import ca.spottedleaf.moonrise.patches.starlight.chunk.StarlightChunk;
import ca.spottedleaf.moonrise.patches.starlight.light.SWMRNibbleArray;
import it.unimi.dsi.fastutil.longs.LongSet;
import java.util.Map;
import java.util.function.BiConsumer;
import java.util.function.Predicate;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Holder;
import net.minecraft.core.HolderLookup.Provider;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.biome.Biome;
import net.minecraft.world.level.biome.BiomeResolver;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.Block.UpdateFlags;
import net.minecraft.world.level.block.entity.BlockEntity;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.status.ChunkStatus;
import net.minecraft.world.level.levelgen.Heightmap;
import net.minecraft.world.level.levelgen.blending.BlendingData;
import net.minecraft.world.level.levelgen.structure.Structure;
import net.minecraft.world.level.levelgen.structure.StructureStart;
import net.minecraft.world.level.lighting.ChunkSkyLightSources;
import net.minecraft.world.level.material.Fluid;
import net.minecraft.world.level.material.FluidState;
import net.minecraft.world.ticks.BlackholeTickAccess;
import net.minecraft.world.ticks.TickContainerAccess;
import org.jspecify.annotations.Nullable;

public class ImposterProtoChunk extends ProtoChunk implements StarlightChunk {
   private final LevelChunk wrapped;
   private final boolean allowWrites;

   public ImposterProtoChunk(LevelChunk wrapped, boolean allowWrites) {
      super(wrapped.getPos(), UpgradeData.EMPTY, wrapped.levelHeightAccessor, wrapped.getLevel().palettedContainerFactory(), wrapped.getBlendingData());
      this.wrapped = wrapped;
      this.allowWrites = allowWrites;
   }

   @Override
   public SWMRNibbleArray[] starlight$getBlockNibbles() {
      return this.wrapped.starlight$getBlockNibbles();
   }

   @Override
   public void starlight$setBlockNibbles(SWMRNibbleArray[] nibbles) {
      this.wrapped.starlight$setBlockNibbles(nibbles);
   }

   @Override
   public SWMRNibbleArray[] starlight$getSkyNibbles() {
      return this.wrapped.starlight$getSkyNibbles();
   }

   @Override
   public void starlight$setSkyNibbles(SWMRNibbleArray[] nibbles) {
      this.wrapped.starlight$setSkyNibbles(nibbles);
   }

   @Override
   public boolean[] starlight$getSkyEmptinessMap() {
      return this.wrapped.starlight$getSkyEmptinessMap();
   }

   @Override
   public void starlight$setSkyEmptinessMap(boolean[] emptinessMap) {
      this.wrapped.starlight$setSkyEmptinessMap(emptinessMap);
   }

   @Override
   public boolean[] starlight$getBlockEmptinessMap() {
      return this.wrapped.starlight$getBlockEmptinessMap();
   }

   @Override
   public void starlight$setBlockEmptinessMap(boolean[] emptinessMap) {
      this.wrapped.starlight$setBlockEmptinessMap(emptinessMap);
   }

   @Override
   public BlockState moonrise$getBlock(int x, int y, int z) {
      return this.wrapped.moonrise$getBlock(x, y, z);
   }

   @Override
   public FluidState moonrise$getFluid(int x, int y, int z) {
      return this.wrapped.moonrise$getFluid(x, y, z);
   }

   @Nullable
   @Override
   public BlockEntity getBlockEntity(BlockPos pos) {
      return this.wrapped.getBlockEntity(pos);
   }

   @Override
   public BlockState getBlockState(BlockPos pos) {
      return this.wrapped.getBlockState(pos);
   }

   @Override
   public final BlockState getBlockState(int x, int y, int z) {
      return this.wrapped.getBlockStateFinal(x, y, z);
   }

   @Override
   public FluidState getFluidState(BlockPos pos) {
      return this.wrapped.getFluidState(pos);
   }

   @Override
   public LevelChunkSection getSection(int sectionIndex) {
      return this.allowWrites ? this.wrapped.getSection(sectionIndex) : super.getSection(sectionIndex);
   }

   @Nullable
   @Override
   public BlockState setBlockState(BlockPos pos, BlockState state, @UpdateFlags int flags) {
      return this.allowWrites ? this.wrapped.setBlockState(pos, state, flags) : null;
   }

   @Override
   public void setBlockEntity(BlockEntity blockEntity) {
      if (this.allowWrites) {
         this.wrapped.setBlockEntity(blockEntity);
      }
   }

   @Override
   public void addEntity(Entity entity) {
      if (this.allowWrites) {
         this.wrapped.addEntity(entity);
      }
   }

   @Override
   public void setPersistedStatus(ChunkStatus status) {
      if (this.allowWrites) {
         super.setPersistedStatus(status);
      }
   }

   @Override
   public LevelChunkSection[] getSections() {
      return this.wrapped.getSections();
   }

   @Override
   public void setHeightmap(Heightmap.Types key, long[] data) {
   }

   private Heightmap.Types fixType(Heightmap.Types type) {
      if (type == Heightmap.Types.WORLD_SURFACE_WG) {
         return Heightmap.Types.WORLD_SURFACE;
      } else {
         return type == Heightmap.Types.OCEAN_FLOOR_WG ? Heightmap.Types.OCEAN_FLOOR : type;
      }
   }

   @Override
   public Heightmap getOrCreateHeightmapUnprimed(Heightmap.Types type) {
      return this.wrapped.getOrCreateHeightmapUnprimed(type);
   }

   @Override
   public int getHeight(Heightmap.Types type, int x, int z) {
      return this.wrapped.getHeight(this.fixType(type), x, z);
   }

   @Override
   public Holder<Biome> getNoiseBiome(int quartX, int quartY, int quartZ) {
      return this.wrapped.getNoiseBiome(quartX, quartY, quartZ);
   }

   @Override
   public ChunkPos getPos() {
      return this.wrapped.getPos();
   }

   @Nullable
   @Override
   public StructureStart getStartForStructure(Structure structure) {
      return this.wrapped.getStartForStructure(structure);
   }

   @Override
   public void setStartForStructure(Structure structure, StructureStart structureStart) {
   }

   @Override
   public Map<Structure, StructureStart> getAllStarts() {
      return this.wrapped.getAllStarts();
   }

   @Override
   public void setAllStarts(Map<Structure, StructureStart> starts) {
   }

   @Override
   public LongSet getReferencesForStructure(Structure structure) {
      return this.wrapped.getReferencesForStructure(structure);
   }

   @Override
   public void addReferenceForStructure(Structure structure, long reference) {
   }

   @Override
   public Map<Structure, LongSet> getAllReferences() {
      return this.wrapped.getAllReferences();
   }

   @Override
   public void setAllReferences(Map<Structure, LongSet> data) {
   }

   @Override
   public void markUnsaved() {
      this.wrapped.markUnsaved();
   }

   @Override
   public boolean canBeSerialized() {
      return false;
   }

   @Override
   public boolean tryMarkSaved() {
      return false;
   }

   @Override
   public boolean isUnsaved() {
      return false;
   }

   @Override
   public ChunkStatus getPersistedStatus() {
      return this.wrapped.getPersistedStatus();
   }

   @Override
   public void removeBlockEntity(BlockPos pos) {
   }

   @Override
   public void markPosForPostProcessing(BlockPos blockPos) {
   }

   @Override
   public void setBlockEntityNbt(CompoundTag entityTag) {
   }

   @Nullable
   @Override
   public CompoundTag getBlockEntityNbt(BlockPos blockPos) {
      return this.wrapped.getBlockEntityNbt(blockPos);
   }

   @Nullable
   @Override
   public CompoundTag getBlockEntityNbtForSaving(BlockPos blockPos, Provider registryAccess) {
      return this.wrapped.getBlockEntityNbtForSaving(blockPos, registryAccess);
   }

   @Override
   public void findBlocks(Predicate<BlockState> predicate, BiConsumer<BlockPos, BlockState> consumer) {
      this.wrapped.findBlocks(predicate, consumer);
   }

   @Override
   public TickContainerAccess<Block> getBlockTicks() {
      return this.allowWrites ? this.wrapped.getBlockTicks() : BlackholeTickAccess.emptyContainer();
   }

   @Override
   public TickContainerAccess<Fluid> getFluidTicks() {
      return this.allowWrites ? this.wrapped.getFluidTicks() : BlackholeTickAccess.emptyContainer();
   }

   @Override
   public ChunkAccess.PackedTicks getTicksForSerialization(long currentTick) {
      return this.wrapped.getTicksForSerialization(currentTick);
   }

   @Nullable
   @Override
   public BlendingData getBlendingData() {
      return this.wrapped.getBlendingData();
   }

   public LevelChunk getWrapped() {
      return this.wrapped;
   }

   @Override
   public boolean isLightCorrect() {
      return this.wrapped.isLightCorrect();
   }

   @Override
   public void setLightCorrect(boolean isLightCorrect) {
      this.wrapped.setLightCorrect(isLightCorrect);
   }

   @Override
   public void fillBiomesFromNoise(BiomeResolver biomeResolver) {
      if (this.allowWrites) {
         this.wrapped.fillBiomesFromNoise(biomeResolver);
      }
   }

   @Override
   public void initializeLightSources() {
      this.wrapped.initializeLightSources();
   }

   @Override
   public ChunkSkyLightSources getSkyLightSources() {
      return this.wrapped.getSkyLightSources();
   }
}
