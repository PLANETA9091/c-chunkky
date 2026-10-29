package net.minecraft.world.level.chunk.storage;

import ca.spottedleaf.moonrise.patches.chunk_system.io.MoonriseRegionFileIO.RegionDataController.WriteData;
import ca.spottedleaf.moonrise.patches.chunk_system.io.MoonriseRegionFileIO.RegionDataController.WriteData.WriteResult;
import ca.spottedleaf.moonrise.patches.chunk_system.storage.ChunkSystemChunkBuffer;
import ca.spottedleaf.moonrise.patches.chunk_system.storage.ChunkSystemRegionFile;
import ca.spottedleaf.moonrise.patches.chunk_system.storage.RegionFileRecovery;
import ca.spottedleaf.moonrise.patches.chunk_system.util.stream.ExternalChunkStreamMarker;
import ca.spottedleaf.moonrise.patches.nbt_rw.FastNBTParser;
import com.destroystokyo.paper.exception.ServerInternalException;
import com.google.common.annotations.VisibleForTesting;
import com.mojang.logging.LogUtils;
import it.unimi.dsi.fastutil.objects.ObjectIterator;
import java.io.ByteArrayInputStream;
import java.io.ByteArrayOutputStream;
import java.io.DataInputStream;
import java.io.DataOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.nio.ByteBuffer;
import java.nio.IntBuffer;
import java.nio.channels.FileChannel;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardCopyOption;
import java.nio.file.StandardOpenOption;
import java.util.BitSet;
import java.util.Objects;
import java.util.concurrent.ThreadLocalRandom;
import java.util.concurrent.atomic.AtomicInteger;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.resources.Identifier;
import net.minecraft.util.Util;
import net.minecraft.util.profiling.jfr.JvmProfiler;
import net.minecraft.world.level.ChunkPos;
import org.jspecify.annotations.Nullable;
import org.slf4j.Logger;

public class RegionFile implements AutoCloseable, ChunkSystemRegionFile {
   private static final Logger LOGGER = LogUtils.getLogger();
   public static final int MAX_CHUNK_SIZE = 524288000;
   private static final int SECTOR_BYTES = 4096;
   @VisibleForTesting
   protected static final int SECTOR_INTS = 1024;
   private static final int CHUNK_HEADER_SIZE = 5;
   private static final int HEADER_OFFSET = 0;
   private static final ByteBuffer PADDING_BUFFER = ByteBuffer.allocateDirect(1);
   private static final String EXTERNAL_FILE_EXTENSION = ".mcc";
   private static final int EXTERNAL_STREAM_FLAG = 128;
   private static final int EXTERNAL_CHUNK_THRESHOLD = 256;
   private static final int CHUNK_NOT_PRESENT = 0;
   private final RegionStorageInfo info;
   private final Path path;
   private final FileChannel file;
   private final Path externalFileDir;
   private final RegionFileVersion version;
   private final ByteBuffer header = ByteBuffer.allocateDirect(8192);
   private final IntBuffer offsets;
   private final IntBuffer timestamps;
   @VisibleForTesting
   protected final RegionBitmap usedSectors = new RegionBitmap();
   private static final int MAX_INTERNAL_SECTORS = 255;
   private static final CompoundTag EXTERNAL_STUB_COMPOUND = new CompoundTag();
   private final AtomicInteger recalculateCount = new AtomicInteger();

   private static long roundToSectors(long bytes) {
      return bytes + 4095L >>> 12;
   }

   private boolean canRecalcHeader() {
      return RegionFileRecovery.isChunkStorage(this.info) && RegionFileRecovery.getRegionFileCoordinates(this.path) != null;
   }

   private int readFully(ByteBuffer buffer, long position) throws IOException {
      int total = 0;

      while (buffer.hasRemaining()) {
         int read = this.file.read(buffer, position);
         if (read < 0) {
            break;
         }

         total += read;
         position += (long)read;
      }

      return total;
   }

   private int readLength(long sector) throws IOException {
      ByteBuffer length = ByteBuffer.allocate(4);
      return 4 != this.readFully(length, sector * 4096L) ? -1 : length.getInt(0);
   }

