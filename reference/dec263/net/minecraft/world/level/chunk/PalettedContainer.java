package net.minecraft.world.level.chunk;

import ca.spottedleaf.moonrise.patches.bitstorage.PaletteReadoutCache;
import ca.spottedleaf.moonrise.patches.fast_palette.FastHashMapPalette;
import com.google.common.annotations.VisibleForTesting;
import com.mojang.serialization.Codec;
import com.mojang.serialization.DataResult;
import com.mojang.serialization.codecs.RecordCodecBuilder;
import io.papermc.paper.annotation.DoNotUse;
import io.papermc.paper.antixray.ChunkPacketInfo;
import it.unimi.dsi.fastutil.ints.Int2IntOpenHashMap;
import it.unimi.dsi.fastutil.ints.IntArraySet;
import it.unimi.dsi.fastutil.ints.IntSet;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.HashSet;
import java.util.List;
import java.util.Objects;
import java.util.Optional;
import java.util.Set;
import java.util.function.Consumer;
import java.util.function.Predicate;
import java.util.stream.LongStream;
import net.minecraft.core.IdMap;
import net.minecraft.network.FriendlyByteBuf;
import net.minecraft.util.BitStorage;
import net.minecraft.util.ExtraCodecs;
import net.minecraft.util.Mth;
import net.minecraft.util.SimpleBitStorage;
import net.minecraft.util.ZeroBitStorage;
import net.minecraft.util.SimpleBitStorage.InitializationException;
import org.jspecify.annotations.Nullable;

public class PalettedContainer<T> implements PaletteResize<T>, PalettedContainerRO<T> {
   private static final int MIN_PALETTE_BITS = 0;
   public volatile PalettedContainer.Data<T> data;
   public final Strategy<T> strategy;
   @Nullable
   private final T[] presetValues;
   private static final ThreadLocal<PaletteReadoutCache> READOUT_CACHE = ThreadLocal.withInitial(() -> new PaletteReadoutCache());
   private static final boolean DEBUG_PACKING = false;

   public void acquire() {
   }

   public void release() {
   }

   @Deprecated
   @DoNotUse
   public static <T> Codec<PalettedContainer<T>> codecRW(Codec<T> elementCodec, Strategy<T> strategy, T defaultValue) {
      return codecRW(elementCodec, strategy, defaultValue, null);
   }

   public static <T> Codec<PalettedContainer<T>> codecRW(Codec<T> elementCodec, Strategy<T> strategy, T defaultValue, @Nullable T[] presetValues) {
      PalettedContainerRO.Unpacker<T, PalettedContainer<T>> unpacker = (strategy1, packedData) -> unpack(strategy1, packedData, defaultValue, presetValues);
      return codec(elementCodec, strategy, defaultValue, unpacker);
   }

   public static <T> Codec<PalettedContainerRO<T>> codecRO(Codec<T> elementCodec, Strategy<T> strategy, T defaultValue) {
      PalettedContainerRO.Unpacker<T, PalettedContainerRO<T>> unpacker = (s, data) -> unpack(s, data, defaultValue, null).map(e -> e);
      return codec(elementCodec, strategy, defaultValue, unpacker);
   }

   private static <T, C extends PalettedContainerRO<T>> Codec<C> codec(
      Codec<T> elementCodec, Strategy<T> strategy, T defaultValue, PalettedContainerRO.Unpacker<T, C> unpacker
   ) {
      return RecordCodecBuilder.create(
            i -> i.group(
                     elementCodec.mapResult(ExtraCodecs.orElsePartial(defaultValue))
                        .listOf()
                        .fieldOf("palette")
                        .forGetter(PalettedContainerRO.PackedData::paletteEntries),
                     Codec.LONG_STREAM.lenientOptionalFieldOf("data").forGetter(PalettedContainerRO.PackedData::storage)
                  )
                  .apply(i, PalettedContainerRO.PackedData::new)
         )
         .comapFlatMap(discData -> unpacker.read(strategy, discData), palettedContainer -> palettedContainer.pack(strategy));
   }

