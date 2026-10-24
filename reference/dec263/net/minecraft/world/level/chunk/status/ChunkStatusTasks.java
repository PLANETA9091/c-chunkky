package net.minecraft.world.level.chunk.status;

import com.mojang.logging.LogUtils;
import io.papermc.paper.configuration.WorldConfiguration.Entities.Spawning.DuplicateUUID.DuplicateUUIDMode;
import it.unimi.dsi.fastutil.objects.ObjectOpenHashSet;
import java.util.EnumSet;
import java.util.Objects;
import java.util.Set;
import java.util.UUID;
import java.util.concurrent.CompletableFuture;
import net.minecraft.SharedConstants;
import net.minecraft.core.Holder;
import net.minecraft.server.level.GenerationChunkHolder;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.server.level.ThreadedLevelLightEngine;
import net.minecraft.server.level.WorldGenRegion;
import net.minecraft.util.StaticCache2D;
import net.minecraft.util.ProblemReporter.ScopedCollector;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.EntitySpawnReason;
import net.minecraft.world.entity.EntityType;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.biome.Biome;
import net.minecraft.world.level.chunk.ChunkAccess;
import net.minecraft.world.level.chunk.ImposterProtoChunk;
import net.minecraft.world.level.chunk.LevelChunk;
import net.minecraft.world.level.chunk.ProtoChunk;
import net.minecraft.world.level.levelgen.BelowZeroRetrogen;
import net.minecraft.world.level.levelgen.Heightmap;
import net.minecraft.world.level.levelgen.blending.Blender;
import net.minecraft.world.level.storage.TagValueInput;
import net.minecraft.world.level.storage.ValueInput.ValueInputList;
import org.bukkit.event.entity.EntityRemoveEvent.Cause;
import org.slf4j.Logger;

public class ChunkStatusTasks {
   private static final Logger LOGGER = LogUtils.getLogger();

   private static boolean isLighted(ChunkAccess chunk) {
      return chunk.getPersistedStatus().isOrAfter(ChunkStatus.LIGHT) && chunk.isLightCorrect();
   }

   public static CompletableFuture<ChunkAccess> passThrough(
      WorldGenContext context, ChunkStep step, StaticCache2D<GenerationChunkHolder> chunks, ChunkAccess chunk
   ) {
      return CompletableFuture.completedFuture(chunk);
   }

   public static CompletableFuture<ChunkAccess> generateStructureStarts(
      WorldGenContext context, ChunkStep step, StaticCache2D<GenerationChunkHolder> chunks, ChunkAccess chunk
   ) {
      ServerLevel level = context.level();
      if (level.worldGenSettings.options().generateStructures()) {
         context.generator()
            .createStructures(
               level.registryAccess(),
               level.getChunkSource().getGeneratorState(),
               level.structureManager(),
               chunk,
               context.structureManager(),
               level.dimension()
            );
      }

      level.onStructureStartsAvailable(chunk);
      return CompletableFuture.completedFuture(chunk);
   }

   public static CompletableFuture<ChunkAccess> loadStructureStarts(
      WorldGenContext context, ChunkStep step, StaticCache2D<GenerationChunkHolder> cache, ChunkAccess chunk
   ) {
      context.level().onStructureStartsAvailable(chunk);
      return CompletableFuture.completedFuture(chunk);
   }

   public static CompletableFuture<ChunkAccess> generateStructureReferences(
      WorldGenContext context, ChunkStep step, StaticCache2D<GenerationChunkHolder> chunks, ChunkAccess chunk
   ) {
      ServerLevel level = context.level();
      WorldGenRegion region = new WorldGenRegion(level, chunks, step, chunk);
      context.generator().createReferences(region, level.structureManager().forWorldGenRegion(region), chunk);
      return CompletableFuture.completedFuture(chunk);
   }

   public static CompletableFuture<ChunkAccess> generateBiomes(
      WorldGenContext context, ChunkStep step, StaticCache2D<GenerationChunkHolder> chunks, ChunkAccess chunk
   ) {
      ServerLevel level = context.level();
      WorldGenRegion region = new WorldGenRegion(level, chunks, step, chunk);
      return context.generator()
         .createBiomes(level.getChunkSource().randomState(), Blender.of(region), level.structureManager().forWorldGenRegion(region), chunk);
   }

   private static Set<Holder<Biome>> collectPossibleBiomes(WorldGenRegion region, int chunkRadius) {
      Set<Holder<Biome>> chunkBiomes = new ObjectOpenHashSet();
      ChunkPos center = region.getCenter();

      for (int z = center.z() - chunkRadius; z <= center.z() + chunkRadius; z++) {
         for (int x = center.x() - chunkRadius; x <= center.x() + chunkRadius; x++) {
            region.getChunk(x, z).collectBiomesInPalette(chunkBiomes);
         }
      }

      return chunkBiomes;
   }

   public static CompletableFuture<ChunkAccess> buildTerrain(
      WorldGenContext context, ChunkStep step, StaticCache2D<GenerationChunkHolder> chunks, ChunkAccess chunk
   ) {
      ServerLevel level = context.level();
      WorldGenRegion region = new WorldGenRegion(level, chunks, step, chunk);
      Set<Holder<Biome>> possibleBiomes = collectPossibleBiomes(region, 1);
      return context.generator()
         .buildTerrain(
            chunk,
            Blender.of(region),
            level.getChunkSource().randomState(),
            level.structureManager().forWorldGenRegion(region),
            region.getBiomeManager(),
            region,
            possibleBiomes
         )
         .thenApply(
            generatedChunk -> {
               if (generatedChunk instanceof ProtoChunk protoChunk) {
                  BelowZeroRetrogen belowZeroRetrogen = protoChunk.getBelowZeroRetrogen();
                  if (belowZeroRetrogen != null) {
                     BelowZeroRetrogen.replaceOldBedrock(protoChunk);
                     if (belowZeroRetrogen.hasBedrockHoles()) {
                        belowZeroRetrogen.applyBedrockMask(protoChunk);
                     }
                  }
               }

               Heightmap.primeHeightmaps(
                  generatedChunk,
                  EnumSet.of(
                     Heightmap.Types.MOTION_BLOCKING, Heightmap.Types.MOTION_BLOCKING_NO_LEAVES, Heightmap.Types.OCEAN_FLOOR, Heightmap.Types.WORLD_SURFACE
                  )
               );
               return (ChunkAccess)generatedChunk;
            }
         );
   }

