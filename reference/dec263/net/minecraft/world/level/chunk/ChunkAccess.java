package net.minecraft.world.level.chunk;

import ca.spottedleaf.moonrise.common.util.WorldUtil;
import ca.spottedleaf.moonrise.patches.starlight.chunk.StarlightChunk;
import ca.spottedleaf.moonrise.patches.starlight.light.SWMRNibbleArray;
import ca.spottedleaf.moonrise.patches.starlight.light.StarLightEngine;
import com.google.common.collect.Maps;
import com.google.common.collect.Sets;
import com.mojang.logging.LogUtils;
import it.unimi.dsi.fastutil.longs.LongOpenHashSet;
import it.unimi.dsi.fastutil.longs.LongSet;
import it.unimi.dsi.fastutil.objects.Object2ObjectOpenHashMap;
import it.unimi.dsi.fastutil.shorts.ShortArrayList;
import it.unimi.dsi.fastutil.shorts.ShortList;
import java.util.Collection;
import java.util.Collections;
import java.util.EnumSet;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.Map.Entry;
import java.util.function.BiConsumer;
import java.util.function.Predicate;
import java.util.function.Supplier;
import net.minecraft.CrashReport;
import net.minecraft.CrashReportCategory;
import net.minecraft.ReportedException;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Holder;
import net.minecraft.core.QuartPos;
import net.minecraft.core.SectionPos;
import net.minecraft.core.HolderLookup.Provider;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.util.Continuation;
import net.minecraft.util.Mth;
import net.minecraft.util.ProblemReporter.PathElement;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.LevelHeightAccessor;
import net.minecraft.world.level.biome.Biome;
import net.minecraft.world.level.biome.BiomeGenerationSettings;
import net.minecraft.world.level.biome.BiomeResolver;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.Block.UpdateFlags;
import net.minecraft.world.level.block.entity.BlockEntity;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.blockscan.BlockScanUtils;
import net.minecraft.world.level.blockscan.BlockStateConsumer;
import net.minecraft.world.level.chunk.status.ChunkStatus;
import net.minecraft.world.level.gameevent.GameEventListenerRegistry;
import net.minecraft.world.level.levelgen.BelowZeroRetrogen;
import net.minecraft.world.level.levelgen.Heightmap;
import net.minecraft.world.level.levelgen.blending.BlendingData;
import net.minecraft.world.level.levelgen.structure.Structure;
import net.minecraft.world.level.levelgen.structure.StructureStart;
import net.minecraft.world.level.lighting.ChunkSkyLightSources;
import net.minecraft.world.level.material.Fluid;
import net.minecraft.world.ticks.SavedTick;
import net.minecraft.world.ticks.TickContainerAccess;
import org.bukkit.craftbukkit.persistence.CraftPersistentDataTypeRegistry;
import org.bukkit.craftbukkit.persistence.DirtyCraftPersistentDataContainer;
import org.jspecify.annotations.Nullable;
import org.slf4j.Logger;

public abstract class ChunkAccess implements LightChunk, StructureAccess, BiomeResolver, StarlightChunk {
   public static final int NO_FILLED_SECTION = -1;
   private static final Logger LOGGER = LogUtils.getLogger();
   private static final LongSet EMPTY_REFERENCE_SET = new LongOpenHashSet();
   protected final ShortList[] postProcessing;
   private volatile boolean unsaved;
   private volatile boolean isLightCorrect;
   protected final ChunkPos chunkPos;
   public final long coordinateKey;
   public final int locX;
   public final int locZ;
   private long inhabitedTime;
   @Deprecated
   @Nullable
   private BiomeGenerationSettings carverBiomeSettings;
   protected final UpgradeData upgradeData;
   @Nullable
   protected final BlendingData blendingData;
   public final Map<Heightmap.Types, Heightmap> heightmaps = Maps.newEnumMap(Heightmap.Types.class);
   private final Map<Structure, StructureStart> structureStarts = Maps.newHashMap();
   private final Map<Structure, LongSet> structureReferences = Maps.newHashMap();
   protected final Map<BlockPos, CompoundTag> pendingBlockEntities = Maps.newHashMap();
   public final Map<BlockPos, BlockEntity> blockEntities = new Object2ObjectOpenHashMap();
   protected final LevelHeightAccessor levelHeightAccessor;
   protected final LevelChunkSection[] sections;
   private static final CraftPersistentDataTypeRegistry DATA_TYPE_REGISTRY = new CraftPersistentDataTypeRegistry();
   public DirtyCraftPersistentDataContainer persistentDataContainer = new DirtyCraftPersistentDataContainer(DATA_TYPE_REGISTRY);
   private volatile SWMRNibbleArray[] blockNibbles;
   private volatile SWMRNibbleArray[] skyNibbles;
   private volatile boolean[] skyEmptinessMap;
   private volatile boolean[] blockEmptinessMap;
   private final int minSection;
   private final int maxSection;
   private final int minY;
   private final int height;
   private final int maxY;
   private final int minSectionY;
   private final int maxSectionY;
   private final int sectionsCount;