   private static <T> T readPalette(PalettedContainer.Data<T> data, int paletteIdx) {
      Palette<T> palette = data.palette;
      return palette instanceof FastHashMapPalette<T> hashMapPalette ? hashMapPalette.valueFor(paletteIdx) : palette.valueFor(paletteIdx);
   }

   private PalettedContainer(
      Strategy<T> strategy, Configuration dataConfiguration, BitStorage storage, Palette<T> palette, List<T> values, T defaultValue, @Nullable T[] presetValues
   ) {
      this.presetValues = presetValues;
      this.strategy = strategy;
      this.data = new PalettedContainer.Data<>(dataConfiguration, storage, palette);
      if (presetValues != null) {
         label40: {
            if (dataConfiguration instanceof Configuration.Simple simpleFactory && simpleFactory.factory() == Strategy.SINGLE_VALUE_PALETTE_FACTORY) {
               if (this.data.palette.valueFor(0) == defaultValue) {
                  return;
               }
               break label40;
            }

            if (dataConfiguration instanceof Configuration.Global) {
               return;
            }
         }

         int maxSize = 1 << dataConfiguration.bitsInMemory();

         for (int i = 0; i < presetValues.length; i++) {
            if (this.data.palette.getSize() >= maxSize) {
               Set<T> allValues = new HashSet<>(values);
               int duplicates = values.size() - allValues.size();
               allValues.addAll(Arrays.asList(presetValues));
               int newBits = Mth.ceillog2(allValues.size() + duplicates);
               if (newBits > dataConfiguration.bitsInMemory()) {
                  this.onResize(newBits, null);
               } else {
                  for (int j = i; j < presetValues.length; j++) {
                     this.data.palette.idFor(presetValues[j], this);
                  }
               }
               break;
            }

            this.data.palette.idFor(presetValues[i], this);
         }
      }
   }

   private PalettedContainer(PalettedContainer<T> source, @Nullable T[] presetValues) {
      this.presetValues = presetValues;
      this.strategy = source.strategy;
      this.data = source.data.copy();
   }

   @Deprecated
   @DoNotUse
   public PalettedContainer(T initialValue, Strategy<T> strategy) {
      this(initialValue, strategy, null);
   }

   public PalettedContainer(T initialValue, Strategy<T> strategy, @Nullable T[] presetValues) {
      this.presetValues = presetValues;
      this.strategy = strategy;
      this.data = this.createOrReuseData(null, 0);
      this.data.palette.idFor(initialValue, this);
   }

   private PalettedContainer.Data<T> createOrReuseData(@Nullable PalettedContainer.Data<T> oldData, int targetBits) {
      Configuration dataConfiguration = this.strategy.getConfigurationForBitCount(targetBits);
      if (oldData != null && dataConfiguration.equals(oldData.configuration())) {
         return oldData;
      } else {
         BitStorage storage = (BitStorage)(dataConfiguration.bitsInMemory() == 0
            ? new ZeroBitStorage(this.strategy.entryCount())
            : new SimpleBitStorage(dataConfiguration.bitsInMemory(), this.strategy.entryCount()));
         Palette<T> palette = dataConfiguration.createPalette(this.strategy, List.of());
         return new PalettedContainer.Data<>(dataConfiguration, storage, palette);
      }
   }

   private void addPresetValues() {
      if (this.presetValues != null && !(this.data.configuration() instanceof Configuration.Global)) {
         for (T presetValue : this.presetValues) {
            this.data.palette.idFor(presetValue, this);
         }
      }
   }

   @Override
   public synchronized int onResize(int bits, T lastAddedValue) {
      PalettedContainer.Data<T> oldData = this.data;
      if (this.presetValues != null
         && lastAddedValue != null
         && oldData.configuration() instanceof Configuration.Simple simpleFactory
         && simpleFactory.factory() == Strategy.SINGLE_VALUE_PALETTE_FACTORY) {
         int duplicates = 0;
         List<T> presetValues = Arrays.asList(this.presetValues);
         duplicates += presetValues.contains(lastAddedValue) ? 1 : 0;
         duplicates += presetValues.contains(oldData.palette.valueFor(0)) ? 1 : 0;
         int size = 1 << this.strategy.getConfigurationForBitCount(bits).bitsInMemory();
         bits = Mth.ceillog2(size + presetValues.size() - duplicates);
      }

      PalettedContainer.Data<T> newData = this.createOrReuseData(oldData, bits);
      newData.copyFrom(oldData.palette, oldData.storage);
      this.data = newData;
      this.addPresetValues();
      return lastAddedValue == null ? -1 : newData.palette.idFor(lastAddedValue, PaletteResize.noResizeExpected());
   }