   @Nullable
   private CompoundTag attemptRead(long sector, int chunkDataLength, long fileLength) {
      try {
         if (chunkDataLength <= 0) {
            return null;
         } else {
            long offset = sector * 4096L + 4L;
            if (offset + (long)chunkDataLength > fileLength) {
               return null;
            } else {
               ByteBuffer chunkData = ByteBuffer.allocate(chunkDataLength);
               if (chunkDataLength != this.readFully(chunkData, offset)) {
                  return null;
               } else {
                  chunkData.flip();
                  byte compressionType = chunkData.get();
                  if (isExternalStreamChunk(compressionType)) {
                     return EXTERNAL_STUB_COMPOUND;
                  } else {
                     RegionFileVersion compression = RegionFileVersion.fromId(compressionType);
                     if (compression == null) {
                        return null;
                     } else {
                        CompoundTag var12;
                        try (DataInputStream input = new DataInputStream(compression.wrap(createStream(chunkData, chunkData.remaining())))) {
                           var12 = FastNBTParser.nbtIoRead(input);
                        }

                        return var12;
                     }
                  }
               }
            }
         }
      } catch (Exception var16) {
         return null;
      }
   }

   @Nullable
   private RegionFileVersion moveToExternalFile(ChunkPos pos, int sectorOffset, int rawLength) {
      Path externalFile = this.getExternalChunkPath(pos);

      try {
         ByteBuffer chunkData = ByteBuffer.allocate(rawLength - 4);
         if (chunkData.remaining() != this.readFully(chunkData, (long)sectorOffset * 4096L + 4L)) {
            throw new IOException("Unexpected end of file");
         } else {
            chunkData.flip();
            RegionFileVersion compression = RegionFileVersion.fromId(chunkData.get());
            Path tmp = Files.createTempFile(this.externalFileDir, "tmp", null);

            try {
               try (FileChannel out = FileChannel.open(tmp, StandardOpenOption.WRITE)) {
                  while (chunkData.hasRemaining()) {
                     out.write(chunkData);
                  }

                  out.force(true);
               }

               Files.move(tmp, externalFile, StandardCopyOption.REPLACE_EXISTING);
            } finally {
               Files.deleteIfExists(tmp);
            }

            return compression;
         }
      } catch (IOException var19) {
         LOGGER.error(
            "Failed to move oversized chunk data for local chunk ({},{}) in regionfile {} to external file {}",
            new Object[]{pos.x() & 31, pos.z() & 31, this.path.toAbsolutePath(), externalFile.toAbsolutePath(), var19}
         );
         return null;
      }
   }

   private synchronized void backupRegionFile() {
      Path backup = this.path.resolveSibling(this.path.getFileName() + "." + Long.toUnsignedString(ThreadLocalRandom.current().nextLong()) + ".backup");

      try {
         this.file.force(true);
         LOGGER.warn("Backing up regionfile \"{}\" to {}", this.path.toAbsolutePath(), backup.toAbsolutePath());
         Files.copy(this.path, backup, StandardCopyOption.COPY_ATTRIBUTES);
         LOGGER.warn("Backed up the regionfile to {}", backup.toAbsolutePath());
      } catch (IOException var3) {
         LOGGER.error("Failed to backup to {}", backup.toAbsolutePath(), var3);
      }
   }

   private void setUsedSectors(BitSet allocated) {
      this.usedSectors.free(0, Integer.MAX_VALUE);
      int start = allocated.nextSetBit(0);

      while (start >= 0) {
         int end = allocated.nextClearBit(start);
         this.usedSectors.force(start, end - start);
         start = allocated.nextSetBit(end);
      }
   }

   @Override
   public final int moonrise$getRecalculateCount() {
      return this.recalculateCount.get();
   }

   @Nullable
   @Override
   public final Boolean moonrise$tryRecalcHeader(int expectedRecalcCount) throws IOException {
      if (!this.canRecalcHeader()) {
         return Boolean.FALSE;
      } else {
         synchronized (this) {
            return this.recalculateCount.get() != expectedRecalcCount ? null : this.moonrise$recalculateHeader();
         }
      }
   }

