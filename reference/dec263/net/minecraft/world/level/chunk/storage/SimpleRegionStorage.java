package net.minecraft.world.level.chunk.storage;

import ca.spottedleaf.dataconverter.minecraft.MCDataConverter;
import ca.spottedleaf.dataconverter.minecraft.datatypes.MCDataType;
import ca.spottedleaf.dataconverter.minecraft.datatypes.MCTypeRegistry;
import ca.spottedleaf.moonrise.patches.chunk_system.storage.ChunkSystemSimpleRegionStorage;
import com.mojang.datafixers.DataFixer;
import com.mojang.logging.LogUtils;
import com.mojang.serialization.Dynamic;
import io.papermc.paper.util.MCUtil;
import java.io.IOException;
import java.nio.file.Path;
import java.util.Optional;
import java.util.concurrent.CompletableFuture;
import java.util.function.Supplier;
import net.minecraft.CrashReport;
import net.minecraft.CrashReportCategory;
import net.minecraft.ReportedException;
import net.minecraft.SharedConstants;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.nbt.NbtUtils;
import net.minecraft.nbt.Tag;
import net.minecraft.server.level.ChunkMap;
import net.minecraft.util.datafix.DataFixTypes;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.chunk.status.ChunkStatus;
import org.jspecify.annotations.Nullable;
import org.slf4j.Logger;
import org.spigotmc.SpigotConfig;

public class SimpleRegionStorage implements ChunkSystemSimpleRegionStorage, AutoCloseable {
   private static final Logger LOGGER = LogUtils.getClassLogger();
   private final RegionFileStorage storage;
   private final DataFixer fixerUpper;
   private final DataFixTypes dataFixType;

   @Override
   public final RegionFileStorage moonrise$getRegionStorage() {
      return this.storage;
   }

   public SimpleRegionStorage(RegionStorageInfo info, Path folder, DataFixer fixerUpper, boolean syncWrites, DataFixTypes dataFixType) {
      this.fixerUpper = fixerUpper;
      this.dataFixType = dataFixType;
      this.storage = (new IOWorker(info, folder, syncWrites)).storage;
   }

   public boolean isOldChunkAround(ChunkPos pos, int range) {
      return true;
   }

   public CompletableFuture<Optional<CompoundTag>> read(ChunkPos pos) {
      try {
         return CompletableFuture.completedFuture(Optional.ofNullable(this.storage.read(pos)));
      } catch (Throwable var3) {
         return CompletableFuture.failedFuture(var3);
      }
   }

   public CompletableFuture<Void> write(ChunkPos pos, CompoundTag value) {
      return this.write(pos, () -> value);
   }

   public CompletableFuture<Void> write(ChunkPos pos, Supplier<CompoundTag> supplier) {
      Supplier<CompoundTag> guardedPosCheck = () -> {
         CompoundTag nbt = supplier.get();
         boolean chunkStorage = this.dataFixType == DataFixTypes.CHUNK;
         if (chunkStorage && nbt != null && !pos.equals(SerializableChunkData.getChunkCoordinate(nbt))) {
            String world = this instanceof ChunkMap chunkMap ? MCUtil.getLevelName(chunkMap.level) : null;
            throw new IllegalArgumentException(
               "Chunk coordinate and serialized data do not have matching coordinates, trying to serialize coordinate "
                  + pos
                  + " but compound says coordinate is "
                  + SerializableChunkData.getChunkCoordinate(nbt)
                  + (world == null ? " for an unknown world" : " for world: " + world)
            );
         } else {
            return nbt;
         }
      };

      try {
         this.storage.write(pos, guardedPosCheck.get());
         return CompletableFuture.completedFuture(null);
      } catch (Throwable var5) {
         return CompletableFuture.failedFuture(var5);
      }
   }

