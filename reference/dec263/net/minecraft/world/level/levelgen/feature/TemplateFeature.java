package net.minecraft.world.level.levelgen.feature;

import com.mojang.serialization.Codec;
import com.mojang.serialization.MapCodec;
import com.mojang.serialization.codecs.RecordCodecBuilder;
import java.util.List;
import java.util.Optional;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Holder;
import net.minecraft.core.Vec3i;
import net.minecraft.core.Direction.Axis;
import net.minecraft.resources.Identifier;
import net.minecraft.util.RandomSource;
import net.minecraft.util.Util;
import net.minecraft.util.random.WeightedList;
import net.minecraft.world.level.WorldGenLevel;
import net.minecraft.world.level.block.Rotation;
import net.minecraft.world.level.chunk.ChunkGenerator;
import net.minecraft.world.level.levelgen.structure.templatesystem.StructurePlaceSettings;
import net.minecraft.world.level.levelgen.structure.templatesystem.StructureProcessor;
import net.minecraft.world.level.levelgen.structure.templatesystem.StructureProcessorList;
import net.minecraft.world.level.levelgen.structure.templatesystem.StructureProcessorType;
import net.minecraft.world.level.levelgen.structure.templatesystem.StructureTemplate;
import net.minecraft.world.level.levelgen.structure.templatesystem.StructureTemplateManager;

public record TemplateFeature(WeightedList<TemplateFeature.TemplateEntry> templates, Optional<Holder<StructureProcessorList>> processors) implements Feature {
   public static final MapCodec<TemplateFeature> CODEC = RecordCodecBuilder.mapCodec(
      i -> i.group(
               WeightedList.codec(TemplateFeature.TemplateEntry.CODEC).fieldOf("templates").forGetter(TemplateFeature::templates),
               StructureProcessorType.LIST_CODEC.optionalFieldOf("processors").forGetter(TemplateFeature::processors)
            )
            .apply(i, TemplateFeature::new)
   );

   public TemplateFeature(WeightedList<TemplateFeature.TemplateEntry> templates) {
      this(templates, Optional.empty());
   }

   public static TemplateFeature simple(Identifier id, Holder<StructureProcessorList> processors) {
      return new TemplateFeature(WeightedList.of(TemplateFeature.TemplateEntry.of(id)), Optional.of(processors));
   }

   public static TemplateFeature simple(Identifier id) {
      return new TemplateFeature(WeightedList.of(TemplateFeature.TemplateEntry.of(id)), Optional.empty());
   }

   @Override
   public MapCodec<TemplateFeature> codec() {
      return CODEC;
   }

   @Override
   public boolean place(WorldGenLevel level, ChunkGenerator chunkGenerator, RandomSource random, BlockPos origin) {
      TemplateFeature.TemplateEntry templateEntry = this.templates.getRandomOrThrow(random);
      Rotation rotation = Util.getRandom(templateEntry.rotations(), random);
      StructureTemplateManager structureTemplateManager = level.getLevel().getServer().getStructureTemplateManager();
      StructureTemplate template = structureTemplateManager.getOrCreate(templateEntry.template());
      Vec3i offsetX = this.getRotatedOffset(rotation, Axis.X, template);
      Vec3i offsetZ = this.getRotatedOffset(rotation, Axis.Z, template);
      BlockPos pos = origin.offset(offsetX).offset(offsetZ);
      StructurePlaceSettings settings = new StructurePlaceSettings().setRotation(rotation).setRandom(random);
      if (this.processors.isPresent()) {
         for (StructureProcessor processor : this.processors.get().value().list()) {
            settings.addProcessor(processor);
         }
      }

      return template.placeInWorld(level, pos, pos, settings, random, 3);
   }

   private Vec3i getRotatedOffset(Rotation rotation, Axis axis, StructureTemplate template) {
      return rotation.rotate(axis.getNegative()).getUnitVec3i().multiply(template.getSize().get(axis) / 2);
   }

   public static record TemplateEntry(Identifier template, List<Rotation> rotations) {
      public static final Codec<TemplateFeature.TemplateEntry> CODEC = RecordCodecBuilder.create(
         i -> i.group(
                  Identifier.CODEC.fieldOf("id").forGetter(TemplateFeature.TemplateEntry::template),
                  Rotation.CODEC.listOf().optionalFieldOf("rotations", List.of(Rotation.values())).forGetter(TemplateFeature.TemplateEntry::rotations)
               )
               .apply(i, TemplateFeature.TemplateEntry::new)
      );

      public static TemplateFeature.TemplateEntry of(Identifier template) {
         return new TemplateFeature.TemplateEntry(template, List.of(Rotation.values()));
      }
   }
}
