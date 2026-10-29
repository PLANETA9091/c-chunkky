package net.minecraft.world.level.levelgen;

import com.mojang.serialization.MapCodec;
import com.mojang.serialization.codecs.RecordCodecBuilder;
import java.util.List;
import java.util.Set;
import java.util.concurrent.CompletableFuture;
import java.util.stream.Stream;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Holder;
import net.minecraft.core.HolderLookup;
import net.minecraft.core.HolderSet;
import net.minecraft.core.BlockPos.MutableBlockPos;
import net.minecraft.server.level.WorldGenRegion;
import net.minecraft.util.Util;
import net.minecraft.world.level.LevelHeightAccessor;
import net.minecraft.world.level.NoiseColumn;
import net.minecraft.world.level.StructureManager;
import net.minecraft.world.level.biome.Biome;
import net.minecraft.world.level.biome.BiomeManager;
import net.minecraft.world.level.biome.BiomeSource;
import net.minecraft.world.level.biome.FixedBiomeSource;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.ChunkAccess;
import net.minecraft.world.level.chunk.ChunkGenerator;
import net.minecraft.world.level.chunk.ChunkGeneratorStructureState;
import net.minecraft.world.level.levelgen.blending.Blender;
import net.minecraft.world.level.levelgen.densityfunction.SamplerContext;
import net.minecraft.world.level.levelgen.flat.FlatLevelGeneratorSettings;
import net.minecraft.world.level.levelgen.structure.StructureSet;
import org.jspecify.annotations.Nullable;
import org.spigotmc.SpigotWorldConfig;

public class FlatLevelSource extends ChunkGenerator {
   public static final MapCodec<FlatLevelSource> CODEC = RecordCodecBuilder.mapCodec(
      i -> i.group(FlatLevelGeneratorSettings.CODEC.fieldOf("settings").forGetter(FlatLevelSource::settings)).apply(i, i.stable(FlatLevelSource::new))
   );
   private final FlatLevelGeneratorSettings settings;

   public FlatLevelSource(FlatLevelGeneratorSettings generatorSettings) {
      this(generatorSettings, new FixedBiomeSource(generatorSettings.getBiome()));
   }

   public FlatLevelSource(FlatLevelGeneratorSettings generatorSettings, BiomeSource biomeSource) {
      super(biomeSource, Util.memoize(generatorSettings::adjustGenerationSettings));
      this.settings = generatorSettings;
   }

   @Override
   public ChunkGeneratorStructureState createState(HolderLookup<StructureSet> structureSets, RandomState randomState, long levelSeed, SpigotWorldConfig config) {
      Stream<Holder<StructureSet>> structures = this.settings
         .structureOverrides()
         .map(HolderSet::stream)
         .orElseGet(() -> structureSets.listElements().map(e -> (Holder<StructureSet>)e));
      return ChunkGeneratorStructureState.createForFlat(randomState, levelSeed, this.getOrigin(randomState), this.biomeSource, structures, config);
   }

   @Override
   protected MapCodec<? extends ChunkGenerator> codec() {
      return CODEC;
   }

   public FlatLevelGeneratorSettings settings() {
      return this.settings;
   }

   @Override
   public int getSpawnHeight(LevelHeightAccessor heightAccessor) {
      return heightAccessor.getMinY() + Math.min(heightAccessor.getHeight(), this.settings.getLayers().size());
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
      List<BlockState> layers = this.settings.getLayers();
      MutableBlockPos blockPos = new MutableBlockPos();
      Heightmap oceanFloor = chunk.getOrCreateHeightmapUnprimed(Heightmap.Types.OCEAN_FLOOR_WG);
      Heightmap worldSurface = chunk.getOrCreateHeightmapUnprimed(Heightmap.Types.WORLD_SURFACE_WG);

      for (int layerIndex = 0; layerIndex < Math.min(chunk.getHeight(), layers.size()); layerIndex++) {
         BlockState blockState = layers.get(layerIndex);
         if (blockState != null) {
            int y = chunk.getMinY() + layerIndex;

            for (int x = 0; x < 16; x++) {
               for (int z = 0; z < 16; z++) {
                  chunk.setBlockState(blockPos.set(x, y, z), blockState);
                  oceanFloor.update(x, y, z, blockState);
                  worldSurface.update(x, y, z, blockState);
               }
            }
         }
      }

      return CompletableFuture.completedFuture(chunk);
   }

   @Override
   public int getBaseHeight(int x, int z, Heightmap.Types type, LevelHeightAccessor heightAccessor, RandomState randomState) {
      List<BlockState> layers = this.settings.getLayers();

      for (int layerIndex = Math.min(layers.size() - 1, heightAccessor.getMaxY()); layerIndex >= 0; layerIndex--) {
         BlockState state = layers.get(layerIndex);
         if (state != null && type.isOpaque().test(state)) {
            return heightAccessor.getMinY() + layerIndex + 1;
         }
      }

      return heightAccessor.getMinY();
   }

   @Override
   public NoiseColumn getBaseColumn(int x, int z, LevelHeightAccessor heightAccessor, RandomState randomState) {
      return new NoiseColumn(
         heightAccessor.getMinY(),
         this.settings
            .getLayers()
            .stream()
            .limit((long)heightAccessor.getHeight())
            .map(state -> state == null ? Blocks.AIR.defaultBlockState() : state)
            .toArray(BlockState[]::new)
      );
   }

   @Override
   public void addDebugScreenInfo(List<String> result, RandomState randomState, BlockPos feetPos, SamplerContext samplerContext) {
   }

   @Override
   public void spawnOriginalMobs(WorldGenRegion worldGenRegion) {
   }

   @Override
   public int getMinY() {
      return 0;
   }

   @Override
   public int getGenDepth() {
      return 384;
   }

   @Override
   public int getSeaLevel() {
      return -63;
   }
}
