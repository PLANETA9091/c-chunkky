package net.minecraft.world.level.chunk.storage;

import ca.spottedleaf.moonrise.patches.chunk_system.io.ChunkSystemRegionFileStorage;
import ca.spottedleaf.moonrise.patches.chunk_system.io.MoonriseRegionFileIO.RegionDataController.ReadData;
import ca.spottedleaf.moonrise.patches.chunk_system.io.MoonriseRegionFileIO.RegionDataController.WriteData;
import ca.spottedleaf.moonrise.patches.chunk_system.io.MoonriseRegionFileIO.RegionDataController.ReadData.ReadResult;
import ca.spottedleaf.moonrise.patches.chunk_system.io.MoonriseRegionFileIO.RegionDataController.WriteData.WriteResult;
import ca.spottedleaf.moonrise.patches.chunk_system.storage.RegionFileRecovery;
import ca.spottedleaf.moonrise.patches.chunk_system.util.stream.ExternalChunkStreamMarker;
import ca.spottedleaf.moonrise.patches.nbt_rw.FastNBTParser;
import com.mojang.logging.LogUtils;
import io.papermc.paper.configuration.GlobalConfiguration;
import it.unimi.dsi.fastutil.longs.Long2ObjectLinkedOpenHashMap;
import it.unimi.dsi.fastutil.longs.LongLinkedOpenHashSet;
import it.unimi.dsi.fastutil.objects.ObjectIterator;
import java.io.DataInputStream;
import java.io.DataOutputStream;
import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Objects;
import java.util.Optional;
import net.minecraft.SharedConstants;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.nbt.NbtAccounter;
import net.minecraft.nbt.NbtIo;
import net.minecraft.nbt.StreamTagVisitor;
import net.minecraft.util.ExceptionCollector;
import net.minecraft.util.FileUtil;
import net.minecraft.world.level.ChunkPos;
import org.jspecify.annotations.Nullable;
import org.slf4j.Logger;

public class RegionFileStorage implements AutoCloseable, ChunkSystemRegionFileStorage {
   private static final Logger LOGGER = LogUtils.getLogger();
   public static final String ANVIL_EXTENSION = ".mca";
   private static final int MAX_CACHE_SIZE = 256;
   private final Long2ObjectLinkedOpenHashMap<Optional<RegionFile>> regionCache = new Long2ObjectLinkedOpenHashMap();
   private final RegionStorageInfo info;
   private final Path folder;
   private final boolean sync;
   private static final int REGION_SHIFT = 5;
   private static final int MAX_NON_EXISTING_CACHE = 4096;
   private final LongLinkedOpenHashSet nonExistingRegionFiles = new LongLinkedOpenHashSet();

   private static String getRegionFileName(int chunkX, int chunkZ) {
      return "r." + (chunkX >> 5) + "." + (chunkZ >> 5) + ".mca";
   }

   private boolean doesRegionFilePossiblyExist(long position) {
      synchronized (this.nonExistingRegionFiles) {
         if (this.nonExistingRegionFiles.contains(position)) {
            this.nonExistingRegionFiles.addAndMoveToFirst(position);
            return false;
         } else {
            return true;
         }
      }
   }

   private void createRegionFile(long position) {
      synchronized (this.nonExistingRegionFiles) {
         this.nonExistingRegionFiles.remove(position);
      }
   }

   private void markNonExisting(long position) {
      synchronized (this.nonExistingRegionFiles) {
         if (this.nonExistingRegionFiles.addAndMoveToFirst(position)) {
            while (this.nonExistingRegionFiles.size() >= 4096) {
               this.nonExistingRegionFiles.removeLastLong();
            }
         }
      }
   }

   @Override
   public final boolean moonrise$doesRegionFileNotExistNoIO(int chunkX, int chunkZ) {
      return !this.doesRegionFilePossiblyExist(ChunkPos.pack(chunkX >> 5, chunkZ >> 5));
   }

   @Override
   public final synchronized RegionFile moonrise$getRegionFileIfLoaded(int chunkX, int chunkZ) {
      Optional<RegionFile> ret = (Optional<RegionFile>)this.regionCache.getAndMoveToFirst(ChunkPos.pack(chunkX >> 5, chunkZ >> 5));
      return ret != null && !ret.isEmpty() ? ret.get() : null;
   }

   @Override
   public final synchronized RegionFile moonrise$getRegionFileIfExists(int chunkX, int chunkZ) throws IOException {
      return this.getRegionFile(new ChunkPos(chunkX, chunkZ), false);
   }