   @Override
   public final boolean moonrise$recalculateHeader() throws IOException {
      if (!this.canRecalcHeader()) {
         return false;
      } else {
         ChunkPos regionPos = RegionFileRecovery.getRegionFileCoordinates(this.path);
         synchronized (this) {
            this.recalculateCount.getAndIncrement();
            LOGGER.warn(
               "Corrupt regionfile header detected! Attempting to re-calculate header offsets for regionfile {}", this.path.toAbsolutePath(), new Throwable()
            );
            this.backupRegionFile();
            boolean[] hasInternal = new boolean[1024];
            long[] saveTimes = new long[1024];
            int[] rawLengths = new int[1024];
            int[] sectorOffsets = new int[1024];
            long fileLength = this.file.size();
            long totalSectors = roundToSectors(fileLength);
            long i = 2L;

            for (long maxSector = Math.min(8388607L, totalSectors); i < maxSector; i++) {
               int chunkDataLength = this.readLength(i);
               CompoundTag compound = this.attemptRead(i, chunkDataLength, fileLength);
               if (compound != null && compound != EXTERNAL_STUB_COMPOUND) {
                  int rawLength = chunkDataLength + 4;
                  long sector = i;
                  i += roundToSectors((long)rawLength) - 1L;
                  ChunkPos chunkPos = RegionFileRecovery.getChunkCoordinate(compound);
                  if (!RegionFileRecovery.inSameRegionFile(regionPos, chunkPos)) {
                     LOGGER.error(
                        "Ignoring absolute chunk {} in regionfile as it is not contained in the bounds of the regionfile '{}'. It should be in regionfile ({},{})",
                        new Object[]{chunkPos, this.path.toAbsolutePath(), chunkPos.x() >> 5, chunkPos.z() >> 5}
                     );
                  } else {
                     int location = chunkPos.x() & 31 | (chunkPos.z() & 31) << 5;
                     long saveTime = RegionFileRecovery.getLastWorldSaveTime(compound);
                     if (!hasInternal[location] || saveTimes[location] <= saveTime) {
                        hasInternal[location] = true;
                        saveTimes[location] = saveTime;
                        rawLengths[location] = rawLength;
                        sectorOffsets[location] = (int)sector;
                     }
                  }
               }
            }

            boolean[] useExternal = new boolean[1024];
            RegionFileVersion[] externalCompressionTypes = new RegionFileVersion[1024];

            for (int location = 0; location < 1024; location++) {
               ChunkPos externalPos = new ChunkPos(regionPos.x() | location & 31, regionPos.z() | location >>> 5);
               Path externalFile = this.getExternalChunkPath(externalPos);
               if (Files.isRegularFile(externalFile)) {
                  byte[] chunkData;
                  try {
                     chunkData = Files.readAllBytes(externalFile);
                  } catch (Exception var30) {
                     LOGGER.error("Failed to read external chunk data in file {}, data will be lost", externalFile.toAbsolutePath(), var30);
                     continue;
                  }

                  CompoundTag compound = null;
                  RegionFileVersion compression = null;
                  ObjectIterator compressionx = RegionFileVersion.VERSIONS.values().iterator();

                  while (compressionx.hasNext()) {
                     RegionFileVersion compressionType = (RegionFileVersion)compressionx.next();

                     CompoundTag read;
                     try (DataInputStream input = new DataInputStream(compressionType.wrap(new ByteArrayInputStream(chunkData)))) {
                        read = FastNBTParser.nbtIoRead(input);
                     } catch (Exception var31) {
                        continue;
                     }

                     compound = read;
                     compression = compressionType;
                     break;
                  }

                  if (compound == null) {
                     LOGGER.error("Failed to read external chunk data in file {}, it's corrupt. Its data will be lost", externalFile.toAbsolutePath());
                  } else {
                     ChunkPos storedPos = RegionFileRecovery.getChunkCoordinate(compound);
                     if (!storedPos.equals(externalPos)) {
                        LOGGER.error(
                           "Can't use external chunk stored in {}, got absolute chunkpos: {}, expected {}",
                           new Object[]{externalFile.toAbsolutePath(), storedPos, externalPos}
                        );
                     } else if (!hasInternal[location] || RegionFileRecovery.getLastWorldSaveTime(compound) > saveTimes[location]) {
                        useExternal[location] = true;
                        externalCompressionTypes[location] = compression;
                     }
                  }
               }
            }

            int[] calculatedOffsets = new int[1024];
            BitSet newSectorAllocations = new BitSet();
            newSectorAllocations.set(0, 2);

            for (int locationx = 0; locationx < 1024; locationx++) {
               if (!useExternal[locationx] && hasInternal[locationx]) {
                  int sectorOffset = sectorOffsets[locationx];
                  int sectorLength = (int)roundToSectors((long)rawLengths[locationx]);
                  if (sectorLength > 255) {
                     ChunkPos pos = new ChunkPos(regionPos.x() | locationx & 31, regionPos.z() | locationx >>> 5);
                     RegionFileVersion compression = this.moveToExternalFile(pos, sectorOffset, rawLengths[locationx]);
                     if (compression != null) {
                        LOGGER.warn(
                           "Moved oversized data for local chunk ({},{}) in regionfile {} to external file",
                           new Object[]{locationx & 31, locationx >>> 5, this.path.toAbsolutePath()}
                        );
                        useExternal[locationx] = true;
                        externalCompressionTypes[locationx] = compression;
                     }
                  } else if (RegionFileRecovery.tryAllocate(newSectorAllocations, sectorOffset, sectorLength)) {
                     calculatedOffsets[locationx] = this.packSectorOffset(sectorOffset, sectorLength);
                  } else {
                     LOGGER.error(
                        "Failed to allocate space for local chunk (overlapping data??) at ({},{}) in regionfile {}, chunk will be regenerated",
                        new Object[]{locationx & 31, locationx >>> 5, this.path.toAbsolutePath()}
                     );
                  }
               }
            }

            for (int locationxx = 0; locationxx < 1024; locationxx++) {
               if (useExternal[locationxx]) {
                  int sectorOffset = RegionFileRecovery.allocate(newSectorAllocations, 1);
                  ByteBuffer stub = ByteBuffer.allocate(5);
                  stub.putInt(1);
                  stub.put((byte)(externalCompressionTypes[locationxx].getId() | 128));
                  stub.flip();

                  try {
                     this.file.write(stub, (long)sectorOffset * 4096L);
                     calculatedOffsets[locationxx] = this.packSectorOffset(sectorOffset, 1);
                  } catch (IOException var27) {
                     newSectorAllocations.clear(sectorOffset);
                     LOGGER.error(
                        "Failed to write new external chunk stub, local chunk at ({},{}) in regionfile {} will be regenerated",
                        new Object[]{locationxx & 31, locationxx >>> 5, this.path.toAbsolutePath(), var27}
                     );
                  }
               }
            }

            this.setUsedSectors(newSectorAllocations);
            LOGGER.info("Starting summary of changes for regionfile {}", this.path.toAbsolutePath());

            for (int locationxxx = 0; locationxxx < 1024; locationxxx++) {
               int oldOffset = this.offsets.get(locationxxx);
               int newOffset = calculatedOffsets[locationxxx];
               if (oldOffset != newOffset) {
                  this.offsets.put(locationxxx, newOffset);
                  if (oldOffset == 0) {
                     LOGGER.info(
                        "Found missing data for local chunk ({},{}) in regionfile {}",
                        new Object[]{locationxxx & 31, locationxxx >>> 5, this.path.toAbsolutePath()}
                     );
                  } else if (newOffset == 0) {
                     LOGGER.warn(
                        "Data for local chunk ({},{}) could not be recovered in regionfile {}, it will be regenerated",
                        new Object[]{locationxxx & 31, locationxxx >>> 5, this.path.toAbsolutePath()}
                     );
                  } else {
                     LOGGER.info(
                        "Local chunk ({},{}) changed to point to newer data or correct chunk in regionfile {}",
                        new Object[]{locationxxx & 31, locationxxx >>> 5, this.path.toAbsolutePath()}
                     );
                  }
               }
            }

            LOGGER.info("End of change summary for regionfile {}", this.path.toAbsolutePath());
            int timestamp = getTimestamp();

            for (int locationxxxx = 0; locationxxxx < 1024; locationxxxx++) {
               this.timestamps.put(locationxxxx, calculatedOffsets[locationxxxx] != 0 ? timestamp : 0);
            }

            try {
               this.writeHeader();
               this.file.force(true);
               LOGGER.info("Successfully wrote new header to disk for regionfile {}", this.path.toAbsolutePath());
            } catch (IOException var26) {
               LOGGER.error("Failed to write new header to disk for regionfile {}", this.path.toAbsolutePath(), var26);
            }

            return true;
         }
      }
   }

