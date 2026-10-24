package net.minecraft.world.level.chunk;

import ca.spottedleaf.moonrise.common.util.WorldUtil;
import ca.spottedleaf.moonrise.patches.getblock.GetBlockChunk;
import com.google.common.collect.Lists;
import com.mojang.logging.LogUtils;
import it.unimi.dsi.fastutil.shorts.ShortList;
import java.util.Collections;
import java.util.EnumSet;
import java.util.List;
import java.util.Map;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Holder;
import net.minecraft.core.SectionPos;
import net.minecraft.core.HolderLookup.Provider;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.util.ProblemReporter.ScopedCollector;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.LevelHeightAccessor;
import net.minecraft.world.level.biome.Biome;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.Block.UpdateFlags;
import net.minecraft.world.level.block.entity.BlockEntity;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.status.ChunkStatus;
import net.minecraft.world.level.levelgen.BelowZeroRetrogen;
import net.minecraft.world.level.levelgen.Heightmap;
import net.minecraft.world.level.levelgen.blending.BlendingData;
import net.minecraft.world.level.levelgen.structure.BoundingBox;
import net.minecraft.world.level.levelgen.structure.Structure;
import net.minecraft.world.level.levelgen.structure.StructureStart;
import net.minecraft.world.level.lighting.LevelLightEngine;
import net.minecraft.world.level.lighting.LightEngine;
import net.minecraft.world.level.material.Fluid;
import net.minecraft.world.level.material.FluidState;
import net.minecraft.world.level.material.Fluids;
import net.minecraft.world.level.storage.TagValueOutput;
import net.minecraft.world.ticks.LevelChunkTicks;
import net.minecraft.world.ticks.ProtoChunkTicks;
import net.minecraft.world.ticks.TickContainerAccess;
import org.jspecify.annotations.Nullable;
import org.slf4j.Logger;

public class ProtoChunk extends ChunkAccess implements GetBlockChunk {
   private static final Logger LOGGER = LogUtils.getLogger();
   @Nullable
   private volatile LevelLightEngine lightEngine;
   private volatile ChunkStatus status = ChunkStatus.EMPTY;
   private final List<CompoundTag> entities = Lists.newArrayList();
   @Nullable
   private BelowZeroRetrogen belowZeroRetrogen;
   private final ProtoChunkTicks<Block> blockTicks;
   private final ProtoChunkTicks<Fluid> fluidTicks;
   private static final BlockState AIR_BLOCKSTATE = Blocks.AIR.defaultBlockState();
   private static final FluidState AIR_FLUIDSTATE = Fluids.EMPTY.defaultFluidState();
   private static final BlockState VOID_AIR_BLOCKSTATE = Blocks.VOID_AIR.defaultBlockState();
   private final int minSection;
   private final int maxSection;

   @Override
   public BlockState moonrise$getBlock(int x, int y, int z) {
      int sectionY = (y >> 4) - this.minSection;
      LevelChunkSection[] sections = this.sections;
      if (sectionY >= 0 && sectionY < sections.length) {
         LevelChunkSection section = sections[sectionY];
         if (!section.hasOnlyAir()) {
            int index = x & 15 | (z & 15) << 4 | (y & 15) << 8;
            return section.getStates().get(index);
         } else {
            return AIR_BLOCKSTATE;
         }
      } else {
         return VOID_AIR_BLOCKSTATE;
      }
   }

   @Override
   public FluidState moonrise$getFluid(int x, int y, int z) {
      int sectionY = (y >> 4) - this.minSection;
      LevelChunkSection[] sections = this.sections;
      if (sectionY >= 0 && sectionY < sections.length) {
         LevelChunkSection section = sections[sectionY];
         if (!section.hasOnlyAir()) {
            int index = x & 15 | (z & 15) << 4 | (y & 15) << 8;
            return section.getStates().get(index).getFluidState();
         } else {
            return AIR_FLUIDSTATE;
         }
      } else {
         return AIR_FLUIDSTATE;
      }
   }

   public ProtoChunk(
      ChunkPos chunkPos,
      UpgradeData upgradeData,
      LevelHeightAccessor levelHeightAccessor,
      PalettedContainerFactory containerFactory,
      @Nullable BlendingData blendingData
   ) {
      this(chunkPos, upgradeData, null, new ProtoChunkTicks<>(), new ProtoChunkTicks<>(), levelHeightAccessor, containerFactory, blendingData);
   }