   @Override
   public SWMRNibbleArray[] starlight$getBlockNibbles() {
      return this.blockNibbles;
   }

   @Override
   public void starlight$setBlockNibbles(SWMRNibbleArray[] nibbles) {
      this.blockNibbles = nibbles;
   }

   @Override
   public SWMRNibbleArray[] starlight$getSkyNibbles() {
      return this.skyNibbles;
   }

   @Override
   public void starlight$setSkyNibbles(SWMRNibbleArray[] nibbles) {
      this.skyNibbles = nibbles;
   }

   @Override
   public boolean[] starlight$getSkyEmptinessMap() {
      return this.skyEmptinessMap;
   }

   @Override
   public void starlight$setSkyEmptinessMap(boolean[] emptinessMap) {
      this.skyEmptinessMap = emptinessMap;
   }

   @Override
   public boolean[] starlight$getBlockEmptinessMap() {
      return this.blockEmptinessMap;
   }

   @Override
   public void starlight$setBlockEmptinessMap(boolean[] emptinessMap) {
      this.blockEmptinessMap = emptinessMap;
   }

   public ChunkAccess(
      ChunkPos chunkPos,
      UpgradeData upgradeData,
      LevelHeightAccessor levelHeightAccessor,
      PalettedContainerFactory containerFactory,
      long inhabitedTime,
      @Nullable LevelChunkSection[] sections,
      @Nullable BlendingData blendingData
   ) {
      this.minY = levelHeightAccessor.getMinY();
      this.height = levelHeightAccessor.getHeight();
      this.maxY = this.minY + this.height - 1;
      this.minSectionY = this.minY >> 4;
      this.maxSectionY = this.maxY >> 4;
      this.sectionsCount = this.maxSectionY - this.minSectionY + 1;
      this.locX = chunkPos.x();
      this.locZ = chunkPos.z();
      this.chunkPos = chunkPos;
      this.coordinateKey = ChunkPos.pack(this.locX, this.locZ);
      this.upgradeData = upgradeData;
      this.levelHeightAccessor = levelHeightAccessor;
      this.sections = new LevelChunkSection[levelHeightAccessor.getSectionsCount()];
      this.inhabitedTime = inhabitedTime;
      this.postProcessing = new ShortList[levelHeightAccessor.getSectionsCount()];
      this.blendingData = blendingData;
      if (sections != null) {
         if (this.sections.length == sections.length) {
            System.arraycopy(sections, 0, this.sections, 0, this.sections.length);
         } else {
            LOGGER.warn("Could not set level chunk sections, array length is {} instead of {}", sections.length, this.sections.length);
         }
      }

      this.replaceMissingSections(containerFactory, this.sections);
      if (!(this instanceof ImposterProtoChunk)) {
         this.starlight$setBlockNibbles(StarLightEngine.getFilledEmptyLight(levelHeightAccessor));
         this.starlight$setSkyNibbles(StarLightEngine.getFilledEmptyLight(levelHeightAccessor));
      }

      this.minSection = WorldUtil.getMinSection(levelHeightAccessor);
      this.maxSection = WorldUtil.getMaxSection(levelHeightAccessor);
   }

   private void replaceMissingSections(PalettedContainerFactory containerFactory, LevelChunkSection[] sections) {
      for (int i = 0; i < sections.length; i++) {
         if (sections[i] == null) {
            sections[i] = new LevelChunkSection(
               containerFactory,
               this.levelHeightAccessor instanceof Level ? (Level)this.levelHeightAccessor : null,
               this.chunkPos,
               this.levelHeightAccessor.getSectionYFromSectionIndex(i)
            );
         }
      }
   }

   public GameEventListenerRegistry getListenerRegistry(int section) {
      return GameEventListenerRegistry.NOOP;
   }

   public abstract BlockState getBlockState(int var1, int var2, int var3);

   @Nullable
   public BlockState setBlockState(BlockPos pos, BlockState state) {
      return this.setBlockState(pos, state, 3);
   }

