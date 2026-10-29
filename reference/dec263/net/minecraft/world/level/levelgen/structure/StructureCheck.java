package net.minecraft.world.level.levelgen.structure;

import ca.spottedleaf.dataconverter.minecraft.MCDataConverter;
import ca.spottedleaf.dataconverter.minecraft.datatypes.MCTypeRegistry;
import ca.spottedleaf.dataconverter.minecraft.util.Version;
import ca.spottedleaf.moonrise.common.map.SynchronisedLong2BooleanMap;
import ca.spottedleaf.moonrise.common.map.SynchronisedLong2ObjectMap;
import com.mojang.datafixers.DataFixer;
import com.mojang.logging.LogUtils;
import it.unimi.dsi.fastutil.longs.LongOpenHashSet;
import it.unimi.dsi.fastutil.longs.LongSet;
import it.unimi.dsi.fastutil.longs.LongSets;
import it.unimi.dsi.fastutil.objects.Object2IntMap;
import it.unimi.dsi.fastutil.objects.Object2IntMaps;
import it.unimi.dsi.fastutil.objects.Object2IntOpenHashMap;
import java.util.Map;
import java.util.Optional;
import java.util.concurrent.ConcurrentHashMap;
import net.minecraft.core.Registry;
import net.minecraft.core.RegistryAccess;
import net.minecraft.core.registries.Registries;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.nbt.IntTag;
import net.minecraft.nbt.NbtUtils;
import net.minecraft.nbt.visitors.CollectFields;
import net.minecraft.nbt.visitors.FieldSelector;
import net.minecraft.resources.Identifier;
import net.minecraft.resources.ResourceKey;
import net.minecraft.server.level.ChunkMap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.util.datafix.DataFixTypes;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.LevelHeightAccessor;
import net.minecraft.world.level.biome.BiomeSource;
import net.minecraft.world.level.biome.Climate;
import net.minecraft.world.level.chunk.ChunkGenerator;
import net.minecraft.world.level.chunk.storage.ChunkScanAccess;
import net.minecraft.world.level.chunk.storage.SimpleRegionStorage;
import net.minecraft.world.level.dimension.LevelStem;
import net.minecraft.world.level.levelgen.RandomState;
import net.minecraft.world.level.levelgen.densityfunction.SamplerContext;
import net.minecraft.world.level.levelgen.structure.placement.StructurePlacement;
import net.minecraft.world.level.levelgen.structure.structures.BuriedTreasureStructure;
import net.minecraft.world.level.levelgen.structure.structures.MineshaftStructure;
import net.minecraft.world.level.levelgen.structure.templatesystem.StructureTemplateManager;
import org.jspecify.annotations.Nullable;
import org.slf4j.Logger;

public class StructureCheck {
   private static final Logger LOGGER = LogUtils.getLogger();
   private static final int NO_STRUCTURE = -1;
   private static final int MAX_CHUNKS_WITHOUT_STORED_STARTS = 131072;
   private final ChunkScanAccess storageAccess;
   private final RegistryAccess registryAccess;
   private final StructureTemplateManager structureTemplateManager;
   private final ResourceKey<Level> dimension;
   private final ChunkGenerator chunkGenerator;
   private final RandomState randomState;
   private final LevelHeightAccessor heightAccessor;
   private final ThreadLocal<Climate.Sampler> moonrise$climateSampler;
   private final BiomeSource biomeSource;
   private final long seed;
   private final DataFixer fixerUpper;
   private static final int CHUNK_TOTAL_LIMIT = 2020050;
   private static final int PER_FEATURE_CHECK_LIMIT = 2020050;
   private final SynchronisedLong2ObjectMap<Object2IntMap<Structure>> loadedChunksSafe = new SynchronisedLong2ObjectMap<>(2020050);
   private final ConcurrentHashMap<Structure, SynchronisedLong2BooleanMap> featureChecksSafe = new ConcurrentHashMap<>();
   private final LongSet chunksWithoutStartsInStorage = LongSets.synchronize(new LongOpenHashSet());
   private final ResourceKey<LevelStem> levelStem;

