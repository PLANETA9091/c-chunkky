/*
 * Decompiled with CFR 0.152.
 * 
 * Could not load the following classes:
 *  com.google.common.collect.ImmutableList
 *  net.minecraft.util.StaticCache2D
 *  net.minecraft.util.profiling.Profiler
 *  net.minecraft.util.profiling.Zone
 *  net.minecraft.util.profiling.jfr.JvmProfiler
 *  net.minecraft.util.profiling.jfr.callback.ProfiledDuration
 *  net.minecraft.world.level.chunk.ChunkAccess
 *  net.minecraft.world.level.chunk.ProtoChunk
 *  org.jspecify.annotations.Nullable
 */
package net.minecraft.world.level.chunk.status;

import ca.spottedleaf.moonrise.patches.chunk_system.status.ChunkSystemChunkStep;
import com.google.common.collect.ImmutableList;
import java.util.Arrays;
import java.util.Objects;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.CompletionStage;
import net.minecraft.server.level.GenerationChunkHolder;
import net.minecraft.util.StaticCache2D;
import net.minecraft.util.profiling.Profiler;
import net.minecraft.util.profiling.Zone;
import net.minecraft.util.profiling.jfr.JvmProfiler;
import net.minecraft.util.profiling.jfr.callback.ProfiledDuration;
import net.minecraft.world.level.chunk.ChunkAccess;
import net.minecraft.world.level.chunk.ProtoChunk;
import net.minecraft.world.level.chunk.status.ChunkDependencies;
import net.minecraft.world.level.chunk.status.ChunkStatus;
import net.minecraft.world.level.chunk.status.ChunkStatusTask;
import net.minecraft.world.level.chunk.status.ChunkStatusTasks;
import net.minecraft.world.level.chunk.status.WorldGenContext;
import org.jspecify.annotations.Nullable;