   @Nullable
   public abstract BlockState setBlockState(BlockPos var1, BlockState var2, @UpdateFlags int var3);

   public abstract void setBlockEntity(BlockEntity var1);

   public abstract void addEntity(Entity var1);

   public int getHighestFilledSectionIndex() {
      LevelChunkSection[] sections = this.getSections();

      for (int sectionIndex = sections.length - 1; sectionIndex >= 0; sectionIndex--) {
         LevelChunkSection section = sections[sectionIndex];
         if (!section.hasOnlyAir()) {
            return sectionIndex;
         }
      }

      return -1;
   }

   @Deprecated(
      forRemoval = true
   )
   public int getHighestSectionPosition() {
      int sectionIndex = this.getHighestFilledSectionIndex();
      return sectionIndex == -1 ? this.getMinY() : SectionPos.sectionToBlockCoord(this.getSectionYFromSectionIndex(sectionIndex));
   }

   public Set<BlockPos> getBlockEntitiesPos() {
      Set<BlockPos> result = Sets.newHashSet(this.pendingBlockEntities.keySet());
      result.addAll(this.blockEntities.keySet());
      return result;
   }

   public LevelChunkSection[] getSections() {
      return this.sections;
   }

   public LevelChunkSection getSection(int sectionIndex) {
      return this.getSections()[sectionIndex];
   }

   public Collection<Entry<Heightmap.Types, Heightmap>> getHeightmaps() {
      return Collections.unmodifiableSet(this.heightmaps.entrySet());
   }

   public void setHeightmap(Heightmap.Types key, long[] data) {
      this.getOrCreateHeightmapUnprimed(key).setRawData(this, key, data);
   }

   public Heightmap getOrCreateHeightmapUnprimed(Heightmap.Types type) {
      return this.heightmaps.computeIfAbsent(type, k -> new Heightmap(this, k));
   }

   public boolean hasPrimedHeightmap(Heightmap.Types type) {
      return this.heightmaps.get(type) != null;
   }

   public int getHeight(Heightmap.Types type, int x, int z) {
      Heightmap heightmap = this.heightmaps.get(type);
      if (heightmap == null) {
         if (SharedConstants.IS_RUNNING_IN_IDE && this instanceof LevelChunk) {
            LOGGER.error("Unprimed heightmap: {} {} {}", new Object[]{type, x, z});
         }

         Heightmap.primeHeightmaps(this, EnumSet.of(type));
         heightmap = this.heightmaps.get(type);
      }

      return heightmap.getFirstAvailable(x & 15, z & 15) - 1;
   }

   public ChunkPos getPos() {
      return this.chunkPos;
   }

   @Nullable
   @Override
   public StructureStart getStartForStructure(Structure structure) {
      return this.structureStarts.get(structure);
   }

   @Override
   public void setStartForStructure(Structure structure, StructureStart structureStart) {
      this.structureStarts.put(structure, structureStart);
      this.markUnsaved();
   }

   public Map<Structure, StructureStart> getAllStarts() {
      return Collections.unmodifiableMap(this.structureStarts);
   }

   public void setAllStarts(Map<Structure, StructureStart> starts) {
      this.structureStarts.clear();
      this.structureStarts.putAll(starts);
      this.markUnsaved();
   }

   @Override
   public LongSet getReferencesForStructure(Structure structure) {
      return this.structureReferences.getOrDefault(structure, EMPTY_REFERENCE_SET);
   }

   @Override
   public void addReferenceForStructure(Structure structure, long reference) {
      this.structureReferences.computeIfAbsent(structure, k -> new LongOpenHashSet()).add(reference);
      this.markUnsaved();
   }

   @Override
   public Map<Structure, LongSet> getAllReferences() {
      return Collections.unmodifiableMap(this.structureReferences);
   }

   @Override
   public void setAllReferences(Map<Structure, LongSet> data) {
      this.structureReferences.clear();
      this.structureReferences.putAll(data);
      this.markUnsaved();
   }

   public boolean isYSpaceEmpty(int yStartInclusive, int yEndInclusive) {
      if (yStartInclusive < this.getMinY()) {
         yStartInclusive = this.getMinY();
      }

      if (yEndInclusive > this.getMaxY()) {
         yEndInclusive = this.getMaxY();
      }

      for (int y = yStartInclusive; y <= yEndInclusive; y += 16) {
         if (!this.getSection(this.getSectionIndex(y)).hasOnlyAir()) {
            return false;
         }
      }

      return true;
   }

   public void markUnsaved() {
      this.unsaved = true;
   }

