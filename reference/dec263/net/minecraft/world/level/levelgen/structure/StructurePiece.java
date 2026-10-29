package net.minecraft.world.level.levelgen.structure;

import com.google.common.collect.ImmutableSet;
import com.mojang.logging.LogUtils;
import java.util.List;
import java.util.Set;
import java.util.stream.Stream;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.core.BlockPos.MutableBlockPos;
import net.minecraft.core.Direction.Axis;
import net.minecraft.core.Direction.Plane;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.resources.ResourceKey;
import net.minecraft.util.RandomSource;
import net.minecraft.util.ProblemReporter.ScopedCollector;
import net.minecraft.world.level.BlockGetter;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.LevelReader;
import net.minecraft.world.level.ServerLevelAccessor;
import net.minecraft.world.level.StructureManager;
import net.minecraft.world.level.WorldGenLevel;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.DispenserBlock;
import net.minecraft.world.level.block.HorizontalDirectionalBlock;
import net.minecraft.world.level.block.Mirror;
import net.minecraft.world.level.block.Rotation;
import net.minecraft.world.level.block.Block.UpdateFlags;
import net.minecraft.world.level.block.entity.BlockEntity;
import net.minecraft.world.level.block.entity.RandomizableContainerBlockEntity;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.ChunkGenerator;
import net.minecraft.world.level.levelgen.Heightmap;
import net.minecraft.world.level.levelgen.structure.pieces.StructurePieceSerializationContext;
import net.minecraft.world.level.levelgen.structure.pieces.StructurePieceType;
import net.minecraft.world.level.levelgen.structure.pieces.StructurePiecesBuilder;
import net.minecraft.world.level.material.FluidState;
import net.minecraft.world.level.storage.TagValueInput;
import net.minecraft.world.level.storage.loot.LootTable;
import org.bukkit.craftbukkit.CraftLootTable;
import org.bukkit.craftbukkit.block.CraftBlockEntityState;
import org.bukkit.craftbukkit.block.CraftBlockState;
import org.bukkit.craftbukkit.block.CraftBlockStates;
import org.bukkit.craftbukkit.block.CraftChest;
import org.bukkit.craftbukkit.block.CraftCreatureSpawner;
import org.bukkit.craftbukkit.block.CraftDispenser;
import org.bukkit.craftbukkit.util.TransformerLevelAccessor;
import org.bukkit.entity.EntityType;
import org.jspecify.annotations.Nullable;
import org.slf4j.Logger;

public abstract class StructurePiece {
   protected static final BlockState CAVE_AIR = Blocks.CAVE_AIR.defaultBlockState();
   protected BoundingBox boundingBox;
   @Nullable
   private Direction orientation;
   private Mirror mirror;
   private Rotation rotation;
   protected int genDepth;
   private final StructurePieceType type;
   public static final Set<Block> SHAPE_CHECK_BLOCKS = ImmutableSet.builder()
      .add(Blocks.NETHER_BRICK_FENCE)
      .add(Blocks.TORCH)
      .add(Blocks.WALL_TORCH)
      .add(Blocks.OAK_FENCE)
      .add(Blocks.SPRUCE_FENCE)
      .add(Blocks.DARK_OAK_FENCE)
      .add(Blocks.PALE_OAK_FENCE)
      .add(Blocks.ACACIA_FENCE)
      .add(Blocks.BIRCH_FENCE)
      .add(Blocks.JUNGLE_FENCE)
      .add(Blocks.LADDER)
      .add(Blocks.IRON_BARS)
      .build();
   private static final Logger LOGGER = LogUtils.getLogger();

   protected StructurePiece(StructurePieceType type, int genDepth, BoundingBox boundingBox) {
      this.type = type;
      this.genDepth = genDepth;
      this.boundingBox = boundingBox;
   }