   public synchronized T getAndSet(int x, int y, int z, T value) {
      this.acquire();

      Object var5;
      try {
         var5 = this.getAndSet(this.strategy.getIndex(x, y, z), value);
      } finally {
         this.release();
      }

      return (T)var5;
   }

   public T getAndSetUnchecked(int x, int y, int z, T value) {
      return this.getAndSet(this.strategy.getIndex(x, y, z), value);
   }

   private T getAndSet(int index, T value) {
      int paletteIdx = this.data.palette.idFor(value, this);
      PalettedContainer.Data<T> data = this.data;
      int prev = data.storage.getAndSet(index, paletteIdx);
      return readPalette(data, prev);
   }

   public synchronized void set(int x, int y, int z, T value) {
      this.acquire();

      try {
         this.set(this.strategy.getIndex(x, y, z), value);
      } finally {
         this.release();
      }
   }

   public void set(int index, T value) {
      int id = this.data.palette.idFor(value, this);
      this.data.storage.set(index, id);
   }

   @Override
   public T get(int x, int y, int z) {
      return this.get(this.strategy.getIndex(x, y, z));
   }

   public T get(int index) {
      PalettedContainer.Data<T> data = this.data;
      return readPalette(data, data.storage.get(index));
   }

   @Override
   public void getAll(Consumer<T> consumer) {
      Palette<T> palette = this.data.palette();
      IntSet allExistingEntries = new IntArraySet();
      this.data.storage.getAll(allExistingEntries::add);
      allExistingEntries.forEach(state -> consumer.accept(palette.valueFor(state)));
   }

   public synchronized void read(FriendlyByteBuf buffer) {
      this.acquire();

      try {
         int newBits = buffer.readByte();
         PalettedContainer.Data<T> newData = this.createOrReuseData(this.data, newBits);
         newData.palette.read(buffer, this.strategy.globalMap());
         buffer.readFixedSizeLongArray(newData.storage.getRaw());
         this.data = newData;
         this.addPresetValues();
      } finally {
         this.release();
      }
   }

   @Deprecated
   @DoNotUse
   @Override
   public synchronized void write(FriendlyByteBuf buffer) {
      this.write(buffer, null, 0);
   }

   @Override
   public synchronized void write(FriendlyByteBuf buffer, @Nullable ChunkPacketInfo<T> chunkPacketInfo, int chunkSectionIndex) {
      this.acquire();

      try {
         this.data.write(buffer, this.strategy.globalMap(), chunkPacketInfo, chunkSectionIndex);
         if (chunkPacketInfo != null) {
            chunkPacketInfo.setPresetValues(chunkSectionIndex, this.presetValues);
         }
      } finally {
         this.release();
      }
   }

