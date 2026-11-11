/*
 * Decompiled with CFR 0.152.
 * 
 * Could not load the following classes:
 *  ca.spottedleaf.concurrentutil.executor.PrioritisedExecutor$PrioritisedTask
 *  ca.spottedleaf.concurrentutil.util.ConcurrentUtil
 *  ca.spottedleaf.concurrentutil.util.Priority
 *  ca.spottedleaf.moonrise.common.util.WorldUtil
 *  net.minecraft.util.StaticCache2D
 *  net.minecraft.world.level.Level
 *  net.minecraft.world.level.chunk.ChunkAccess
 *  net.minecraft.world.level.chunk.ProtoChunk
 *  org.slf4j.Logger
 *  org.slf4j.LoggerFactory
 */
package ca.spottedleaf.moonrise.patches.chunk_system.scheduling.task;

import ca.spottedleaf.concurrentutil.executor.PrioritisedExecutor;
import ca.spottedleaf.concurrentutil.util.ConcurrentUtil;
import ca.spottedleaf.concurrentutil.util.Priority;
import ca.spottedleaf.moonrise.common.util.WorldUtil;
import ca.spottedleaf.moonrise.patches.chunk_system.scheduling.ChunkTaskScheduler;
import ca.spottedleaf.moonrise.patches.chunk_system.scheduling.task.ChunkProgressionTask;
import java.lang.invoke.VarHandle;
import java.util.Map;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.CompletionStage;
import net.minecraft.server.level.ChunkMap;
import net.minecraft.server.level.GenerationChunkHolder;
import net.minecraft.server.level.ServerChunkCache;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.util.StaticCache2D;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.chunk.ChunkAccess;
import net.minecraft.world.level.chunk.ProtoChunk;
import net.minecraft.world.level.chunk.status.ChunkPyramid;
import net.minecraft.world.level.chunk.status.ChunkStatus;
import net.minecraft.world.level.chunk.status.WorldGenContext;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

