/*
 * Decompiled with CFR 0.152.
 * 
 * Could not load the following classes:
 *  ca.spottedleaf.concurrentutil.util.Priority
 *  com.google.common.collect.ImmutableList
 *  com.google.common.collect.ImmutableList$Builder
 *  com.google.common.collect.ImmutableMap
 *  io.papermc.paper.world.chunk.ChunkGenerationState
 *  it.unimi.dsi.fastutil.longs.Long2ObjectMap$Entry
 *  it.unimi.dsi.fastutil.longs.LongIterator
 *  it.unimi.dsi.fastutil.longs.LongOpenHashSet
 *  it.unimi.dsi.fastutil.longs.LongSet
 *  it.unimi.dsi.fastutil.longs.LongSets
 *  it.unimi.dsi.fastutil.objects.ObjectOpenHashSet
 *  it.unimi.dsi.fastutil.objects.ObjectSet
 *  it.unimi.dsi.fastutil.objects.ObjectSets
 *  org.apache.logging.log4j.LogManager
 *  org.apache.logging.log4j.core.Appender
 *  org.apache.logging.log4j.core.LoggerContext
 *  org.apache.logging.log4j.spi.LoggerContext
 *  org.bukkit.Chunk
 *  org.bukkit.entity.Entity
 *  org.bukkit.plugin.Plugin
 */
package io.papermc.paper;

import ca.spottedleaf.concurrentutil.util.Priority;
import ca.spottedleaf.moonrise.patches.chunk_system.io.MoonriseRegionFileIO;
import ca.spottedleaf.moonrise.patches.chunk_system.level.entity.EntityLookup;
import ca.spottedleaf.moonrise.patches.chunk_system.player.RegionizedPlayerChunkLoader;
import ca.spottedleaf.moonrise.patches.chunk_system.scheduling.ChunkTaskScheduler;
import ca.spottedleaf.moonrise.patches.chunk_system.scheduling.NewChunkHolder;
import ca.spottedleaf.moonrise.patches.collisions.CollisionUtil;
import com.google.common.collect.ImmutableList;
import com.google.common.collect.ImmutableMap;
import io.papermc.paper.command.PaperSubcommand;
import io.papermc.paper.command.subcommands.ChunkDebugCommand;
import io.papermc.paper.command.subcommands.FixLightCommand;
import io.papermc.paper.world.chunk.ChunkGenerationState;
import it.unimi.dsi.fastutil.longs.Long2ObjectMap;
import it.unimi.dsi.fastutil.longs.LongIterator;
import it.unimi.dsi.fastutil.longs.LongOpenHashSet;
import it.unimi.dsi.fastutil.longs.LongSet;
import it.unimi.dsi.fastutil.longs.LongSets;
import it.unimi.dsi.fastutil.objects.ObjectOpenHashSet;
import it.unimi.dsi.fastutil.objects.ObjectSet;
import it.unimi.dsi.fastutil.objects.ObjectSets;
import java.util.Collection;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.CompletionException;
import java.util.concurrent.Executor;
import java.util.concurrent.TimeUnit;
import java.util.function.BiConsumer;
import net.minecraft.SharedConstants;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.nbt.NbtUtils;
import net.minecraft.network.protocol.game.ClientboundLevelChunkWithLightPacket;
import net.minecraft.server.MinecraftServer;
import net.minecraft.server.level.DistanceManager;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.server.level.ServerPlayer;
import net.minecraft.server.level.Ticket;
import net.minecraft.server.level.TicketType;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.border.WorldBorder;
import net.minecraft.world.level.chunk.ChunkAccess;
import net.minecraft.world.level.chunk.LevelChunk;
import net.minecraft.world.level.chunk.LevelChunkSection;
import net.minecraft.world.level.chunk.PalettedContainer;
import net.minecraft.world.level.chunk.PalettedContainerFactory;
import net.minecraft.world.level.chunk.Strategy;
import net.minecraft.world.level.chunk.status.ChunkStatus;
import net.minecraft.world.level.levelgen.BelowZeroRetrogen;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.core.Appender;
import org.apache.logging.log4j.core.LoggerContext;
import org.apache.logging.log4j.core.appender.AsyncAppender;
import org.bukkit.Chunk;
import org.bukkit.craftbukkit.CraftWorld;
import org.bukkit.entity.Entity;
import org.bukkit.plugin.Plugin;

