package net.minecraft.world.level.levelgen;

import com.google.common.base.Suppliers;
import com.google.common.collect.Sets;
import com.mojang.serialization.MapCodec;
import com.mojang.serialization.codecs.RecordCodecBuilder;
import io.papermc.paper.FeatureHooks;
import java.text.DecimalFormat;
import java.text.DecimalFormatSymbols;
import java.util.List;
import java.util.Locale;
import java.util.OptionalInt;
import java.util.Set;
import java.util.concurrent.CompletableFuture;
import java.util.function.Function;
import java.util.function.Predicate;
import java.util.function.Supplier;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.core.Holder;
import net.minecraft.core.QuartPos;
import net.minecraft.core.SectionPos;
import net.minecraft.core.BlockPos.MutableBlockPos;
import net.minecraft.resources.ResourceKey;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.server.level.WorldGenRegion;
import net.minecraft.tags.BlockTags;
import net.minecraft.util.Mth;
import net.minecraft.util.profiling.Profiler;
import net.minecraft.util.profiling.ProfilerFiller;
import net.minecraft.util.profiling.Zone;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.LevelHeightAccessor;
import net.minecraft.world.level.NaturalSpawner;
import net.minecraft.world.level.NoiseColumn;
import net.minecraft.world.level.StructureManager;
import net.minecraft.world.level.biome.Biome;
import net.minecraft.world.level.biome.BiomeGenerationSettings;
import net.minecraft.world.level.biome.BiomeManager;
import net.minecraft.world.level.biome.BiomeResolver;
import net.minecraft.world.level.biome.BiomeSource;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.CarvingMask;
import net.minecraft.world.level.chunk.ChunkAccess;
import net.minecraft.world.level.chunk.ChunkGenerator;
import net.minecraft.world.level.chunk.LevelChunkSection;
import net.minecraft.world.level.levelgen.blending.Blender;
import net.minecraft.world.level.levelgen.carver.WorldCarver;
import net.minecraft.world.level.levelgen.densityfunction.DensityFunction;
import net.minecraft.world.level.levelgen.densityfunction.DensitySampler;
import net.minecraft.world.level.levelgen.densityfunction.DensitySamplerSet;
import net.minecraft.world.level.levelgen.densityfunction.DensityVolume;
import net.minecraft.world.level.levelgen.densityfunction.SamplerContext;
import net.minecraft.world.level.levelgen.densityfunction.ScopedDensityBuffer;
import net.minecraft.world.level.levelgen.material.rule.MaterialRule;
import org.apache.commons.lang3.mutable.MutableObject;
import org.bukkit.craftbukkit.generator.CustomChunkGenerator;
import org.jspecify.annotations.Nullable;

public final class NoiseBasedChunkGenerator extends ChunkGenerator {
   public static final MapCodec<NoiseBasedChunkGenerator> CODEC = RecordCodecBuilder.mapCodec(
      i -> i.group(
               BiomeSource.CODEC.fieldOf("biome_source").forGetter(g -> g.biomeSource),
               NoiseGeneratorSettings.CODEC.fieldOf("settings").forGetter(g -> g.settings)
            )
            .apply(i, i.stable(NoiseBasedChunkGenerator::new))
   );
   private static final BlockState AIR = Blocks.AIR.defaultBlockState();
   private static final DecimalFormat DEBUG_DENSITY_FORMAT = new DecimalFormat("0.000", DecimalFormatSymbols.getInstance(Locale.ROOT));
   private final Holder<NoiseGeneratorSettings> settings;
   private final Supplier<Aquifer.FluidPicker> globalFluidPicker;

   public NoiseBasedChunkGenerator(BiomeSource biomeSource, Holder<NoiseGeneratorSettings> settings) {
      super(biomeSource);
      this.settings = settings;
      this.globalFluidPicker = Suppliers.memoize(() -> createFluidPicker(settings.value()));
   }

