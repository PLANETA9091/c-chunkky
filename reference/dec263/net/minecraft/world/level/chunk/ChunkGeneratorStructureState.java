package net.minecraft.world.level.chunk;

import com.google.common.base.Stopwatch;
import com.mojang.datafixers.util.Pair;
import com.mojang.logging.LogUtils;
import it.unimi.dsi.fastutil.objects.Object2ObjectArrayMap;
import it.unimi.dsi.fastutil.objects.Object2ObjectOpenHashMap;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.Optional;
import java.util.Set;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.TimeUnit;
import java.util.stream.Collectors;
import java.util.stream.Stream;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Holder;
import net.minecraft.core.HolderLookup;
import net.minecraft.core.HolderSet;
import net.minecraft.core.SectionPos;
import net.minecraft.core.Vec3i;
import net.minecraft.resources.Identifier;
import net.minecraft.resources.ResourceKey;
import net.minecraft.util.RandomSource;
import net.minecraft.util.Util;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.biome.Biome;
import net.minecraft.world.level.biome.BiomeSource;
import net.minecraft.world.level.levelgen.RandomState;
import net.minecraft.world.level.levelgen.structure.BuiltinStructureSets;
import net.minecraft.world.level.levelgen.structure.Structure;
import net.minecraft.world.level.levelgen.structure.StructureSet;
import net.minecraft.world.level.levelgen.structure.placement.AbstractSpreadingStructurePlacement;
import net.minecraft.world.level.levelgen.structure.placement.ConcentricRingsStructurePlacement;
import net.minecraft.world.level.levelgen.structure.placement.RandomSpreadStructurePlacement;
import net.minecraft.world.level.levelgen.structure.placement.RandomSpreadType;
import net.minecraft.world.level.levelgen.structure.placement.StructurePlacement;
import org.jspecify.annotations.Nullable;
import org.slf4j.Logger;
import org.spigotmc.SpigotWorldConfig;

public class ChunkGeneratorStructureState {
   private static final Logger LOGGER = LogUtils.getLogger();
   private final RandomState randomState;
   private final BiomeSource biomeSource;
   private final long levelSeed;
   private final long concentricRingsSeed;
   private final Map<Structure, List<StructurePlacement>> placementsForStructure = new Object2ObjectOpenHashMap();
   private final Map<ConcentricRingsStructurePlacement, CompletableFuture<List<ChunkPos>>> ringPositions = new Object2ObjectArrayMap();
   private boolean hasGeneratedPositions;
   private final ChunkPos origin;
   private final List<Holder<StructureSet>> possibleStructureSets;
   public final SpigotWorldConfig config;

   public static ChunkGeneratorStructureState createForFlat(
      RandomState randomState,
      long levelSeed,
      ChunkPos origin,
      BiomeSource biomeSource,
      Stream<Holder<StructureSet>> structureOverrides,
      SpigotWorldConfig config
   ) {
      List<Holder<StructureSet>> structures = structureOverrides.filter(structureSet -> hasBiomesForStructureSet(structureSet.value(), biomeSource)).toList();
      return new ChunkGeneratorStructureState(randomState, biomeSource, levelSeed, origin, 0L, injectSpigot(structures, config), config);
   }

   public static ChunkGeneratorStructureState createForNormal(
      RandomState randomState, long levelSeed, ChunkPos origin, BiomeSource biomeSource, HolderLookup<StructureSet> allStructures, SpigotWorldConfig config
   ) {
      List<Holder<StructureSet>> structures = allStructures.listElements()
         .filter(structureSet -> hasBiomesForStructureSet(structureSet.value(), biomeSource))
         .collect(Collectors.toUnmodifiableList());
      return new ChunkGeneratorStructureState(randomState, biomeSource, levelSeed, origin, levelSeed, injectSpigot(structures, config), config);
   }