public final class FeatureHooks {
    public static Iterable<net.minecraft.world.entity.Entity> getAllEntities(ServerLevel level) {
        return ((EntityLookup)level.getEntities()).getAllMapped();
    }

    public static void setPlayerChunkUnloadDelay(long ticks) {
        RegionizedPlayerChunkLoader.setUnloadDelay(ticks);
    }

    public static void registerPaperCommands(Map<Set<String>, PaperSubcommand> commands) {
        commands.put(Set.of("fixlight"), new FixLightCommand());
        commands.put(Set.of("debug", "chunkinfo", "holderinfo"), new ChunkDebugCommand());
    }

    public static LevelChunkSection createSection(PalettedContainerFactory palettedContainerFactory, Level level, ChunkPos chunkPos, int chunkSection) {
        return new LevelChunkSection(palettedContainerFactory, level, chunkPos, chunkSection);
    }

    public static void sendChunkRefreshPackets(List<ServerPlayer> playersInRange, LevelChunk chunk) {
        HashMap<Object, ClientboundLevelChunkWithLightPacket> refreshPackets = new HashMap<Object, ClientboundLevelChunkWithLightPacket>();
        for (ServerPlayer player : playersInRange) {
            if (player.connection == null) continue;
            Boolean shouldModify = chunk.getLevel().chunkPacketBlockController.shouldModify(player, chunk);
            player.connection.send(refreshPackets.computeIfAbsent(shouldModify, s -> new ClientboundLevelChunkWithLightPacket(chunk, chunk.getLevel().getLightEngine(), null, null, (Boolean)s)));
        }
    }

    public static PalettedContainer<BlockState> emptyPalettedBlockContainer() {
        return new PalettedContainer<BlockState>(Blocks.AIR.defaultBlockState(), Strategy.createForBlockStates(Block.BLOCK_STATE_REGISTRY), null);
    }

    public static Set<Long> getSentChunkKeys(ServerPlayer player) {
        return LongSets.unmodifiable((LongSet)player.moonrise$getChunkLoader().getSentChunksRaw().clone());
    }

    public static Set<Chunk> getSentChunks(ServerPlayer player) {
        if (player.moonrise$getChunkLoader() == null) {
            return ObjectSets.EMPTY_SET;
        }
        LongOpenHashSet rawChunkKeys = player.moonrise$getChunkLoader().getSentChunksRaw();
        ObjectOpenHashSet chunks = new ObjectOpenHashSet(rawChunkKeys.size());
        CraftWorld world = player.level().getWorld();
        LongIterator iter = player.moonrise$getChunkLoader().getSentChunksRaw().longIterator();
        while (iter.hasNext()) {
            chunks.add((Object)world.getChunkAt(iter.nextLong(), false));
        }
        return ObjectSets.unmodifiable((ObjectSet)chunks);
    }

    public static boolean isChunkSent(ServerPlayer player, long chunkKey) {
        return player.moonrise$getChunkLoader() != null && player.moonrise$getChunkLoader().getSentChunksRaw().contains(chunkKey);
    }

    public static boolean isCollidingWithWorldBorder(net.minecraft.world.entity.Entity entity) {
        WorldBorder border = entity.level().getWorldBorder();
        return CollisionUtil.isCollidingWithBorder(border, entity.getBoundingBox().inflate(1.0E-7)) && border.isInsideCloseToBorder(entity, entity.getBoundingBox());
    }

    public static void dumpAllChunkLoadInfo(MinecraftServer server, boolean isLongTimeout) {
        ChunkTaskScheduler.dumpAllChunkLoadInfo(server, isLongTimeout);
    }

    private static void dumpEntity(net.minecraft.world.entity.Entity entity) {
    }

    public static Entity[] getChunkEntities(ServerLevel level, int chunkX, int chunkZ) {
        return level.getChunkEntities(chunkX, chunkZ);
    }

    public static Collection<Plugin> getPluginChunkTickets(ServerLevel level, int x, int z) {
        return level.moonrise$getChunkTaskScheduler().chunkHolderManager.getPluginChunkTickets(x, z);
    }