   private static Aquifer.FluidPicker createFluidPicker(NoiseGeneratorSettings settings) {
      Aquifer.FluidStatus lavaStatus = new Aquifer.FluidStatus(-54, Blocks.LAVA.defaultBlockState());
      int seaLevel = settings.seaLevel();
      Aquifer.FluidStatus seaStatus = new Aquifer.FluidStatus(seaLevel, settings.defaultFluid());
      Aquifer.FluidStatus emptyStatus = new Aquifer.FluidStatus(-4064, Blocks.AIR.defaultBlockState());
      return (x, y, z) -> {
         if (SharedConstants.DEBUG_DISABLE_FLUID_GENERATION) {
            return emptyStatus;
         } else {
            return y < Math.min(-54, seaLevel) ? lavaStatus : seaStatus;
         }
      };
   }

   @Override
   protected BiomeResolver decorateBiomeResolver(Blender blender, ChunkAccess protoChunk, BiomeResolver biomeResolver) {
      biomeResolver = blender.getBiomeResolver(biomeResolver);
      return BelowZeroRetrogen.getBiomeResolver(biomeResolver, protoChunk);
   }

   private NoiseChunk createNoiseChunk(
      ChunkAccess chunk, StructureManager structureManager, Blender blender, RandomState randomState, NoiseSettings noiseSettings
   ) {
      Beardifier beardifier = Beardifier.forStructuresInChunk(structureManager, chunk.getPos());
      return new NoiseChunk(randomState, beardifier, this.settings.value(), this.globalFluidPicker.get(), blender, chunkVolume(chunk, noiseSettings));
   }

   private static DensityVolume chunkVolume(ChunkAccess chunk, NoiseSettings noiseSettings) {
      ChunkPos pos = chunk.getPos();
      return new DensityVolume(16, noiseSettings.height(), 16, pos.getMinBlockX(), noiseSettings.minY(), pos.getMinBlockZ());
   }

   @Override
   public ChunkPos getOrigin(RandomState randomState) {
      List<SpawnTargetPoint> spawnTarget = this.settings.value().spawnTarget();
      if (spawnTarget.isEmpty()) {
         return super.getOrigin(randomState);
      } else {
         SamplerContext samplerContext = SamplerContext.builder().enableCaches().build();
         return ChunkPos.containing(NoiseSpawnFinder.findSpawnPosition(spawnTarget, randomState.samplersWithContext(samplerContext)));
      }
   }

   @Override
   protected MapCodec<? extends ChunkGenerator> codec() {
      return CODEC;
   }

   public Holder<NoiseGeneratorSettings> generatorSettings() {
      return this.settings;
   }

   public boolean stable(ResourceKey<NoiseGeneratorSettings> expectedPreset) {
      return this.settings.is(expectedPreset);
   }

   @Override
   public int getBaseHeight(int x, int z, Heightmap.Types type, LevelHeightAccessor heightAccessor, RandomState randomState) {
      return this.iterateNoiseColumn(heightAccessor, randomState, x, z, null, type.isOpaque()).orElse(heightAccessor.getMinY());
   }

   @Override
   public NoiseColumn getBaseColumn(int x, int z, LevelHeightAccessor heightAccessor, RandomState randomState) {
      MutableObject<NoiseColumn> result = new MutableObject();
      this.iterateNoiseColumn(heightAccessor, randomState, x, z, result, null);
      return (NoiseColumn)result.get();
   }

   @Override
   public void addDebugScreenInfo(List<String> result, RandomState randomState, BlockPos feetPos, SamplerContext samplerContext) {
      List<NoiseGeneratorSettings.DebugFunctionEntry> functions = this.settings.value().debugFunctions().functions();
      if (!functions.isEmpty()) {
         DensitySamplerSet samplers = randomState.samplersWithContext(samplerContext);
         StringBuilder builder = new StringBuilder("Density ");

         for (NoiseGeneratorSettings.DebugFunctionEntry entry : functions) {
            builder.append(entry.label()).append(": ");
            builder.append(DEBUG_DENSITY_FORMAT.format((double)samplers.sampleValue(entry.function(), feetPos.getX(), feetPos.getY(), feetPos.getZ())));
            builder.append(' ');
         }

         builder.deleteCharAt(builder.length() - 1);
         result.add(builder.toString());
      }
   }