   public ProtoChunk(
      ChunkPos chunkPos,
      UpgradeData upgradeData,
      @Nullable LevelChunkSection[] sections,
      ProtoChunkTicks<Block> blockTicks,
      ProtoChunkTicks<Fluid> fluidTicks,
      LevelHeightAccessor levelHeightAccessor,
      PalettedContainerFactory containerFactory,
      @Nullable BlendingData blendingData
   ) {
      super(chunkPos, upgradeData, levelHeightAccessor, containerFactory, 0L, sections, blendingData);
      this.blockTicks = blockTicks;
      this.fluidTicks = fluidTicks;
      this.minSection = WorldUtil.getMinSection(levelHeightAccessor);
      this.maxSection = WorldUtil.getMaxSection(levelHeightAccessor);
   }

   @Override
   public TickContainerAccess<Block> getBlockTicks() {
      return this.blockTicks;
   }

   @Override
   public TickContainerAccess<Fluid> getFluidTicks() {
      return this.fluidTicks;
   }

   @Override
   public ChunkAccess.PackedTicks getTicksForSerialization(long currentTick) {
      return new ChunkAccess.PackedTicks(this.blockTicks.pack(currentTick), this.fluidTicks.pack(currentTick));
   }

   @Override
   public final FluidState getFluidIfLoaded(BlockPos pos) {
      return this.getFluidState(pos);
   }

   @Override
   public final BlockState getBlockStateIfLoaded(BlockPos pos) {
      return this.getBlockState(pos);
   }

   @Override
   public BlockState getBlockState(BlockPos pos) {
      return this.getBlockState(pos.getX(), pos.getY(), pos.getZ());
   }

   @Override
   public BlockState getBlockState(int x, int y, int z) {
      return this.moonrise$getBlock(x, y, z);
   }

   @Override
   public FluidState getFluidState(BlockPos pos) {
      return this.moonrise$getFluid(pos.getX(), pos.getY(), pos.getZ());
   }

   @Nullable
   @Override
   public BlockState setBlockState(BlockPos pos, BlockState state, @UpdateFlags int flags) {
      int x = pos.getX();
      int y = pos.getY();
      int z = pos.getZ();
      if (this.isOutsideBuildHeight(y)) {
         return Blocks.VOID_AIR.defaultBlockState();
      } else {
         int sectionIndex = this.getSectionIndex(y);
         LevelChunkSection section = this.getSection(sectionIndex);
         boolean wasEmpty = section.hasOnlyAir();
         if (wasEmpty && state.is(Blocks.AIR)) {
            return state;
         } else {
            int localX = SectionPos.sectionRelative(x);
            int localY = SectionPos.sectionRelative(y);
            int localZ = SectionPos.sectionRelative(z);
            BlockState oldState = section.setBlockState(localX, localY, localZ, state);
            if (this.status.isOrAfter(ChunkStatus.INITIALIZE_LIGHT)) {
               boolean isEmpty = section.hasOnlyAir();
               if (isEmpty != wasEmpty) {
                  this.lightEngine.updateSectionStatus(pos, isEmpty);
               }

               if (LightEngine.hasDifferentLightProperties(oldState, state)) {
                  this.lightEngine.checkBlock(pos);
               }
            }

            EnumSet<Heightmap.Types> heightmapsAfter = this.getPersistedStatus().heightmapsAfter();
            EnumSet<Heightmap.Types> toPrime = null;

            for (Heightmap.Types type : heightmapsAfter) {
               Heightmap heightmap = this.heightmaps.get(type);
               if (heightmap == null) {
                  if (toPrime == null) {
                     toPrime = EnumSet.noneOf(Heightmap.Types.class);
                  }

                  toPrime.add(type);
               } else {
                  heightmap.update(localX, y, localZ, state);
               }
            }

            if (toPrime != null) {
               Heightmap.primeHeightmaps(this, toPrime);
            }

            return oldState;
         }
      }
   }

   @Override
   public void setBlockEntity(BlockEntity blockEntity) {
      this.pendingBlockEntities.remove(blockEntity.getBlockPos());
      this.blockEntities.put(blockEntity.getBlockPos(), blockEntity);
   }

   @Nullable
   @Override
   public BlockEntity getBlockEntity(BlockPos pos) {
      return this.blockEntities.get(pos);
   }

   public Map<BlockPos, BlockEntity> getBlockEntities() {
      return this.blockEntities;
   }

   public void addEntity(CompoundTag tag) {
      this.entities.add(tag);
   }

   @Override
   public void addEntity(Entity entity) {
      if (!entity.isPassenger()) {
         try (ScopedCollector reporter = new ScopedCollector(entity.problemPath(), LOGGER)) {
            TagValueOutput output = TagValueOutput.createWithContext(reporter, entity.registryAccess());
            entity.save(output);
            this.addEntity(output.buildResult());
         }
      }
   }