   public boolean tryMarkSaved() {
      if (this.unsaved) {
         this.unsaved = false;
         this.persistentDataContainer.dirty(false);
         return true;
      } else {
         return false;
      }
   }

   public boolean isUnsaved() {
      return this.unsaved || this.persistentDataContainer.dirty();
   }

   public abstract ChunkStatus getPersistedStatus();

   public ChunkStatus getHighestGeneratedStatus() {
      ChunkStatus status = this.getPersistedStatus();
      BelowZeroRetrogen belowZeroRetrogen = this.getBelowZeroRetrogen();
      if (belowZeroRetrogen != null) {
         ChunkStatus targetStatus = belowZeroRetrogen.targetStatus();
         return ChunkStatus.max(targetStatus, status);
      } else {
         return status;
      }
   }

   public abstract void removeBlockEntity(BlockPos var1);

   public void markPosForPostProcessing(BlockPos blockPos) {
      LOGGER.warn("Trying to mark a block for post processing @ {}, but this operation is not supported.", blockPos);
   }

   public ShortList[] getPostProcessing() {
      return this.postProcessing;
   }

   public void addPackedPostProcess(ShortList packedOffsets, int sectionIndex) {
      getOrCreateOffsetList(this.getPostProcessing(), sectionIndex).addAll(packedOffsets);
   }

   public void setBlockEntityNbt(CompoundTag entityTag) {
      BlockPos posFromTag = BlockEntity.getPosFromTag(this.chunkPos, entityTag);
      if (!this.blockEntities.containsKey(posFromTag)) {
         this.pendingBlockEntities.put(posFromTag, entityTag);
      }
   }

   @Nullable
   public CompoundTag getBlockEntityNbt(BlockPos blockPos) {
      return this.pendingBlockEntities.get(blockPos);
   }

   @Nullable
   public abstract CompoundTag getBlockEntityNbtForSaving(BlockPos var1, Provider var2);

   @Override
   public final void findBlockLightSources(BiConsumer<BlockPos, BlockState> consumer) {
      this.findBlocks(state -> state.getLightEmission() != 0, consumer);
   }

   public void findBlocks(Predicate<BlockState> predicate, BiConsumer<BlockPos, BlockState> consumer) {
      BlockStateConsumer blockStateConsumer = (pos, state) -> {
         consumer.accept(pos, state);
         return Continuation.CONTINUE;
      };
      int max = 15;

      for (int sectionY = this.getMinSectionY(); sectionY <= this.getMaxSectionY(); sectionY++) {
         LevelChunkSection section = this.getSection(this.getSectionIndexFromSectionY(sectionY));
         if (section.maybeHas(predicate)) {
            BlockPos origin = SectionPos.of(this.chunkPos, sectionY).origin();
            BlockScanUtils.findBlocksInSection(section, origin, 0, 0, 0, 15, 15, 15, predicate, blockStateConsumer);
         }
      }
   }

   public abstract TickContainerAccess<Block> getBlockTicks();

   public abstract TickContainerAccess<Fluid> getFluidTicks();

   public void collectBiomesInPalette(Set<Holder<Biome>> output) {
      for (LevelChunkSection section : this.sections) {
         section.getBiomes().forEachInPalette(output::add);
      }
   }

   public boolean canBeSerialized() {
      return true;
   }

   public abstract ChunkAccess.PackedTicks getTicksForSerialization(long var1);

   public UpgradeData getUpgradeData() {
      return this.upgradeData;
   }

   public boolean isOldNoiseGeneration() {
      return this.blendingData != null;
   }

   @Nullable
   public BlendingData getBlendingData() {
      return this.blendingData;
   }

   public long getInhabitedTime() {
      return this.inhabitedTime;
   }

   public void incrementInhabitedTime() {
      this.inhabitedTime++;
   }

   public void setInhabitedTime(long inhabitedTime) {
      this.inhabitedTime = inhabitedTime;
   }

   public static ShortList getOrCreateOffsetList(ShortList[] list, int sectionIndex) {
      ShortList result = list[sectionIndex];
      if (result == null) {
         result = new ShortArrayList();
         list[sectionIndex] = result;
      }

      return result;
   }

   public boolean isLightCorrect() {
      return this.isLightCorrect;
   }

   public void setLightCorrect(boolean isLightCorrect) {
      this.isLightCorrect = isLightCorrect;
      this.markUnsaved();
   }

   @Override
   public int getMinY() {
      return this.minY;
   }

   @Override
   public int getHeight() {
      return this.height;
   }

   @Override
   public int getMaxY() {
      return this.maxY;
   }