    public static Map<Plugin, Collection<Chunk>> getPluginChunkTickets(ServerLevel level) {
        HashMap<Plugin, ImmutableList.Builder> ret = new HashMap<Plugin, ImmutableList.Builder>();
        DistanceManager chunkDistanceManager = level.getChunkSource().chunkMap.getDistanceManager();
        for (Long2ObjectMap.Entry chunkTickets : chunkDistanceManager.moonrise$getChunkHolderManager().getTicketsCopy().long2ObjectEntrySet()) {
            long chunkKey = chunkTickets.getLongKey();
            Collection tickets = (Collection)chunkTickets.getValue();
            Chunk chunk = null;
            for (Ticket ticket : tickets) {
                if (ticket.getType() != TicketType.PLUGIN_TICKET) continue;
                if (chunk == null) {
                    chunk = level.getWorld().getChunkAt(ChunkPos.getX(chunkKey), ChunkPos.getZ(chunkKey));
                }
                ret.computeIfAbsent((Plugin)ticket.getIdentifier(), plugin -> ImmutableList.builder()).add((Object)chunk);
            }
        }
        return (Map)ret.entrySet().stream().collect(ImmutableMap.toImmutableMap(Map.Entry::getKey, entry -> ((ImmutableList.Builder)entry.getValue()).build()));
    }

    public static int getViewDistance(ServerLevel level) {
        return level.moonrise$getPlayerChunkLoader().getAPIViewDistance();
    }

    public static int getSimulationDistance(ServerLevel level) {
        return level.moonrise$getPlayerChunkLoader().getAPITickDistance();
    }

    public static int getSendViewDistance(ServerLevel level) {
        return level.moonrise$getPlayerChunkLoader().getAPISendViewDistance();
    }

    public static void setViewDistance(ServerLevel level, int distance) {
        if (distance < 2 || distance > 32) {
            throw new IllegalArgumentException("View distance " + distance + " is out of range of [2, 32]");
        }
        level.getChunkSource().chunkMap.setServerViewDistance(distance);
    }

    public static void setSimulationDistance(ServerLevel level, int distance) {
        if (distance < 2 || distance > 32) {
            throw new IllegalArgumentException("Simulation distance " + distance + " is out of range of [2, 32]");
        }
        level.getChunkSource().chunkMap.getDistanceManager().updateSimulationDistance(distance);
    }

    public static void setSendViewDistance(ServerLevel level, int distance) {
        level.getChunkSource().setSendViewDistance(distance);
    }

    public static void tickEntityManager(ServerLevel level) {
    }

    public static void closeEntityManager(ServerLevel level, boolean save) {
    }

    public static Executor getWorldgenExecutor() {
        return Runnable::run;
    }

    public static void setViewDistance(ServerPlayer player, int distance) {
        player.moonrise$getViewDistanceHolder().setLoadViewDistance(distance == -1 ? distance : distance + 1);
    }

    public static void setSimulationDistance(ServerPlayer player, int distance) {
        player.moonrise$getViewDistanceHolder().setTickViewDistance(distance);
    }

    public static void setSendViewDistance(ServerPlayer player, int distance) {
        player.moonrise$getViewDistanceHolder().setSendViewDistance(distance);
    }

    public static ChunkGenerationState getChunkGenerationState(ServerLevel level, int chunkX, int chunkZ) {
        CompoundTag tag2;
        ChunkGenerationState loaded = FeatureHooks.getLoadedChunkGenerationState(level, chunkX, chunkZ);
        if (loaded != null) {
            return loaded;
        }
        CompletableFuture data = new CompletableFuture();
        MoonriseRegionFileIO.loadDataAsync(level, chunkX, chunkZ, MoonriseRegionFileIO.RegionFileType.CHUNK_DATA, (BiConsumer<CompoundTag, Throwable> & MoonriseRegionFileIO.NoCopyNBTData)(tag, throwable) -> {
            if (throwable != null) {
                data.completeExceptionally((Throwable)throwable);
            } else {
                data.complete(tag);
            }
        }, true, MoonriseRegionFileIO.getIOBlockingPriorityForCurrentThread());
        try {
            tag2 = (CompoundTag)data.join();
        }
        catch (CompletionException ex) {
            throw new RuntimeException("Failed to read chunk data at (" + chunkX + ", " + chunkZ + ")", ex.getCause());
        }
        return FeatureHooks.classifyChunkGenerationState(tag2, chunkX, chunkZ);
    }