   public StructureCheck(
      ResourceKey<LevelStem> levelStem,
      ChunkScanAccess storageAccess,
      RegistryAccess registryAccess,
      StructureTemplateManager structureTemplateManager,
      ResourceKey<Level> dimension,
      ChunkGenerator chunkGenerator,
      RandomState randomState,
      LevelHeightAccessor heightAccessor,
      BiomeSource biomeSource,
      long seed,
      DataFixer fixerUpper
   ) {
      this.storageAccess = storageAccess;
      this.registryAccess = registryAccess;
      this.structureTemplateManager = structureTemplateManager;
      this.dimension = dimension;
      this.chunkGenerator = chunkGenerator;
      this.randomState = randomState;
      this.heightAccessor = heightAccessor;
      this.biomeSource = biomeSource;
      this.moonrise$climateSampler = ThreadLocal.withInitial(() -> randomState.createClimateSampler(SamplerContext.builder().enableCaches().build()));
      this.seed = seed;
      this.fixerUpper = fixerUpper;
      this.levelStem = levelStem;
   }

   @Nullable
   private Integer getSaltOverride(Structure type) {
      if (this.heightAccessor instanceof ServerLevel serverLevel) {
         if (type instanceof MineshaftStructure) {
            return serverLevel.spigotConfig.mineshaftSeed;
         }

         if (type instanceof BuriedTreasureStructure) {
            return serverLevel.spigotConfig.buriedTreasureSeed;
         }
      }

      return null;
   }

   public StructureCheckResult checkStart(ChunkPos pos, Structure structure, StructurePlacement placement, boolean requireUnreferenced) {
      long posKey = pos.pack();
      Object2IntMap<Structure> cachedResult = this.loadedChunksSafe.get(posKey);
      if (cachedResult != null) {
         return this.checkStructureInfo(cachedResult, structure, requireUnreferenced);
      } else {
         if (!this.chunksWithoutStartsInStorage.contains(posKey)) {
            StructureCheckResult storageCheckResult = this.tryLoadFromStorage(pos, structure, requireUnreferenced);
            if (storageCheckResult != null) {
               return storageCheckResult;
            }

            if (this.chunksWithoutStartsInStorage.size() >= 131072) {
               this.chunksWithoutStartsInStorage.clear();
            }

            this.chunksWithoutStartsInStorage.add(posKey);
         }

         if (!placement.applyAdditionalChunkRestrictions(pos.x(), pos.z(), this.seed, this.getSaltOverride(structure))) {
            return StructureCheckResult.START_NOT_PRESENT;
         } else {
            boolean isFeatureChunk = this.featureChecksSafe
               .computeIfAbsent(structure, k -> new SynchronisedLong2BooleanMap(2020050))
               .getOrCompute(posKey, k -> this.canCreateStructure(pos, structure));
            return !isFeatureChunk ? StructureCheckResult.START_NOT_PRESENT : StructureCheckResult.CHUNK_LOAD_NEEDED;
         }
      }
   }

   private boolean canCreateStructure(ChunkPos pos, Structure structure) {
      return structure.findValidGenerationPoint(
            new Structure.GenerationContext(
               this.registryAccess,
               this.chunkGenerator,
               this.biomeSource,
               this.moonrise$climateSampler.get(),
               this.randomState,
               this.structureTemplateManager,
               this.seed,
               pos,
               this.heightAccessor,
               structure.biomes()::contains
            )
         )
         .isPresent();
   }

   @Nullable
   private StructureCheckResult tryLoadFromStorage(ChunkPos pos, Structure structure, boolean requireUnreferenced) {
      CollectFields collectFields = new CollectFields(
         new FieldSelector(IntTag.TYPE, "DataVersion"),
         new FieldSelector("Level", "Structures", CompoundTag.TYPE, "Starts"),
         new FieldSelector("structures", CompoundTag.TYPE, "starts")
      );

      try {
         this.storageAccess.scanChunk(pos, collectFields).join();
      } catch (Exception var10) {
         LOGGER.warn("Failed to read chunk {}", pos, var10);
         return StructureCheckResult.CHUNK_LOAD_NEEDED;
      }

      if (collectFields.getResult() instanceof CompoundTag chunkTag) {
         int version = NbtUtils.getDataVersion(chunkTag);
         SimpleRegionStorage.injectDatafixingContext(
            chunkTag, ChunkMap.getChunkDataFixContextTag(this.dimension, this.chunkGenerator.getTypeNameForDataFixer(), this.levelStem)
         );

         CompoundTag fixedChunkTag;
         try {
            fixedChunkTag = MCDataConverter.convertTag(MCTypeRegistry.CHUNK, DataFixTypes.CHUNK, chunkTag, version, Version.getCurrentVersion());
         } catch (Exception var9) {
            LOGGER.warn("Failed to partially datafix chunk {}", pos, var9);
            return StructureCheckResult.CHUNK_LOAD_NEEDED;
         }

         Object2IntMap<Structure> knownStarts = this.loadStructures(fixedChunkTag);
         if (knownStarts == null) {
            return null;
         } else {
            this.storeFullResults(pos.pack(), knownStarts);
            return this.checkStructureInfo(knownStarts, structure, requireUnreferenced);
         }
      } else {
         return null;
      }
   }