   public StructurePiece(StructurePieceType type, CompoundTag tag) {
      this(type, tag.getIntOr("GD", 0), tag.<BoundingBox>read("BB", BoundingBox.CODEC).orElseThrow());
      int orientation = tag.getIntOr("O", 0);
      this.setOrientation(orientation == -1 ? null : Direction.from2DDataValue(orientation));
   }

   protected static BoundingBox makeBoundingBox(int x, int y, int z, Direction direction, int width, int height, int depth) {
      return direction.getAxis() == Axis.Z
         ? new BoundingBox(x, y, z, x + width - 1, y + height - 1, z + depth - 1)
         : new BoundingBox(x, y, z, x + depth - 1, y + height - 1, z + width - 1);
   }

   protected static Direction getRandomHorizontalDirection(RandomSource random) {
      return Plane.HORIZONTAL.getRandomDirection(random);
   }

   public final CompoundTag createTag(StructurePieceSerializationContext context) {
      CompoundTag tag = new CompoundTag();
      tag.putString("id", BuiltInRegistries.STRUCTURE_PIECE.getKey(this.getType()).toString());
      tag.store("BB", BoundingBox.CODEC, this.boundingBox);
      Direction orientation = this.getOrientation();
      tag.putInt("O", orientation == null ? -1 : orientation.get2DDataValue());
      tag.putInt("GD", this.genDepth);
      this.addAdditionalSaveData(context, tag);
      return tag;
   }

   protected abstract void addAdditionalSaveData(StructurePieceSerializationContext var1, CompoundTag var2);

   public void addChildren(StructurePiece startPiece, StructurePiecesBuilder builder, RandomSource random) {
   }

   public abstract void postProcess(
      WorldGenLevel var1, StructureManager var2, ChunkGenerator var3, RandomSource var4, BoundingBox var5, ChunkPos var6, BlockPos var7
   );

   public BoundingBox getBoundingBox() {
      return this.boundingBox;
   }

   public int getGenDepth() {
      return this.genDepth;
   }

   public void setGenDepth(int genDepth) {
      this.genDepth = genDepth;
   }

   public boolean isCloseToChunk(ChunkPos pos, int distance) {
      int cx = pos.getMinBlockX();
      int cz = pos.getMinBlockZ();
      return this.boundingBox.intersects(cx - distance, cz - distance, cx + 15 + distance, cz + 15 + distance);
   }

   public BlockPos getLocatorPosition() {
      return this.boundingBox.getCenter();
   }

   protected MutableBlockPos getWorldPos(int x, int y, int z) {
      return new MutableBlockPos(this.getWorldX(x, z), this.getWorldY(y), this.getWorldZ(x, z));
   }

   protected int getWorldX(int x, int z) {
      Direction orientation = this.getOrientation();
      if (orientation == null) {
         return x;
      } else {
         return switch (orientation) {
            case NORTH, SOUTH -> this.boundingBox.minX() + x;
            case WEST -> this.boundingBox.maxX() - z;
            case EAST -> this.boundingBox.minX() + z;
            default -> x;
         };
      }
   }

   protected int getWorldY(int y) {
      return this.getOrientation() == null ? y : y + this.boundingBox.minY();
   }

   protected int getWorldZ(int x, int z) {
      Direction orientation = this.getOrientation();
      if (orientation == null) {
         return z;
      } else {
         return switch (orientation) {
            case NORTH -> this.boundingBox.maxZ() - z;
            case SOUTH -> this.boundingBox.minZ() + z;
            case WEST, EAST -> this.boundingBox.minZ() + x;
            default -> z;
         };
      }
   }