   private static List<Holder<StructureSet>> injectSpigot(List<Holder<StructureSet>> structures, SpigotWorldConfig config) {
      return structures.stream()
         .map(
            structureSet -> {
               StructureSet structureset = structureSet.value();
               Identifier id = structureSet.unwrapKey().orElseThrow().identifier();
               if (structureset.placement() instanceof RandomSpreadStructurePlacement randomConfig && id.getNamespace().equals("minecraft")) {
                  String name = id.getPath();

                  int seed = switch (name) {
                     case "villages" -> config.villageSeed;
                     case "desert_pyramids" -> config.desertSeed;
                     case "igloos" -> config.iglooSeed;
                     case "jungle_temples" -> config.jungleSeed;
                     case "swamp_huts" -> config.swampSeed;
                     case "ocean_monuments" -> config.monumentSeed;
                     case "shipwrecks" -> config.shipwreckSeed;
                     case "ocean_ruins" -> config.oceanSeed;
                     case "pillager_outposts" -> config.outpostSeed;
                     case "end_cities" -> config.endCitySeed;
                     case "nether_complexes" -> config.netherSeed;
                     case "woodland_mansions" -> config.mansionSeed;
                     case "nether_fossils" -> config.fossilSeed;
                     case "ruined_portals" -> config.portalSeed;
                     case "ancient_cities" -> config.ancientCitySeed;
                     case "trail_ruins" -> config.trailRuinsSeed;
                     case "trial_chambers" -> config.trialChambersSeed;
                     case "abandoned_camp" -> config.abandonedCampSeed;
                     default -> randomConfig.salt;
                  };
                  structureset = new StructureSet(
                     structureset.structures(),
                     new ChunkGeneratorStructureState.KeyedRandomSpreadStructurePlacement(
                        structureSet.unwrapKey().orElseThrow(),
                        randomConfig.locateOffset,
                        randomConfig.frequencyReductionMethod,
                        randomConfig.frequency,
                        seed,
                        randomConfig.exclusionZone,
                        randomConfig.spacing(),
                        randomConfig.separation(),
                        randomConfig.spreadType()
                     )
                  );
                  return Holder.direct(structureset);
               }

               return structureSet;
            }
         )
         .collect(Collectors.toUnmodifiableList());
   }

   private static boolean hasBiomesForStructureSet(StructureSet structureSet, BiomeSource biomeSource) {
      Stream<Holder<Biome>> structureBiomes = structureSet.structures().stream().flatMap(entry -> {
         Structure structure = entry.structure().value();
         return structure.biomes().stream();
      });
      return structureBiomes.anyMatch(biomeSource.possibleBiomes()::contains);
   }

   private ChunkGeneratorStructureState(
      RandomState randomState,
      BiomeSource biomeSource,
      long levelSeed,
      ChunkPos origin,
      long concentricRingsSeed,
      List<Holder<StructureSet>> possibleStructureSets,
      SpigotWorldConfig config
   ) {
      this.randomState = randomState;
      this.levelSeed = levelSeed;
      this.origin = origin;
      this.biomeSource = biomeSource;
      this.concentricRingsSeed = concentricRingsSeed;
      this.possibleStructureSets = possibleStructureSets;
      this.config = config;
   }

   public List<Holder<StructureSet>> possibleStructureSets() {
      return this.possibleStructureSets;
   }

   private void generatePositions() {
      Set<Holder<Biome>> possibleBiomes = this.biomeSource.possibleBiomes();
      this.possibleStructureSets().forEach(setHolder -> {
         StructureSet set = setHolder.value();
         boolean hasAnyPlaceableStructures = false;

         for (StructureSet.StructureSelectionEntry entry : set.structures()) {
            Structure structure = entry.structure().value();
            if (structure.biomes().stream().anyMatch(possibleBiomes::contains)) {
               this.placementsForStructure.computeIfAbsent(structure, s -> new ArrayList<>()).add(set.placement());
               hasAnyPlaceableStructures = true;
            }
         }

         if (hasAnyPlaceableStructures && set.placement() instanceof ConcentricRingsStructurePlacement ringsPlacement) {
            this.ringPositions.put(ringsPlacement, this.generateRingPositions((Holder<StructureSet>)setHolder, ringsPlacement));
         }
      });
   }

