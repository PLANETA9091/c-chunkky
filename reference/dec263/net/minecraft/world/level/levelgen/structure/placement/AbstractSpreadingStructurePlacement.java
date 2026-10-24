package net.minecraft.world.level.levelgen.structure.placement;

import com.mojang.datafixers.Products.P5;
import com.mojang.serialization.Codec;
import com.mojang.serialization.MapCodec;
import com.mojang.serialization.codecs.RecordCodecBuilder;
import com.mojang.serialization.codecs.RecordCodecBuilder.Instance;
import com.mojang.serialization.codecs.RecordCodecBuilder.Mu;
import java.util.Optional;
import net.minecraft.core.Holder;
import net.minecraft.core.Vec3i;
import net.minecraft.core.registries.Registries;
import net.minecraft.core.registries.codec.RegistryCodecs;
import net.minecraft.resources.ResourceKey;
import net.minecraft.util.ExtraCodecs;
import net.minecraft.util.StringRepresentable;
import net.minecraft.world.level.chunk.ChunkGeneratorStructureState;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.level.levelgen.WorldgenRandom;
import net.minecraft.world.level.levelgen.structure.BuiltinStructureSets;
import net.minecraft.world.level.levelgen.structure.StructureSet;
import org.jspecify.annotations.Nullable;

public abstract class AbstractSpreadingStructurePlacement implements StructurePlacement {
   private static final int HIGHLY_ARBITRARY_RANDOM_SALT = 10387320;
   public final Vec3i locateOffset;
   public final AbstractSpreadingStructurePlacement.FrequencyReductionMethod frequencyReductionMethod;
   public final float frequency;
   public final int salt;
   public final Optional<AbstractSpreadingStructurePlacement.ExclusionZone> exclusionZone;

   protected static <S extends AbstractSpreadingStructurePlacement> P5<Mu<S>, Vec3i, AbstractSpreadingStructurePlacement.FrequencyReductionMethod, Float, Integer, Optional<AbstractSpreadingStructurePlacement.ExclusionZone>> placementCodec(
      Instance<S> i
   ) {
      return i.group(
         Vec3i.offsetCodec(16).optionalFieldOf("locate_offset", Vec3i.ZERO).forGetter(AbstractSpreadingStructurePlacement::locateOffset),
         AbstractSpreadingStructurePlacement.FrequencyReductionMethod.CODEC
            .optionalFieldOf("frequency_reduction_method", AbstractSpreadingStructurePlacement.FrequencyReductionMethod.DEFAULT)
            .forGetter(AbstractSpreadingStructurePlacement::frequencyReductionMethod),
         Codec.floatRange(0.0F, 1.0F).optionalFieldOf("frequency", 1.0F).forGetter(AbstractSpreadingStructurePlacement::frequency),
         ExtraCodecs.NON_NEGATIVE_INT.fieldOf("salt").forGetter(AbstractSpreadingStructurePlacement::salt),
         AbstractSpreadingStructurePlacement.ExclusionZone.CODEC
            .optionalFieldOf("exclusion_zone")
            .forGetter(AbstractSpreadingStructurePlacement::exclusionZone)
      );
   }

   protected AbstractSpreadingStructurePlacement(
      Vec3i locateOffset,
      AbstractSpreadingStructurePlacement.FrequencyReductionMethod frequencyReductionMethod,
      float frequency,
      int salt,
      Optional<AbstractSpreadingStructurePlacement.ExclusionZone> exclusionZone
   ) {
      this.locateOffset = locateOffset;
      this.frequencyReductionMethod = frequencyReductionMethod;
      this.frequency = frequency;
      this.salt = salt;
      this.exclusionZone = exclusionZone;
   }

   @Override
   public abstract MapCodec<? extends AbstractSpreadingStructurePlacement> codec();

   @Override
   public Vec3i locateOffset() {
      return this.locateOffset;
   }

   protected AbstractSpreadingStructurePlacement.FrequencyReductionMethod frequencyReductionMethod() {
      return this.frequencyReductionMethod;
   }

   protected float frequency() {
      return this.frequency;
   }

   protected int salt() {
      return this.salt;
   }

   protected Optional<AbstractSpreadingStructurePlacement.ExclusionZone> exclusionZone() {
      return this.exclusionZone;
   }

   @Override
   public boolean isStructureChunk(ChunkGeneratorStructureState state, int sourceX, int sourceZ, @Nullable ResourceKey<StructureSet> structureSet) {
      Integer saltOverride = null;
      if (structureSet != null) {
         if (structureSet == BuiltinStructureSets.MINESHAFTS) {
            saltOverride = state.config.mineshaftSeed;
         } else if (structureSet == BuiltinStructureSets.BURIED_TREASURES) {
            saltOverride = state.config.buriedTreasureSeed;
         }
      }

      return this.isPlacementChunk(state, sourceX, sourceZ)
         && this.applyAdditionalChunkRestrictions(sourceX, sourceZ, state.getLevelSeed(), saltOverride)
         && this.applyInteractionsWithOtherStructures(state, sourceX, sourceZ);
   }

