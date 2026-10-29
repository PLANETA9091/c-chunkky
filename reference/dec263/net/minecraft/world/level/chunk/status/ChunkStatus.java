package net.minecraft.world.level.chunk.status;

import ca.spottedleaf.moonrise.patches.chunk_system.level.chunk.ChunkSystemChunkStatus;
import com.google.common.annotations.VisibleForTesting;
import com.google.common.collect.Lists;
import com.mojang.serialization.Codec;
import java.util.Collections;
import java.util.EnumSet;
import java.util.List;
import java.util.concurrent.atomic.AtomicBoolean;
import net.minecraft.core.Registry;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.resources.Identifier;
import net.minecraft.world.level.levelgen.Heightmap;
import org.jspecify.annotations.Nullable;

public class ChunkStatus implements ChunkSystemChunkStatus {
   public static final int MAX_STRUCTURE_DISTANCE = 8;
   private static final EnumSet<Heightmap.Types> WORLDGEN_HEIGHTMAPS = EnumSet.of(Heightmap.Types.OCEAN_FLOOR_WG, Heightmap.Types.WORLD_SURFACE_WG);
   public static final EnumSet<Heightmap.Types> FINAL_HEIGHTMAPS = EnumSet.of(
      Heightmap.Types.OCEAN_FLOOR, Heightmap.Types.WORLD_SURFACE, Heightmap.Types.MOTION_BLOCKING, Heightmap.Types.MOTION_BLOCKING_NO_LEAVES
   );
   public static final ChunkStatus EMPTY = register("empty", null, WORLDGEN_HEIGHTMAPS, ChunkType.PROTOCHUNK);
   public static final ChunkStatus STRUCTURE_STARTS = register("structure_starts", EMPTY, WORLDGEN_HEIGHTMAPS, ChunkType.PROTOCHUNK);
   public static final ChunkStatus STRUCTURE_REFERENCES = register("structure_references", STRUCTURE_STARTS, WORLDGEN_HEIGHTMAPS, ChunkType.PROTOCHUNK);
   public static final ChunkStatus BIOMES = register("biomes", STRUCTURE_REFERENCES, WORLDGEN_HEIGHTMAPS, ChunkType.PROTOCHUNK);
   public static final ChunkStatus TERRAIN = register("terrain", BIOMES, FINAL_HEIGHTMAPS, ChunkType.PROTOCHUNK);
   public static final ChunkStatus FEATURES = register("features", TERRAIN, FINAL_HEIGHTMAPS, ChunkType.PROTOCHUNK);
   public static final ChunkStatus INITIALIZE_LIGHT = register("initialize_light", FEATURES, FINAL_HEIGHTMAPS, ChunkType.PROTOCHUNK);
   public static final ChunkStatus LIGHT = register("light", INITIALIZE_LIGHT, FINAL_HEIGHTMAPS, ChunkType.PROTOCHUNK);
   public static final ChunkStatus SPAWN = register("spawn", LIGHT, FINAL_HEIGHTMAPS, ChunkType.PROTOCHUNK);
   public static final ChunkStatus FULL = register("full", SPAWN, FINAL_HEIGHTMAPS, ChunkType.LEVELCHUNK);
   public static final Codec<ChunkStatus> CODEC = BuiltInRegistries.CHUNK_STATUS.byNameCodec();
   private final int index;
   private final ChunkStatus parent;
   private final ChunkType chunkType;
   private final EnumSet<Heightmap.Types> heightmapsAfter;
   private boolean isParallelCapable = false;
   private boolean emptyLoadTask;
   private int writeRadius = -1;
   private ChunkStatus nextStatus = this;
   private AtomicBoolean warnedAboutNoImmediateComplete;

   private static ChunkStatus register(String name, @Nullable ChunkStatus parent, EnumSet<Heightmap.Types> heightmaps, ChunkType chunkType) {
      return Registry.register(BuiltInRegistries.CHUNK_STATUS, name, new ChunkStatus(parent, heightmaps, chunkType));
   }

   public static List<ChunkStatus> getStatusList() {
      List<ChunkStatus> list = Lists.newArrayList();

      ChunkStatus status;
      for (status = FULL; status.getParent() != status; status = status.getParent()) {
         list.add(status);
      }

      list.add(status);
      Collections.reverse(list);
      return list;
   }

   @Override
   public final boolean moonrise$isParallelCapable() {
      return this.isParallelCapable;
   }

   @Override
   public final void moonrise$setParallelCapable(boolean value) {
      this.isParallelCapable = value;
   }

   @Override
   public final int moonrise$getWriteRadius() {
      return this.writeRadius;
   }

   @Override
   public final void moonrise$setWriteRadius(int value) {
      this.writeRadius = value;
   }

   @Override
   public final ChunkStatus moonrise$getNextStatus() {
      return this.nextStatus;
   }

   @Override
   public final boolean moonrise$isEmptyLoadStatus() {
      return this.emptyLoadTask;
   }

   @Override
   public void moonrise$setEmptyLoadStatus(boolean value) {
      this.emptyLoadTask = value;
   }

   @Override
   public final boolean moonrise$isEmptyGenStatus() {
      return this == EMPTY;
   }

   @Override
   public final AtomicBoolean moonrise$getWarnedAboutNoImmediateComplete() {
      return this.warnedAboutNoImmediateComplete;
   }

   @VisibleForTesting
   protected ChunkStatus(@Nullable ChunkStatus parent, EnumSet<Heightmap.Types> heightmapsAfter, ChunkType chunkType) {
      if (parent != null) {
         parent.nextStatus = this;
      }

      this.warnedAboutNoImmediateComplete = new AtomicBoolean();
      this.parent = parent == null ? this : parent;
      this.chunkType = chunkType;
      this.heightmapsAfter = heightmapsAfter;
      this.index = parent == null ? 0 : parent.getIndex() + 1;
   }

   public int getIndex() {
      return this.index;
   }

   public ChunkStatus getParent() {
      return this.parent;
   }

   public ChunkType getChunkType() {
      return this.chunkType;
   }

   public static ChunkStatus byName(String key) {
      return BuiltInRegistries.CHUNK_STATUS.getValue(Identifier.tryParse(key));
   }

   public EnumSet<Heightmap.Types> heightmapsAfter() {
      return this.heightmapsAfter;
   }

   public boolean isOrAfter(ChunkStatus step) {
      return this.getIndex() >= step.getIndex();
   }

   public boolean isAfter(ChunkStatus step) {
      return this.getIndex() > step.getIndex();
   }

   public boolean isOrBefore(ChunkStatus step) {
      return this.getIndex() <= step.getIndex();
   }

   public boolean isBefore(ChunkStatus step) {
      return this.getIndex() < step.getIndex();
   }

   public static ChunkStatus max(ChunkStatus a, ChunkStatus b) {
      return a.isAfter(b) ? a : b;
   }

   @Override
   public String toString() {
      return this.getName();
   }

   public String getName() {
      return BuiltInRegistries.CHUNK_STATUS.getKey(this).toString();
   }
}
