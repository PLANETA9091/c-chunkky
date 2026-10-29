package net.minecraft.world.level.chunk.storage;

import ca.spottedleaf.moonrise.patches.chunk_system.level.storage.ChunkSystemSectionStorage;
import com.google.common.collect.ImmutableMap;
import com.google.common.collect.Maps;
import com.mojang.logging.LogUtils;
import com.mojang.serialization.Codec;
import com.mojang.serialization.DataResult;
import com.mojang.serialization.Dynamic;
import com.mojang.serialization.DynamicOps;
import com.mojang.serialization.OptionalDynamic;
import it.unimi.dsi.fastutil.ints.Int2ObjectMap;
import it.unimi.dsi.fastutil.ints.Int2ObjectOpenHashMap;
import it.unimi.dsi.fastutil.longs.Long2ObjectMap;
import it.unimi.dsi.fastutil.longs.Long2ObjectMaps;
import it.unimi.dsi.fastutil.longs.Long2ObjectOpenHashMap;
import it.unimi.dsi.fastutil.longs.LongIterator;
import it.unimi.dsi.fastutil.longs.LongLinkedOpenHashSet;
import it.unimi.dsi.fastutil.longs.LongOpenHashSet;
import it.unimi.dsi.fastutil.longs.LongSet;
import it.unimi.dsi.fastutil.longs.Long2ObjectMap.Entry;
import java.io.IOException;
import java.util.Iterator;
import java.util.Map;
import java.util.Optional;
import java.util.concurrent.CompletableFuture;
import java.util.function.BiFunction;
import java.util.function.BooleanSupplier;
import java.util.function.Function;
import net.minecraft.SharedConstants;
import net.minecraft.core.RegistryAccess;
import net.minecraft.core.SectionPos;
import net.minecraft.nbt.Tag;
import net.minecraft.util.Util;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.LevelHeightAccessor;
import org.jspecify.annotations.Nullable;
import org.slf4j.Logger;

public class SectionStorage<R, P> implements AutoCloseable, ChunkSystemSectionStorage {
   private static final Logger LOGGER = LogUtils.getLogger();
   private static final String SECTIONS_TAG = "Sections";
   private final Long2ObjectMap<Optional<R>> storage = new Long2ObjectOpenHashMap();
   private final LongLinkedOpenHashSet dirtyChunks = new LongLinkedOpenHashSet();
   private final Codec<P> codec;
   private final Function<R, P> packer;
   private final BiFunction<P, Runnable, R> unpacker;
   private final Function<Runnable, R> factory;
   private final RegistryAccess registryAccess;
   private final ChunkIOErrorReporter errorReporter;
   public final LevelHeightAccessor levelHeightAccessor;
   private final LongSet loadedChunks = new LongOpenHashSet();
   private final Long2ObjectMap<CompletableFuture<Optional<SectionStorage.PackedChunk<P>>>> pendingLoads = new Long2ObjectOpenHashMap();
   private final Object loadLock = new Object();
   private final RegionFileStorage regionStorage;

   @Override
   public final RegionFileStorage moonrise$getRegionStorage() {
      return this.regionStorage;
   }

   @Override
   public void moonrise$close() throws IOException {
   }

   public SectionStorage(
      SimpleRegionStorage simpleRegionStorage,
      Codec<P> codec,
      Function<R, P> packer,
      BiFunction<P, Runnable, R> unpacker,
      Function<Runnable, R> factory,
      RegistryAccess registryAccess,
      ChunkIOErrorReporter errorReporter,
      LevelHeightAccessor levelHeightAccessor
   ) {
      this.codec = codec;
      this.packer = packer;
      this.unpacker = unpacker;
      this.factory = factory;
      this.registryAccess = registryAccess;
      this.errorReporter = errorReporter;
      this.levelHeightAccessor = levelHeightAccessor;
      this.regionStorage = simpleRegionStorage.moonrise$getRegionStorage();
   }