    public static CompletableFuture<ChunkGenerationState> getChunkGenerationStateAsync(ServerLevel level, int chunkX, int chunkZ) {
        ChunkGenerationState loaded = FeatureHooks.getLoadedChunkGenerationState(level, chunkX, chunkZ);
        if (loaded != null) {
            return CompletableFuture.completedFuture(loaded);
        }
        CompletableFuture<ChunkGenerationState> ret = new CompletableFuture<ChunkGenerationState>();
        MoonriseRegionFileIO.loadDataAsync(level, chunkX, chunkZ, MoonriseRegionFileIO.RegionFileType.CHUNK_DATA, (BiConsumer<CompoundTag, Throwable> & MoonriseRegionFileIO.NoCopyNBTData)(tag, throwable) -> level.moonrise$getChunkTaskScheduler().loadExecutor.createTask(() -> {
            if (throwable != null) {
                ret.completeExceptionally((Throwable)throwable);
                return;
            }
            try {
                ret.complete(FeatureHooks.classifyChunkGenerationState(tag, chunkX, chunkZ));
            }
            catch (Throwable thr) {
                ret.completeExceptionally(thr);
            }
        }, Priority.NORMAL).queue(), false, Priority.NORMAL);
        return ret;
    }

    private static ChunkGenerationState getLoadedChunkGenerationState(ServerLevel level, int chunkX, int chunkZ) {
        NewChunkHolder holder = level.moonrise$getChunkTaskScheduler().chunkHolderManager.getChunkHolder(chunkX, chunkZ);
        if (holder == null) {
            return null;
        }
        ChunkAccess chunk = holder.getChunkIfPresentUnchecked(ChunkStatus.EMPTY);
        if (chunk == null) {
            return null;
        }
        if (chunk.getPersistedStatus().isOrAfter(ChunkStatus.FULL)) {
            return ChunkGenerationState.GENERATED;
        }
        return ChunkGenerationState.PARTIAL;
    }

    private static ChunkGenerationState classifyChunkGenerationState(CompoundTag data, int chunkX, int chunkZ) {
        int currentVersion;
        if (data == null) {
            return ChunkGenerationState.NOT_GENERATED;
        }
        int dataVersion = NbtUtils.getDataVersion(data, -1);
        if (dataVersion > (currentVersion = SharedConstants.getCurrentVersion().dataVersion().version())) {
            throw new IllegalStateException("Chunk (" + chunkX + ", " + chunkZ + ") was saved by a newer version (data version " + dataVersion + ", current " + currentVersion + ")");
        }
        if (dataVersion < currentVersion) {
            return ChunkGenerationState.OUTDATED;
        }
        if (data.getIntOr("xPos", 0) != chunkX || data.getIntOr("zPos", 0) != chunkZ) {
            return ChunkGenerationState.OUTDATED;
        }
        ChunkStatus status = data.read("Status", ChunkStatus.CODEC).orElse(ChunkStatus.EMPTY);
        BelowZeroRetrogen retrogen = data.read("below_zero_retrogen", BelowZeroRetrogen.CODEC).orElse(null);
        if (retrogen == null) {
            return status.isOrAfter(ChunkStatus.FULL) ? ChunkGenerationState.GENERATED : ChunkGenerationState.PARTIAL;
        }
        if (status.isOrAfter(ChunkStatus.FULL) || retrogen.targetStatus().isOrAfter(ChunkStatus.SPAWN)) {
            return ChunkGenerationState.OUTDATED;
        }
        return ChunkGenerationState.PARTIAL;
    }

    public static void flushAsyncAppenders() {
        org.apache.logging.log4j.spi.LoggerContext loggerContext = LogManager.getContext((boolean)false);
        if (!(loggerContext instanceof LoggerContext)) {
            return;
        }
        LoggerContext context = (LoggerContext)loggerContext;
        for (Appender appender : context.getConfiguration().getAppenders().values()) {
            AsyncAppender asyncAppender;
            boolean flushed;
            if (!(appender instanceof AsyncAppender) || (flushed = (asyncAppender = (AsyncAppender)appender).flush(100L, TimeUnit.MILLISECONDS))) continue;
            MinecraftServer.LOGGER.warn("Failed to flush log messages before plugin unload.");
        }
    }
}

