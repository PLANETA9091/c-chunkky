package net.minecraft.world.level.levelgen.blockpredicates;

import com.mojang.serialization.Codec;
import java.util.List;
import java.util.function.BiPredicate;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.core.Directional;
import net.minecraft.core.HolderSet;
import net.minecraft.core.Vec3i;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.tags.BlockTags;
import net.minecraft.tags.TagKey;
import net.minecraft.world.level.LevelAccessor;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.levelgen.VerticalAnchor;
import net.minecraft.world.level.material.Fluid;
import net.minecraft.world.level.material.Fluids;

public interface BlockPredicate extends BiPredicate<LevelAccessor, BlockPos> {
   Codec<BlockPredicate> CODEC = BuiltInRegistries.BLOCK_PREDICATE_TYPE.byNameCodec().dispatch(BlockPredicate::type, BlockPredicateType::codec);
   BlockPredicate ONLY_IN_AIR_PREDICATE = matchesTag(BlockTags.AIR);
   BlockPredicate ONLY_IN_AIR_OR_WATER_PREDICATE = anyOf(ONLY_IN_AIR_PREDICATE, matchesBlocks(Blocks.WATER));

   BlockPredicateType<?> type();

   static BlockPredicate allOf(List<BlockPredicate> predicates) {
      return new AllOfPredicate(predicates);
   }

   static BlockPredicate allOf(BlockPredicate... predicates) {
      return allOf(List.of(predicates));
   }

   static BlockPredicate allOf(BlockPredicate a, BlockPredicate b) {
      return allOf(List.of(a, b));
   }

   static BlockPredicate anyOf(List<BlockPredicate> predicates) {
      return new AnyOfPredicate(predicates);
   }

   static BlockPredicate anyOf(BlockPredicate... predicates) {
      return anyOf(List.of(predicates));
   }

   static BlockPredicate anyOf(BlockPredicate a, BlockPredicate b) {
      return anyOf(List.of(a, b));
   }

   static BlockPredicate matchesBlocks(Vec3i offset, List<Block> blocks) {
      return new MatchingBlocksPredicate(offset, HolderSet.direct(Block::builtInRegistryHolder, blocks));
   }

   static BlockPredicate matchesBlocks(Directional directional, Block... blocks) {
      return matchesBlocks(directional.getStep(), List.of(blocks));
   }

   static BlockPredicate matchesBlocks(Block... blocks) {
      return matchesBlocks(Vec3i.ZERO, List.of(blocks));
   }

   static BlockPredicate matchesTag(Vec3i offset, TagKey<Block> tag) {
      return new MatchingBlockTagPredicate(offset, tag);
   }

   static BlockPredicate matchesTag(Directional directional, TagKey<Block> tag) {
      return matchesTag(directional.getStep(), tag);
   }

   static BlockPredicate matchesTag(TagKey<Block> tag) {
      return matchesTag(Vec3i.ZERO, tag);
   }

   static BlockPredicate matchesFluids(Directional directional, Fluid... fluids) {
      return matchesFluids(directional.getStep(), List.of(fluids));
   }

   static BlockPredicate matchesFluids(Vec3i offset, List<Fluid> fluids) {
      return new MatchingFluidsPredicate(offset, HolderSet.direct(Fluid::builtInRegistryHolder, fluids));
   }

   static BlockPredicate matchesFluids(Fluid... fluids) {
      return matchesFluids(Vec3i.ZERO, List.of(fluids));
   }

   static BlockPredicate not(BlockPredicate predicate) {
      return new NotPredicate(predicate);
   }

   static BlockPredicate replaceable() {
      return new ReplaceablePredicate(Vec3i.ZERO);
   }

   static BlockPredicate wouldSurvive(Block block) {
      return new WouldSurvivePredicate(Vec3i.ZERO, block.defaultBlockState());
   }

   static BlockPredicate hasSturdyFace(Direction direction) {
      return new HasSturdyFacePredicate(Vec3i.ZERO, direction);
   }

   static BlockPredicate hasSturdyFace(Directional offset, Direction direction) {
      return new HasSturdyFacePredicate(offset.getStep(), direction);
   }

   static BlockPredicate solid(Directional directional) {
      return new SolidPredicate(directional.getStep());
   }

   static BlockPredicate solid() {
      return new SolidPredicate(Vec3i.ZERO);
   }

   static BlockPredicate noFluid() {
      return matchesFluids(Vec3i.ZERO, List.of(Fluids.EMPTY));
   }

   static BlockPredicate insideWorld(Vec3i offset) {
      return new InsideWorldBoundsPredicate(offset);
   }

   static BlockPredicate alwaysTrue() {
      return TrueBlockPredicate.INSTANCE;
   }

   static BlockPredicate unobstructed() {
      return new UnobstructedPredicate(Vec3i.ZERO);
   }

   static BlockPredicate heightRange(VerticalAnchor minInclusive, VerticalAnchor maxInclusive) {
      return new HeightRangePredicate(minInclusive, maxInclusive);
   }

   static BlockPredicate volumeMatch(Vec3i min, Vec3i max, BlockPredicate match) {
      return new VolumeMatchPredicate(min, max, match);
   }
}