   private OptionalInt iterateNoiseColumn(
      LevelHeightAccessor heightAccessor,
      RandomState randomState,
      int blockX,
      int blockZ,
      @Nullable MutableObject<NoiseColumn> columnReference,
      @Nullable Predicate<BlockState> tester
   ) {
      NoiseSettings noiseSettings = this.settings.value().noiseSettings().clampToHeightAccessor(heightAccessor);
      if (noiseSettings.height() <= 0) {
         return OptionalInt.empty();
      } else {
         DensityVolume volume = new DensityVolume(1, noiseSettings.height(), 1, blockX, noiseSettings.minY(), blockZ);
         BlockState[] writeTo;
         if (columnReference == null) {
            writeTo = null;
         } else {
            writeTo = new BlockState[volume.sizeY()];
            columnReference.setValue(new NoiseColumn(volume.minBlockY(), writeTo));
         }

         try (NoiseChunk noiseChunk = new NoiseChunk(randomState, null, this.settings.value(), this.globalFluidPicker.get(), Blender.empty(), volume)) {
            Aquifer aquifer = noiseChunk.aquifer();
            BlockState defaultState = this.settings.value().defaultBlock();
            DensitySampler.Bound finalDensity = noiseChunk.cachingSamplers().get(this.settings.value().noiseRouter().finalDensity());

            try (ScopedDensityBuffer densityBuffer = finalDensity.sampleVolume(noiseChunk.volume())) {
               for (int y = volume.sizeY() - 1; y >= 0; y--) {
                  float density = densityBuffer.get(volume.indexUnchecked(0, y, 0));
                  int blockY = volume.blockY(y);
                  BlockState baseState = aquifer.computeSubstance(blockX, blockY, blockZ, (double)density);
                  BlockState state = baseState == null ? defaultState : baseState;
                  if (writeTo != null) {
                     writeTo[y] = state;
                  }

                  if (tester != null && tester.test(state)) {
                     return OptionalInt.of(blockY + 1);
                  }
               }
            }
         }

         return OptionalInt.empty();
      }
   }

   private void buildSurface(
      ChunkAccess protoChunk,
      NoiseChunk noiseChunk,
      RandomState randomState,
      BiomeManager biomeManager,
      Set<Holder<Biome>> possibleBiomes,
      MaterialRule materialRule,
      ServerLevel serverLevel
   ) {
      if (!SharedConstants.debugVoidTerrain(protoChunk.getPos()) && !SharedConstants.DEBUG_DISABLE_SURFACE) {
         WorldGenerationContext context = new WorldGenerationContext(this, protoChunk.getHeightAccessorForGeneration(), serverLevel);
         randomState.surfaceSystem().buildSurface(randomState, biomeManager, context, protoChunk, noiseChunk, materialRule, possibleBiomes);
      }
   }

