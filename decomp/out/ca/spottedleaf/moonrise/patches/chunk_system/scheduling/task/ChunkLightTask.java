/*
 * Decompiled with CFR 0.152.
 * 
 * Could not load the following classes:
 *  ca.spottedleaf.concurrentutil.util.Priority
 *  ca.spottedleaf.moonrise.common.util.WorldUtil
 *  ca.spottedleaf.moonrise.patches.starlight.light.StarLightEngine
 *  ca.spottedleaf.moonrise.patches.starlight.light.StarLightInterface
 *  ca.spottedleaf.moonrise.patches.starlight.light.StarLightInterface$ServerLightQueue
 *  net.minecraft.world.level.ChunkPos
 *  net.minecraft.world.level.Level
 *  net.minecraft.world.level.chunk.ChunkAccess
 *  net.minecraft.world.level.chunk.ProtoChunk
 *  org.apache.logging.log4j.LogManager
 *  org.apache.logging.log4j.Logger
 */
package ca.spottedleaf.moonrise.patches.chunk_system.scheduling.task;

import ca.spottedleaf.concurrentutil.util.Priority;
import ca.spottedleaf.moonrise.common.util.WorldUtil;
import ca.spottedleaf.moonrise.patches.chunk_system.scheduling.ChunkTaskScheduler;
import ca.spottedleaf.moonrise.patches.chunk_system.scheduling.PriorityHolder;
import ca.spottedleaf.moonrise.patches.chunk_system.scheduling.task.ChunkProgressionTask;
import ca.spottedleaf.moonrise.patches.starlight.light.StarLightEngine;
import ca.spottedleaf.moonrise.patches.starlight.light.StarLightInterface;
import java.util.function.BooleanSupplier;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.chunk.ChunkAccess;
import net.minecraft.world.level.chunk.ProtoChunk;
import net.minecraft.world.level.chunk.status.ChunkStatus;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

public final class ChunkLightTask
extends ChunkProgressionTask {
    private static final Logger LOGGER = LogManager.getLogger();
    private final ChunkAccess fromChunk;
    private final LightTaskPriorityHolder priorityHolder;

    public ChunkLightTask(ChunkTaskScheduler scheduler, ServerLevel world, int chunkX, int chunkZ, ChunkAccess chunk, Priority priority) {
        super(scheduler, world, chunkX, chunkZ);
        if (!Priority.isValidPriority((Priority)priority)) {
            throw new IllegalArgumentException("Invalid priority " + String.valueOf(priority));
        }
        this.priorityHolder = new LightTaskPriorityHolder(priority, this);
        this.fromChunk = chunk;
    }

    @Override
    public boolean isScheduled() {
        return this.priorityHolder.isScheduled();
    }

    @Override
    public ChunkStatus getTargetStatus() {
        return ChunkStatus.LIGHT;
    }

    @Override
    public void schedule() {
        this.priorityHolder.schedule();
    }

    @Override
    public void cancel() {
        this.priorityHolder.cancel();
    }

    @Override
    public Priority getPriority() {
        return this.priorityHolder.getPriority();
    }

    @Override
    public void lowerPriority(Priority priority) {
        this.priorityHolder.raisePriority(priority);
    }

    @Override
    public void setPriority(Priority priority) {
        this.priorityHolder.setPriority(priority);
    }

    @Override
    public void raisePriority(Priority priority) {
        this.priorityHolder.raisePriority(priority);
    }

    private static final class LightTaskPriorityHolder
    extends PriorityHolder {
        private final ChunkLightTask task;

        private LightTaskPriorityHolder(Priority priority, ChunkLightTask task) {
            super(priority);
            this.task = task;
        }

        @Override
        protected void cancelScheduled() {
            ChunkLightTask task = this.task;
            task.complete(null, null);
        }

        @Override
        protected Priority getScheduledPriority() {
            ChunkLightTask task = this.task;
            return task.world.getChunkSource().getLightEngine().starlight$getLightEngine().getServerLightQueue().getPriority(task.chunkX, task.chunkZ);
        }

        @Override
        protected void scheduleTask(Priority priority) {
            ChunkLightTask task = this.task;
            StarLightInterface starLightInterface = task.world.getChunkSource().getLightEngine().starlight$getLightEngine();
            StarLightInterface.ServerLightQueue lightQueue = starLightInterface.getServerLightQueue();
            lightQueue.queueChunkLightTask(new ChunkPos(task.chunkX, task.chunkZ), (BooleanSupplier)new LightTask(starLightInterface, task), priority);
            lightQueue.setPriority(task.chunkX, task.chunkZ, priority);
        }

        @Override
        protected void lowerPriorityScheduled(Priority priority) {
            ChunkLightTask task = this.task;
            StarLightInterface starLightInterface = task.world.getChunkSource().getLightEngine().starlight$getLightEngine();
            StarLightInterface.ServerLightQueue lightQueue = starLightInterface.getServerLightQueue();
            lightQueue.lowerPriority(task.chunkX, task.chunkZ, priority);
        }

        @Override
        protected void setPriorityScheduled(Priority priority) {
            ChunkLightTask task = this.task;
            StarLightInterface starLightInterface = task.world.getChunkSource().getLightEngine().starlight$getLightEngine();
            StarLightInterface.ServerLightQueue lightQueue = starLightInterface.getServerLightQueue();
            lightQueue.setPriority(task.chunkX, task.chunkZ, priority);
        }

        @Override
        protected void raisePriorityScheduled(Priority priority) {
            ChunkLightTask task = this.task;
            StarLightInterface starLightInterface = task.world.getChunkSource().getLightEngine().starlight$getLightEngine();
            StarLightInterface.ServerLightQueue lightQueue = starLightInterface.getServerLightQueue();
            lightQueue.raisePriority(task.chunkX, task.chunkZ, priority);
        }
    }

    private static final class LightTask
    implements BooleanSupplier {
        private final StarLightInterface lightEngine;
        private final ChunkLightTask task;

        public LightTask(StarLightInterface lightEngine, ChunkLightTask task) {
            this.lightEngine = lightEngine;
            this.task = task;
        }

        @Override
        public boolean getAsBoolean() {
            ChunkLightTask task = this.task;
            if (!task.priorityHolder.markExecuting()) {
                return false;
            }
            try {
                ProtoChunk chunk;
                Boolean[] emptySections = StarLightEngine.getEmptySectionsForChunk((ChunkAccess)task.fromChunk);
                if (task.fromChunk.isLightCorrect() && task.fromChunk.getPersistedStatus().isOrAfter(ChunkStatus.LIGHT)) {
                    this.lightEngine.forceLoadInChunk(task.fromChunk, emptySections);
                    this.lightEngine.checkChunkEdges(task.chunkX, task.chunkZ);
                } else {
                    task.fromChunk.setLightCorrect(false);
                    this.lightEngine.lightChunk(task.fromChunk, emptySections);
                    task.fromChunk.setLightCorrect(true);
                }
                ChunkAccess chunkAccess = task.fromChunk;
                if (chunkAccess instanceof ProtoChunk && (chunk = (ProtoChunk)chunkAccess).getPersistedStatus() == ChunkStatus.LIGHT.getParent()) {
                    chunk.setPersistedStatus(ChunkStatus.LIGHT);
                }
            }
            catch (Throwable thr) {
                LOGGER.fatal("Failed to light chunk " + task.fromChunk.getPos().toString() + " in world '" + WorldUtil.getWorldName((Level)this.lightEngine.getWorld()) + "'", thr);
                task.complete(null, thr);
                return true;
            }
            task.complete(task.fromChunk, null);
            return true;
        }
    }
}