   @Override
   public void setStartForStructure(Structure structure, StructureStart structureStart) {
      BelowZeroRetrogen belowZeroRetrogen = this.getBelowZeroRetrogen();
      if (belowZeroRetrogen != null && structureStart.isValid()) {
         BoundingBox boundingBox = structureStart.getBoundingBox();
         LevelHeightAccessor heightAccessor = this.getHeightAccessorForGeneration();
         if (boundingBox.minY() < heightAccessor.getMinY() || boundingBox.maxY() > heightAccessor.getMaxY()) {
            return;
         }
      }

      super.setStartForStructure(structure, structureStart);
   }

   public List<CompoundTag> getEntities() {
      return this.entities;
   }

   @Override
   public ChunkStatus getPersistedStatus() {
      return this.status;
   }

   public void setPersistedStatus(ChunkStatus status) {
      this.status = status;
      if (this.belowZeroRetrogen != null && status.isOrAfter(this.belowZeroRetrogen.targetStatus())) {
         this.setBelowZeroRetrogen(null);
      }

      this.markUnsaved();
   }

   @Override
   public Holder<Biome> getNoiseBiome(int quartX, int quartY, int quartZ) {
      if (this.getHighestGeneratedStatus().isOrAfter(ChunkStatus.BIOMES)) {
         return super.getNoiseBiome(quartX, quartY, quartZ);
      } else {
         throw new IllegalStateException("Asking for biomes before we have biomes");
      }
   }

   public static short packOffsetCoordinates(BlockPos pos) {
      int x = pos.getX();
      int y = pos.getY();
      int z = pos.getZ();
      int dx = x & 15;
      int dy = y & 15;
      int dz = z & 15;
      return (short)(dx | dy << 4 | dz << 8);
   }

   public static BlockPos unpackOffsetCoordinates(short packedData, int sectionY, ChunkPos chunkPos) {
      int posX = SectionPos.sectionToBlockCoord(chunkPos.x(), packedData & 15);
      int posY = SectionPos.sectionToBlockCoord(sectionY, packedData >>> 4 & 15);
      int posZ = SectionPos.sectionToBlockCoord(chunkPos.z(), packedData >>> 8 & 15);
      return new BlockPos(posX, posY, posZ);
   }

   @Override
   public void markPosForPostProcessing(BlockPos blockPos) {
      if (this.isInsideBuildHeight(blockPos)) {
         ChunkAccess.getOrCreateOffsetList(this.postProcessing, this.getSectionIndex(blockPos.getY())).add(packOffsetCoordinates(blockPos));
      }
   }

   @Override
   public void addPackedPostProcess(ShortList packedOffsets, int sectionIndex) {
      ChunkAccess.getOrCreateOffsetList(this.postProcessing, sectionIndex).addAll(packedOffsets);
   }

   public Map<BlockPos, CompoundTag> getBlockEntityNbts() {
      return Collections.unmodifiableMap(this.pendingBlockEntities);
   }

   @Nullable
   @Override
   public CompoundTag getBlockEntityNbtForSaving(BlockPos blockPos, Provider registryAccess) {
      BlockEntity blockEntity = this.getBlockEntity(blockPos);
      return blockEntity != null ? blockEntity.saveWithFullMetadata(registryAccess) : this.pendingBlockEntities.get(blockPos);
   }

   @Override
   public void removeBlockEntity(BlockPos pos) {
      this.blockEntities.remove(pos);
      this.pendingBlockEntities.remove(pos);
   }

   public void setLightEngine(LevelLightEngine lightEngine) {
      this.lightEngine = lightEngine;
   }

   public void setBelowZeroRetrogen(@Nullable BelowZeroRetrogen belowZeroRetrogen) {
      this.belowZeroRetrogen = belowZeroRetrogen;
   }

   @Nullable
   @Override
   public BelowZeroRetrogen getBelowZeroRetrogen() {
      return this.belowZeroRetrogen;
   }

   private static <T> LevelChunkTicks<T> unpackTicks(ProtoChunkTicks<T> ticks) {
      return new LevelChunkTicks<>(ticks.scheduledTicks());
   }

   public LevelChunkTicks<Block> unpackBlockTicks() {
      return unpackTicks(this.blockTicks);
   }

   public LevelChunkTicks<Fluid> unpackFluidTicks() {
      return unpackTicks(this.fluidTicks);
   }

   @Override
   public LevelHeightAccessor getHeightAccessorForGeneration() {
      return (LevelHeightAccessor)(this.isUpgrading() ? BelowZeroRetrogen.UPGRADE_HEIGHT_ACCESSOR : this);
   }
}