   private void generateCarvers(
      ChunkAccess chunk,
      Blender blender,
      NoiseChunk noiseChunk,
      RandomState randomState,
      BiomeManager biomeManager,
      @Nullable WorldGenRegion carverBiomeRegion,
      MaterialRule materialRule
   ) {
      if (!SharedConstants.DEBUG_DISABLE_CARVERS && !SharedConstants.debugVoidTerrain(chunk.getPos())) {
         BiomeResolver biomeResolver = this.biomeSource.createUncachedResolver(randomState);
         WorldgenRandom random = new WorldgenRandom(new LegacyRandomSource(RandomSupport.generateUniqueSeed()));
         int range = 8;
         ChunkPos pos = chunk.getPos();
         WorldGenerationContext context = new WorldGenerationContext(this, chunk.getHeightAccessorForGeneration(), carverBiomeRegion.getMinecraftWorld());
         int protectedBlocksOnTop = chunk.isUpgrading() ? 0 : 7;
         int maxY = context.getMinGenY() + context.getGenDepth() - 1 - protectedBlocksOnTop;
         CarvingMask mask = new CarvingMask(context.getMinGenY() + 1, maxY);

         for (int dx = -8; dx <= 8; dx++) {
            for (int dz = -8; dz <= 8; dz++) {
               ChunkPos sourcePos = new ChunkPos(pos.x() + dx, pos.z() + dz);
               BiomeGenerationSettings sourceBiomeGenerationSettings;
               if (carverBiomeRegion != null) {
                  ChunkAccess carverCenterChunk = carverBiomeRegion.getChunk(sourcePos.x(), sourcePos.z());
                  sourceBiomeGenerationSettings = carverCenterChunk.carverBiome(() -> this.getBiomeGenerationSettingsForCarver(biomeResolver, sourcePos));
               } else {
                  sourceBiomeGenerationSettings = this.getBiomeGenerationSettingsForCarver(biomeResolver, sourcePos);
               }

               Iterable<Holder<WorldCarver>> carvers = sourceBiomeGenerationSettings.getCarvers();
               int index = 0;

               for (Holder<WorldCarver> carverHolder : carvers) {
                  WorldCarver carver = carverHolder.value();
                  random.setLargeFeatureSeed(randomState.seed() + (long)index, sourcePos.x(), sourcePos.z());
                  if (carver.isStartChunk(random)) {
                     carver.carve(context, random, chunk.getPos(), sourcePos, mask);
                  }

                  index++;
               }
            }
         }

         if (!mask.isEmpty()) {
            try (Zone var27 = Profiler.get().zone("applyCarvingMask")) {
               BiomeManager correctBiomeManager = biomeManager.withDifferentSource(biomeResolver);
               this.applyCarvingMask(chunk, mask, randomState, materialRule, context, noiseChunk, correctBiomeManager::getBiome, blender.getCarvingFilter());
            }
         }
      }
   }

   private void applyCarvingMask(
      ChunkAccess chunk,
      CarvingMask mask,
      RandomState randomState,
      MaterialRule materialRule,
      WorldGenerationContext context,
      NoiseChunk noiseChunk,
      Function<BlockPos, Holder<Biome>> biomeGetter,
      @Nullable CarvingMask.Filter filter
   ) {
      ChunkPos chunkPos = chunk.getPos();
      MutableBlockPos blockPos = new MutableBlockPos();
      MutableBlockPos helperPos = new MutableBlockPos();
      Aquifer aquifer = noiseChunk.aquifer();
      mask.visit(
         (x, z, bottomY, topY) -> {
            boolean hasGrass = false;
            int worldX = chunkPos.getBlockX(x);
            int worldZ = chunkPos.getBlockZ(z);

            for (int worldY = topY; worldY >= bottomY; worldY--) {
               if (filter == null || filter.test(x, worldY, z)) {
                  blockPos.set(worldX, worldY, worldZ);
                  BlockState blockState = chunk.getBlockState(blockPos);
                  if (!blockState.is(BlockTags.UNCARVABLE)) {
                     if (blockState.is(Blocks.GRASS_BLOCK) || blockState.is(Blocks.MYCELIUM)) {
                        hasGrass = true;
                     }

                     BlockState state = aquifer.computeSubstance(worldX, worldY, worldZ, 0.0);
                     if (state != null) {
                        chunk.setBlockState(blockPos, state);
                        if (aquifer.shouldScheduleFluidUpdate() && !state.getFluidState().isEmpty()) {
                           chunk.markPosForPostProcessing(blockPos);
                        }

                        if (hasGrass) {
                           helperPos.setWithOffset(blockPos, Direction.DOWN);
                           if (chunk.getBlockState(helperPos).is(Blocks.DIRT)) {
                              randomState.surfaceSystem()
                                 .topMaterial(
                                    materialRule,
                                    randomState,
                                    context,
                                    biomeGetter,
                                    chunk,
                                    noiseChunk.cachingSamplers(),
                                    helperPos,
                                    !state.getFluidState().isEmpty()
                                 )
                                 .ifPresent(topMaterial -> {
                                    chunk.setBlockState(helperPos, topMaterial);
                                    if (!topMaterial.getFluidState().isEmpty()) {
                                       chunk.markPosForPostProcessing(helperPos);
                                    }
                                 });
                           }
                        }
                     }
                  }
               }
            }
         }
      );
   }