   private CompletableFuture<List<ChunkPos>> generateRingPositions(Holder<StructureSet> structureSet, ConcentricRingsStructurePlacement placement) {
      if (placement.count() == 0) {
         return CompletableFuture.completedFuture(List.of());
      } else {
         Stopwatch stopwatch = Stopwatch.createStarted(Util.TICKER);
         int distance = placement.distance();
         int count = placement.count();
         List<CompletableFuture<ChunkPos>> tasks = new ArrayList<>(count);
         int spread = placement.spread();
         HolderSet<Biome> preferredBiomes = placement.preferredBiomes();
         RandomSource random = RandomSource.create();
         if (this.config.strongholdSeed != null && structureSet.is(BuiltinStructureSets.STRONGHOLDS)) {
            random.setSeed(this.config.strongholdSeed);
         } else {
            random.setSeed(this.concentricRingsSeed);
         }

         double angle = random.nextDouble() * Math.PI * 2.0;
         int positionInCircle = 0;
         int circle = 0;

         for (int i = 0; i < count; i++) {
            double dist = (double)(4 * distance + distance * circle * 6) + (random.nextDouble() - 0.5) * (double)distance * 2.5;
            int initialX = (int)Math.round(Math.cos(angle) * dist);
            int initialZ = (int)Math.round(Math.sin(angle) * dist);
            RandomSource biomeSearchGenerator = random.fork();
            tasks.add(
               CompletableFuture.supplyAsync(
                  () -> {
                     Pair<BlockPos, Holder<Biome>> closestBiome = this.biomeSource
                        .findBiomeHorizontal(
                           SectionPos.sectionToBlockCoord(initialX, 8),
                           0,
                           SectionPos.sectionToBlockCoord(initialZ, 8),
                           112,
                           preferredBiomes::contains,
                           biomeSearchGenerator,
                           this.randomState
                        );
                     if (closestBiome != null) {
                        BlockPos position = (BlockPos)closestBiome.getFirst();
                        return new ChunkPos(SectionPos.blockToSectionCoord(position.getX()), SectionPos.blockToSectionCoord(position.getZ()));
                     } else {
                        return new ChunkPos(initialX, initialZ);
                     }
                  },
                  Util.backgroundExecutor().forName("structureRings")
               )
            );
            angle += (Math.PI * 2) / (double)spread;
            if (++positionInCircle == spread) {
               circle++;
               positionInCircle = 0;
               spread += 2 * spread / (circle + 1);
               spread = Math.min(spread, count - i);
               angle += random.nextDouble() * Math.PI * 2.0;
            }
         }

         return Util.sequence(tasks).thenApply(ringPositions -> {
            double elapsedSeconds = (double)stopwatch.stop().elapsed(TimeUnit.MILLISECONDS) / 1000.0;
            LOGGER.debug("Calculation for {} took {}s", structureSet, elapsedSeconds);
            return ringPositions;
         });
      }
   }

   public ChunkPos getDimensionOrigin() {
      return this.origin;
   }

   public void ensureStructuresGenerated() {
      if (!this.hasGeneratedPositions) {
         this.generatePositions();
         this.hasGeneratedPositions = true;
      }
   }

   @Nullable
   public List<ChunkPos> getRingPositionsFor(ConcentricRingsStructurePlacement placement) {
      this.ensureStructuresGenerated();
      CompletableFuture<List<ChunkPos>> result = this.ringPositions.get(placement);
      return result != null ? result.join() : null;
   }

   public List<StructurePlacement> getPlacementsForStructure(Holder<Structure> structure) {
      this.ensureStructuresGenerated();
      return this.placementsForStructure.getOrDefault(structure.value(), List.of());
   }

   public RandomState randomState() {
      return this.randomState;
   }

   public boolean hasStructureChunkInRange(Holder<StructureSet> structureSet, int sourceX, int sourceZ, int range) {
      StructurePlacement placement = structureSet.value().placement();

      for (int testX = sourceX - range; testX <= sourceX + range; testX++) {
         for (int testZ = sourceZ - range; testZ <= sourceZ + range; testZ++) {
            if (placement.isStructureChunk(
               this, testX, testZ, placement instanceof ChunkGeneratorStructureState.KeyedRandomSpreadStructurePlacement keyed ? keyed.key : null
            )) {
               return true;
            }
         }
      }

      return false;
   }

   public long getLevelSeed() {
      return this.levelSeed;
   }

   public static final class KeyedRandomSpreadStructurePlacement extends RandomSpreadStructurePlacement {
      public final ResourceKey<StructureSet> key;

      public KeyedRandomSpreadStructurePlacement(
         ResourceKey<StructureSet> key,
         Vec3i locateOffset,
         AbstractSpreadingStructurePlacement.FrequencyReductionMethod frequencyReductionMethod,
         float frequency,
         int salt,
         Optional<AbstractSpreadingStructurePlacement.ExclusionZone> exclusionZone,
         int spacing,
         int separation,
         RandomSpreadType spreadType
      ) {
         super(locateOffset, frequencyReductionMethod, frequency, salt, exclusionZone, spacing, separation, spreadType);
         this.key = key;
      }
   }
}