   @VisibleForTesting
   public static <T> DataResult<PalettedContainer<T>> unpack(
      Strategy<T> strategy, PalettedContainerRO.PackedData<T> discData, T defaultValue, @Nullable T[] presetValues
   ) {
      List<T> paletteEntries = discData.paletteEntries();
      int entryCount = strategy.entryCount();
      Configuration storedConfiguration = strategy.getConfigurationForPaletteSize(paletteEntries.size());
      int bitsOnDisc = storedConfiguration.bitsInStorage();
      if (discData.bitsPerEntry() != -1 && bitsOnDisc != discData.bitsPerEntry()) {
         return DataResult.error(() -> "Invalid bit count, calculated " + bitsOnDisc + ", but container declared " + discData.bitsPerEntry());
      } else {
         BitStorage storage;
         Palette<T> palette;
         if (storedConfiguration.bitsInMemory() == 0) {
            palette = storedConfiguration.createPalette(strategy, paletteEntries);
            storage = new ZeroBitStorage(entryCount);
         } else {
            Optional<LongStream> dataOpt = discData.storage();
            if (dataOpt.isEmpty()) {
               return DataResult.error(() -> "Missing values for non-zero storage");
            }

            long[] data = dataOpt.get().toArray();

            try {
               if (!storedConfiguration.alwaysRepack() && storedConfiguration.bitsInMemory() == bitsOnDisc) {
                  palette = storedConfiguration.createPalette(strategy, paletteEntries);
                  storage = new SimpleBitStorage(storedConfiguration.bitsInMemory(), entryCount, data);
               } else {
                  Palette<T> oldPalette = new HashMapPalette<>(bitsOnDisc, paletteEntries);
                  SimpleBitStorage oldStorage = new SimpleBitStorage(bitsOnDisc, entryCount, data);
                  Palette<T> newPalette = storedConfiguration.createPalette(strategy, paletteEntries);
                  int[] newContents = reencodeContents(oldStorage, oldPalette, newPalette);
                  palette = newPalette;
                  storage = new SimpleBitStorage(storedConfiguration.bitsInMemory(), entryCount, newContents);
               }
            } catch (InitializationException var16) {
               return DataResult.error(() -> "Failed to read PalettedContainer: " + var16.getMessage());
            }
         }

         return DataResult.success(new PalettedContainer<>(strategy, storedConfiguration, storage, palette, paletteEntries, defaultValue, presetValues));
      }
   }

   private static <T> void validate(List<T> palette, long[] data, int bits, BitStorage againstStorage, Palette<T> againstPalette) {
      BitStorage storage = (BitStorage)(data == null
         ? new ZeroBitStorage(againstStorage.getSize())
         : new SimpleBitStorage(bits, againstStorage.getSize(), data));

      for (int i = 0; i < storage.getSize(); i++) {
         T expect = againstPalette.valueFor(againstStorage.get(i));
         T got = palette.get(storage.get(i));
         if (expect != got) {
            throw new IllegalStateException();
         }
      }
   }

   @Override
   public synchronized PalettedContainerRO.PackedData<T> pack(Strategy<T> strategy) {
      this.acquire();

      try {
         PalettedContainer.Data<T> data = this.data;
         BitStorage storage = data.storage();
         Palette<T> palette = data.palette();
         PaletteReadoutCache readoutCache = READOUT_CACHE.get();
         boolean zero = !readoutCache.readout(storage, palette);
         int newPaletteSize = zero ? 1 : readoutCache.newPaletteCount;
         List<T> newPalette = new ArrayList<>(newPaletteSize);
         Configuration configuration = strategy.getConfigurationForPaletteSize(newPaletteSize);
         int bitsToUse = configuration.bitsInStorage();
         long[] dataArray;
         if (zero) {
            newPalette.add(palette.valueFor(0));
            if (bitsToUse == 0) {
               dataArray = null;
            } else {
               dataArray = new SimpleBitStorage(bitsToUse, storage.getSize()).getRaw();
            }
         } else {
            if (bitsToUse != 0 && newPaletteSize == palette.getSize() && bitsToUse == storage.getBits()) {
               List<T> oldPalette = new ArrayList<>(newPaletteSize);

               for (int i = 0; i < newPaletteSize; i++) {
                  oldPalette.add(palette.valueFor(i));
               }

               Arrays.fill(readoutCache.newPalette, 0, newPaletteSize, null);
               long[] cpy = Arrays.copyOf(readoutCache.copyCache, storage.getRaw().length);
               return new PalettedContainerRO.PackedData<>(oldPalette, Optional.of(Arrays.stream(cpy)), bitsToUse);
            }

            Object[] newPaletteRaw = readoutCache.newPalette;
            Objects.checkFromToIndex(0, readoutCache.newPaletteCount, newPaletteRaw.length);
            int i = 0;

            for (int len = readoutCache.newPaletteCount; i < len; i++) {
               T entry = (T)newPaletteRaw[i];
               newPaletteRaw[i] = null;
               newPalette.add(entry);
            }

            if (bitsToUse == 0) {
               dataArray = null;
            } else {
               dataArray = readoutCache.repack(bitsToUse, storage.getSize()).getRaw();
            }
         }

         return new PalettedContainerRO.PackedData<>(newPalette, dataArray == null ? Optional.empty() : Optional.of(Arrays.stream(dataArray)), bitsToUse);
      } finally {
         this.release();
      }
   }