   @Override
   public boolean applyAdditionalChunkRestrictions(int sourceX, int sourceZ, long levelSeed, @Nullable Integer saltOverride) {
      return !(this.frequency < 1.0F) || this.frequencyReductionMethod.shouldGenerate(levelSeed, this.salt, sourceX, sourceZ, this.frequency, saltOverride);
   }

   public boolean applyInteractionsWithOtherStructures(ChunkGeneratorStructureState state, int sourceX, int sourceZ) {
      return !this.exclusionZone.isPresent() || !this.exclusionZone.get().isPlacementForbidden(state, sourceX, sourceZ);
   }

   protected abstract boolean isPlacementChunk(ChunkGeneratorStructureState var1, int var2, int var3);

   private static boolean probabilityReducer(long seed, int salt, int sourceX, int sourceZ, float probability, @Nullable Integer saltOverride) {
      WorldgenRandom random = new WorldgenRandom(new LegacyRandomSource(0L));
      random.setLargeFeatureWithSalt(seed, salt, sourceX, sourceZ);
      return random.nextFloat() < probability;
   }

   private static boolean legacyProbabilityReducerWithDouble(long seed, int salt, int sourceX, int sourceZ, float probability, @Nullable Integer saltOverride) {
      WorldgenRandom random = new WorldgenRandom(new LegacyRandomSource(0L));
      if (saltOverride == null) {
         random.setLargeFeatureSeed(seed, sourceX, sourceZ);
      } else {
         random.setLargeFeatureWithSalt(seed, sourceX, sourceZ, saltOverride);
      }

      return random.nextDouble() < (double)probability;
   }

   private static boolean legacyArbitrarySaltProbabilityReducer(
      long seed, int salt, int sourceX, int sourceZ, float probability, @Nullable Integer saltOverride
   ) {
      WorldgenRandom random = new WorldgenRandom(new LegacyRandomSource(0L));
      random.setLargeFeatureWithSalt(seed, sourceX, sourceZ, saltOverride != null ? saltOverride : 10387320);
      return random.nextFloat() < probability;
   }

   private static boolean legacyPillagerOutpostReducer(long seed, int salt, int sourceX, int sourceZ, float probability, @Nullable Integer saltOverride) {
      int cx = sourceX >> 4;
      int cz = sourceZ >> 4;
      WorldgenRandom random = new WorldgenRandom(new LegacyRandomSource(0L));
      random.setSeed((long)(cx ^ cz << 4) ^ seed);
      random.nextInt();
      return random.nextInt((int)(1.0F / probability)) == 0;
   }

   @Deprecated
   public static record ExclusionZone(Holder<StructureSet> otherSet, int chunkCount) {
      public static final Codec<AbstractSpreadingStructurePlacement.ExclusionZone> CODEC = RecordCodecBuilder.create(
         i -> i.group(
                  RegistryCodecs.holder(Registries.STRUCTURE_SET, StructureSet.DIRECT_CODEC, false)
                     .fieldOf("other_set")
                     .forGetter(AbstractSpreadingStructurePlacement.ExclusionZone::otherSet),
                  Codec.intRange(1, 16).fieldOf("chunk_count").forGetter(AbstractSpreadingStructurePlacement.ExclusionZone::chunkCount)
               )
               .apply(i, AbstractSpreadingStructurePlacement.ExclusionZone::new)
      );

      private boolean isPlacementForbidden(ChunkGeneratorStructureState state, int sourceX, int sourceZ) {
         return state.hasStructureChunkInRange(this.otherSet, sourceX, sourceZ, this.chunkCount);
      }
   }

   @FunctionalInterface
   public interface FrequencyReducer {
      boolean shouldGenerate(long var1, int var3, int var4, int var5, float var6, @Nullable Integer var7);
   }

   public static enum FrequencyReductionMethod implements StringRepresentable {
      DEFAULT("default", AbstractSpreadingStructurePlacement::probabilityReducer),
      LEGACY_TYPE_1("legacy_type_1", AbstractSpreadingStructurePlacement::legacyPillagerOutpostReducer),
      LEGACY_TYPE_2("legacy_type_2", AbstractSpreadingStructurePlacement::legacyArbitrarySaltProbabilityReducer),
      LEGACY_TYPE_3("legacy_type_3", AbstractSpreadingStructurePlacement::legacyProbabilityReducerWithDouble);

      public static final Codec<AbstractSpreadingStructurePlacement.FrequencyReductionMethod> CODEC = StringRepresentable.fromEnum(
         AbstractSpreadingStructurePlacement.FrequencyReductionMethod::values
      );
      private final String name;
      private final AbstractSpreadingStructurePlacement.FrequencyReducer reducer;

      private FrequencyReductionMethod(final String name, final AbstractSpreadingStructurePlacement.FrequencyReducer reducer) {
         this.name = name;
         this.reducer = reducer;
      }

      public boolean shouldGenerate(long seed, int salt, int sourceX, int sourceZ, float probability, @Nullable Integer saltOverride) {
         return this.reducer.shouldGenerate(seed, salt, sourceX, sourceZ, probability, saltOverride);
      }

      @Override
      public String getSerializedName() {
         return this.name;
      }
   }
}
