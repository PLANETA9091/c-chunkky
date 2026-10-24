package net.minecraft.world.level.chunk;

import ca.spottedleaf.moonrise.common.list.ShortList;
import ca.spottedleaf.moonrise.patches.block_counting.BlockCountingChunkSection;
import ca.spottedleaf.moonrise.patches.block_counting.BlockCountingEntry;
import ca.spottedleaf.moonrise.patches.collisions.CollisionUtil;
import io.papermc.paper.annotation.DoNotUse;
import io.papermc.paper.antixray.ChunkPacketInfo;
import it.unimi.dsi.fastutil.shorts.ShortArrayList;
import java.util.ArrayList;
import java.util.List;
import java.util.Objects;
import java.util.function.Predicate;
import net.minecraft.core.Holder;
import net.minecraft.network.FriendlyByteBuf;
import net.minecraft.util.BitStorage;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.biome.Biome;
import net.minecraft.world.level.biome.BiomeResolver;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.material.FluidState;

public class LevelChunkSection implements BlockCountingChunkSection {
   public static final int BIOME_CONTAINER_BITS = 2;
   short nonEmptyBlockCount;
   private short fluidCount;
   private short tickingBlockCount;
   private short tickingFluidCount;
   private final PalettedContainer<BlockState> states;
   private PalettedContainer<Holder<Biome>> biomes;
   private static final ShortArrayList FULL_LIST = new ShortArrayList(4096);
   private boolean isClient;
   private static final short CLIENT_FORCED_SPECIAL_COLLIDING_BLOCKS = 9999;
   private short specialCollidingBlocks;
   private final ShortList tickingBlocks = new ShortList();

   @Override
   public final boolean moonrise$hasSpecialCollidingBlocks() {
      return this.specialCollidingBlocks != 0;
   }

   @Override
   public final ShortList moonrise$getTickingBlockList() {
      return this.tickingBlocks;
   }

   private LevelChunkSection(LevelChunkSection source) {
      this.nonEmptyBlockCount = source.nonEmptyBlockCount;
      this.fluidCount = source.fluidCount;
      this.tickingBlockCount = source.tickingBlockCount;
      this.tickingFluidCount = source.tickingFluidCount;
      this.states = source.states.copy();
      this.biomes = source.biomes.copy();
   }

   public LevelChunkSection(PalettedContainer<BlockState> states, PalettedContainer<Holder<Biome>> biomes) {
      this.states = states;
      this.biomes = biomes;
      this.recalcBlockCounts();
   }

   @Deprecated
   @DoNotUse
   public LevelChunkSection(PalettedContainerFactory containerFactory) {
      this(containerFactory, null, null, 0);
   }

   public LevelChunkSection(PalettedContainerFactory containerFactory, Level level, ChunkPos chunkPos, int chunkSectionY) {
      this.states = containerFactory.createForBlockStates(level, chunkPos, chunkSectionY);
      this.biomes = containerFactory.createForBiomes();
   }

   public BlockState getBlockState(int sectionX, int sectionY, int sectionZ) {
      return this.states.get(sectionX, sectionY, sectionZ);
   }

   public FluidState getFluidState(int sectionX, int sectionY, int sectionZ) {
      return this.states.get(sectionX, sectionY, sectionZ).getFluidState();
   }

   public void acquire() {
      this.states.acquire();
   }

   public void release() {
      this.states.release();
   }

   public BlockState setBlockState(int sectionX, int sectionY, int sectionZ, BlockState state) {
      return this.setBlockState(sectionX, sectionY, sectionZ, state, true);
   }