   protected void placeBlock(WorldGenLevel level, BlockState blockState, int x, int y, int z, BoundingBox chunkBB) {
      BlockPos pos = this.getWorldPos(x, y, z);
      if (chunkBB.isInside(pos) && this.canBeReplaced(level, x, y, z, chunkBB)) {
         if (this.mirror != Mirror.NONE) {
            blockState = blockState.mirror(this.mirror);
         }

         if (this.rotation != Rotation.NONE) {
            blockState = blockState.rotate(this.rotation);
         }

         level.setBlock(pos, blockState, 2);
         if (level instanceof TransformerLevelAccessor transformerAccessor && transformerAccessor.canTransformBlocks()) {
            return;
         }

         FluidState fluidState = level.getFluidState(pos);
         if (!fluidState.isEmpty()) {
            level.scheduleTick(pos, fluidState.getType(), 0);
         }

         if (SHAPE_CHECK_BLOCKS.contains(blockState.getBlock())) {
            level.getChunk(pos).markPosForPostProcessing(pos);
         }
      }
   }

   protected boolean placeCraftBlockEntity(
      ServerLevelAccessor level, BlockPos pos, CraftBlockEntityState<?> craftBlockEntityState, @UpdateFlags int updateFlags
   ) {
      if (level instanceof TransformerLevelAccessor transformerAccessor && transformerAccessor.canTransformBlocks()) {
         return transformerAccessor.setCraftBlock(pos, craftBlockEntityState, updateFlags);
      }

      boolean result = level.setBlock(pos, craftBlockEntityState.getHandle(), updateFlags);
      BlockEntity blockEntity = level.getBlockEntity(pos);
      if (blockEntity != null) {
         try (ScopedCollector problemReporter = new ScopedCollector(() -> "StructurePieceTranformers@" + pos.toShortString(), LOGGER)) {
            blockEntity.loadWithComponents(TagValueInput.create(problemReporter, level.registryAccess(), craftBlockEntityState.getSnapshotNBT()));
         }
      }

      return result;
   }

   protected void placeCraftSpawner(ServerLevelAccessor level, BlockPos pos, EntityType entityType, @UpdateFlags int updateFlags) {
      CraftCreatureSpawner spawner = (CraftCreatureSpawner)CraftBlockStates.getBlockState(level, pos, Blocks.SPAWNER.defaultBlockState(), null);
      spawner.setSpawnedType(entityType);
      this.placeCraftBlockEntity(level, pos, spawner, updateFlags);
   }

   protected void setCraftLootTable(ServerLevelAccessor level, BlockPos pos, RandomSource randomSource, ResourceKey<LootTable> lootTable) {
      BlockEntity blockEntity = level.getBlockEntity(pos);
      if (blockEntity instanceof RandomizableContainerBlockEntity lootContainerBlockEntity) {
         lootContainerBlockEntity.setLootTable(lootTable, randomSource.nextLong());
         if (level instanceof TransformerLevelAccessor transformerAccessor && transformerAccessor.canTransformBlocks()) {
            transformerAccessor.setCraftBlock(
               pos,
               (CraftBlockState)CraftBlockStates.getBlockState(
                  level, pos, blockEntity.getBlockState(), lootContainerBlockEntity.saveWithFullMetadata(level.registryAccess())
               ),
               3
            );
         }
      }
   }

   protected boolean canBeReplaced(LevelReader level, int x, int y, int z, BoundingBox chunkBB) {
      return true;
   }

   protected BlockState getBlock(BlockGetter level, int x, int y, int z, BoundingBox chunkBB) {
      BlockPos blockPos = this.getWorldPos(x, y, z);
      return !chunkBB.isInside(blockPos) ? Blocks.AIR.defaultBlockState() : level.getBlockState(blockPos);
   }

   protected boolean isInterior(LevelReader level, int x, int y, int z, BoundingBox chunkBB) {
      BlockPos pos = this.getWorldPos(x, y + 1, z);
      return chunkBB.isInside(pos) && pos.getY() < level.getHeight(Heightmap.Types.OCEAN_FLOOR_WG, pos.getX(), pos.getZ());
   }

   protected void generateAirBox(WorldGenLevel level, BoundingBox chunkBB, int x0, int y0, int z0, int x1, int y1, int z1) {
      for (int y = y0; y <= y1; y++) {
         for (int x = x0; x <= x1; x++) {
            for (int z = z0; z <= z1; z++) {
               this.placeBlock(level, Blocks.AIR.defaultBlockState(), x, y, z, chunkBB);
            }
         }
      }
   }