   public static CompletableFuture<ChunkAccess> generateFeatures(
      WorldGenContext context, ChunkStep step, StaticCache2D<GenerationChunkHolder> chunks, ChunkAccess chunk
   ) {
      ServerLevel level = context.level();
      WorldGenRegion region = new WorldGenRegion(level, chunks, step, chunk);
      if (!SharedConstants.DEBUG_DISABLE_FEATURES) {
         context.generator().applyBiomeDecoration(region, chunk, level.structureManager().forWorldGenRegion(region));
      }

      Blender.generateBorderTicks(region, chunk);
      return CompletableFuture.completedFuture(chunk);
   }

   public static CompletableFuture<ChunkAccess> initializeLight(
      WorldGenContext context, ChunkStep step, StaticCache2D<GenerationChunkHolder> chunks, ChunkAccess chunk
   ) {
      ThreadedLevelLightEngine lightEngine = context.lightEngine();
      chunk.initializeLightSources();
      ((ProtoChunk)chunk).setLightEngine(lightEngine);
      boolean lighted = isLighted(chunk);
      return lightEngine.initializeLight(chunk, lighted);
   }

   public static CompletableFuture<ChunkAccess> light(WorldGenContext context, ChunkStep step, StaticCache2D<GenerationChunkHolder> chunks, ChunkAccess chunk) {
      boolean lighted = isLighted(chunk);
      return context.lightEngine().lightChunk(chunk, lighted);
   }

   public static CompletableFuture<ChunkAccess> generateSpawn(
      WorldGenContext context, ChunkStep step, StaticCache2D<GenerationChunkHolder> chunks, ChunkAccess chunk
   ) {
      if (!chunk.isUpgrading()) {
         context.generator().spawnOriginalMobs(new WorldGenRegion(context.level(), chunks, step, chunk));
      }

      return CompletableFuture.completedFuture(chunk);
   }

   public static CompletableFuture<ChunkAccess> full(WorldGenContext context, ChunkStep step, StaticCache2D<GenerationChunkHolder> chunks, ChunkAccess chunk) {
      ChunkPos pos = chunk.getPos();
      GenerationChunkHolder holder = chunks.get(pos.x(), pos.z());
      return CompletableFuture.supplyAsync(() -> {
         ProtoChunk protoChunk = (ProtoChunk)chunk;
         ServerLevel level = context.level();
         LevelChunk levelChunk;
         if (protoChunk instanceof ImposterProtoChunk imposter) {
            levelChunk = imposter.getWrapped();
         } else {
            levelChunk = new LevelChunk(level, protoChunk, lc -> {
               try (ScopedCollector reporter = new ScopedCollector(chunk.problemPath(), LOGGER)) {
                  postLoadProtoChunk(level, TagValueInput.create(reporter, level.registryAccess(), protoChunk.getEntities()), protoChunk.getPos());
               }
            });
            holder.replaceProtoChunk(new ImposterProtoChunk(levelChunk, false));
         }

         levelChunk.setFullStatus(holder::getFullStatus);
         levelChunk.runPostLoad();
         levelChunk.setLoaded(true);
         levelChunk.registerAllBlockEntitiesAfterLevelLoad();
         levelChunk.registerTickContainerInLevel(level);
         levelChunk.setUnsavedListener(context.unsavedListener());
         return levelChunk;
      }, context.mainThreadExecutor());
   }

   public static void postLoadProtoChunk(ServerLevel level, ValueInputList entities, ChunkPos pos) {
      if (!entities.isEmpty()) {
         level.addWorldGenChunkEntities(
            EntityType.loadEntitiesRecursive(entities, level, EntitySpawnReason.LOAD).filter(entity -> !checkDupeUUID(level, entity)), pos
         );
      }
   }

   public static boolean checkDupeUUID(ServerLevel level, Entity entity) {
      DuplicateUUIDMode mode = level.paperConfig().entities.spawning.duplicateUuid.mode;
      if (mode != DuplicateUUIDMode.WARN && mode != DuplicateUUIDMode.DELETE && mode != DuplicateUUIDMode.SAFE_REGEN) {
         return false;
      } else {
         Entity other = level.getEntity(entity.getUUID());
         if (other != null && other != entity) {
            if (mode == DuplicateUUIDMode.SAFE_REGEN
               && other != null
               && !other.isRemoved()
               && Objects.equals(other.getEncodeId(), entity.getEncodeId())
               && entity.getBukkitEntity().getLocation().distance(other.getBukkitEntity().getLocation())
                  < (double)level.paperConfig().entities.spawning.duplicateUuid.safeRegenDeleteRange) {
               entity.discard(null);
               return true;
            } else {
               if (!other.isRemoved()) {
                  switch (mode) {
                     case SAFE_REGEN:
                        entity.setUUID(UUID.randomUUID());
                        break;
                     case DELETE:
                        entity.discard(Cause.DISCARD);
                        return true;
                  }
               }

               return false;
            }
         } else {
            return false;
         }
      }
   }
}