   private void updateBlockCallback(int x, int y, int z, BlockState newState, BlockState oldState) {
      if (oldState != newState) {
         if (this.isClient) {
            if (CollisionUtil.isSpecialCollidingBlock(newState)) {
               this.specialCollidingBlocks = 9999;
            }
         } else {
            boolean isSpecialOld = CollisionUtil.isSpecialCollidingBlock(oldState);
            boolean isSpecialNew = CollisionUtil.isSpecialCollidingBlock(newState);
            if (isSpecialOld != isSpecialNew) {
               if (isSpecialOld) {
                  this.specialCollidingBlocks--;
               } else {
                  this.specialCollidingBlocks++;
               }
            }

            boolean oldTicking = oldState.isRandomlyTicking();
            boolean newTicking = newState.isRandomlyTicking();
            if (oldTicking != newTicking) {
               ShortList tickingBlocks = this.tickingBlocks;
               short position = (short)(x | z << 4 | y << 8);
               if (oldTicking) {
                  tickingBlocks.remove(position);
               } else {
                  tickingBlocks.add(position);
               }
            }
         }
      }
   }

   public BlockState setBlockState(int sectionX, int sectionY, int sectionZ, BlockState state, boolean checkThreading) {
      BlockState previous;
      if (checkThreading) {
         previous = this.states.getAndSet(sectionX, sectionY, sectionZ, state);
      } else {
         previous = this.states.getAndSetUnchecked(sectionX, sectionY, sectionZ, state);
      }

      if (!previous.isAir()) {
         this.nonEmptyBlockCount--;
         if (previous.isRandomlyTicking()) {
            this.tickingBlockCount--;
         }

         FluidState previousFluid = previous.getFluidState();
         if (!previousFluid.isEmpty()) {
            this.fluidCount--;
            if (previousFluid.isRandomlyTicking()) {
               this.tickingFluidCount--;
            }
         }
      }

      if (!state.isAir()) {
         this.nonEmptyBlockCount++;
         if (state.isRandomlyTicking()) {
            this.tickingBlockCount++;
         }

         FluidState fluid = state.getFluidState();
         if (!fluid.isEmpty()) {
            this.fluidCount++;
            if (fluid.isRandomlyTicking()) {
               this.tickingFluidCount++;
            }
         }
      }

      this.updateBlockCallback(sectionX, sectionY, sectionZ, state, previous);
      return previous;
   }

   public boolean hasOnlyAir() {
      return this.nonEmptyBlockCount == 0;
   }

   public boolean hasFluid() {
      return this.fluidCount > 0;
   }

   public boolean isRandomlyTicking() {
      return this.isRandomlyTickingBlocks() || this.isRandomlyTickingFluids();
   }

   public boolean isRandomlyTickingBlocks() {
      return this.tickingBlockCount > 0;
   }

   public boolean isRandomlyTickingFluids() {
      return this.tickingFluidCount > 0;
   }

   public void recalcBlockCounts() {
      this.nonEmptyBlockCount = 0;
      this.fluidCount = 0;
      this.tickingBlockCount = 0;
      this.tickingFluidCount = 0;
      this.specialCollidingBlocks = 0;
      this.tickingBlocks.clear();
      if (this.maybeHas(statex -> !statex.isAir())) {
         PalettedContainer.Data<BlockState> data = this.states.data;
         Palette<BlockState> palette = data.palette();
         int paletteSize = palette.getSize();
         BitStorage storage = data.storage();
         List<BlockCountingEntry> counts;
         if (paletteSize == 1) {
            counts = new ArrayList<>(1);
            counts.add(new BlockCountingEntry(FULL_LIST.size(), palette.valueFor(0), FULL_LIST));
         } else {
            counts = storage.moonrise$countBlocks(palette);
         }

         int i = 0;

         for (int len = counts.size(); i < len; i++) {
            BlockCountingEntry blockCountingEntry = counts.get(i);
            int paletteCount = blockCountingEntry.count;
            BlockState state = blockCountingEntry.state;
            if (!state.isAir()) {
               if (CollisionUtil.isSpecialCollidingBlock(state)) {
                  this.specialCollidingBlocks += (short)paletteCount;
               }

               this.nonEmptyBlockCount += (short)paletteCount;
               if (state.isRandomlyTicking()) {
                  ShortArrayList coordinates = blockCountingEntry.coords;
                  this.tickingBlockCount += (short)paletteCount;
                  short[] raw = coordinates.elements();
                  int rawLen = raw.length;
                  ShortList tickingBlocks = this.tickingBlocks;
                  tickingBlocks.setMinCapacity(Math.min((rawLen + tickingBlocks.size()) * 3 / 2, 4096));
                  Objects.checkFromToIndex(0, paletteCount, raw.length);

                  for (int k = 0; k < paletteCount; k++) {
                     tickingBlocks.add(raw[k]);
                  }
               }

               FluidState fluid = state.getFluidState();
               if (!fluid.isEmpty()) {
                  this.fluidCount += (short)paletteCount;
                  if (fluid.isRandomlyTicking()) {
                     this.tickingFluidCount += (short)paletteCount;
                  }
               }
            }
         }
      }
   }

