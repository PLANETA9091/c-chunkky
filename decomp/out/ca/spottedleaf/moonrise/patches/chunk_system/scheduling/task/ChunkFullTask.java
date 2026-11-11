/*
 * Decompiled with CFR 0.152.
 * 
 * Could not load the following classes:
 *  ca.spottedleaf.concurrentutil.executor.PrioritisedExecutor$PrioritisedTask
 *  ca.spottedleaf.concurrentutil.util.ConcurrentUtil
 *  ca.spottedleaf.concurrentutil.util.Priority
 *  ca.spottedleaf.moonrise.common.PlatformHooks
 *  net.minecraft.world.level.chunk.ChunkAccess
 *  net.minecraft.world.level.chunk.ImposterProtoChunk
 *  net.minecraft.world.level.chunk.LevelChunk
 *  net.minecraft.world.level.chunk.ProtoChunk
 *  org.slf4j.Logger
 *  org.slf4j.LoggerFactory
 */
package ca.spottedleaf.moonrise.patches.chunk_system.scheduling.task;

import ca.spottedleaf.concurrentutil.executor.PrioritisedExecutor;
import ca.spottedleaf.concurrentutil.util.ConcurrentUtil;
import ca.spottedleaf.concurrentutil.util.Priority;
import ca.spottedleaf.moonrise.common.PlatformHooks;
import ca.spottedleaf.moonrise.patches.chunk_system.level.poi.PoiChunk;
import ca.spottedleaf.moonrise.patches.chunk_system.scheduling.ChunkTaskScheduler;
import ca.spottedleaf.moonrise.patches.chunk_system.scheduling.NewChunkHolder;
import ca.spottedleaf.moonrise.patches.chunk_system.scheduling.task.ChunkProgressionTask;
import java.lang.invoke.VarHandle;
import net.minecraft.server.level.GenerationChunkHolder;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.world.level.chunk.ChunkAccess;
import net.minecraft.world.level.chunk.ImposterProtoChunk;
import net.minecraft.world.level.chunk.LevelChunk;
import net.minecraft.world.level.chunk.ProtoChunk;
import net.minecraft.world.level.chunk.status.ChunkStatus;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

public final class ChunkFullTask
extends ChunkProgressionTask
implements Runnable {
    private static final Logger LOGGER = LoggerFactory.getLogger(ChunkFullTask.class);
    private final NewChunkHolder chunkHolder;
    private final ChunkAccess fromChunk;
    private final PrioritisedExecutor.PrioritisedTask convertToFullTask;
    protected volatile boolean scheduled;
    protected static final VarHandle SCHEDULED_HANDLE = ConcurrentUtil.getVarHandle(ChunkFullTask.class, (String)"scheduled", Boolean.TYPE);

    public ChunkFullTask(ChunkTaskScheduler scheduler, ServerLevel world, int chunkX, int chunkZ, NewChunkHolder chunkHolder, ChunkAccess fromChunk, Priority priority) {
        super(scheduler, world, chunkX, chunkZ);
        this.chunkHolder = chunkHolder;
        this.fromChunk = fromChunk;
        this.convertToFullTask = scheduler.createChunkTask(chunkX, chunkZ, this, priority);
    }

    @Override
    public ChunkStatus getTargetStatus() {
        return ChunkStatus.FULL;
    }

    /*
     * WARNING - Removed try catching itself - possible behaviour change.
     */
    @Override
    public void run() {
        LevelChunk chunk;
        PlatformHooks platformHooks = PlatformHooks.get();
        try {
            PoiChunk poiChunk = this.chunkHolder.getPoiChunk();
            if (poiChunk == null) {
                LOGGER.error("Expected poi chunk to be loaded with chunk for task " + this.toString());
            } else {
                poiChunk.load();
                this.world.getPoiManager().moonrise$checkConsistency(this.fromChunk);
            }
            ChunkAccess chunkAccess = this.fromChunk;
            if (chunkAccess instanceof ImposterProtoChunk) {
                ImposterProtoChunk wrappedFull = (ImposterProtoChunk)chunkAccess;
                chunk = wrappedFull.getWrapped();
            } else {
                ServerLevel world = this.world;
                ProtoChunk protoChunk = (ProtoChunk)this.fromChunk;
                chunk = new LevelChunk(this.world, protoChunk, unused -> PlatformHooks.get().postLoadProtoChunk(world, protoChunk));
                this.chunkHolder.replaceProtoChunk(new ImposterProtoChunk(chunk, false));
            }
            chunk.moonrise$setChunkHolder(this.chunkHolder);
            NewChunkHolder chunkHolder = this.chunkHolder;
            chunk.setFullStatus(chunkHolder::getChunkStatus);
            try {
                platformHooks.setCurrentlyLoading((GenerationChunkHolder)this.chunkHolder.vanillaChunkHolder, chunk);
                this.world.moonrise$getChunkTaskScheduler().chunkHolderManager.fullTaskLoadEntityChunk(this.chunkX, this.chunkZ);
                chunk.runPostLoad();
                chunk.setLoaded(true);
                chunk.registerAllBlockEntitiesAfterLevelLoad();
                chunk.registerTickContainerInLevel(this.world);
                chunk.setUnsavedListener(this.world.getChunkSource().chunkMap.worldGenContext.unsavedListener());
                platformHooks.chunkFullStatusComplete(chunk, (ProtoChunk)this.fromChunk);
            }
            finally {
                platformHooks.setCurrentlyLoading((GenerationChunkHolder)this.chunkHolder.vanillaChunkHolder, null);
            }
        }
        catch (Throwable throwable) {
            this.complete(null, throwable);
            return;
        }
        this.complete((ChunkAccess)chunk, null);
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
        this.convertToFullTask.queue();
    }

    @Override
    public void cancel() {
        if (this.convertToFullTask.cancel()) {
            this.complete(null, null);
        }
    }

    @Override
    public Priority getPriority() {
        return this.convertToFullTask.getPriority();
    }

    @Override
    public void lowerPriority(Priority priority) {
        if (!Priority.isValidPriority((Priority)priority)) {
            throw new IllegalArgumentException("Invalid priority " + String.valueOf(priority));
        }
        this.convertToFullTask.lowerPriority(priority);
    }

    @Override
    public void setPriority(Priority priority) {
        if (!Priority.isValidPriority((Priority)priority)) {
            throw new IllegalArgumentException("Invalid priority " + String.valueOf(priority));
        }
        this.convertToFullTask.setPriority(priority);
    }

    @Override
    public void raisePriority(Priority priority) {
        if (!Priority.isValidPriority((Priority)priority)) {
            throw new IllegalArgumentException("Invalid priority " + String.valueOf(priority));
        }
        this.convertToFullTask.raisePriority(priority);
    }
}