   @Override
   public final WriteData moonrise$startWrite(int chunkX, int chunkZ, CompoundTag compound) throws IOException {
      if (compound == null) {
         return new WriteData(compound, WriteResult.DELETE, null, null);
      } else {
         ChunkPos pos = new ChunkPos(chunkX, chunkZ);
         RegionFile regionFile = this.getRegionFile(pos, true);
         WriteData writeData = regionFile.moonrise$startWrite(compound, pos);

         try {
            try {
               NbtIo.write(compound, writeData.output());
            } finally {
               writeData.output().close();
            }

            return writeData;
         } catch (RegionFileStorage.RegionFileSizeException var12) {
            int maxSize = 500;
            LOGGER.error(
               "Chunk at ("
                  + chunkX
                  + ","
                  + chunkZ
                  + ") in regionfile '"
                  + regionFile.getPath().toString()
                  + "' exceeds max size of 500MiB, it has been deleted from disk."
            );
            return new WriteData(compound, WriteResult.DELETE, null, null);
         }
      }
   }

   @Override
   public final void moonrise$finishWrite(int chunkX, int chunkZ, WriteData writeData) throws IOException {
      ChunkPos pos = new ChunkPos(chunkX, chunkZ);
      if (writeData.result() == WriteResult.DELETE) {
         RegionFile regionFile = this.moonrise$getRegionFileIfExists(chunkX, chunkZ);
         if (regionFile != null) {
            regionFile.clear(pos);
         }
      } else {
         writeData.write().run(this.getRegionFile(pos, true));
      }
   }

   @Override
   public final ReadData moonrise$readData(int chunkX, int chunkZ) throws IOException {
      RegionFile regionFile = this.moonrise$getRegionFileIfExists(chunkX, chunkZ);
      int recalculateCount;
      DataInputStream input;
      if (regionFile == null) {
         recalculateCount = 0;
         input = null;
      } else {
         synchronized (regionFile) {
            input = regionFile.getChunkDataInputStream(new ChunkPos(chunkX, chunkZ));
            recalculateCount = regionFile.moonrise$getRecalculateCount();
         }
      }

      if (input == null) {
         return new ReadData(ReadResult.NO_DATA, null, null, recalculateCount);
      } else {
         ReadData ret = new ReadData(ReadResult.HAS_DATA, input, null, recalculateCount);
         if (!(input instanceof ExternalChunkStreamMarker)) {
            return ret;
         } else {
            CompoundTag syncRead = this.moonrise$finishRead(chunkX, chunkZ, ret);
            return syncRead == null ? this.moonrise$readData(chunkX, chunkZ) : new ReadData(ReadResult.SYNC_READ, null, syncRead, recalculateCount);
         }
      }
   }

   @Override
   public final CompoundTag moonrise$finishRead(int chunkX, int chunkZ, ReadData readData) throws IOException {
      CompoundTag ret;
      try {
         ret = FastNBTParser.nbtIoRead(readData.input());
      } finally {
         readData.input().close();
      }

      if (!RegionFileRecovery.isChunkStorage(this.info)) {
         return ret;
      } else {
         RegionFile regionFile = this.moonrise$getRegionFileIfExists(chunkX, chunkZ);
         if (regionFile != null && regionFile.moonrise$getRecalculateCount() == readData.recalculateCount()) {
            ChunkPos pos = new ChunkPos(chunkX, chunkZ);
            ChunkPos storedPos = RegionFileRecovery.getChunkCoordinate(ret);
            if (storedPos.equals(pos)) {
               return ret;
            } else {
               LOGGER.error(
                  "Attempting to read chunk data at {} but got chunk data for {} instead! Attempting regionfile recalculation for regionfile {}",
                  new Object[]{pos, storedPos, regionFile.getPath().toAbsolutePath()}
               );
               Boolean recalc = regionFile.moonrise$tryRecalcHeader(readData.recalculateCount());
               if (recalc != null && !recalc) {
                  LOGGER.error("Can't recalculate regionfile header for regionfile {}", regionFile.getPath().toAbsolutePath());
                  return ret;
               } else {
                  return null;
               }
            }
         } else {
            return null;
         }
      }
   }

   public RegionFileStorage(RegionStorageInfo info, Path folder, boolean sync) {
      this.folder = folder;
      this.sync = sync;
      this.info = info;
   }

   @Nullable
   private RegionFile getRegionFile(ChunkPos pos, boolean create) throws IOException {
      synchronized (this) {
         long key = ChunkPos.pack(pos.x() >> 5, pos.z() >> 5);
         Optional<RegionFile> cached = (Optional<RegionFile>)this.regionCache.getAndMoveToFirst(key);
         if (cached != null) {
            if (cached.isPresent()) {
               return cached.get();
            }

            if (!create) {
               return null;
            }
         }

         if (!create && !this.doesRegionFilePossiblyExist(key)) {
            return null;
         } else {
            Path regionPath = this.folder.resolve(getRegionFileName(pos.x(), pos.z()));
            if (!create && !Files.isRegularFile(regionPath)) {
               this.markNonExisting(key);
               return null;
            } else {
               int cacheSize = GlobalConfiguration.get() == null ? 256 : GlobalConfiguration.get().misc.regionFileCacheSize;
               if (this.regionCache.size() >= cacheSize) {
                  Optional<RegionFile> evicted = (Optional<RegionFile>)this.regionCache.removeLast();
                  if (evicted.isPresent()) {
                     evicted.get().close();
                  }
               }

               this.createRegionFile(key);
               FileUtil.createDirectoriesSafe(this.folder);
               RegionFile ret = new RegionFile(this.info, regionPath, this.folder, this.sync);
               this.regionCache.putAndMoveToFirst(key, Optional.of(ret));
               return ret;
            }
         }
      }
   }