   public CompoundTag upgradeChunkTag(CompoundTag chunkTag, int defaultVersion, @Nullable CompoundTag dataFixContextTag, int targetVersion) {
      int version = NbtUtils.getDataVersion(chunkTag, defaultVersion);
      if (version >= targetVersion) {
         return chunkTag;
      } else {
         try {
            boolean stopBelowZero = false;
            boolean chunkStorage = this.dataFixType == DataFixTypes.CHUNK;
            if (chunkStorage) {
               boolean belowZeroGenerationInExistingChunks = this instanceof ChunkMap chunkMap
                  ? chunkMap.level.spigotConfig.belowZeroGenerationInExistingChunks
                  : SpigotConfig.belowZeroGenerationInExistingChunks;
               if (version <= 2730 && !belowZeroGenerationInExistingChunks) {
                  stopBelowZero = "full".equals(chunkTag.getCompound("Level").flatMap(level -> level.getString("Status")).orElse(null));
               }
            }

            injectDatafixingContext(chunkTag, dataFixContextTag);
            chunkTag = MCDataConverter.convertTag(this.getDataConverterType(), this.dataFixType, chunkTag, version, targetVersion);
            if (stopBelowZero) {
               chunkTag.putString("Status", BuiltInRegistries.CHUNK_STATUS.getKey(ChunkStatus.SPAWN).toString());
            }

            removeDatafixingContext(chunkTag);
            NbtUtils.addDataVersion(chunkTag, targetVersion);
            return chunkTag;
         } catch (Exception var11) {
            CrashReport report = CrashReport.forThrowable(var11, "Updated chunk");
            CrashReportCategory details = report.addCategory("Updated chunk details");
            details.setDetail("Data version", version);
            details.setDetail("Target version", targetVersion);
            throw new ReportedException(report);
         }
      }
   }

   private MCDataType getDataConverterType() {
      if (this.dataFixType == DataFixTypes.CHUNK) {
         return MCTypeRegistry.CHUNK;
      } else if (this.dataFixType == DataFixTypes.ENTITY_CHUNK) {
         return MCTypeRegistry.ENTITY_CHUNK;
      } else if (this.dataFixType == DataFixTypes.POI_CHUNK) {
         return MCTypeRegistry.POI_CHUNK;
      } else {
         throw new UnsupportedOperationException("For " + this.dataFixType.name());
      }
   }

   public CompoundTag upgradeChunkTag(CompoundTag chunkTag, int defaultVersion) {
      return this.upgradeChunkTag(chunkTag, defaultVersion, null, SharedConstants.getCurrentVersion().dataVersion().version());
   }

   public Dynamic<Tag> upgradeChunkTag(Dynamic<Tag> chunkTag, int defaultVersion) {
      return new Dynamic<>(
         chunkTag.getOps(),
         this.upgradeChunkTag((CompoundTag)chunkTag.getValue(), defaultVersion, null, SharedConstants.getCurrentVersion().dataVersion().version())
      );
   }

   public static void injectDatafixingContext(CompoundTag chunkTag, @Nullable CompoundTag contextTag) {
      if (contextTag != null) {
         chunkTag.put("__context", contextTag);
      }
   }

   private static void removeDatafixingContext(CompoundTag chunkTag) {
      chunkTag.remove("__context");
   }

   public CompletableFuture<Void> synchronize(boolean flush) {
      try {
         this.storage.flush();
         return CompletableFuture.completedFuture(null);
      } catch (IOException var3) {
         LOGGER.error("Failed to flush chunk storage", var3);
         return CompletableFuture.failedFuture(var3);
      }
   }

   @Override
   public void close() throws IOException {
      this.storage.close();
   }

   public ChunkScanAccess chunkScanner() {
      return (chunkPos, streamTagVisitor) -> {
         try {
            this.storage.scanChunk(chunkPos, streamTagVisitor);
            return CompletableFuture.completedFuture(null);
         } catch (IOException var4) {
            throw new RuntimeException(var4);
         }
      };
   }

   public RegionStorageInfo storageInfo() {
      return this.storage.info();
   }
}