   public PalettedContainer<BlockState> getStates() {
      return this.states;
   }

   public PalettedContainerRO<Holder<Biome>> getBiomes() {
      return this.biomes;
   }

   public void read(FriendlyByteBuf buffer) {
      this.nonEmptyBlockCount = buffer.readShort();
      this.fluidCount = buffer.readShort();
      this.states.read(buffer);
      PalettedContainer<Holder<Biome>> biomes = this.biomes.recreate();
      biomes.read(buffer);
      this.biomes = biomes;
      this.isClient = true;
      this.specialCollidingBlocks = (short)(this.nonEmptyBlockCount != 0 && this.maybeHas(CollisionUtil::isSpecialCollidingBlock) ? 9999 : 0);
   }

   public void readBiomes(FriendlyByteBuf buffer) {
      PalettedContainer<Holder<Biome>> biomes = this.biomes.recreate();
      biomes.read(buffer);
      this.biomes = biomes;
   }

   @Deprecated
   @DoNotUse
   public void write(FriendlyByteBuf buffer) {
      this.write(buffer, null, 0);
   }

   public void write(FriendlyByteBuf buffer, ChunkPacketInfo<BlockState> chunkPacketInfo, int chunkSectionIndex) {
      buffer.writeShort(this.nonEmptyBlockCount);
      buffer.writeShort(this.fluidCount);
      this.states.write(buffer, chunkPacketInfo, chunkSectionIndex);
      this.biomes.write(buffer, null, chunkSectionIndex);
   }

   public int getSerializedSize() {
      return 4 + this.states.getSerializedSize() + this.biomes.getSerializedSize();
   }

   public boolean maybeHas(Predicate<BlockState> predicate) {
      return this.states.maybeHas(predicate);
   }

   public Holder<Biome> getNoiseBiome(int quartX, int quartY, int quartZ) {
      return this.biomes.get(quartX, quartY, quartZ);
   }

   public void setNoiseBiome(int quartX, int quartY, int quartZ, Holder<Biome> biome) {
      this.biomes.set(quartX, quartY, quartZ, biome);
   }

   public void fillBiomesFromNoise(BiomeResolver biomeResolver, int quartMinX, int quartMinY, int quartMinZ) {
      Holder<Biome> first = biomeResolver.getNoiseBiome(quartMinX, quartMinY, quartMinZ);
      PalettedContainer<Holder<Biome>> newBiomes = new PalettedContainer<>(first, this.biomes.strategy, null);

      for (int x = 0; x < 4; x++) {
         for (int y = 0; y < 4; y++) {
            int startZ = x == 0 && y == 0 ? 1 : 0;

            for (int z = startZ; z < 4; z++) {
               newBiomes.set(y << 4 | z << 2 | x, biomeResolver.getNoiseBiome(quartMinX + x, quartMinY + y, quartMinZ + z));
            }
         }
      }

      this.biomes = newBiomes;
   }

   public LevelChunkSection copy() {
      return new LevelChunkSection(this);
   }

   static {
      for (short i = 0; i < 4096; i++) {
         FULL_LIST.add(i);
      }
   }
}