   @Override
   public final WriteData moonrise$startWrite(CompoundTag data, ChunkPos pos) throws IOException {
      RegionFile.ChunkBuffer buffer = (RegionFile)this.new ChunkBuffer(pos);
      buffer.moonrise$setWriteOnClose(false);
      DataOutputStream out = new DataOutputStream(this.version.wrap(buffer));
      return new WriteData(data, WriteResult.WRITE, out, buffer::moonrise$write);
   }

   public RegionFile(RegionStorageInfo info, Path path, Path externalFileDir, boolean sync) throws IOException {
      this(info, path, externalFileDir, RegionFileVersion.getSelected(), sync);
   }

   public RegionFile(RegionStorageInfo info, Path path, Path externalFileDir, RegionFileVersion version, boolean sync) throws IOException {
      this.info = info;
      this.path = path;
      this.version = version;
      if (!Files.isDirectory(externalFileDir)) {
         throw new IllegalArgumentException("Expected directory, got " + externalFileDir.toAbsolutePath());
      } else {
         this.externalFileDir = externalFileDir;
         this.offsets = this.header.asIntBuffer();
         this.offsets.limit(1024);
         this.header.position(4096);
         this.timestamps = this.header.asIntBuffer();
         if (sync) {
            this.file = FileChannel.open(path, StandardOpenOption.CREATE, StandardOpenOption.READ, StandardOpenOption.WRITE, StandardOpenOption.DSYNC);
         } else {
            this.file = FileChannel.open(path, StandardOpenOption.CREATE, StandardOpenOption.READ, StandardOpenOption.WRITE);
         }

         this.usedSectors.force(0, 2);
         this.header.position(0);
         int readHeaderBytes = this.file.read(this.header, 0L);
         if (readHeaderBytes != -1) {
            if (readHeaderBytes != 8192) {
               LOGGER.warn("Region file {} has truncated header: {}", path, readHeaderBytes);
            }

            long size = Files.size(path);
            boolean canRecalc = this.canRecalcHeader();
            BitSet allocated = new BitSet();
            allocated.set(0, 2);
            boolean needsRecalc = false;
            boolean hasBackedUp = false;

            for (int i = 0; i < 1024; i++) {
               int offset = this.offsets.get(i);
               if (offset != 0) {
                  int sectorNumber = getSectorNumber(offset);
                  int numSectors = getNumSectors(offset);
                  if (numSectors == 255) {
                     ByteBuffer realLen = ByteBuffer.allocate(4);
                     this.file.read(realLen, (long)sectorNumber * 4096L);
                     numSectors = sizeToSectors(realLen.getInt(0) + 4);
                  }

                  if (sectorNumber < 2) {
                     LOGGER.warn("Region file {} has invalid sector at index: {}; sector {} overlaps with header", new Object[]{path, i, sectorNumber});
                  } else if (numSectors == 0) {
                     LOGGER.warn("Region file {} has an invalid sector at index: {}; size has to be > 0", path, i);
                  } else if ((long)sectorNumber * 4096L > size) {
                     LOGGER.warn("Region file {} has an invalid sector at index: {}; sector {} is out of bounds", new Object[]{path, i, sectorNumber});
                  }

                  boolean invalid = sectorNumber < 2 || numSectors <= 0 || (long)sectorNumber * 4096L > size;
                  if (invalid || !RegionFileRecovery.tryAllocate(allocated, sectorNumber, numSectors)) {
                     if (!invalid) {
                        LOGGER.error(
                           "Overlapping allocation by local chunk ({},{}) in regionfile {}", new Object[]{i & 31, i >>> 5, this.path.toAbsolutePath()}
                        );
                     } else {
                        LOGGER.error(
                           "Invalid allocation by local chunk ({},{}) in regionfile {}: sector {}, length {} sectors, file size {} bytes",
                           new Object[]{i & 31, i >>> 5, this.path.toAbsolutePath(), sectorNumber, numSectors, size}
                        );
                     }

                     if (canRecalc) {
                        needsRecalc = true;
                     } else {
                        LOGGER.error(
                           "Detected invalid header for regionfile {}! Cannot recalculate, removing local chunk ({},{}) from header",
                           new Object[]{this.path.toAbsolutePath(), i & 31, i >>> 5}
                        );
                        if (!hasBackedUp) {
                           hasBackedUp = true;
                           this.backupRegionFile();
                        }

                        this.timestamps.put(i, 0);
                        this.offsets.put(i, 0);
                     }
                  }
               }
            }

            this.setUsedSectors(allocated);
            if (needsRecalc) {
               LOGGER.error("Recalculating regionfile {}, header gave erroneous offsets & locations", this.path.toAbsolutePath());

               try {
                  this.moonrise$recalculateHeader();
               } catch (RuntimeException | IOException var20) {
                  try {
                     this.file.close();
                  } catch (IOException var18) {
                     var20.addSuppressed(var18);
                  }

                  throw var20;
               }
            } else if (hasBackedUp) {
               try {
                  this.writeHeader();
                  this.file.force(true);
               } catch (IOException var19) {
                  LOGGER.error("Failed to write new header to disk for regionfile {}", this.path.toAbsolutePath(), var19);
               }
            }
         }
      }
   }