   protected void generateBox(
      WorldGenLevel level, BoundingBox chunkBB, int x0, int y0, int z0, int x1, int y1, int z1, BlockState edgeBlock, BlockState fillBlock, boolean skipAir
   ) {
      for (int y = y0; y <= y1; y++) {
         for (int x = x0; x <= x1; x++) {
            for (int z = z0; z <= z1; z++) {
               if (!skipAir || !this.getBlock(level, x, y, z, chunkBB).isAir()) {
                  if (y != y0 && y != y1 && x != x0 && x != x1 && z != z0 && z != z1) {
                     this.placeBlock(level, fillBlock, x, y, z, chunkBB);
                  } else {
                     this.placeBlock(level, edgeBlock, x, y, z, chunkBB);
                  }
               }
            }
         }
      }
   }

   protected void generateBox(WorldGenLevel level, BoundingBox chunkBB, BoundingBox boxBB, BlockState edgeBlock, BlockState fillBlock, boolean skipAir) {
      this.generateBox(level, chunkBB, boxBB.minX(), boxBB.minY(), boxBB.minZ(), boxBB.maxX(), boxBB.maxY(), boxBB.maxZ(), edgeBlock, fillBlock, skipAir);
   }

   protected void generateBox(
      WorldGenLevel level,
      BoundingBox chunkBB,
      int x0,
      int y0,
      int z0,
      int x1,
      int y1,
      int z1,
      boolean skipAir,
      RandomSource random,
      StructurePiece.BlockSelector selector
   ) {
      for (int y = y0; y <= y1; y++) {
         for (int x = x0; x <= x1; x++) {
            for (int z = z0; z <= z1; z++) {
               if (!skipAir || !this.getBlock(level, x, y, z, chunkBB).isAir()) {
                  selector.next(random, x, y, z, y == y0 || y == y1 || x == x0 || x == x1 || z == z0 || z == z1);
                  this.placeBlock(level, selector.getNext(), x, y, z, chunkBB);
               }
            }
         }
      }
   }

   protected void generateBox(
      WorldGenLevel level, BoundingBox chunkBB, BoundingBox boxBB, boolean skipAir, RandomSource random, StructurePiece.BlockSelector selector
   ) {
      this.generateBox(level, chunkBB, boxBB.minX(), boxBB.minY(), boxBB.minZ(), boxBB.maxX(), boxBB.maxY(), boxBB.maxZ(), skipAir, random, selector);
   }

   protected void generateMaybeBox(
      WorldGenLevel level,
      BoundingBox chunkBB,
      RandomSource random,
      float probability,
      int x0,
      int y0,
      int z0,
      int x1,
      int y1,
      int z1,
      BlockState edgeBlock,
      BlockState fillBlock,
      boolean skipAir,
      boolean hasToBeInside
   ) {
      for (int y = y0; y <= y1; y++) {
         for (int x = x0; x <= x1; x++) {
            for (int z = z0; z <= z1; z++) {
               if (!(random.nextFloat() > probability)
                  && (!skipAir || !this.getBlock(level, x, y, z, chunkBB).isAir())
                  && (!hasToBeInside || this.isInterior(level, x, y, z, chunkBB))) {
                  if (y != y0 && y != y1 && x != x0 && x != x1 && z != z0 && z != z1) {
                     this.placeBlock(level, fillBlock, x, y, z, chunkBB);
                  } else {
                     this.placeBlock(level, edgeBlock, x, y, z, chunkBB);
                  }
               }
            }
         }
      }
   }

   protected void maybeGenerateBlock(
      WorldGenLevel level, BoundingBox chunkBB, RandomSource random, float probability, int x, int y, int z, BlockState blockState
   ) {
      if (random.nextFloat() < probability) {
         this.placeBlock(level, blockState, x, y, z, chunkBB);
      }
   }