   @Nullable
   private Object2IntMap<Structure> loadStructures(CompoundTag chunkTag) {
      Optional<CompoundTag> maybeStartsTag = chunkTag.getCompound("structures").flatMap(tag -> tag.getCompound("starts"));
      if (maybeStartsTag.isEmpty()) {
         return null;
      } else {
         CompoundTag startsTag = maybeStartsTag.get();
         if (startsTag.isEmpty()) {
            return Object2IntMaps.emptyMap();
         } else {
            Object2IntMap<Structure> knownStarts = new Object2IntOpenHashMap();
            Registry<Structure> structuresRegistry = this.registryAccess.lookupOrThrow(Registries.STRUCTURE);
            startsTag.forEach((key, tag) -> {
               Identifier id = Identifier.tryParse(key);
               if (id != null) {
                  Structure foundFeature = structuresRegistry.getValue(id);
                  if (foundFeature != null) {
                     tag.asCompound().ifPresent(structureData -> {
                        String pieceId = structureData.getStringOr("id", "");
                        if (!"INVALID".equals(pieceId)) {
                           int referenceCount = structureData.getIntOr("references", 0);
                           knownStarts.put(foundFeature, referenceCount);
                        }
                     });
                  }
               }
            });
            return knownStarts;
         }
      }
   }

   private static Object2IntMap<Structure> deduplicateEmptyMap(Object2IntMap<Structure> map) {
      return map.isEmpty() ? Object2IntMaps.emptyMap() : map;
   }

   private StructureCheckResult checkStructureInfo(Object2IntMap<Structure> cachedResult, Structure structure, boolean requireUnreferenced) {
      int referenceCount = cachedResult.getOrDefault(structure, -1);
      return referenceCount != -1 && (!requireUnreferenced || referenceCount == 0)
         ? StructureCheckResult.START_PRESENT
         : StructureCheckResult.START_NOT_PRESENT;
   }

   public void onStructureLoad(ChunkPos pos, Map<Structure, StructureStart> starts) {
      long posKey = pos.pack();
      Object2IntMap<Structure> startsToReferences = new Object2IntOpenHashMap();
      starts.forEach((structure, structureStart) -> {
         if (structureStart.isValid()) {
            startsToReferences.put(structure, structureStart.getReferences());
         }
      });
      this.storeFullResults(posKey, startsToReferences);
   }

   private void storeFullResults(long posKey, Object2IntMap<Structure> starts) {
      this.loadedChunksSafe.put(posKey, deduplicateEmptyMap(starts));

      for (SynchronisedLong2BooleanMap value : this.featureChecksSafe.values()) {
         value.remove(posKey);
      }

      this.chunksWithoutStartsInStorage.remove(posKey);
   }

   public void incrementReference(ChunkPos chunkPos, Structure structure) {
      long posKey = chunkPos.pack();
      this.loadedChunksSafe.compute(chunkPos.pack(), (key, counts) -> {
         Object2IntOpenHashMap var4;
         if (counts == null) {
            var4 = new Object2IntOpenHashMap();
         } else {
            var4 = counts instanceof Object2IntOpenHashMap<Structure> fastClone ? fastClone.clone() : new Object2IntOpenHashMap(counts);
         }

         var4.computeInt(structure, (k, value) -> value == null ? 1 : value + 1);
         return var4;
      });
      this.chunksWithoutStartsInStorage.remove(posKey);
   }
}