   public Path getPath() {
      return this.path;
   }

   private Path getExternalChunkPath(ChunkPos pos) {
      String externalFileName = "c." + pos.x() + "." + pos.z() + ".mcc";
      return this.externalFileDir.resolve(externalFileName);
   }

   @Nullable
   public synchronized DataInputStream getChunkDataInputStream(ChunkPos pos) throws IOException {
      return this.getChunkDataInputStream(pos, true);
   }

   @Nullable
   private DataInputStream recalculateAndRetry(ChunkPos pos, boolean allowRecalc) throws IOException {
      return allowRecalc && this.moonrise$recalculateHeader() ? this.getChunkDataInputStream(pos, false) : null;
   }

   @Nullable
   private synchronized DataInputStream getChunkDataInputStream(ChunkPos pos, boolean allowRecalc) throws IOException {
      int offset = this.getOffset(pos);
      if (offset == 0) {
         return null;
      } else {
         int sectorNumber = getSectorNumber(offset);
         int numSectors = getNumSectors(offset);
         if (numSectors == 255) {
            ByteBuffer realLen = ByteBuffer.allocate(4);
            this.file.read(realLen, (long)sectorNumber * 4096L);
            numSectors = sizeToSectors(realLen.getInt(0) + 4);
         }

         int sectorsLength = numSectors * 4096;
         ByteBuffer buffer = ByteBuffer.allocate(sectorsLength);
         this.file.read(buffer, (long)sectorNumber * 4096L);
         buffer.flip();
         if (buffer.remaining() < 5) {
            LOGGER.error(
               "Chunk {} in {} header is truncated: expected {} but read {}", new Object[]{pos, this.path.toAbsolutePath(), sectorsLength, buffer.remaining()}
            );
            return this.recalculateAndRetry(pos, allowRecalc);
         } else {
            int length = buffer.getInt();
            byte versionId = buffer.get();
            if (length == 0) {
               LOGGER.warn("Chunk {} in {} is allocated, but stream is missing", pos, this.path.toAbsolutePath());
               return this.recalculateAndRetry(pos, allowRecalc);
            } else {
               int streamLength = length - 1;
               if (isExternalStreamChunk(versionId)) {
                  if (streamLength != 0) {
                     LOGGER.warn("Chunk {} in {} has both internal and external streams", pos, this.path.toAbsolutePath());
                     if (allowRecalc && this.moonrise$recalculateHeader()) {
                        return this.getChunkDataInputStream(pos, false);
                     }
                  }

                  DataInputStream ret = this.createExternalChunkInputStream(pos, getExternalChunkVersion(versionId));
                  return ret != null ? ret : this.recalculateAndRetry(pos, allowRecalc);
               } else if (streamLength > buffer.remaining()) {
                  LOGGER.error(
                     "Chunk {} in {} stream is truncated: expected {} but read {}",
                     new Object[]{pos, this.path.toAbsolutePath(), streamLength, buffer.remaining()}
                  );
                  return this.recalculateAndRetry(pos, allowRecalc);
               } else if (streamLength < 0) {
                  LOGGER.error("Declared size {} of chunk {} in {} is negative", new Object[]{length, pos, this.path.toAbsolutePath()});
                  return this.recalculateAndRetry(pos, allowRecalc);
               } else {
                  JvmProfiler.INSTANCE.onRegionFileRead(this.info, pos, this.version, streamLength);
                  DataInputStream ret = this.createChunkInputStream(pos, versionId, createStream(buffer, streamLength));
                  return ret == null && versionId != RegionFileVersion.VERSION_CUSTOM.getId() ? this.recalculateAndRetry(pos, allowRecalc) : ret;
               }
            }
         }
      }
   }