   protected void generateUpperHalfSphere(
      WorldGenLevel level, BoundingBox chunkBB, int x0, int y0, int z0, int x1, int y1, int z1, BlockState fillBlock, boolean skipAir
   ) {
      float diagX = (float)(x1 - x0 + 1);
      float diagY = (float)(y1 - y0 + 1);
      float diagZ = (float)(z1 - z0 + 1);
      float cx = (float)x0 + diagX / 2.0F;
      float cz = (float)z0 + diagZ / 2.0F;

      for (int y = y0; y <= y1; y++) {
         float normalizedYDistance = (float)(y - y0) / diagY;

         for (int x = x0; x <= x1; x++) {
            float normalizedXDistance = ((float)x - cx) / (diagX * 0.5F);

            for (int z = z0; z <= z1; z++) {
               float normalizedZDistance = ((float)z - cz) / (diagZ * 0.5F);
               if (!skipAir || !this.getBlock(level, x, y, z, chunkBB).isAir()) {
                  float dist = normalizedXDistance * normalizedXDistance
                     + normalizedYDistance * normalizedYDistance
                     + normalizedZDistance * normalizedZDistance;
                  if (dist <= 1.05F) {
                     this.placeBlock(level, fillBlock, x, y, z, chunkBB);
                  }
               }
            }
         }
      }
   }

   protected void fillColumnDown(WorldGenLevel level, BlockState blockState, int x, int startY, int z, BoundingBox chunkBB) {
      MutableBlockPos pos = this.getWorldPos(x, startY, z);
      if (chunkBB.isInside(pos)) {
         while (this.isReplaceableByStructures(level.getBlockState(pos)) && pos.getY() > level.getMinY() + 1) {
            level.setBlock(pos, blockState, 2);
            pos.move(Direction.DOWN);
         }
      }
   }

   protected boolean isReplaceableByStructures(BlockState state) {
      return state.isAir() || state.liquid() || state.is(Blocks.GLOW_LICHEN) || state.is(Blocks.SEAGRASS) || state.is(Blocks.TALL_SEAGRASS);
   }

   protected boolean createChest(WorldGenLevel level, BoundingBox chunkBB, RandomSource random, int x, int y, int z, ResourceKey<LootTable> lootTable) {
      return this.createChest(level, chunkBB, random, this.getWorldPos(x, y, z), lootTable, null);
   }

   public static BlockState reorient(BlockGetter level, BlockPos blockPos, BlockState blockState) {
      Direction solidNeighbor = null;

      for (Direction direction : Plane.HORIZONTAL) {
         BlockPos relativePos = blockPos.relative(direction);
         BlockState state = level.getBlockState(relativePos);
         if (state.is(Blocks.CHEST)) {
            return blockState;
         }

         if (state.isSolidRender()) {
            if (solidNeighbor != null) {
               solidNeighbor = null;
               break;
            }

            solidNeighbor = direction;
         }
      }

      if (solidNeighbor != null) {
         return blockState.setValue(HorizontalDirectionalBlock.FACING, solidNeighbor.getOpposite());
      } else {
         Direction lockDir = blockState.getValue(HorizontalDirectionalBlock.FACING);
         BlockPos relativePosx = blockPos.relative(lockDir);
         if (level.getBlockState(relativePosx).isSolidRender()) {
            lockDir = lockDir.getOpposite();
            relativePosx = blockPos.relative(lockDir);
         }

         if (level.getBlockState(relativePosx).isSolidRender()) {
            lockDir = lockDir.getClockWise();
            relativePosx = blockPos.relative(lockDir);
         }

         if (level.getBlockState(relativePosx).isSolidRender()) {
            lockDir = lockDir.getOpposite();
            relativePosx = blockPos.relative(lockDir);
         }

         return blockState.setValue(HorizontalDirectionalBlock.FACING, lockDir);
      }
   }