   protected void tick(BooleanSupplier haveTime) {
      LongIterator iterator = this.dirtyChunks.iterator();

      while (iterator.hasNext() && haveTime.getAsBoolean()) {
         ChunkPos chunkPos = ChunkPos.unpack(iterator.nextLong());
         iterator.remove();
         this.writeChunk(chunkPos);
      }

      this.unpackPendingLoads();
   }

   private void unpackPendingLoads() {
      synchronized (this.loadLock) {
         Iterator<Entry<CompletableFuture<Optional<SectionStorage.PackedChunk<P>>>>> iterator = Long2ObjectMaps.fastIterator(this.pendingLoads);

         while (iterator.hasNext()) {
            Entry<CompletableFuture<Optional<SectionStorage.PackedChunk<P>>>> entry = iterator.next();
            Optional<SectionStorage.PackedChunk<P>> chunk = (Optional<SectionStorage.PackedChunk<P>>)((CompletableFuture)entry.getValue()).getNow(null);
            if (chunk != null) {
               long chunkKey = entry.getLongKey();
               this.unpackChunk(ChunkPos.unpack(chunkKey), chunk.orElse(null));
               iterator.remove();
               this.loadedChunks.add(chunkKey);
            }
         }
      }
   }

   public void flushAll() {
      if (!this.dirtyChunks.isEmpty()) {
         this.dirtyChunks.forEach(pos -> this.writeChunk(ChunkPos.unpack(pos)));
         this.dirtyChunks.clear();
      }
   }

   public boolean hasWork() {
      return !this.dirtyChunks.isEmpty();
   }

   @Nullable
   public Optional<R> get(long sectionPos) {
      return (Optional<R>)this.storage.get(sectionPos);
   }

   public Optional<R> getOrLoad(long sectionPos) {
      if (this.outsideStoredRange(sectionPos)) {
         return Optional.empty();
      } else {
         Optional<R> r = this.get(sectionPos);
         if (r != null) {
            return r;
         } else {
            this.unpackChunk(SectionPos.of(sectionPos).chunk());
            r = this.get(sectionPos);
            if (r == null) {
               throw (IllegalStateException)Util.pauseInIde(new IllegalStateException());
            } else {
               return r;
            }
         }
      }
   }

   protected boolean outsideStoredRange(long sectionPos) {
      int y = SectionPos.sectionToBlockCoord(SectionPos.y(sectionPos));
      return this.levelHeightAccessor.isOutsideBuildHeight(y);
   }

   protected R getOrCreate(long sectionPos) {
      if (this.outsideStoredRange(sectionPos)) {
         throw (IllegalArgumentException)Util.pauseInIde(new IllegalArgumentException("sectionPos out of bounds"));
      } else {
         Optional<R> r = this.getOrLoad(sectionPos);
         if (r.isPresent()) {
            return r.get();
         } else {
            R newR = this.factory.apply(() -> this.setDirty(sectionPos));
            this.storage.put(sectionPos, Optional.of(newR));
            return newR;
         }
      }
   }

   public CompletableFuture<?> prefetch(ChunkPos chunkPos) {
      synchronized (this.loadLock) {
         long chunkKey = chunkPos.pack();
         return this.loadedChunks.contains(chunkKey)
            ? CompletableFuture.completedFuture(null)
            : (CompletableFuture)this.pendingLoads.computeIfAbsent(chunkKey, k -> this.tryRead(chunkPos));
      }
   }

   private void unpackChunk(ChunkPos chunkPos) {
      long chunkKey = chunkPos.pack();
      CompletableFuture<Optional<SectionStorage.PackedChunk<P>>> future;
      synchronized (this.loadLock) {
         if (!this.loadedChunks.add(chunkKey)) {
            return;
         }

         future = (CompletableFuture<Optional<SectionStorage.PackedChunk<P>>>)this.pendingLoads.computeIfAbsent(chunkKey, k -> this.tryRead(chunkPos));
      }

      this.unpackChunk(chunkPos, future.join().orElse(null));
      synchronized (this.loadLock) {
         this.pendingLoads.remove(chunkKey);
      }
   }