   @Override
   public int getSectionsCount() {
      return this.sectionsCount;
   }

   @Override
   public int getMinSectionY() {
      return this.minSectionY;
   }

   @Override
   public int getMaxSectionY() {
      return this.maxSectionY;
   }

   @Override
   public boolean isInsideBuildHeight(int blockY) {
      return blockY >= this.minY && blockY <= this.maxY;
   }

   @Override
   public boolean isOutsideBuildHeight(BlockPos pos) {
      return this.isOutsideBuildHeight(pos.getY());
   }

   @Override
   public boolean isOutsideBuildHeight(int blockY) {
      return blockY < this.minY || blockY > this.maxY;
   }

   @Override
   public int getSectionIndex(int blockY) {
      return (blockY >> 4) - this.minSectionY;
   }

   @Override
   public int getSectionIndexFromSectionY(int sectionY) {
      return sectionY - this.minSectionY;
   }

   @Override
   public int getSectionYFromSectionIndex(int sectionIdx) {
      return sectionIdx + this.minSectionY;
   }

   @Deprecated
   public BiomeGenerationSettings carverBiome(Supplier<BiomeGenerationSettings> source) {
      if (this.carverBiomeSettings == null) {
         this.carverBiomeSettings = source.get();
      }

      return this.carverBiomeSettings;
   }

   @Override
   public Holder<Biome> getNoiseBiome(int quartX, int quartY, int quartZ) {
      int sectionY = (quartY >> 2) - this.minSection;
      int rel = quartY & 3;
      LevelChunkSection[] sections = this.sections;
      if (sectionY < 0) {
         sectionY = 0;
         rel = 0;
      } else if (sectionY >= sections.length) {
         sectionY = sections.length - 1;
         rel = 3;
      }

      return sections[sectionY].getNoiseBiome(quartX & 3, rel, quartZ & 3);
   }

   public void setNoiseBiome(int quartX, int quartY, int quartZ, Holder<Biome> biome) {
      try {
         int quartMinY = QuartPos.fromBlock(this.getMinY());
         int quartMaxY = quartMinY + QuartPos.fromBlock(this.getHeight()) - 1;
         int clampedQuartY = Mth.clamp(quartY, quartMinY, quartMaxY);
         int sectionIndex = this.getSectionIndex(QuartPos.toBlock(clampedQuartY));
         this.sections[sectionIndex].setNoiseBiome(quartX & 3, clampedQuartY & 3, quartZ & 3, biome);
      } catch (Throwable var9) {
         CrashReport report = CrashReport.forThrowable(var9, "Setting biome");
         CrashReportCategory category = report.addCategory("Biome being set");
         category.setDetail("Location", () -> CrashReportCategory.formatLocation(this, quartX, quartY, quartZ));
         throw new ReportedException(report);
      }
   }

   public void fillBiomesFromNoise(BiomeResolver biomeResolver) {
      ChunkPos pos = this.getPos();
      int quartMinX = QuartPos.fromBlock(pos.getMinBlockX());
      int quartMinZ = QuartPos.fromBlock(pos.getMinBlockZ());
      LevelHeightAccessor heightAccessor = this.getHeightAccessorForGeneration();

      for (int sectionY = heightAccessor.getMinSectionY(); sectionY <= heightAccessor.getMaxSectionY(); sectionY++) {
         LevelChunkSection section = this.getSection(this.getSectionIndexFromSectionY(sectionY));
         int quartMinY = QuartPos.fromSection(sectionY);
         section.fillBiomesFromNoise(biomeResolver, quartMinX, quartMinY, quartMinZ);
      }
   }

   @Nullable
   public BelowZeroRetrogen getBelowZeroRetrogen() {
      return null;
   }

   public boolean isUpgrading() {
      return this.getBelowZeroRetrogen() != null;
   }

   public LevelHeightAccessor getHeightAccessorForGeneration() {
      return this;
   }

   public void initializeLightSources() {
   }

   @Override
   public ChunkSkyLightSources getSkyLightSources() {
      return null;
   }

   public static PathElement problemPath(ChunkPos pos) {
      return new ChunkAccess.ChunkPathElement(pos);
   }

   public PathElement problemPath() {
      return problemPath(this.getPos());
   }

   private static record ChunkPathElement(ChunkPos pos) implements PathElement {
      @Override
      public String get() {
         return "chunk@" + this.pos;
      }
   }

   public static record PackedTicks(List<SavedTick<Block>> blocks, List<SavedTick<Fluid>> fluids) {
   }
}