   private static <T> int[] reencodeContents(BitStorage storage, Palette<T> oldPalette, Palette<T> newPalette) {
      int[] buffer = new int[storage.getSize()];
      storage.unpack(buffer);
      PaletteResize<T> dummyResizer = PaletteResize.noResizeExpected();
      int lastReadId = -1;
      int lastWrittenId = -1;

      for (int index = 0; index < buffer.length; index++) {
         int id = buffer[index];
         if (id != lastReadId) {
            lastReadId = id;
            lastWrittenId = newPalette.idFor(oldPalette.valueFor(id), dummyResizer);
         }

         buffer[index] = lastWrittenId;
      }

      return buffer;
   }

   @Override
   public int getSerializedSize() {
      return this.data.getSerializedSize(this.strategy.globalMap());
   }

   @Override
   public int bitsPerEntry() {
      return this.data.storage().getBits();
   }

   @Override
   public boolean maybeHas(Predicate<T> predicate) {
      return this.data.palette.maybeHas(predicate);
   }

   @Override
   public void forEachInPalette(Consumer<T> consumer) {
      for (int i = 0; i < this.data.palette.getSize(); i++) {
         consumer.accept(this.data.palette.valueFor(i));
      }
   }

   @Override
   public PalettedContainer<T> copy() {
      return new PalettedContainer<>(this, this.presetValues);
   }

   @Override
   public PalettedContainer<T> recreate() {
      return new PalettedContainer<>(this.data.palette.valueFor(0), this.strategy, this.presetValues);
   }

   @Override
   public void count(PalettedContainer.CountConsumer<T> output) {
      if (this.data.palette.getSize() == 1) {
         output.accept(this.data.palette.valueFor(0), this.data.storage.getSize());
      } else {
         Int2IntOpenHashMap counts = new Int2IntOpenHashMap();
         this.data.storage.getAll(state -> counts.addTo(state, 1));
         counts.int2IntEntrySet().forEach(entry -> output.accept(this.data.palette.valueFor(entry.getIntKey()), entry.getIntValue()));
      }
   }

   @FunctionalInterface
   public interface CountConsumer<T> {
      void accept(T var1, int var2);
   }

   public static record Data<T>(Configuration configuration, BitStorage storage, Palette<T> palette) {
      public void copyFrom(Palette<T> oldPalette, BitStorage oldStorage) {
         PaletteResize<T> dummyResizer = PaletteResize.noResizeExpected();
         if (oldPalette.getSize() == 1) {
            T value = oldPalette.valueFor(0);
            this.storage.fill(this.palette.idFor(value, dummyResizer));
         } else {
            int lastOldId = -1;
            int newId = -1;

            for (int i = 0; i < oldStorage.getSize(); i++) {
               int oldId = oldStorage.get(i);
               if (oldId != lastOldId) {
                  T value = oldPalette.valueFor(oldId);
                  newId = this.palette.idFor(value, dummyResizer);
                  lastOldId = oldId;
               }

               this.storage.set(i, newId);
            }
         }
      }

      public int getSerializedSize(IdMap<T> globalMap) {
         return 1 + this.palette.getSerializedSize(globalMap) + this.storage.getRaw().length * 8;
      }

      public void write(FriendlyByteBuf buffer, IdMap<T> globalMap, @Nullable ChunkPacketInfo<T> chunkPacketInfo, int chunkSectionIndex) {
         buffer.writeByte(this.storage.getBits());
         this.palette.write(buffer, globalMap);
         if (chunkPacketInfo != null) {
            chunkPacketInfo.setBits(chunkSectionIndex, this.configuration.bitsInMemory());
            chunkPacketInfo.setPalette(chunkSectionIndex, this.palette);
            chunkPacketInfo.setIndex(chunkSectionIndex, buffer.writerIndex());
         }

         buffer.writeFixedSizeLongArray(this.storage.getRaw());
      }

      public PalettedContainer.Data<T> copy() {
         return new PalettedContainer.Data<>(this.configuration, this.storage.copy(), this.palette.copy());
      }
   }
}