   protected boolean createChest(
      ServerLevelAccessor level, BoundingBox chunkBB, RandomSource random, BlockPos pos, ResourceKey<LootTable> lootTable, @Nullable BlockState blockState
   ) {
      if (chunkBB.isInside(pos) && !level.getBlockState(pos).is(Blocks.CHEST)) {
         if (blockState == null) {
            blockState = reorient(level, pos, Blocks.CHEST.defaultBlockState());
         }

         CraftChest chestState = (CraftChest)CraftBlockStates.getBlockState(level, pos, blockState, null);
         chestState.setLootTable(CraftLootTable.minecraftToBukkit(lootTable));
         chestState.setSeed(random.nextLong());
         this.placeCraftBlockEntity(level, pos, chestState, 2);
         return true;
      } else {
         return false;
      }
   }

   protected boolean createDispenser(
      WorldGenLevel level, BoundingBox chunkBB, RandomSource random, int x, int y, int z, Direction facing, ResourceKey<LootTable> lootTable
   ) {
      BlockPos pos = this.getWorldPos(x, y, z);
      if (!chunkBB.isInside(pos) || level.getBlockState(pos).is(Blocks.DISPENSER)) {
         return false;
      } else if (!this.canBeReplaced(level, x, y, z, this.boundingBox)) {
         return true;
      } else {
         BlockState dispenserBlockState = Blocks.DISPENSER.defaultBlockState().setValue(DispenserBlock.FACING, facing);
         if (this.mirror != Mirror.NONE) {
            dispenserBlockState = dispenserBlockState.mirror(this.mirror);
         }

         if (this.rotation != Rotation.NONE) {
            dispenserBlockState = dispenserBlockState.rotate(this.rotation);
         }

         CraftDispenser dispenserState = (CraftDispenser)CraftBlockStates.getBlockState(level, pos, dispenserBlockState, null);
         dispenserState.setLootTable(CraftLootTable.minecraftToBukkit(lootTable));
         dispenserState.setSeed(random.nextLong());
         this.placeCraftBlockEntity(level, pos, dispenserState, 2);
         return true;
      }
   }

   public void move(int dx, int dy, int dz) {
      this.boundingBox.move(dx, dy, dz);
   }

   public static BoundingBox createBoundingBox(Stream<StructurePiece> pieces) {
      return BoundingBox.encapsulatingBoxes(pieces.map(StructurePiece::getBoundingBox)::iterator)
         .orElseThrow(() -> new IllegalStateException("Unable to calculate boundingbox without pieces"));
   }

   @Nullable
   public static StructurePiece findCollisionPiece(List<StructurePiece> pieces, BoundingBox box) {
      for (StructurePiece piece : pieces) {
         if (piece.getBoundingBox().intersects(box)) {
            return piece;
         }
      }

      return null;
   }

   @Nullable
   public Direction getOrientation() {
      return this.orientation;
   }

   public void setOrientation(@Nullable Direction orientation) {
      this.orientation = orientation;
      if (orientation == null) {
         this.rotation = Rotation.NONE;
         this.mirror = Mirror.NONE;
      } else {
         switch (orientation) {
            case SOUTH:
               this.mirror = Mirror.LEFT_RIGHT;
               this.rotation = Rotation.NONE;
               break;
            case WEST:
               this.mirror = Mirror.LEFT_RIGHT;
               this.rotation = Rotation.CLOCKWISE_90;
               break;
            case EAST:
               this.mirror = Mirror.NONE;
               this.rotation = Rotation.CLOCKWISE_90;
               break;
            default:
               this.mirror = Mirror.NONE;
               this.rotation = Rotation.NONE;
         }
      }
   }

   public Rotation getRotation() {
      return this.rotation;
   }

   public Mirror getMirror() {
      return this.mirror;
   }

   public StructurePieceType getType() {
      return this.type;
   }

   public abstract static class BlockSelector {
      protected BlockState next = Blocks.AIR.defaultBlockState();

      public abstract void next(RandomSource var1, int var2, int var3, int var4, boolean var5);

      public BlockState getNext() {
         return this.next;
      }
   }
}