   private RegionFile getOrCreateRegionFile(ChunkPos pos) throws IOException {
      return Objects.requireNonNull(this.getRegionFile(pos, true));
   }

   @Nullable
   public CompoundTag read(ChunkPos pos) throws IOException {
      RegionFile region = this.getRegionFile(pos, false);
      if (region == null) {
         return null;
      } else {
         CompoundTag serialisedChunkData;
         try (DataInputStream regionChunkInputStream = region.getChunkDataInputStream(pos)) {
            serialisedChunkData = regionChunkInputStream == null ? null : FastNBTParser.nbtIoRead(regionChunkInputStream);
         }

         if (serialisedChunkData != null && RegionFileRecovery.isChunkStorage(this.info)) {
            ChunkPos storedPos = RegionFileRecovery.getChunkCoordinate(serialisedChunkData);
            if (storedPos.equals(pos)) {
               return serialisedChunkData;
            } else {
               RegionFile regionFile = this.getRegionFile(pos, false);
               if (regionFile == null) {
                  return serialisedChunkData;
               } else {
                  LOGGER.error(
                     "Attempting to read chunk data at {} but got chunk data for {} instead! Attempting regionfile recalculation for regionfile {}",
                     new Object[]{pos, storedPos, regionFile.getPath().toAbsolutePath()}
                  );
                  if (!regionFile.moonrise$recalculateHeader()) {
                     LOGGER.error("Can't recalculate regionfile header for regionfile {}", regionFile.getPath().toAbsolutePath());
                     return serialisedChunkData;
                  } else {
                     CompoundTag var7;
                     try (DataInputStream input = regionFile.getChunkDataInputStream(pos)) {
                        var7 = input == null ? null : FastNBTParser.nbtIoRead(input);
                     }

                     return var7;
                  }
               }
            }
         } else {
            return serialisedChunkData;
         }
      }
   }

   public void scanChunk(ChunkPos pos, StreamTagVisitor scanner) throws IOException {
      RegionFile region = this.getRegionFile(pos, false);
      if (region != null) {
         try (DataInputStream regionChunkInputStream = region.getChunkDataInputStream(pos)) {
            if (regionChunkInputStream != null) {
               NbtIo.parse(regionChunkInputStream, scanner, NbtAccounter.unlimitedHeap());
            }
         }
      }
   }

   public void write(ChunkPos pos, @Nullable CompoundTag value) throws IOException {
      if (value != null || this.moonrise$getRegionFileIfExists(pos.x(), pos.z()) != null) {
         if (!SharedConstants.DEBUG_DONT_SAVE_WORLD) {
            RegionFile region = this.getOrCreateRegionFile(pos);
            if (value == null) {
               region.clear(pos);
            } else {
               DataOutputStream output = region.getChunkDataOutputStream(pos);

               try {
                  NbtIo.write(value, output);
                  output.close();
               } catch (RegionFileStorage.RegionFileSizeException var7) {
                  region.clear(pos);
                  int maxSize = 500;
                  LOGGER.error(
                     "Chunk at ({},{}) in regionfile '{}' exceeds max size of {}MiB, it has been deleted from disk.",
                     new Object[]{pos.x(), pos.z(), region.getPath().toString(), 500}
                  );
               }
            }
         }
      }
   }

   @Override
   public void close() throws IOException {
      synchronized (this) {
         ExceptionCollector<IOException> exceptionCollector = new ExceptionCollector<>();
         ObjectIterator var3 = this.regionCache.values().iterator();

         while (var3.hasNext()) {
            Optional<RegionFile> entry = (Optional<RegionFile>)var3.next();
            if (!entry.isEmpty()) {
               try {
                  entry.get().close();
               } catch (IOException var7) {
                  exceptionCollector.add(var7);
               }
            }
         }

         exceptionCollector.throwIfPresent();
      }
   }

   public void flush() throws IOException {
      synchronized (this) {
         ExceptionCollector<IOException> exceptionCollector = new ExceptionCollector<>();
         ObjectIterator var3 = this.regionCache.values().iterator();

         while (var3.hasNext()) {
            Optional<RegionFile> entry = (Optional<RegionFile>)var3.next();
            if (!entry.isEmpty()) {
               try {
                  entry.get().flush();
               } catch (IOException var7) {
                  exceptionCollector.add(var7);
               }
            }
         }

         exceptionCollector.throwIfPresent();
      }
   }

   public RegionStorageInfo info() {
      return this.info;
   }

   public static final class RegionFileSizeException extends RuntimeException {
      public RegionFileSizeException(String message) {
         super(message);
      }
   }
}