   private static int getTimestamp() {
      return (int)(Util.getEpochMillis() / 1000L);
   }

   private static boolean isExternalStreamChunk(byte version) {
      return (version & 128) != 0;
   }

   private static byte getExternalChunkVersion(byte version) {
      return (byte)(version & -129);
   }

   @Nullable
   private DataInputStream createChunkInputStream(ChunkPos pos, byte versionId, InputStream chunkStream) throws IOException {
      RegionFileVersion version = RegionFileVersion.fromId(versionId);
      if (version == RegionFileVersion.VERSION_CUSTOM) {
         String type = new DataInputStream(chunkStream).readUTF();
         Identifier id = Identifier.tryParse(type);
         if (id != null) {
            LOGGER.error("Unrecognized custom compression {}", id);
            return null;
         } else {
            LOGGER.error("Invalid custom compression id {}", type);
            return null;
         }
      } else if (version == null) {
         LOGGER.error("Chunk {} has invalid chunk stream version {}", pos, versionId);
         return null;
      } else {
         return new DataInputStream(version.wrap(chunkStream));
      }
   }

   @Nullable
   private DataInputStream createExternalChunkInputStream(ChunkPos pos, byte versionId) throws IOException {
      DataInputStream is = this.createExternalChunkInputStream0(pos, versionId);
      return (DataInputStream)(is == null ? is : new ExternalChunkStreamMarker(is));
   }