public final class ChunkUpgradeGenericStatusTask
extends ChunkProgressionTask
implements Runnable {
    private static final Logger LOGGER = LoggerFactory.getLogger(ChunkUpgradeGenericStatusTask.class);
    private final ChunkAccess fromChunk;
    private final ChunkStatus fromStatus;
    private final ChunkStatus toStatus;
    private final StaticCache2D<GenerationChunkHolder> neighbours;
    private final PrioritisedExecutor.PrioritisedTask generateTask;
    private volatile boolean scheduled;
    private static final VarHandle SCHEDULED_HANDLE = ConcurrentUtil.getVarHandle(ChunkUpgradeGenericStatusTask.class, (String)"scheduled", Boolean.TYPE);

    public ChunkUpgradeGenericStatusTask(ChunkTaskScheduler scheduler, ServerLevel world, int chunkX, int chunkZ, ChunkAccess chunk, StaticCache2D<GenerationChunkHolder> neighbours, ChunkStatus toStatus, Priority priority) {
        super(scheduler, world, chunkX, chunkZ);
        if (!Priority.isValidPriority((Priority)priority)) {
            throw new IllegalArgumentException("Invalid priority " + String.valueOf(priority));
        }
        this.fromChunk = chunk;
        this.fromStatus = chunk.getPersistedStatus();
        this.toStatus = toStatus;
        this.neighbours = neighbours;
        if (this.toStatus.moonrise$isParallelCapable()) {
            this.generateTask = this.scheduler.parallelGenExecutor.createTask((Runnable)this, priority);
        } else {
            int writeRadius = this.toStatus.moonrise$getWriteRadius();
            if (writeRadius < 0) {
                throw new IllegalStateException("Infinite write radius is not supported");
            }
            this.generateTask = this.scheduler.radiusAwareScheduler.createTask(chunkX, chunkZ, writeRadius, (Runnable)this, priority);
        }
    }

    @Override
    public ChunkStatus getTargetStatus() {
        return this.toStatus;
    }

    private boolean isEmptyTask() {
        boolean generation = !this.fromStatus.isOrAfter(this.toStatus);
        return generation && this.toStatus.moonrise$isEmptyGenStatus() || !generation && this.toStatus.moonrise$isEmptyLoadStatus();
    }

    @Override
    public void run() {
        ChunkAccess newChunk;
        CompletionStage<Object> completeFuture;
        boolean generation;
        ChunkAccess chunk = this.fromChunk;
        ServerChunkCache serverChunkCache = this.world.getChunkSource();
        ChunkMap chunkMap = serverChunkCache.chunkMap;
        boolean completing = false;
        WorldGenContext ctx = chunkMap.worldGenContext;
        try {
            boolean bl = generation = !chunk.getPersistedStatus().isOrAfter(this.toStatus);
            if (generation) {
                if (this.toStatus.moonrise$isEmptyGenStatus()) {
                    if (chunk instanceof ProtoChunk) {
                        ((ProtoChunk)chunk).setPersistedStatus(this.toStatus);
                    }
                    completing = true;
                    this.complete(chunk, null);
                    return;
                }
                completeFuture = ChunkPyramid.GENERATION_PYRAMID.getStepTo(this.toStatus).apply(ctx, this.neighbours, this.fromChunk).whenComplete((either, throwable) -> {
                    if (either instanceof ProtoChunk) {
                        ProtoChunk proto = (ProtoChunk)either;
                        proto.setPersistedStatus(this.toStatus);
                    }
                });
            } else {
                if (this.toStatus.moonrise$isEmptyLoadStatus()) {
                    completing = true;
                    this.complete(chunk, null);
                    return;
                }
                completeFuture = ChunkPyramid.LOADING_PYRAMID.getStepTo(this.toStatus).apply(ctx, this.neighbours, this.fromChunk);
            }
        }
        catch (Throwable throwable2) {
            if (!completing) {
                this.complete(null, throwable2);
                return;
            }
            this.scheduler.unrecoverableChunkSystemFailure(this.chunkX, this.chunkZ, Map.of("Target status", ChunkTaskScheduler.stringIfNull(this.toStatus), "From status", ChunkTaskScheduler.stringIfNull(this.fromStatus), "Generation task", this), throwable2);
            LOGGER.error("Failed to complete status for chunk: status:" + String.valueOf(this.toStatus) + ", chunk: (" + this.chunkX + "," + this.chunkZ + "), world: " + WorldUtil.getWorldName((Level)this.world), throwable2);
            return;
        }
        if (!((CompletableFuture)completeFuture).isDone() && !this.toStatus.moonrise$getWarnedAboutNoImmediateComplete().getAndSet(true)) {
            LOGGER.warn("Future status not complete after scheduling: " + this.toStatus.toString() + ", generate: " + generation);
        }
        try {
            newChunk = (ChunkAccess)((CompletableFuture)completeFuture).join();
        }
        catch (Throwable throwable3) {
            this.complete(null, throwable3);
            return;
        }
        if (newChunk == null) {
            this.complete(null, new IllegalStateException("Chunk for status: " + this.toStatus.toString() + ", generation: " + generation + " should not be null! Future: " + String.valueOf(completeFuture)).fillInStackTrace());
            return;
        }
        this.complete(newChunk, null);
    }

    @Override
    public boolean isScheduled() {
        return this.scheduled;
    }

    @Override
    public void schedule() {
        if (SCHEDULED_HANDLE.getAndSet(this, true)) {
            throw new IllegalStateException("Cannot double call schedule()");
        }
        if (this.isEmptyTask()) {
            if (this.generateTask.cancel()) {
                this.run();
            }
        } else {
            this.generateTask.queue();
        }
    }

    @Override
    public void cancel() {
        if (this.generateTask.cancel()) {
            this.complete(null, null);
        }
    }

    @Override
    public Priority getPriority() {
        return this.generateTask.getPriority();
    }

    @Override
    public void lowerPriority(Priority priority) {
        if (!Priority.isValidPriority((Priority)priority)) {
            throw new IllegalArgumentException("Invalid priority " + String.valueOf(priority));
        }
        this.generateTask.lowerPriority(priority);
    }

    @Override
    public void setPriority(Priority priority) {
        if (!Priority.isValidPriority((Priority)priority)) {
            throw new IllegalArgumentException("Invalid priority " + String.valueOf(priority));
        }
        this.generateTask.setPriority(priority);
    }

    @Override
    public void raisePriority(Priority priority) {
        if (!Priority.isValidPriority((Priority)priority)) {
            throw new IllegalArgumentException("Invalid priority " + String.valueOf(priority));
        }
        this.generateTask.raisePriority(priority);
    }
}