   private BiomeGenerationSettings getBiomeGenerationSettingsForCarver(BiomeResolver biomeResolver, ChunkPos sourcePos) {
      int quartX = QuartPos.fromBlock(sourcePos.getMinBlockX());
      int quartZ = QuartPos.fromBlock(sourcePos.getMinBlockZ());
      return this.getBiomeGenerationSettings(biomeResolver.getNoiseBiome(quartX, 0, quartZ));
   }

   @Override
   public CompletableFuture<ChunkAccess> buildTerrain(
      ChunkAccess chunk,
      Blender blender,
      RandomState randomState,
      StructureManager structureManager,
      BiomeManager biomeManager,
      @Nullable WorldGenRegion carverBiomeRegion,
      Set<Holder<Biome>> possibleBiomes
   ) {
      return this.buildTerrain(chunk, blender, randomState, structureManager, biomeManager, carverBiomeRegion, possibleBiomes, null);
   }

   public CompletableFuture<ChunkAccess> buildTerrain(
      ChunkAccess chunk,
      Blender blender,
      RandomState randomState,
      StructureManager structureManager,
      BiomeManager biomeManager,
      @Nullable WorldGenRegion carverBiomeRegion,
      Set<Holder<Biome>> possibleBiomes,
      @Nullable CustomChunkGenerator customGenerator
   ) {
      NoiseSettings noiseSettings = this.settings.value().noiseSettings().clampToHeightAccessor(chunk.getHeightAccessorForGeneration());
      if (customGenerator == null || noiseSettings.height() > 0 && !SharedConstants.debugVoidTerrain(chunk.getPos())) {
         return noiseSettings.height() > 0 && !SharedConstants.debugVoidTerrain(chunk.getPos()) ? CompletableFuture.supplyAsync(() -> {
            ProfilerFiller profiler = Profiler.get();

            ChunkAccess var44;
            try (NoiseChunk noiseChunk = this.createNoiseChunk(chunk, structureManager, blender, randomState, noiseSettings)) {
               DensityVolume volume = noiseChunk.volume();
               int topSectionIndex = chunk.getSectionIndex(volume.maxBlockY());
               int bottomSectionIndex = chunk.getSectionIndex(volume.minBlockY());
               Set<LevelChunkSection> sections = Sets.newHashSet();

               for (int sectionIndex = topSectionIndex; sectionIndex >= bottomSectionIndex; sectionIndex--) {
                  LevelChunkSection section = chunk.getSection(sectionIndex);
                  section.acquire();
                  sections.add(section);
               }

               try (Zone var39x = profiler.zone("doFill")) {
                  if (customGenerator == null || customGenerator.shouldGenerateNoise(chunk)) {
                     this.doFill(noiseChunk, chunk);
                  }
               } finally {
                  for (LevelChunkSection section : sections) {
                     section.release();
                  }
               }

               if (customGenerator != null) {
                  customGenerator.fillFromNoise(chunk);
               }

               MaterialRule materialRule = this.settings.value().materialRule().value();

               try (Zone var41 = profiler.zone("buildSurface")) {
                  if (customGenerator == null || customGenerator.shouldGenerateSurface(chunk)) {
                     this.buildSurface(chunk, noiseChunk, randomState, biomeManager, possibleBiomes, materialRule, structureManager.level.getMinecraftWorld());
                  }
               }

               if (customGenerator != null) {
                  customGenerator.buildSurface(chunk);
               }

               try (Zone var42 = profiler.zone("generateCarvers")) {
                  if (customGenerator == null || customGenerator.shouldGenerateCaves(chunk)) {
                     this.generateCarvers(chunk, blender, noiseChunk, randomState, biomeManager, carverBiomeRegion, materialRule);
                  }
               }

               if (customGenerator != null) {
                  customGenerator.applyCarvers(chunk);
               }

               var44 = chunk;
            }

            return var44;
         }, Runnable::run) : CompletableFuture.completedFuture(chunk);
      } else {
         return CompletableFuture.supplyAsync(() -> {
            customGenerator.fillFromNoise(chunk);
            customGenerator.buildSurface(chunk);
            customGenerator.applyCarvers(chunk);
            return chunk;
         }, FeatureHooks.getWorldgenExecutor());
      }
   }