   @Nullable
   private DataInputStream createExternalChunkInputStream0(ChunkPos pos, byte versionId) throws IOException {
      Path externalFile = this.getExternalChunkPath(pos);
      if (!Files.isRegularFile(externalFile)) {
         LOGGER.error("External chunk path {} is not file", externalFile);
         return null;
      } else {
         return this.createChunkInputStream(pos, versionId, Files.newInputStream(externalFile));
      }
   }

   private static ByteArrayInputStream createStream(ByteBuffer buffer, int length) {
      return new ByteArrayInputStream(buffer.array(), buffer.position(), length);
   }

   private int packSectorOffset(int index, int size) {
      return index << 8 | size;
   }

   private static int getNumSectors(int offset) {
      return offset & 0xFF;
   }

   private static int getSectorNumber(int offset) {
      return offset >> 8 & 16777215;
   }

   private static int sizeToSectors(int size) {
      return (size + 4096 - 1) / 4096;
   }

   public synchronized boolean doesChunkExist(ChunkPos pos) {
      int offset = this.getOffset(pos);
      if (offset == 0) {
         return false;
      } else {
         int sectorNumber = getSectorNumber(offset);
         int numSectors = getNumSectors(offset);
         ByteBuffer streamHeader = ByteBuffer.allocate(5);

         try {
            this.file.read(streamHeader, (long)sectorNumber * 4096L);
            streamHeader.flip();
            if (streamHeader.remaining() != 5) {
               return false;
            } else {
               int length = streamHeader.getInt();
               byte versionId = streamHeader.get();
               if (isExternalStreamChunk(versionId)) {
                  if (!RegionFileVersion.isValidVersion(getExternalChunkVersion(versionId))) {
                     return false;
                  }

                  if (!Files.isRegularFile(this.getExternalChunkPath(pos))) {
                     return false;
                  }
               } else {
                  if (!RegionFileVersion.isValidVersion(versionId)) {
                     return false;
                  }

                  if (length == 0) {
                     return false;
                  }

                  int streamLength = length - 1;
                  if (streamLength < 0 || streamLength > 4096 * numSectors) {
                     return false;
                  }
               }

               return true;
            }
         } catch (IOException var9) {
            ServerInternalException.reportInternalException(var9);
            return false;
         }
      }
   }

   public DataOutputStream getChunkDataOutputStream(ChunkPos pos) throws IOException {
      return new DataOutputStream(this.version.wrap(new RegionFile.ChunkBuffer(pos)));
   }

   public void flush() throws IOException {
      this.file.force(true);
   }

   public synchronized void clear(ChunkPos pos) throws IOException {
      int offsetIndex = getOffsetIndex(pos);
      int offset = this.offsets.get(offsetIndex);
      if (offset != 0) {
         this.offsets.put(offsetIndex, 0);
         this.timestamps.put(offsetIndex, getTimestamp());
         this.writeHeader();
         Files.deleteIfExists(this.getExternalChunkPath(pos));
         this.usedSectors.free(getSectorNumber(offset), getNumSectors(offset));
      }
   }

   protected synchronized void write(ChunkPos pos, ByteBuffer data) throws IOException {
      int offsetIndex = getOffsetIndex(pos);
      int offset = this.offsets.get(offsetIndex);
      int sectorNumber = getSectorNumber(offset);
      int currentSectorCount = getNumSectors(offset);
      int dataSize = data.remaining();
      int sectorsNeeded = sizeToSectors(dataSize);
      int newSectorNumber;
      RegionFile.CommitOp commitOp;
      if (sectorsNeeded >= 256) {
         Path externalChunkPath = this.getExternalChunkPath(pos);
         LOGGER.warn("Saving oversized chunk {} ({} bytes} to external file {}", new Object[]{pos, dataSize, externalChunkPath});
         sectorsNeeded = 1;
         newSectorNumber = this.usedSectors.allocate(sectorsNeeded);
         commitOp = this.writeToExternalFile(externalChunkPath, data);
         ByteBuffer stub = this.createExternalStub();
         this.file.write(stub, (long)(newSectorNumber * 4096));
      } else {
         newSectorNumber = this.usedSectors.allocate(sectorsNeeded);
         commitOp = () -> Files.deleteIfExists(this.getExternalChunkPath(pos));
         this.file.write(data, (long)(newSectorNumber * 4096));
      }

      this.offsets.put(offsetIndex, this.packSectorOffset(newSectorNumber, sectorsNeeded));
      this.timestamps.put(offsetIndex, getTimestamp());
      this.writeHeader();
      commitOp.run();
      if (sectorNumber != 0) {
         this.usedSectors.free(sectorNumber, currentSectorCount);
      }
   }

