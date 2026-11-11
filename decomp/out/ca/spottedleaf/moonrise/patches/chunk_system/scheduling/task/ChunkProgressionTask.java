/*
 * Decompiled with CFR 0.152.
 * 
 * Could not load the following classes:
 *  ca.spottedleaf.concurrentutil.collection.MultiThreadedQueue
 *  ca.spottedleaf.concurrentutil.util.ConcurrentUtil
 *  ca.spottedleaf.concurrentutil.util.Priority
 *  ca.spottedleaf.moonrise.common.util.WorldUtil
 *  net.minecraft.world.level.Level
 *  net.minecraft.world.level.chunk.ChunkAccess
 */
package ca.spottedleaf.moonrise.patches.chunk_system.scheduling.task;

import ca.spottedleaf.concurrentutil.collection.MultiThreadedQueue;
import ca.spottedleaf.concurrentutil.util.ConcurrentUtil;
import ca.spottedleaf.concurrentutil.util.Priority;
import ca.spottedleaf.moonrise.common.util.WorldUtil;
import ca.spottedleaf.moonrise.patches.chunk_system.scheduling.ChunkTaskScheduler;
import java.lang.invoke.VarHandle;
import java.util.Map;
import java.util.function.BiConsumer;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.chunk.ChunkAccess;
import net.minecraft.world.level.chunk.status.ChunkStatus;

public abstract class ChunkProgressionTask {
    private final MultiThreadedQueue<BiConsumer<ChunkAccess, Throwable>> waiters = new MultiThreadedQueue();
    private ChunkAccess completedChunk;
    private Throwable completedThrowable;
    protected final ChunkTaskScheduler scheduler;
    protected final ServerLevel world;
    protected final int chunkX;
    protected final int chunkZ;
    protected volatile boolean completed;
    protected static final VarHandle COMPLETED_HANDLE = ConcurrentUtil.getVarHandle(ChunkProgressionTask.class, (String)"completed", Boolean.TYPE);

    protected ChunkProgressionTask(ChunkTaskScheduler scheduler, ServerLevel world, int chunkX, int chunkZ) {
        this.scheduler = scheduler;
        this.world = world;
        this.chunkX = chunkX;
        this.chunkZ = chunkZ;
    }

    public abstract boolean isScheduled();

    public abstract ChunkStatus getTargetStatus();

    public abstract void schedule();

    public abstract void cancel();

    public abstract Priority getPriority();

    public abstract void lowerPriority(Priority var1);

    public abstract void setPriority(Priority var1);

    public abstract void raisePriority(Priority var1);

    public final void onComplete(BiConsumer<ChunkAccess, Throwable> onComplete) {
        if (!this.waiters.add(onComplete)) {
            try {
                onComplete.accept(this.completedChunk, this.completedThrowable);
            }
            catch (Throwable throwable) {
                this.scheduler.unrecoverableChunkSystemFailure(this.chunkX, this.chunkZ, Map.of("Consumer", ChunkTaskScheduler.stringIfNull(onComplete), "Completed throwable", ChunkTaskScheduler.stringIfNull(this.completedThrowable)), throwable);
            }
        }
    }

    protected final void complete(ChunkAccess chunk, Throwable throwable) {
        try {
            this.complete0(chunk, throwable);
        }
        catch (Throwable thr2) {
            this.scheduler.unrecoverableChunkSystemFailure(this.chunkX, this.chunkZ, Map.of("Completed throwable", ChunkTaskScheduler.stringIfNull(throwable)), thr2);
        }
    }

    private void complete0(ChunkAccess chunk, Throwable throwable) {
        BiConsumer consumer;
        if (COMPLETED_HANDLE.getAndSet(this, true)) {
            throw new IllegalStateException("Already completed");
        }
        this.completedChunk = chunk;
        this.completedThrowable = throwable;
        while ((consumer = (BiConsumer)this.waiters.pollOrBlockAdds()) != null) {
            consumer.accept(chunk, throwable);
        }
    }

    public String toString() {
        return "ChunkProgressionTask{class: " + this.getClass().getName() + ", for world: " + WorldUtil.getWorldName((Level)this.world) + ", chunk: (" + this.chunkX + "," + this.chunkZ + "), hashcode: " + System.identityHashCode(this) + ", priority: " + String.valueOf(this.getPriority()) + ", status: " + this.getTargetStatus().toString() + ", scheduled: " + this.isScheduled() + "}";
    }
}