   private void doFill(NoiseChunk noiseChunk, ChunkAccess chunk) {
      Heightmap oceanFloor = chunk.getOrCreateHeightmapUnprimed(Heightmap.Types.OCEAN_FLOOR_WG);
      Heightmap worldSurface = chunk.getOrCreateHeightmapUnprimed(Heightmap.Types.WORLD_SURFACE_WG);
      Aquifer aquifer = noiseChunk.aquifer();
      MutableBlockPos blockPos = new MutableBlockPos();
      DensityVolume volume = noiseChunk.volume();
      DensitySampler.Bound finalDensity = noiseChunk.cachingSamplers().get(this.settings.value().noiseRouter().finalDensity());

      try (ScopedDensityBuffer densityBuffer = finalDensity.sampleVolume(volume)) {
         for (int z = 0; z < volume.sizeZ(); z++) {
            int blockZ = volume.blockZ(z);

            for (int x = 0; x < volume.sizeX(); x++) {
               int blockX = volume.blockX(x);

               for (int y = volume.sizeY() - 1; y >= 0; y--) {
                  int blockY = volume.blockY(y);
                  LevelChunkSection section = chunk.getSection(chunk.getSectionIndex(blockY));
                  float density = densityBuffer.get(volume.indexUnchecked(x, y, z));
                  BlockState state = aquifer.computeSubstance(blockX, blockY, blockZ, (double)density);
                  if (state == null) {
                     state = this.settings.value().defaultBlock();
                  }

                  state = this.debugPreliminarySurfaceLevel(noiseChunk, blockX, blockY, blockZ, state);
                  if (state != AIR) {
                     section.setBlockState(x, SectionPos.sectionRelative(blockY), z, state, false);
                     oceanFloor.update(x, blockY, z, state);
                     worldSurface.update(x, blockY, z, state);
                     if (aquifer.shouldScheduleFluidUpdate() && !state.getFluidState().isEmpty()) {
                        blockPos.set(blockX, blockY, blockZ);
                        chunk.markPosForPostProcessing(blockPos);
                     }
                  }
               }
            }
         }
      }
   }

   private BlockState debugPreliminarySurfaceLevel(NoiseChunk noiseChunk, int posX, int posY, int posZ, BlockState state) {
      if (SharedConstants.DEBUG_AQUIFERS && posZ >= 0 && posZ % 4 == 0) {
         DensityFunction surfaceLevelFunction = this.settings.value().aquifers().map(Aquifer.Config::surfaceLevel).orElse(null);
         if (surfaceLevelFunction == null) {
            return state;
         }

         int preliminarySurfaceLevel = Mth.floor(noiseChunk.cachingSamplers().sampleValue(surfaceLevelFunction, posX, 0, posZ));
         int adjustedSurfaceLevel = preliminarySurfaceLevel + 8;
         if (posY == adjustedSurfaceLevel) {
            state = adjustedSurfaceLevel < this.getSeaLevel() ? Blocks.SLIME_BLOCK.defaultBlockState() : Blocks.HONEY_BLOCK.defaultBlockState();
         }
      }

      return state;
   }

   @Override
   public int getGenDepth() {
      return this.settings.value().noiseSettings().height();
   }

   @Override
   public int getSeaLevel() {
      return this.settings.value().seaLevel();
   }

   @Override
   public int getMinY() {
      return this.settings.value().noiseSettings().minY();
   }

   @Override
   public void spawnOriginalMobs(WorldGenRegion worldGenRegion) {
      if (!this.settings.value().disableMobGeneration()) {
         ChunkPos center = worldGenRegion.getCenter();
         BlockPos sourcePos = center.getWorldPosition().atY(worldGenRegion.getMaxY());
         WorldgenRandom random = new WorldgenRandom(new LegacyRandomSource(RandomSupport.generateUniqueSeed()));
         random.setDecorationSeed(worldGenRegion.getSeed(), center.getMinBlockX(), center.getMinBlockZ());
         NaturalSpawner.spawnMobsForChunkGeneration(worldGenRegion, sourcePos, center, random);
      }
   }
}
