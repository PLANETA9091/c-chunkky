/*
 * Decompiled with CFR 0.152.
 * 
 * Could not load the following classes:
 *  com.mojang.datafixers.util.Pair
 *  net.minecraft.util.StaticCache2D
 *  net.minecraft.util.VisibleForDebug
 *  net.minecraft.world.level.ChunkPos
 *  net.minecraft.world.level.chunk.ChunkAccess
 *  net.minecraft.world.level.chunk.ImposterProtoChunk
 *  org.jspecify.annotations.Nullable
 */
package net.minecraft.server.level;

import ca.spottedleaf.moonrise.patches.chunk_system.level.chunk.ChunkSystemChunkHolder;
import ca.spottedleaf.moonrise.patches.chunk_system.scheduling.NewChunkHolder;
import com.mojang.datafixers.util.Pair;
import java.util.List;
import java.util.concurrent.CompletableFuture;
import net.minecraft.server.level.ChunkGenerationTask;
import net.minecraft.server.level.ChunkMap;
import net.minecraft.server.level.ChunkResult;
import net.minecraft.server.level.FullChunkStatus;
import net.minecraft.server.level.GeneratingChunkMap;
import net.minecraft.util.StaticCache2D;
import net.minecraft.util.VisibleForDebug;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.chunk.ChunkAccess;
import net.minecraft.world.level.chunk.ImposterProtoChunk;
import net.minecraft.world.level.chunk.status.ChunkStatus;
import net.minecraft.world.level.chunk.status.ChunkStep;
import org.jspecify.annotations.Nullable;

public abstract class GenerationChunkHolder {
    private static final List<ChunkStatus> CHUNK_STATUSES = ChunkStatus.getStatusList();
    private static final ChunkResult<ChunkAccess> NOT_DONE_YET = ChunkResult.error("Not done yet");
    public static final ChunkResult<ChunkAccess> UNLOADED_CHUNK = ChunkResult.error("Unloaded chunk");
    public static final CompletableFuture<ChunkResult<ChunkAccess>> UNLOADED_CHUNK_FUTURE = CompletableFuture.completedFuture(UNLOADED_CHUNK);
    protected final ChunkPos pos;

    public GenerationChunkHolder(ChunkPos pos) {
        this.pos = pos;
        if (!pos.isValid()) {
            throw new IllegalStateException("Trying to create chunk out of reasonable bounds: " + String.valueOf(pos));
        }
    }

    public CompletableFuture<ChunkResult<ChunkAccess>> scheduleChunkGenerationTask(ChunkStatus status, ChunkMap scheduler) {
        throw new UnsupportedOperationException();
    }

    CompletableFuture<ChunkResult<ChunkAccess>> applyStep(ChunkStep step, GeneratingChunkMap chunkMap, StaticCache2D<GenerationChunkHolder> cache) {
        throw new UnsupportedOperationException();
    }

    protected void updateHighestAllowedStatus(ChunkMap scheduler) {
        throw new UnsupportedOperationException();
    }

    public void replaceProtoChunk(ImposterProtoChunk chunk) {
        throw new UnsupportedOperationException();
    }

    void removeTask(ChunkGenerationTask task) {
        throw new UnsupportedOperationException();
    }

    private void rescheduleChunkTask(ChunkMap scheduler, @Nullable ChunkStatus status) {
        throw new UnsupportedOperationException();
    }

    private CompletableFuture<ChunkResult<ChunkAccess>> getOrCreateFuture(ChunkStatus status) {
        throw new UnsupportedOperationException();
    }

    private void failAndClearPendingFuturesBetween(@Nullable ChunkStatus fromExclusive, ChunkStatus toInclusive) {
        throw new UnsupportedOperationException();
    }

    private void failAndClearPendingFuture(int index, CompletableFuture<ChunkResult<ChunkAccess>> previous) {
        throw new UnsupportedOperationException();
    }

    private void completeFuture(ChunkStatus status, ChunkAccess chunk) {
        throw new UnsupportedOperationException();
    }

    private @Nullable ChunkStatus findHighestStatusWithPendingFuture(@Nullable ChunkStatus newStatus) {
        throw new UnsupportedOperationException();
    }

    private boolean acquireStatusBump(ChunkStatus status) {
        throw new UnsupportedOperationException();
    }

    private boolean isStatusDisallowed(ChunkStatus status) {
        throw new UnsupportedOperationException();
    }

    protected abstract void addSaveDependency(CompletableFuture<?> var1);

    public void increaseGenerationRefCount() {
        throw new UnsupportedOperationException();
    }

    public void decreaseGenerationRefCount() {
        throw new UnsupportedOperationException();
    }

    public @Nullable ChunkAccess getChunkIfPresentUnchecked(ChunkStatus status) {
        return ((ChunkSystemChunkHolder)((Object)this)).moonrise$getRealChunkHolder().getChunkIfPresentUnchecked(status);
    }

    public @Nullable ChunkAccess getChunkIfPresent(ChunkStatus status) {
        return ((ChunkSystemChunkHolder)((Object)this)).moonrise$getRealChunkHolder().getChunkIfPresent(status);
    }

    public @Nullable ChunkAccess getLatestChunk() {
        NewChunkHolder.ChunkCompletion lastCompletion = ((ChunkSystemChunkHolder)((Object)this)).moonrise$getRealChunkHolder().getLastChunkCompletion();
        return lastCompletion == null ? null : lastCompletion.chunk();
    }

    public @Nullable ChunkStatus getPersistedStatus() {
        ChunkAccess chunk = this.getLatestChunk();
        return chunk == null ? null : chunk.getPersistedStatus();
    }

    public ChunkPos getPos() {
        return this.pos;
    }

    public FullChunkStatus getFullStatus() {
        return ((ChunkSystemChunkHolder)((Object)this)).moonrise$getRealChunkHolder().getChunkStatus();
    }

    public abstract int getTicketLevel();

    public abstract int getQueueLevel();

    @VisibleForDebug
    public List<Pair<ChunkStatus, @Nullable CompletableFuture<ChunkResult<ChunkAccess>>>> getAllFutures() {
        throw new UnsupportedOperationException();
    }

    @VisibleForDebug
    public @Nullable ChunkStatus getLatestStatus() {
        NewChunkHolder.ChunkCompletion lastCompletion = ((ChunkSystemChunkHolder)((Object)this)).moonrise$getRealChunkHolder().getLastChunkCompletion();
        return lastCompletion == null ? null : lastCompletion.genStatus();
    }
}