   private CompletableFuture<Optional<SectionStorage.PackedChunk<P>>> tryRead(ChunkPos chunkPos) {
      throw new IllegalStateException("Only chunk system can write state, offending class:" + this.getClass().getName());
   }

   private void unpackChunk(ChunkPos pos, @Nullable SectionStorage.PackedChunk<P> packedChunk) {
      throw new IllegalStateException("Only chunk system can load in state, offending class:" + this.getClass().getName());
   }

   private void writeChunk(ChunkPos chunkPos) {
      throw new IllegalStateException("Only chunk system can write state, offending class:" + this.getClass().getName());
   }

   private <T> Dynamic<T> writeChunk(ChunkPos chunkPos, DynamicOps<T> ops) {
      Map<T, T> sections = Maps.newHashMap();

      for (int sectionY = this.levelHeightAccessor.getMinSectionY(); sectionY <= this.levelHeightAccessor.getMaxSectionY(); sectionY++) {
         long key = getKey(chunkPos, sectionY);
         Optional<R> r = (Optional<R>)this.storage.get(key);
         if (r != null && !r.isEmpty()) {
            DataResult<T> serializedSection = this.codec.encodeStart(ops, this.packer.apply(r.get()));
            String yName = Integer.toString(sectionY);
            serializedSection.resultOrPartial(LOGGER::error).ifPresent(s -> sections.put((T)ops.createString(yName), (T)s));
         }
      }

      return new Dynamic<>(
         ops,
         (T)ops.createMap(
            ImmutableMap.of(
               ops.createString("Sections"),
               ops.createMap(sections),
               ops.createString("DataVersion"),
               ops.createInt(SharedConstants.getCurrentVersion().dataVersion().version())
            )
         )
      );
   }

   private static long getKey(ChunkPos chunkPos, int sectionY) {
      return SectionPos.asLong(chunkPos.x(), sectionY, chunkPos.z());
   }

   protected void onSectionLoad(long sectionPos) {
   }

   public void setDirty(long sectionPos) {
      Optional<R> r = (Optional<R>)this.storage.get(sectionPos);
      if (r != null && !r.isEmpty()) {
         this.dirtyChunks.add(ChunkPos.pack(SectionPos.x(sectionPos), SectionPos.z(sectionPos)));
      } else {
         LOGGER.warn("No data for position: {}", SectionPos.of(sectionPos));
      }
   }

   public void flush(ChunkPos chunkPos) {
      if (this.dirtyChunks.remove(chunkPos.pack())) {
         this.writeChunk(chunkPos);
      }
   }

   @Override
   public void close() throws IOException {
      this.moonrise$close();
   }

   private static record PackedChunk<T>(Int2ObjectMap<T> sectionsByY, boolean versionChanged) {
      public static <T> SectionStorage.PackedChunk<T> parse(
         Codec<T> codec, DynamicOps<Tag> ops, Tag tag, SimpleRegionStorage simpleRegionStorage, LevelHeightAccessor levelHeightAccessor
      ) {
         Dynamic<Tag> originalTag = new Dynamic<>(ops, tag);
         Dynamic<Tag> fixedTag = simpleRegionStorage.upgradeChunkTag(originalTag, 1945);
         boolean versionChanged = originalTag != fixedTag;
         OptionalDynamic<Tag> sections = fixedTag.get("Sections");
         Int2ObjectMap<T> sectionsByY = new Int2ObjectOpenHashMap();

         for (int sectionY = levelHeightAccessor.getMinSectionY(); sectionY <= levelHeightAccessor.getMaxSectionY(); sectionY++) {
            Optional<T> section = sections.get(Integer.toString(sectionY))
               .result()
               .flatMap(sectionData -> codec.parse(sectionData).resultOrPartial(SectionStorage.LOGGER::error));
            if (section.isPresent()) {
               sectionsByY.put(sectionY, section.get());
            }
         }

         return new SectionStorage.PackedChunk<>(sectionsByY, versionChanged);
      }
   }
}