public final class ChunkStep
implements ChunkSystemChunkStep {
    private final ChunkStatus targetStatus;
    private final ChunkDependencies directDependencies;
    private final ChunkDependencies accumulatedDependencies;
    private final int blockStateWriteRadius;
    private final ChunkStatusTask task;
    private final ChunkStatus[] byRadius;

    public ChunkStep(ChunkStatus targetStatus, ChunkDependencies directDependencies, ChunkDependencies accumulatedDependencies, int blockStateWriteRadius, ChunkStatusTask task) {
        this.targetStatus = targetStatus;
        this.directDependencies = directDependencies;
        this.accumulatedDependencies = accumulatedDependencies;
        this.blockStateWriteRadius = blockStateWriteRadius;
        this.task = task;
        this.byRadius = new ChunkStatus[this.getAccumulatedRadiusOf(ChunkStatus.EMPTY) + 1];
        this.byRadius[0] = targetStatus.getParent();
        for (ChunkStatus status = targetStatus.getParent(); status != ChunkStatus.EMPTY; status = status.getParent()) {
            int radius = this.getAccumulatedRadiusOf(status);
            for (int j = 0; j <= radius; ++j) {
                if (this.byRadius[j] != null) continue;
                this.byRadius[j] = status;
            }
        }
    }

    @Override
    public final ChunkStatus moonrise$getRequiredStatusAtRadius(int radius) {
        return this.byRadius[radius];
    }

    public int getAccumulatedRadiusOf(ChunkStatus status) {
        return status == this.targetStatus ? 0 : this.accumulatedDependencies.getRadiusOf(status);
    }

    public CompletableFuture<ChunkAccess> apply(WorldGenContext context, StaticCache2D<GenerationChunkHolder> cache, ChunkAccess chunk) {
        if (chunk.getPersistedStatus().isBefore(this.targetStatus)) {
            String stepName = this.targetStatus.getName();
            try (Zone var5 = Profiler.get().zone(stepName);){
                ProfiledDuration profiledDuration = JvmProfiler.INSTANCE.onChunkGenerate(chunk.getPos(), context.level().dimension(), stepName);
                CompletionStage completionStage = this.task.doWork(context, this, cache, chunk).thenApply(newCenterChunk -> this.completeChunkGeneration((ChunkAccess)newCenterChunk, profiledDuration));
                return completionStage;
            }
        }
        return this.task.doWork(context, this, cache, chunk);
    }

    private ChunkAccess completeChunkGeneration(ChunkAccess newCenterChunk, @Nullable ProfiledDuration profiledDuration) {
        ProtoChunk protochunk;
        if (newCenterChunk instanceof ProtoChunk && (protochunk = (ProtoChunk)newCenterChunk).getPersistedStatus().isBefore(this.targetStatus)) {
            protochunk.setPersistedStatus(this.targetStatus);
        }
        if (profiledDuration != null) {
            profiledDuration.finish(true);
        }
        return newCenterChunk;
    }

    public ChunkStatus targetStatus() {
        return this.targetStatus;
    }

    public ChunkDependencies directDependencies() {
        return this.directDependencies;
    }

    public ChunkDependencies accumulatedDependencies() {
        return this.accumulatedDependencies;
    }

    public int blockStateWriteRadius() {
        return this.blockStateWriteRadius;
    }

    public ChunkStatusTask task() {
        return this.task;
    }

    public boolean equals(Object obj) {
        if (obj == this) {
            return true;
        }
        if (obj == null || obj.getClass() != this.getClass()) {
            return false;
        }
        ChunkStep that = (ChunkStep)obj;
        return Objects.equals(this.targetStatus, that.targetStatus) && Objects.equals(this.directDependencies, that.directDependencies) && Objects.equals(this.accumulatedDependencies, that.accumulatedDependencies) && this.blockStateWriteRadius == that.blockStateWriteRadius && Objects.equals(this.task, that.task);
    }

    public int hashCode() {
        return Objects.hash(this.targetStatus, this.directDependencies, this.accumulatedDependencies, this.blockStateWriteRadius, this.task);
    }

    public String toString() {
        return "ChunkStep[targetStatus=" + String.valueOf(this.targetStatus) + ", directDependencies=" + String.valueOf(this.directDependencies) + ", accumulatedDependencies=" + String.valueOf(this.accumulatedDependencies) + ", blockStateWriteRadius=" + this.blockStateWriteRadius + ", task=" + String.valueOf(this.task) + "]";
    }

    public static class Builder {
        private final ChunkStatus status;
        private final @Nullable ChunkStep parent;
        private ChunkStatus[] directDependenciesByRadius;
        private int blockStateWriteRadius = -1;
        private ChunkStatusTask task = ChunkStatusTasks::passThrough;

        protected Builder(ChunkStatus status) {
            if (status.getParent() != status) {
                throw new IllegalArgumentException("Not starting with the first status: " + String.valueOf(status));
            }
            this.status = status;
            this.parent = null;
            this.directDependenciesByRadius = new ChunkStatus[0];
        }

        protected Builder(ChunkStatus status, ChunkStep parent) {
            if (parent.targetStatus.getIndex() != status.getIndex() - 1) {
                throw new IllegalArgumentException("Out of order status: " + String.valueOf(status));
            }
            this.status = status;
            this.parent = parent;
            this.directDependenciesByRadius = new ChunkStatus[]{parent.targetStatus};
        }

        public Builder addRequirement(ChunkStatus status, int radius) {
            if (status.isOrAfter(this.status)) {
                throw new IllegalArgumentException("Status " + String.valueOf(status) + " can not be required by " + String.valueOf(this.status));
            }
            int newLength = radius + 1;
            ChunkStatus[] previous = this.directDependenciesByRadius;
            if (newLength > previous.length) {
                this.directDependenciesByRadius = new ChunkStatus[newLength];
                Arrays.fill(this.directDependenciesByRadius, status);
            }
            for (int i = 0; i < Math.min(newLength, previous.length); ++i) {
                this.directDependenciesByRadius[i] = ChunkStatus.max(previous[i], status);
            }
            return this;
        }

        public Builder blockStateWriteRadius(int radius) {
            this.blockStateWriteRadius = radius;
            return this;
        }

        public Builder setTask(ChunkStatusTask task) {
            this.task = task;
            return this;
        }

        public ChunkStep build() {
            return new ChunkStep(this.status, new ChunkDependencies((ImmutableList<ChunkStatus>)ImmutableList.copyOf((Object[])this.directDependenciesByRadius)), new ChunkDependencies((ImmutableList<ChunkStatus>)ImmutableList.copyOf((Object[])this.buildAccumulatedDependencies())), this.blockStateWriteRadius, this.task);
        }

        private ChunkStatus[] buildAccumulatedDependencies() {
            if (this.parent == null) {
                return this.directDependenciesByRadius;
            }
            int radiusOfParent = this.getRadiusOfParent(this.parent.targetStatus);
            ChunkDependencies parentDependencies = this.parent.accumulatedDependencies;
            ChunkStatus[] accumulatedDependencies = new ChunkStatus[Math.max(radiusOfParent + parentDependencies.size(), this.directDependenciesByRadius.length)];
            for (int distance = 0; distance < accumulatedDependencies.length; ++distance) {
                int distanceInParent = distance - radiusOfParent;
                accumulatedDependencies[distance] = distanceInParent < 0 || distanceInParent >= parentDependencies.size() ? this.directDependenciesByRadius[distance] : (distance >= this.directDependenciesByRadius.length ? parentDependencies.get(distanceInParent) : ChunkStatus.max(this.directDependenciesByRadius[distance], parentDependencies.get(distanceInParent)));
            }
            return accumulatedDependencies;
        }

        private int getRadiusOfParent(ChunkStatus status) {
            for (int i = this.directDependenciesByRadius.length - 1; i >= 0; --i) {
                if (!this.directDependenciesByRadius[i].isOrAfter(status)) continue;
                return i;
            }
            return 0;
        }
    }
}