   private ByteBuffer createExternalStub() {
      ByteBuffer stub = ByteBuffer.allocate(5);
      stub.putInt(1);
      stub.put((byte)(this.version.getId() | 128));
      stub.flip();
      return stub;
   }

   private RegionFile.CommitOp writeToExternalFile(Path path, ByteBuffer data) throws IOException {
      Path tmpPath = Files.createTempFile(this.externalFileDir, "tmp", null);

      try (FileChannel extFile = FileChannel.open(tmpPath, StandardOpenOption.CREATE, StandardOpenOption.WRITE)) {
         data.position(5);
         extFile.write(data);
      } catch (Throwable var9) {
         ServerInternalException.reportInternalException(var9);
         throw var9;
      }

      return () -> Files.move(tmpPath, path, StandardCopyOption.REPLACE_EXISTING);
   }

   private void writeHeader() throws IOException {
      this.header.position(0);
      this.file.write(this.header, 0L);
   }

   private int getOffset(ChunkPos pos) {
      return this.offsets.get(getOffsetIndex(pos));
   }

   public synchronized boolean hasChunk(ChunkPos pos) {
      return this.getOffset(pos) != 0;
   }

   private static int getOffsetIndex(ChunkPos pos) {
      return pos.getRegionLocalX() + pos.getRegionLocalZ() * 32;
   }

   @Override
   public synchronized void close() throws IOException {
      try {
         this.padToFullSector();
      } finally {
         try {
            this.file.force(true);
         } finally {
            this.file.close();
         }
      }
   }

   private void padToFullSector() throws IOException {
      int fileSize = (int)this.file.size();
      int paddedSize = sizeToSectors(fileSize) * 4096;
      if (fileSize != paddedSize) {
         ByteBuffer padding = PADDING_BUFFER.duplicate();
         padding.position(0);
         this.file.write(padding, (long)(paddedSize - 1));
      }
   }

   private class ChunkBuffer extends ByteArrayOutputStream implements ChunkSystemChunkBuffer {
      private final ChunkPos pos;
      private boolean writeOnClose;

      @Override
      public final boolean moonrise$getWriteOnClose() {
         return this.writeOnClose;
      }

      @Override
      public final void moonrise$setWriteOnClose(boolean value) {
         this.writeOnClose = value;
      }

      @Override
      public final void moonrise$write(RegionFile regionFile) throws IOException {
         regionFile.write(this.pos, ByteBuffer.wrap(this.buf, 0, this.count));
      }

      public ChunkBuffer(final ChunkPos pos) {
         Objects.requireNonNull(RegionFile.this);
         super(8096);
         this.writeOnClose = true;
         super.write(0);
         super.write(0);
         super.write(0);
         super.write(0);
         super.write(RegionFile.this.version.getId());
         this.pos = pos;
      }

      @Override
      public void write(int b) {
         if (this.count > 524288000) {
            throw new RegionFileStorage.RegionFileSizeException("Region file too large: " + this.count);
         } else {
            super.write(b);
         }
      }

      @Override
      public void write(byte[] b, int off, int len) {
         if (this.count + len > 524288000) {
            throw new RegionFileStorage.RegionFileSizeException("Region file too large: " + (this.count + len));
         } else {
            super.write(b, off, len);
         }
      }

      @Override
      public void close() throws IOException {
         ByteBuffer result = ByteBuffer.wrap(this.buf, 0, this.count);
         int streamLength = this.count - 5 + 1;
         JvmProfiler.INSTANCE.onRegionFileWrite(RegionFile.this.info, this.pos, RegionFile.this.version, streamLength);
         result.putInt(0, streamLength);
         if (this.writeOnClose) {
            RegionFile.this.write(this.pos, result);
         }
      }
   }

   private interface CommitOp {
      void run() throws IOException;
   }
}
