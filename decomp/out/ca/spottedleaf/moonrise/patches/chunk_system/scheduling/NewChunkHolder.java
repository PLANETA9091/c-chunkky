/*
 * Decompiled with CFR 0.152.
 * 
 * Could not load the following classes:
 *  ca.spottedleaf.concurrentutil.completable.CallbackCompletable
 *  ca.spottedleaf.concurrentutil.completable.Completable
 *  ca.spottedleaf.concurrentutil.executor.Cancellable
 *  ca.spottedleaf.concurrentutil.executor.PrioritisedExecutor$PrioritisedTask
 *  ca.spottedleaf.concurrentutil.lock.ReentrantAreaLock$Node
 *  ca.spottedleaf.concurrentutil.util.ConcurrentUtil
 *  ca.spottedleaf.concurrentutil.util.Priority
 *  ca.spottedleaf.moonrise.common.PlatformHooks
 *  ca.spottedleaf.moonrise.common.misc.LazyRunnable
 *  ca.spottedleaf.moonrise.common.util.CoordinateUtils
 *  ca.spottedleaf.moonrise.common.util.TickThread
 *  ca.spottedleaf.moonrise.common.util.WorldUtil
 *  com.google.gson.JsonArray
 *  com.google.gson.JsonElement
 *  com.google.gson.JsonNull
 *  com.google.gson.JsonObject
 *  com.google.gson.JsonPrimitive
 *  it.unimi.dsi.fastutil.objects.ObjectBidirectionalIterator
 *  it.unimi.dsi.fastutil.objects.Reference2ObjectLinkedOpenHashMap
 *  it.unimi.dsi.fastutil.objects.Reference2ObjectMap$Entry
 *  it.unimi.dsi.fastutil.objects.Reference2ObjectOpenHashMap
 *  it.unimi.dsi.fastutil.objects.ReferenceLinkedOpenHashSet
 *  net.minecraft.nbt.CompoundTag
 *  net.minecraft.world.entity.Entity
 *  net.minecraft.world.level.ChunkPos
 *  net.minecraft.world.level.Level
 *  net.minecraft.world.level.LevelHeightAccessor
 *  net.minecraft.world.level.chunk.ChunkAccess
 *  net.minecraft.world.level.chunk.ImposterProtoChunk
 *  net.minecraft.world.level.chunk.LevelChunk
 *  net.minecraft.world.level.chunk.storage.SerializableChunkData
 *  org.slf4j.Logger
 *  org.slf4j.LoggerFactory
 */
package ca.spottedleaf.moonrise.patches.chunk_system.scheduling;

import ca.spottedleaf.concurrentutil.completable.CallbackCompletable;
import ca.spottedleaf.concurrentutil.completable.Completable;
import ca.spottedleaf.concurrentutil.executor.Cancellable;
import ca.spottedleaf.concurrentutil.executor.PrioritisedExecutor;
import ca.spottedleaf.concurrentutil.lock.ReentrantAreaLock;
import ca.spottedleaf.concurrentutil.util.ConcurrentUtil;
import ca.spottedleaf.concurrentutil.util.Priority;
import ca.spottedleaf.moonrise.common.PlatformHooks;
import ca.spottedleaf.moonrise.common.misc.LazyRunnable;
import ca.spottedleaf.moonrise.common.util.CoordinateUtils;
import ca.spottedleaf.moonrise.common.util.TickThread;
import ca.spottedleaf.moonrise.common.util.WorldUtil;
import ca.spottedleaf.moonrise.patches.chunk_system.io.MoonriseRegionFileIO;
import ca.spottedleaf.moonrise.patches.chunk_system.level.chunk.ChunkData;
import ca.spottedleaf.moonrise.patches.chunk_system.level.entity.ChunkEntitySlices;
import ca.spottedleaf.moonrise.patches.chunk_system.level.poi.PoiChunk;
import ca.spottedleaf.moonrise.patches.chunk_system.scheduling.ChunkHolderManager;
import ca.spottedleaf.moonrise.patches.chunk_system.scheduling.ChunkTaskScheduler;
import ca.spottedleaf.moonrise.patches.chunk_system.scheduling.task.ChunkLoadTask;
import ca.spottedleaf.moonrise.patches.chunk_system.scheduling.task.ChunkProgressionTask;
import ca.spottedleaf.moonrise.patches.chunk_system.scheduling.task.GenericDataLoadTask;
import com.google.gson.JsonArray;
import com.google.gson.JsonElement;
import com.google.gson.JsonNull;
import com.google.gson.JsonObject;
import com.google.gson.JsonPrimitive;
import it.unimi.dsi.fastutil.objects.ObjectBidirectionalIterator;
import it.unimi.dsi.fastutil.objects.Reference2ObjectLinkedOpenHashMap;
import it.unimi.dsi.fastutil.objects.Reference2ObjectMap;
import it.unimi.dsi.fastutil.objects.Reference2ObjectOpenHashMap;
import it.unimi.dsi.fastutil.objects.ReferenceLinkedOpenHashSet;
import java.lang.invoke.VarHandle;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.Objects;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.function.Consumer;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.server.level.ChunkHolder;
import net.minecraft.server.level.ChunkLevel;
import net.minecraft.server.level.FullChunkStatus;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.LevelHeightAccessor;
import net.minecraft.world.level.chunk.ChunkAccess;
import net.minecraft.world.level.chunk.ImposterProtoChunk;
import net.minecraft.world.level.chunk.LevelChunk;
import net.minecraft.world.level.chunk.status.ChunkStatus;
import net.minecraft.world.level.chunk.storage.SerializableChunkData;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

public final class NewChunkHolder {
    private static final Logger LOGGER = LoggerFactory.getLogger(NewChunkHolder.class);
    public final ChunkData holderData;
    public final ServerLevel world;
    public final int chunkX;
    public final int chunkZ;
    public final ChunkTaskScheduler scheduler;
    private ChunkEntitySlices entityChunk;
    private CompoundTag pendingEntityChunk;
    private static final CompoundTag EMPTY_ENTITY_CHUNK = new CompoundTag();
    private ChunkLoadTask.EntityDataLoadTask entityDataLoadTask;
    private List<GenericDataLoadTaskCallback> entityDataLoadTaskWaiters;
    private PoiChunk poiChunk;
    private ChunkLoadTask.PoiDataLoadTask poiDataLoadTask;
    private List<GenericDataLoadTaskCallback> poiDataLoadTaskWaiters;
    private ChunkAccess currentChunk;
    private ChunkStatus currentGenStatus;
    private static final ChunkStatus[] ALL_STATUSES = ChunkStatus.getStatusList().toArray(new ChunkStatus[0]);
    private static final VarHandle CHUNK_COMPLETION_ARRAY_HANDLE = ConcurrentUtil.getArrayHandle(ChunkCompletion[].class);
    private final ChunkCompletion[] chunkCompletions = new ChunkCompletion[ALL_STATUSES.length];
    private volatile ChunkCompletion lastChunkCompletion;
    private ChunkStatus requestedGenStatus;
    private ChunkProgressionTask generationTask;
    private ChunkStatus generationTaskStatus;
    private final ReferenceLinkedOpenHashSet<NewChunkHolder> neighboursBlockingGenTask = new ReferenceLinkedOpenHashSet(4);
    private final Reference2ObjectLinkedOpenHashMap<NewChunkHolder, ChunkStatus> neighboursWaitingForUs = new Reference2ObjectLinkedOpenHashMap();
    private Priority priority = null;
    private boolean priorityLocked;
    private Priority neighbourRequestedPriority = null;
    private ChunkStatus failedGenStatus;
    private Throwable genTaskException;
    private Thread genTaskFailedThread;
    private boolean failedLightUpdate;
    private int oldTicketLevel = ChunkHolderManager.MAX_TICKET_LEVEL + 1;
    private int currentTicketLevel = ChunkHolderManager.MAX_TICKET_LEVEL + 1;
    public final ChunkHolder vanillaChunkHolder;
    private int totalNeighboursUsingThisChunk = 0;
    private boolean unloaded;
    private boolean inUnloadQueue = false;
    private UnloadTask chunkDataUnload;
    private UnloadTask entityDataUnload;
    private UnloadTask poiDataUnload;
    private UnloadState unloadState;
    static final int NEIGHBOUR_RADIUS = 2;
    private long fullNeighbourChunksLoadedBitset;
    private static final long CHUNK_LOADED_MASK_RAD0 = NewChunkHolder.getLoadedMask(0);
    private static final long CHUNK_LOADED_MASK_RAD1 = NewChunkHolder.getLoadedMask(1);
    private static final long CHUNK_LOADED_MASK_RAD2 = NewChunkHolder.getLoadedMask(2);
    private FullChunkStatus pendingFullChunkStatus = FullChunkStatus.INACCESSIBLE;
    private FullChunkStatus currentFullChunkStatus = FullChunkStatus.INACCESSIBLE;
    private boolean processingFullStatus = false;
    private final Reference2ObjectOpenHashMap<ChunkStatus, List<Consumer<ChunkAccess>>> statusWaiters = new Reference2ObjectOpenHashMap();
    private final Reference2ObjectOpenHashMap<FullChunkStatus, List<Consumer<LevelChunk>>> fullStatusWaiters = new Reference2ObjectOpenHashMap();
    public long lastAutoSave;
    private static final MoonriseRegionFileIO.RegionFileType[] REGION_FILE_TYPES = MoonriseRegionFileIO.RegionFileType.values();
    public static final ThreadLocal<Boolean> DO_NOT_COPY_SAVED_DATA = ThreadLocal.withInitial(() -> Boolean.FALSE);
    private boolean lastEntitySaveNull;
    private boolean lastPoiSaveNull;

    /*
     * WARNING - Removed try catching itself - possible behaviour change.
     */
    ChunkEntitySlices loadInEntityChunk(boolean transientChunk) {
        CompoundTag entityChunk;
        ChunkEntitySlices ret;
        TickThread.ensureTickThread((Level)this.world, (int)this.chunkX, (int)this.chunkZ, (String)"Cannot sync load entity data off-main");
        ReentrantAreaLock.Node schedulingLock = this.scheduler.schedulingLockArea.lock(this.chunkX, this.chunkZ);
        try {
            if (this.entityChunk != null && (transientChunk || !this.entityChunk.isTransient())) {
                ChunkEntitySlices chunkEntitySlices = this.entityChunk;
                return chunkEntitySlices;
            }
            CompoundTag pendingEntityChunk = this.pendingEntityChunk;
            if (!transientChunk && pendingEntityChunk == null) {
                throw new IllegalStateException("Must load entity data from disk before loading in the entity chunk!");
            }
            if (this.entityChunk == null) {
                ret = this.entityChunk = new ChunkEntitySlices(this.world, this.chunkX, this.chunkZ, this.getChunkStatus(), this.holderData, WorldUtil.getMinSection((Level)this.world), WorldUtil.getMaxSection((Level)this.world));
                ret.setTransient(transientChunk);
                this.world.moonrise$getEntityLookup().entitySectionLoad(this.chunkX, this.chunkZ, ret);
            } else {
                ret = this.entityChunk;
                this.entityChunk.setTransient(false);
            }
            if (!transientChunk) {
                this.pendingEntityChunk = null;
                entityChunk = pendingEntityChunk == EMPTY_ENTITY_CHUNK ? null : pendingEntityChunk;
            } else {
                entityChunk = null;
            }
        }
        finally {
            this.scheduler.schedulingLockArea.unlock(schedulingLock);
        }
        if (!transientChunk && entityChunk != null) {
            ChunkPos pos = new ChunkPos(this.chunkX, this.chunkZ);
            List<Entity> entities = ChunkEntitySlices.readEntities(this.world, pos, entityChunk);
            this.world.moonrise$getEntityLookup().addEntityChunkEntities(entities, pos);
        }
        return ret;
    }

    public ChunkLoadTask.EntityDataLoadTask getEntityDataLoadTask() {
        return this.entityDataLoadTask;
    }

    public boolean isEntityChunkNBTLoaded() {
        return this.entityChunk != null && !this.entityChunk.isTransient() || this.pendingEntityChunk != null;
    }

    /*
     * WARNING - Removed try catching itself - possible behaviour change.
     */
    private void completeEntityLoad(GenericDataLoadTask.TaskResult<CompoundTag, Throwable> result) {
        List<GenericDataLoadTaskCallback> completeWaiters;
        GenericDataLoadTask entityDataLoadTask = null;
        boolean scheduleEntityTask = false;
        ReentrantAreaLock.Node schedulingLock = this.scheduler.schedulingLockArea.lock(this.chunkX, this.chunkZ);
        try {
            List<GenericDataLoadTaskCallback> waiters = this.entityDataLoadTaskWaiters;
            this.entityDataLoadTask = null;
            if (result != null) {
                this.entityDataLoadTaskWaiters = null;
                CompoundTag compoundTag = this.pendingEntityChunk = result.left() == null ? EMPTY_ENTITY_CHUNK : result.left();
                if (result.right() != null) {
                    LOGGER.error("Unhandled entity data load exception, data data will be lost: ", result.right());
                }
                for (GenericDataLoadTaskCallback callback : waiters) {
                    callback.markCompleted();
                }
                completeWaiters = waiters;
            } else {
                completeWaiters = null;
                if (waiters.isEmpty()) {
                    this.entityDataLoadTaskWaiters = null;
                } else {
                    this.entityDataLoadTask = new ChunkLoadTask.EntityDataLoadTask(this.scheduler, this.world, this.chunkX, this.chunkZ, this.getEffectivePriority(Priority.NORMAL));
                    entityDataLoadTask = this.entityDataLoadTask;
                    ((ChunkLoadTask.EntityDataLoadTask)entityDataLoadTask).addCallback(this::completeEntityLoad);
                    for (GenericDataLoadTaskCallback callback : waiters) {
                        scheduleEntityTask |= entityDataLoadTask.schedule(true);
                    }
                }
            }
        }
        finally {
            this.scheduler.schedulingLockArea.unlock(schedulingLock);
        }
        if (scheduleEntityTask) {
            entityDataLoadTask.scheduleNow();
        }
        if (completeWaiters != null) {
            for (GenericDataLoadTaskCallback callback : completeWaiters) {
                callback.acceptCompleted(result);
            }
        }
        schedulingLock = this.scheduler.schedulingLockArea.lock(this.chunkX, this.chunkZ);
        try {
            this.checkUnload();
        }
        finally {
            this.scheduler.schedulingLockArea.unlock(schedulingLock);
        }
    }

    public GenericDataLoadTaskCallback getOrLoadEntityData(Consumer<GenericDataLoadTask.TaskResult<CompoundTag, Throwable>> consumer) {
        if (this.isEntityChunkNBTLoaded()) {
            throw new IllegalStateException("Cannot load entity data, it is already loaded");
        }
        if (!this.scheduler.schedulingLockArea.isHeldByCurrentThread(this.chunkX, this.chunkZ)) {
            throw new IllegalStateException("Must hold scheduling lock");
        }
        EntityDataLoadTaskCallback ret = new EntityDataLoadTaskCallback(consumer, this);
        if (this.entityDataLoadTask == null) {
            this.entityDataLoadTask = new ChunkLoadTask.EntityDataLoadTask(this.scheduler, this.world, this.chunkX, this.chunkZ, this.getEffectivePriority(Priority.NORMAL));
            this.entityDataLoadTask.addCallback(this::completeEntityLoad);
            this.entityDataLoadTaskWaiters = new ArrayList<GenericDataLoadTaskCallback>();
        }
        this.entityDataLoadTaskWaiters.add(ret);
        if (this.entityDataLoadTask.schedule(true)) {
            ret.schedule = this.entityDataLoadTask;
        }
        this.checkUnload();
        return ret;
    }

    public ChunkLoadTask.PoiDataLoadTask getPoiDataLoadTask() {
        return this.poiDataLoadTask;
    }

    public boolean isPoiChunkLoaded() {
        return this.poiChunk != null;
    }

    /*
     * WARNING - Removed try catching itself - possible behaviour change.
     */
    private void completePoiLoad(GenericDataLoadTask.TaskResult<PoiChunk, Throwable> result) {
        List<GenericDataLoadTaskCallback> completeWaiters;
        GenericDataLoadTask poiDataLoadTask = null;
        boolean schedulePoiTask = false;
        ReentrantAreaLock.Node schedulingLock = this.scheduler.schedulingLockArea.lock(this.chunkX, this.chunkZ);
        try {
            List<GenericDataLoadTaskCallback> waiters = this.poiDataLoadTaskWaiters;
            this.poiDataLoadTask = null;
            if (result != null) {
                this.poiDataLoadTaskWaiters = null;
                this.poiChunk = result.left();
                if (result.right() != null) {
                    LOGGER.error("Unhandled poi load exception, poi data will be lost: ", result.right());
                }
                for (GenericDataLoadTaskCallback callback : waiters) {
                    callback.markCompleted();
                }
                completeWaiters = waiters;
            } else {
                completeWaiters = null;
                if (waiters.isEmpty()) {
                    this.poiDataLoadTaskWaiters = null;
                } else {
                    this.poiDataLoadTask = new ChunkLoadTask.PoiDataLoadTask(this.scheduler, this.world, this.chunkX, this.chunkZ, this.getEffectivePriority(Priority.NORMAL));
                    poiDataLoadTask = this.poiDataLoadTask;
                    ((ChunkLoadTask.PoiDataLoadTask)poiDataLoadTask).addCallback(this::completePoiLoad);
                    for (GenericDataLoadTaskCallback callback : waiters) {
                        schedulePoiTask |= poiDataLoadTask.schedule(true);
                    }
                }
            }
        }
        finally {
            this.scheduler.schedulingLockArea.unlock(schedulingLock);
        }
        if (schedulePoiTask) {
            poiDataLoadTask.scheduleNow();
        }
        if (completeWaiters != null) {
            for (GenericDataLoadTaskCallback callback : completeWaiters) {
                callback.acceptCompleted(result);
            }
        }
        schedulingLock = this.scheduler.schedulingLockArea.lock(this.chunkX, this.chunkZ);
        try {
            this.checkUnload();
        }
        finally {
            this.scheduler.schedulingLockArea.unlock(schedulingLock);
        }
    }

    public GenericDataLoadTaskCallback getOrLoadPoiData(Consumer<GenericDataLoadTask.TaskResult<PoiChunk, Throwable>> consumer) {
        if (this.isPoiChunkLoaded()) {
            throw new IllegalStateException("Cannot load poi data, it is already loaded");
        }
        if (!this.scheduler.schedulingLockArea.isHeldByCurrentThread(this.chunkX, this.chunkZ)) {
            throw new IllegalStateException("Must hold scheduling lock");
        }
        PoiDataLoadTaskCallback ret = new PoiDataLoadTaskCallback(consumer, this);
        if (this.poiDataLoadTask == null) {
            this.poiDataLoadTask = new ChunkLoadTask.PoiDataLoadTask(this.scheduler, this.world, this.chunkX, this.chunkZ, this.getEffectivePriority(Priority.NORMAL));
            this.poiDataLoadTask.addCallback(this::completePoiLoad);
            this.poiDataLoadTaskWaiters = new ArrayList<GenericDataLoadTaskCallback>();
        }
        this.poiDataLoadTaskWaiters.add(ret);
        if (this.poiDataLoadTask.schedule(true)) {
            ret.schedule = this.poiDataLoadTask;
        }
        this.checkUnload();
        return ret;
    }

    public ChunkCompletion getLastChunkCompletion() {
        return this.lastChunkCompletion;
    }

    public ChunkAccess getChunkIfPresentUnchecked(ChunkStatus status) {
        ChunkCompletion completion = CHUNK_COMPLETION_ARRAY_HANDLE.getVolatile(this.chunkCompletions, status.getIndex());
        return completion == null ? null : completion.chunk;
    }

    public ChunkAccess getChunkIfPresent(ChunkStatus status) {
        ChunkStatus maxStatus = ChunkLevel.generationStatus(this.getTicketLevel());
        if (maxStatus == null || status.isAfter(maxStatus)) {
            return null;
        }
        return this.getChunkIfPresentUnchecked(status);
    }

    public void replaceProtoChunk(ImposterProtoChunk imposterProtoChunk) {
        int max = ChunkStatus.FULL.getIndex();
        for (int i = 0; i < max; ++i) {
            CHUNK_COMPLETION_ARRAY_HANDLE.setVolatile(this.chunkCompletions, i, new ChunkCompletion((ChunkAccess)imposterProtoChunk, ALL_STATUSES[i]));
        }
    }

    public void addGenerationBlockingNeighbour(NewChunkHolder neighbour) {
        this.neighboursBlockingGenTask.add((Object)neighbour);
    }

    public void addWaitingNeighbour(NewChunkHolder neighbour, ChunkStatus requiredStatus) {
        boolean wasEmpty = this.neighboursWaitingForUs.isEmpty();
        this.neighboursWaitingForUs.put((Object)neighbour, (Object)requiredStatus);
        if (wasEmpty) {
            this.checkUnload();
        }
    }

    public Priority getEffectivePriority(Priority dfl) {
        Priority neighbour = this.neighbourRequestedPriority;
        Priority us = this.priority;
        if (neighbour == null) {
            return us == null ? dfl : us;
        }
        if (us == null) {
            return neighbour;
        }
        return Priority.max((Priority)us, (Priority)neighbour);
    }

    private void recalculateNeighbourRequestedPriority() {
        if (this.neighboursWaitingForUs.isEmpty()) {
            this.neighbourRequestedPriority = null;
            return;
        }
        Priority max = null;
        for (NewChunkHolder holder : this.neighboursWaitingForUs.keySet()) {
            Priority neighbourPriority = holder.getEffectivePriority(null);
            if (neighbourPriority == null || max != null && !neighbourPriority.isHigherPriority(max)) continue;
            max = neighbourPriority;
        }
        Priority current = this.getEffectivePriority(Priority.NORMAL);
        this.neighbourRequestedPriority = max;
        Priority next = this.getEffectivePriority(Priority.NORMAL);
        if (current == next) {
            return;
        }
        if (this.generationTask != null) {
            this.generationTask.setPriority(next);
        }
        this.recalculateNeighbourPriorities();
    }

    public void recalculateNeighbourPriorities() {
        for (NewChunkHolder holder : this.neighboursBlockingGenTask) {
            holder.recalculateNeighbourRequestedPriority();
        }
    }

    public void raisePriority(Priority priority) {
        if (this.priority != null && this.priority.isHigherOrEqualPriority(priority)) {
            return;
        }
        this.setPriority(priority);
    }

    private void lockPriority() {
        this.priority = null;
        this.priorityLocked = true;
    }

    public void setPriority(Priority priority) {
        if (this.priorityLocked) {
            return;
        }
        Priority old = this.getEffectivePriority(null);
        this.priority = priority;
        Priority newPriority = this.getEffectivePriority(Priority.NORMAL);
        if (old != newPriority && this.generationTask != null) {
            this.generationTask.setPriority(newPriority);
        }
        this.recalculateNeighbourPriorities();
    }

    public void lowerPriority(Priority priority) {
        if (this.priority != null && this.priority.isLowerOrEqualPriority(priority)) {
            return;
        }
        this.setPriority(priority);
    }

    public void failedLightUpdate() {
        this.failedLightUpdate = true;
    }

    public boolean hasFailedGeneration() {
        return this.genTaskException != null;
    }

    public int getTicketLevel() {
        return this.currentTicketLevel;
    }

    public NewChunkHolder(ServerLevel world, int chunkX, int chunkZ, ChunkTaskScheduler scheduler) {
        this.world = world;
        this.chunkX = chunkX;
        this.chunkZ = chunkZ;
        this.scheduler = scheduler;
        this.vanillaChunkHolder = new ChunkHolder(new ChunkPos(chunkX, chunkZ), ChunkHolderManager.MAX_TICKET_LEVEL, (LevelHeightAccessor)world, world.getLightEngine(), null, world.getChunkSource().chunkMap);
        this.vanillaChunkHolder.moonrise$setRealChunkHolder(this);
        this.holderData = this.world.moonrise$requestChunkData(CoordinateUtils.getChunkKey((int)chunkX, (int)chunkZ));
    }

    public ChunkAccess getCurrentChunk() {
        return this.currentChunk;
    }

    int getCurrentTicketLevel() {
        return this.currentTicketLevel;
    }

    void updateTicketLevel(int toLevel) {
        this.currentTicketLevel = toLevel;
    }

    public void addNeighbourUsingChunk() {
        int now;
        if ((now = ++this.totalNeighboursUsingThisChunk) == 1) {
            this.checkUnload();
        }
    }

    public void removeNeighbourUsingChunk() {
        int now;
        if ((now = --this.totalNeighboursUsingThisChunk) == 0) {
            this.checkUnload();
        }
        if (now < 0) {
            throw new IllegalStateException("Neighbours using this chunk cannot be negative");
        }
    }

    public final String isSafeToUnload() {
        if (this.oldTicketLevel <= ChunkHolderManager.MAX_TICKET_LEVEL) {
            return "ticket_level";
        }
        if (this.totalNeighboursUsingThisChunk != 0) {
            return "neighbours_generating";
        }
        if (!this.neighboursWaitingForUs.isEmpty()) {
            return "neighbours_waiting";
        }
        if (this.getChunkStatus() != FullChunkStatus.INACCESSIBLE) {
            return "fullchunkstatus";
        }
        if (this.generationTask != null) {
            return "generating";
        }
        if (this.requestedGenStatus != null) {
            return "requested_generation";
        }
        if (this.entityDataLoadTask != null) {
            return "entity_data_requested";
        }
        if (this.poiDataLoadTask != null) {
            return "poi_data_requested";
        }
        if (this.entityDataUnload != null) {
            return "entity_serialization";
        }
        if (this.poiDataUnload != null) {
            return "poi_serialization";
        }
        if (this.chunkDataUnload != null) {
            return "chunk_serialization";
        }
        return null;
    }

    void onUnload() {
        this.unloaded = true;
        this.world.moonrise$releaseChunkData(CoordinateUtils.getChunkKey((int)this.chunkX, (int)this.chunkZ));
    }

    void removeFromUnloadQueue() {
        this.inUnloadQueue = false;
    }

    private void checkUnload() {
        if (this.unloaded) {
            return;
        }
        if (this.isSafeToUnload() == null) {
            if (!this.inUnloadQueue) {
                this.inUnloadQueue = true;
                this.scheduler.chunkHolderManager.unloadQueue.addChunk(this.chunkX, this.chunkZ);
            }
        } else if (this.inUnloadQueue) {
            this.inUnloadQueue = false;
            this.scheduler.chunkHolderManager.unloadQueue.removeChunk(this.chunkX, this.chunkZ);
        }
    }

    public UnloadTask getUnloadTask(MoonriseRegionFileIO.RegionFileType type) {
        switch (type) {
            case CHUNK_DATA: {
                return this.chunkDataUnload;
            }
            case ENTITY_DATA: {
                return this.entityDataUnload;
            }
            case POI_DATA: {
                return this.poiDataUnload;
            }
        }
        throw new IllegalStateException("Unknown regionfile type " + String.valueOf((Object)type));
    }

    private void asyncCompleteUnloadTask(MoonriseRegionFileIO.RegionFileType type, Completable<CompoundTag> writeCompletable) {
        UnloadTask task = this.getUnloadTask(type);
        if (writeCompletable == null) {
            task.completable().complete(null);
        } else {
            writeCompletable.whenComplete((write, thr) -> task.completable().complete(write));
        }
    }

    UnloadState unloadStage1() {
        ChunkAccess chunk = this.currentChunk;
        ChunkEntitySlices entityChunk = this.entityChunk;
        PoiChunk poiChunk = this.poiChunk;
        this.currentChunk = null;
        this.currentGenStatus = null;
        for (int i = 0; i < this.chunkCompletions.length; ++i) {
            CHUNK_COMPLETION_ARRAY_HANDLE.setRelease(this.chunkCompletions, i, null);
        }
        this.lastChunkCompletion = null;
        this.entityChunk = null;
        this.pendingEntityChunk = null;
        this.poiChunk = null;
        this.priorityLocked = false;
        if (chunk != null) {
            LazyRunnable toRun = new LazyRunnable();
            this.chunkDataUnload = new UnloadTask((CallbackCompletable<CompoundTag>)new CallbackCompletable(), this.scheduler.saveExecutor.createTask((Runnable)toRun), toRun);
        }
        if (poiChunk != null) {
            this.poiDataUnload = new UnloadTask((CallbackCompletable<CompoundTag>)new CallbackCompletable(), null, null);
        }
        if (entityChunk != null) {
            this.entityDataUnload = new UnloadTask((CallbackCompletable<CompoundTag>)new CallbackCompletable(), null, null);
        }
        this.unloadState = chunk != null || entityChunk != null || poiChunk != null ? new UnloadState(this, chunk, entityChunk, poiChunk) : null;
        return this.unloadState;
    }

    /*
     * WARNING - Removed try catching itself - possible behaviour change.
     */
    void unloadStage2(UnloadState state) {
        this.unloadState = null;
        ChunkAccess chunk = state.chunk();
        ChunkEntitySlices entityChunk = state.entityChunk();
        PoiChunk poiChunk = state.poiChunk();
        boolean shouldLevelChunkNotSave = PlatformHooks.get().forceNoSave(chunk);
        if (chunk != null) {
            if (chunk instanceof LevelChunk) {
                LevelChunk levelChunk = (LevelChunk)chunk;
                levelChunk.setLoaded(false);
                PlatformHooks.get().chunkUnloadFromWorld(levelChunk);
            }
            Completable[] chunkWrite = new Completable[1];
            if (!shouldLevelChunkNotSave) {
                this.saveChunk(chunk, true, chunkWrite);
            }
            if (chunk instanceof LevelChunk) {
                LevelChunk levelChunk = (LevelChunk)chunk;
                this.world.unload(levelChunk);
            }
            this.asyncCompleteUnloadTask(MoonriseRegionFileIO.RegionFileType.CHUNK_DATA, (Completable<CompoundTag>)chunkWrite[0]);
        }
        if (entityChunk != null) {
            Completable[] entityWrite = new Completable[1];
            this.saveEntities(entityChunk, true, entityWrite);
            if (entityChunk.unload()) {
                ReentrantAreaLock.Node schedulingLock = this.scheduler.schedulingLockArea.lock(this.chunkX, this.chunkZ);
                try {
                    entityChunk.setTransient(true);
                    this.entityChunk = entityChunk;
                }
                finally {
                    this.scheduler.schedulingLockArea.unlock(schedulingLock);
                }
            } else {
                this.world.moonrise$getEntityLookup().entitySectionUnload(this.chunkX, this.chunkZ);
            }
            this.asyncCompleteUnloadTask(MoonriseRegionFileIO.RegionFileType.ENTITY_DATA, (Completable<CompoundTag>)entityWrite[0]);
        }
        if (poiChunk != null) {
            Completable[] poiWrite = new Completable[1];
            if (poiChunk.isDirty() && !shouldLevelChunkNotSave) {
                this.savePOI(poiChunk, true, poiWrite);
            }
            if (poiChunk.isLoaded()) {
                this.world.getPoiManager().moonrise$onUnload(CoordinateUtils.getChunkKey((int)this.chunkX, (int)this.chunkZ));
            }
            this.asyncCompleteUnloadTask(MoonriseRegionFileIO.RegionFileType.POI_DATA, (Completable<CompoundTag>)poiWrite[0]);
        }
    }

    boolean unloadStage3() {
        this.chunkDataUnload = null;
        this.poiDataUnload = null;
        this.entityDataUnload = null;
        if (this.entityChunk != null || this.poiChunk != null || this.currentChunk != null) {
            return false;
        }
        return this.isSafeToUnload() == null;
    }

    private void cancelGenTask() {
        if (this.generationTask != null) {
            this.generationTask.cancel();
        } else if (!this.neighboursBlockingGenTask.isEmpty()) {
            for (NewChunkHolder neighbour : this.neighboursBlockingGenTask) {
                if (neighbour.neighboursWaitingForUs.remove((Object)this) == null) {
                    throw new IllegalStateException("Corrupt state");
                }
                if (!neighbour.neighboursWaitingForUs.isEmpty()) continue;
                neighbour.checkUnload();
            }
            this.neighboursBlockingGenTask.clear();
            this.checkUnload();
        }
    }

    public void processTicketLevelUpdate(List<ChunkProgressionTask> scheduledTasks, List<NewChunkHolder> changedLoadStatus) {
        int oldLevel = this.oldTicketLevel;
        int newLevel = this.currentTicketLevel;
        if (oldLevel == newLevel) {
            return;
        }
        this.oldTicketLevel = newLevel;
        FullChunkStatus oldState = ChunkLevel.fullStatus(oldLevel);
        FullChunkStatus newState = ChunkLevel.fullStatus(newLevel);
        boolean oldUnloaded = oldLevel > ChunkHolderManager.MAX_TICKET_LEVEL;
        boolean newUnloaded = newLevel > ChunkHolderManager.MAX_TICKET_LEVEL;
        ChunkStatus maxGenerationStatusOld = ChunkLevel.generationStatus(oldLevel);
        ChunkStatus maxGenerationStatusNew = ChunkLevel.generationStatus(newLevel);
        if (this.requestedGenStatus != null && !newState.isOrAfter(FullChunkStatus.FULL) && newLevel > oldLevel) {
            if (newUnloaded) {
                this.requestedGenStatus = null;
                this.cancelGenTask();
            } else {
                ChunkStatus currentRequestedStatus = this.requestedGenStatus;
                ChunkStatus toCancel = maxGenerationStatusNew.moonrise$getNextStatus();
                if (currentRequestedStatus.isOrAfter(toCancel)) {
                    if (this.currentGenStatus != null && this.currentGenStatus.isOrAfter(maxGenerationStatusNew)) {
                        this.requestedGenStatus = null;
                        this.cancelGenTask();
                    } else {
                        this.requestedGenStatus = maxGenerationStatusNew;
                        if (this.generationTaskStatus != null && this.generationTaskStatus.isOrAfter(toCancel)) {
                            throw new IllegalStateException("?????");
                        }
                    }
                }
            }
        }
        if (oldState != newState) {
            if (newState.isOrAfter(oldState)) {
                if (!oldState.isOrAfter(FullChunkStatus.FULL) && newState.isOrAfter(FullChunkStatus.FULL) && this.currentGenStatus != ChunkStatus.FULL) {
                    if (this.requestedGenStatus != null) {
                        this.requestedGenStatus = ChunkStatus.FULL;
                    } else {
                        this.scheduler.schedule(this.chunkX, this.chunkZ, ChunkStatus.FULL, this, scheduledTasks);
                    }
                }
            } else {
                if (!newState.isOrAfter(FullChunkStatus.ENTITY_TICKING) && oldState.isOrAfter(FullChunkStatus.ENTITY_TICKING)) {
                    this.completeFullStatusConsumers(FullChunkStatus.ENTITY_TICKING, null);
                }
                if (!newState.isOrAfter(FullChunkStatus.BLOCK_TICKING) && oldState.isOrAfter(FullChunkStatus.BLOCK_TICKING)) {
                    this.completeFullStatusConsumers(FullChunkStatus.BLOCK_TICKING, null);
                }
                if (!newState.isOrAfter(FullChunkStatus.FULL) && oldState.isOrAfter(FullChunkStatus.FULL)) {
                    this.completeFullStatusConsumers(FullChunkStatus.FULL, null);
                }
            }
            if (this.updatePendingStatus()) {
                changedLoadStatus.add(this);
            }
        }
        if (oldUnloaded != newUnloaded) {
            this.checkUnload();
        }
        PlatformHooks.get().onChunkHolderTicketChange(this.world, this.vanillaChunkHolder, oldLevel, newLevel);
    }

    private static int getFullNeighbourIndex(int relativeX, int relativeZ) {
        return relativeX + relativeZ * 5 + 12;
    }

    public final boolean isNeighbourFullLoaded(int relativeX, int relativeZ) {
        return (this.fullNeighbourChunksLoadedBitset & 1L << NewChunkHolder.getFullNeighbourIndex(relativeX, relativeZ)) != 0L;
    }

    public final boolean setNeighbourFullLoaded(int relativeX, int relativeZ) {
        int index = NewChunkHolder.getFullNeighbourIndex(relativeX, relativeZ);
        this.fullNeighbourChunksLoadedBitset |= 1L << index;
        return this.updatePendingStatus();
    }

    public final boolean setNeighbourFullUnloaded(int relativeX, int relativeZ) {
        int index = NewChunkHolder.getFullNeighbourIndex(relativeX, relativeZ);
        this.fullNeighbourChunksLoadedBitset &= 1L << index ^ 0xFFFFFFFFFFFFFFFFL;
        return this.updatePendingStatus();
    }

    private static long getLoadedMask(int radius) {
        long mask = 0L;
        for (int dx = -radius; dx <= radius; ++dx) {
            for (int dz = -radius; dz <= radius; ++dz) {
                mask |= 1L << NewChunkHolder.getFullNeighbourIndex(dx, dz);
            }
        }
        return mask;
    }

    public FullChunkStatus getChunkStatus() {
        return this.currentFullChunkStatus;
    }

    public boolean isEntityTickingReady() {
        return this.getChunkStatus().isOrAfter(FullChunkStatus.ENTITY_TICKING);
    }

    public boolean isTickingReady() {
        return this.getChunkStatus().isOrAfter(FullChunkStatus.BLOCK_TICKING);
    }

    public boolean isFullChunkReady() {
        return this.getChunkStatus().isOrAfter(FullChunkStatus.FULL);
    }

    private static FullChunkStatus getStatusForBitset(long bitset) {
        if ((bitset & CHUNK_LOADED_MASK_RAD2) == CHUNK_LOADED_MASK_RAD2) {
            return FullChunkStatus.ENTITY_TICKING;
        }
        if ((bitset & CHUNK_LOADED_MASK_RAD1) == CHUNK_LOADED_MASK_RAD1) {
            return FullChunkStatus.BLOCK_TICKING;
        }
        if ((bitset & CHUNK_LOADED_MASK_RAD0) == CHUNK_LOADED_MASK_RAD0) {
            return FullChunkStatus.FULL;
        }
        return FullChunkStatus.INACCESSIBLE;
    }

    private boolean updatePendingStatus() {
        FullChunkStatus byTicketLevel = ChunkLevel.fullStatus(this.oldTicketLevel);
        FullChunkStatus pending = NewChunkHolder.getStatusForBitset(this.fullNeighbourChunksLoadedBitset);
        if (pending == FullChunkStatus.INACCESSIBLE && byTicketLevel.isOrAfter(FullChunkStatus.FULL) && this.currentGenStatus == ChunkStatus.FULL) {
            pending = FullChunkStatus.FULL;
        }
        if (pending.isOrAfter(byTicketLevel)) {
            pending = byTicketLevel;
        }
        if (this.pendingFullChunkStatus == pending) {
            return false;
        }
        this.pendingFullChunkStatus = pending;
        return true;
    }

    /*
     * WARNING - Removed try catching itself - possible behaviour change.
     */
    private void onFullChunkLoadChange(boolean loaded, List<NewChunkHolder> changedFullStatus) {
        ReentrantAreaLock.Node schedulingLock = this.scheduler.schedulingLockArea.lock(this.chunkX, this.chunkZ, 2);
        try {
            for (int dz = -2; dz <= 2; ++dz) {
                for (int dx = -2; dx <= 2; ++dx) {
                    NewChunkHolder holder;
                    NewChunkHolder newChunkHolder = holder = (dx | dz) == 0 ? this : this.scheduler.chunkHolderManager.getChunkHolder(dx + this.chunkX, dz + this.chunkZ);
                    if (loaded) {
                        if (!holder.setNeighbourFullLoaded(-dx, -dz)) continue;
                        changedFullStatus.add(holder);
                        continue;
                    }
                    if (holder == null || !holder.setNeighbourFullUnloaded(-dx, -dz)) continue;
                    changedFullStatus.add(holder);
                }
            }
        }
        finally {
            this.scheduler.schedulingLockArea.unlock(schedulingLock);
        }
    }

    private void changeEntityChunkStatus(FullChunkStatus toStatus) {
        this.world.moonrise$getEntityLookup().chunkStatusChange(this.chunkX, this.chunkZ, toStatus);
    }

    private void updateCurrentState(FullChunkStatus to) {
        this.currentFullChunkStatus = to;
    }

    /*
     * WARNING - Removed try catching itself - possible behaviour change.
     */
    public boolean handleFullStatusChange(List<NewChunkHolder> changedFullStatus) {
        TickThread.ensureTickThread((Level)this.world, (int)this.chunkX, (int)this.chunkZ, (String)"Cannot update full status thread off-main");
        boolean ret = false;
        if (this.processingFullStatus) {
            return ret;
        }
        this.processingFullStatus = true;
        try {
            while (true) {
                FullChunkStatus current;
                FullChunkStatus pending;
                if ((pending = this.pendingFullChunkStatus) == (current = this.currentFullChunkStatus)) {
                    if (pending == FullChunkStatus.INACCESSIBLE) {
                        ReentrantAreaLock.Node schedulingLock = this.scheduler.schedulingLockArea.lock(this.chunkX, this.chunkZ);
                        try {
                            this.checkUnload();
                        }
                        finally {
                            this.scheduler.schedulingLockArea.unlock(schedulingLock);
                        }
                    }
                    boolean schedulingLock = ret;
                    return schedulingLock;
                }
                ret = true;
                LevelChunk chunk = (LevelChunk)this.currentChunk;
                if (pending.isOrAfter(current)) {
                    if (!current.isOrAfter(FullChunkStatus.FULL) && pending.isOrAfter(FullChunkStatus.FULL)) {
                        this.updateCurrentState(FullChunkStatus.FULL);
                        PlatformHooks.get().onChunkPreBorder(chunk, this.vanillaChunkHolder);
                        this.scheduler.chunkHolderManager.ensureInAutosave(this);
                        this.changeEntityChunkStatus(FullChunkStatus.FULL);
                        PlatformHooks.get().onChunkBorder(chunk, this.vanillaChunkHolder);
                        this.onFullChunkLoadChange(true, changedFullStatus);
                        this.completeFullStatusConsumers(FullChunkStatus.FULL, chunk);
                    }
                    if (!current.isOrAfter(FullChunkStatus.BLOCK_TICKING) && pending.isOrAfter(FullChunkStatus.BLOCK_TICKING)) {
                        this.updateCurrentState(FullChunkStatus.BLOCK_TICKING);
                        this.changeEntityChunkStatus(FullChunkStatus.BLOCK_TICKING);
                        PlatformHooks.get().onChunkTicking(chunk, this.vanillaChunkHolder);
                        this.completeFullStatusConsumers(FullChunkStatus.BLOCK_TICKING, chunk);
                    }
                    if (current.isOrAfter(FullChunkStatus.ENTITY_TICKING) || !pending.isOrAfter(FullChunkStatus.ENTITY_TICKING)) continue;
                    this.updateCurrentState(FullChunkStatus.ENTITY_TICKING);
                    this.changeEntityChunkStatus(FullChunkStatus.ENTITY_TICKING);
                    PlatformHooks.get().onChunkEntityTicking(chunk, this.vanillaChunkHolder);
                    this.completeFullStatusConsumers(FullChunkStatus.ENTITY_TICKING, chunk);
                    continue;
                }
                if (current.isOrAfter(FullChunkStatus.ENTITY_TICKING) && !pending.isOrAfter(FullChunkStatus.ENTITY_TICKING)) {
                    this.changeEntityChunkStatus(FullChunkStatus.BLOCK_TICKING);
                    PlatformHooks.get().onChunkNotEntityTicking(chunk, this.vanillaChunkHolder);
                    this.updateCurrentState(FullChunkStatus.BLOCK_TICKING);
                }
                if (current.isOrAfter(FullChunkStatus.BLOCK_TICKING) && !pending.isOrAfter(FullChunkStatus.BLOCK_TICKING)) {
                    this.changeEntityChunkStatus(FullChunkStatus.FULL);
                    PlatformHooks.get().onChunkNotTicking(chunk, this.vanillaChunkHolder);
                    this.updateCurrentState(FullChunkStatus.FULL);
                }
                if (!current.isOrAfter(FullChunkStatus.FULL) || pending.isOrAfter(FullChunkStatus.FULL)) continue;
                this.onFullChunkLoadChange(false, changedFullStatus);
                this.changeEntityChunkStatus(FullChunkStatus.INACCESSIBLE);
                PlatformHooks.get().onChunkNotBorder(chunk, this.vanillaChunkHolder);
                PlatformHooks.get().onChunkPostNotBorder(chunk, this.vanillaChunkHolder);
                this.updateCurrentState(FullChunkStatus.INACCESSIBLE);
            }
        }
        finally {
            this.processingFullStatus = false;
        }
    }

    boolean upgradeGenTarget(ChunkStatus toStatus) {
        if (toStatus == null) {
            throw new NullPointerException("toStatus cannot be null");
        }
        if (this.requestedGenStatus == null && this.generationTask == null) {
            return false;
        }
        if (this.requestedGenStatus == null || !this.requestedGenStatus.isOrAfter(toStatus)) {
            this.requestedGenStatus = toStatus;
        }
        return true;
    }

    public void setGenerationTarget(ChunkStatus toStatus) {
        this.requestedGenStatus = toStatus;
    }

    public boolean hasGenerationTask() {
        return this.generationTask != null;
    }

    public ChunkStatus getCurrentGenStatus() {
        return this.currentGenStatus;
    }

    public ChunkStatus getRequestedGenStatus() {
        return this.requestedGenStatus;
    }

    void addStatusConsumer(ChunkStatus status, Consumer<ChunkAccess> consumer) {
        ((List)this.statusWaiters.computeIfAbsent((Object)status, keyInMap -> new ArrayList(4))).add(consumer);
    }

    private void completeStatusConsumers(ChunkStatus status, ChunkAccess chunk) {
        do {
            this.completeStatusConsumers0(status, chunk);
        } while (chunk == null && status != (status = status.moonrise$getNextStatus()));
    }

    private void completeStatusConsumers0(ChunkStatus status, ChunkAccess chunk) {
        List consumers = (List)this.statusWaiters.remove((Object)status);
        if (consumers == null) {
            return;
        }
        this.scheduler.scheduleChunkTask(this.chunkX, this.chunkZ, () -> {
            for (Consumer consumer : consumers) {
                try {
                    consumer.accept(chunk);
                }
                catch (Throwable thr) {
                    LOGGER.error("Failed to process chunk status callback", thr);
                }
            }
        }, Priority.HIGHEST);
    }

    void addFullStatusConsumer(FullChunkStatus status, Consumer<LevelChunk> consumer) {
        ((List)this.fullStatusWaiters.computeIfAbsent((Object)status, keyInMap -> new ArrayList(4))).add(consumer);
    }

    private void completeFullStatusConsumers(FullChunkStatus status, LevelChunk chunk) {
        List consumers = (List)this.fullStatusWaiters.remove((Object)status);
        if (consumers == null) {
            return;
        }
        this.scheduler.scheduleChunkTask(this.chunkX, this.chunkZ, () -> {
            for (Consumer consumer : consumers) {
                try {
                    consumer.accept(chunk);
                }
                catch (Throwable thr) {
                    LOGGER.error("Failed to process chunk status callback", thr);
                }
            }
        }, Priority.HIGHEST);
    }

    private void onChunkGenComplete(ChunkAccess newChunk, ChunkStatus newStatus, List<ChunkProgressionTask> scheduleList, List<NewChunkHolder> changedLoadStatus) {
        if (!this.neighboursBlockingGenTask.isEmpty()) {
            throw new IllegalStateException("Cannot have neighbours blocking this gen task");
        }
        if (newChunk != null || this.requestedGenStatus == null || !this.requestedGenStatus.isOrAfter(newStatus)) {
            this.completeStatusConsumers(newStatus, newChunk);
        }
        this.generationTask = null;
        this.generationTaskStatus = null;
        if (newChunk == null) {
            ChunkStatus requestedGenStatus = this.requestedGenStatus;
            this.requestedGenStatus = null;
            if (requestedGenStatus != null) {
                if (!this.neighboursWaitingForUs.isEmpty()) {
                    ObjectBidirectionalIterator iterator = this.neighboursWaitingForUs.reference2ObjectEntrySet().fastIterator();
                    while (iterator.hasNext()) {
                        Reference2ObjectMap.Entry entry = (Reference2ObjectMap.Entry)iterator.next();
                        NewChunkHolder chunkHolder = (NewChunkHolder)entry.getKey();
                        ChunkStatus toStatus = (ChunkStatus)entry.getValue();
                        if (requestedGenStatus.isOrAfter(toStatus)) continue;
                        if (!chunkHolder.neighboursBlockingGenTask.remove((Object)this)) {
                            throw new IllegalStateException("Corrupt state");
                        }
                        if (chunkHolder.neighboursBlockingGenTask.isEmpty()) {
                            chunkHolder.checkUnload();
                        }
                        iterator.remove();
                    }
                }
                this.scheduler.schedule(this.chunkX, this.chunkZ, requestedGenStatus, this, scheduleList);
                return;
            }
            if (!this.neighboursWaitingForUs.isEmpty()) {
                for (NewChunkHolder chunkHolder : this.neighboursWaitingForUs.keySet()) {
                    if (!chunkHolder.neighboursBlockingGenTask.remove((Object)this)) {
                        throw new IllegalStateException("Corrupt state");
                    }
                    if (!chunkHolder.neighboursBlockingGenTask.isEmpty()) continue;
                    chunkHolder.checkUnload();
                }
                this.neighboursWaitingForUs.clear();
            }
            this.setPriority(null);
            this.checkUnload();
            return;
        }
        this.currentChunk = newChunk;
        this.currentGenStatus = newStatus;
        ChunkCompletion completion = new ChunkCompletion(newChunk, newStatus);
        CHUNK_COMPLETION_ARRAY_HANDLE.setVolatile(this.chunkCompletions, newStatus.getIndex(), completion);
        this.lastChunkCompletion = completion;
        ChunkStatus requestedGenStatus = this.requestedGenStatus;
        ArrayList<NewChunkHolder> needsScheduling = null;
        boolean recalculatePriority = false;
        ObjectBidirectionalIterator iterator = this.neighboursWaitingForUs.reference2ObjectEntrySet().fastIterator();
        while (iterator.hasNext()) {
            Reference2ObjectMap.Entry entry = (Reference2ObjectMap.Entry)iterator.next();
            NewChunkHolder neighbour = (NewChunkHolder)entry.getKey();
            ChunkStatus requiredStatus = (ChunkStatus)entry.getValue();
            if (!newStatus.isOrAfter(requiredStatus)) {
                if (requestedGenStatus != null && requestedGenStatus.isOrAfter(requiredStatus)) continue;
                if (!neighbour.neighboursBlockingGenTask.remove((Object)this)) {
                    throw new IllegalStateException("Neighbour is not waiting for us?");
                }
                if (neighbour.neighboursBlockingGenTask.isEmpty()) {
                    neighbour.checkUnload();
                }
                iterator.remove();
                continue;
            }
            recalculatePriority = true;
            if (!neighbour.neighboursBlockingGenTask.remove((Object)this)) {
                throw new IllegalStateException("Neighbour is not waiting for us?");
            }
            if (neighbour.neighboursBlockingGenTask.isEmpty()) {
                if (neighbour.requestedGenStatus != null) {
                    if (needsScheduling == null) {
                        needsScheduling = new ArrayList<NewChunkHolder>();
                    }
                    needsScheduling.add(neighbour);
                } else {
                    neighbour.checkUnload();
                }
            }
            iterator.remove();
        }
        if (newStatus == ChunkStatus.FULL) {
            this.lockPriority();
            if (this.updatePendingStatus()) {
                changedLoadStatus.add(this);
            }
        }
        if (recalculatePriority) {
            this.recalculateNeighbourRequestedPriority();
        }
        if (requestedGenStatus != null && !newStatus.isOrAfter(requestedGenStatus)) {
            this.scheduleNeighbours(needsScheduling, scheduleList);
            this.scheduler.schedule(this.chunkX, this.chunkZ, requestedGenStatus, this, scheduleList);
        } else {
            if (requestedGenStatus != null) {
                this.requestedGenStatus = null;
            }
            this.setPriority(null);
            this.checkUnload();
            this.scheduleNeighbours(needsScheduling, scheduleList);
        }
    }

    private void scheduleNeighbours(List<NewChunkHolder> needsScheduling, List<ChunkProgressionTask> scheduleList) {
        if (needsScheduling != null) {
            int len = needsScheduling.size();
            for (int i = 0; i < len; ++i) {
                NewChunkHolder neighbour = needsScheduling.get(i);
                this.scheduler.schedule(neighbour.chunkX, neighbour.chunkZ, neighbour.requestedGenStatus, neighbour, scheduleList);
            }
        }
    }

    public void setGenerationTask(ChunkProgressionTask generationTask, ChunkStatus taskStatus, List<NewChunkHolder> neighbours) {
        if (this.generationTask != null || this.currentGenStatus != null && this.currentGenStatus.isOrAfter(taskStatus)) {
            throw new IllegalStateException("Currently generating or provided task is trying to generate to a level we are already at!");
        }
        if (this.requestedGenStatus == null || !this.requestedGenStatus.isOrAfter(taskStatus)) {
            throw new IllegalStateException("Cannot schedule generation task when not requested");
        }
        this.generationTask = generationTask;
        this.generationTaskStatus = taskStatus;
        int len = neighbours.size();
        for (int i = 0; i < len; ++i) {
            neighbours.get(i).addNeighbourUsingChunk();
        }
        this.checkUnload();
        generationTask.onComplete((access, thr) -> {
            int i;
            int len;
            boolean scheduleTasks;
            if (generationTask != this.generationTask) {
                throw new IllegalStateException("Cannot complete generation task '" + String.valueOf(generationTask) + "' because we are waiting on '" + String.valueOf(this.generationTask) + "' instead!");
            }
            if (thr != null) {
                if (this.genTaskException != null) {
                    LOGGER.warn("Ignoring exception for " + this.toString(), thr);
                    return;
                }
                this.genTaskException = thr;
                this.failedGenStatus = taskStatus;
                this.genTaskFailedThread = Thread.currentThread();
                this.scheduler.unrecoverableChunkSystemFailure(this.chunkX, this.chunkZ, Map.of("Generation task", ChunkTaskScheduler.stringIfNull(generationTask), "Task to status", ChunkTaskScheduler.stringIfNull(taskStatus)), (Throwable)thr);
                return;
            }
            List<ChunkProgressionTask> tasks = ChunkHolderManager.getCurrentTicketUpdateScheduling();
            if (tasks == null) {
                scheduleTasks = true;
                tasks = new ArrayList<ChunkProgressionTask>();
            } else {
                scheduleTasks = false;
            }
            ArrayList<NewChunkHolder> changedLoadStatus = new ArrayList<NewChunkHolder>();
            ReentrantAreaLock.Node schedulingLock = this.scheduler.schedulingLockArea.lock(this.chunkX, this.chunkZ, 2 * ChunkTaskScheduler.getMaxAccessRadius());
            try {
                len = neighbours.size();
                for (i = 0; i < len; ++i) {
                    ((NewChunkHolder)neighbours.get(i)).removeNeighbourUsingChunk();
                }
                this.onChunkGenComplete((ChunkAccess)access, taskStatus, tasks, (List<NewChunkHolder>)changedLoadStatus);
            }
            finally {
                this.scheduler.schedulingLockArea.unlock(schedulingLock);
            }
            this.scheduler.chunkHolderManager.addChangedStatuses(changedLoadStatus);
            if (scheduleTasks) {
                len = tasks.size();
                for (i = 0; i < len; ++i) {
                    tasks.get(i).schedule();
                }
            }
        });
    }

    public PoiChunk getPoiChunk() {
        return this.poiChunk;
    }

    public ChunkEntitySlices getEntityChunk() {
        return this.entityChunk;
    }

    public SaveStat save(boolean shutdown) {
        boolean canSaveEntities;
        boolean forceNoSaveChunk;
        TickThread.ensureTickThread((Level)this.world, (int)this.chunkX, (int)this.chunkZ, (String)"Cannot save data off-main");
        ChunkAccess chunk = this.getCurrentChunk();
        PoiChunk poi = this.getPoiChunk();
        ChunkEntitySlices entities = this.getEntityChunk();
        boolean executedUnloadTask = false;
        boolean[] executedUnloadTasks = new boolean[REGION_FILE_TYPES.length];
        if (shutdown) {
            if (this.unloadState != null) {
                chunk = this.unloadState.chunk();
                poi = this.unloadState.poiChunk();
                entities = this.unloadState.entityChunk();
            }
            for (MoonriseRegionFileIO.RegionFileType regionFileType : REGION_FILE_TYPES) {
                PrioritisedExecutor.PrioritisedTask task;
                UnloadTask unloadTask = this.getUnloadTask(regionFileType);
                if (unloadTask == null || (task = unloadTask.task()) == null || !task.isQueued()) continue;
                boolean executed = task.execute();
                executedUnloadTask |= executed;
                executedUnloadTasks[regionFileType.ordinal()] = executed;
            }
        }
        boolean canSaveChunk = !(forceNoSaveChunk = PlatformHooks.get().forceNoSave(chunk)) && chunk != null && (shutdown || chunk instanceof LevelChunk) && chunk.isUnsaved();
        boolean canSavePOI = !forceNoSaveChunk && poi != null && poi.isDirty();
        boolean bl = canSaveEntities = entities != null;
        if (canSaveChunk) {
            canSaveChunk = this.saveChunk(chunk, false, null);
        }
        if (canSavePOI) {
            canSavePOI = this.savePOI(poi, false, null);
        }
        if (canSaveEntities) {
            canSaveEntities = this.saveEntities(entities, shutdown, null);
        }
        return executedUnloadTask | canSaveChunk | canSaveEntities | canSavePOI ? new SaveStat(canSaveChunk | executedUnloadTasks[MoonriseRegionFileIO.RegionFileType.CHUNK_DATA.ordinal()], canSaveEntities | executedUnloadTasks[MoonriseRegionFileIO.RegionFileType.ENTITY_DATA.ordinal()], canSavePOI | executedUnloadTasks[MoonriseRegionFileIO.RegionFileType.POI_DATA.ordinal()]) : null;
    }

    /*
     * WARNING - Removed try catching itself - possible behaviour change.
     */
    private boolean saveChunk(ChunkAccess chunk, boolean unloading, Completable<CompoundTag>[] chunkSave) {
        if (!chunk.isUnsaved()) {
            return false;
        }
        try {
            PrioritisedExecutor.PrioritisedTask task;
            SerializableChunkData chunkData;
            DO_NOT_COPY_SAVED_DATA.set(unloading);
            try {
                chunkData = SerializableChunkData.copyOf((ServerLevel)this.world, (ChunkAccess)chunk);
            }
            finally {
                DO_NOT_COPY_SAVED_DATA.set(Boolean.FALSE);
            }
            PlatformHooks.get().chunkSyncSave(this.world, chunk, chunkData);
            chunk.tryMarkSaved();
            Completable completable = new Completable();
            Runnable run = () -> {
                CompoundTag data;
                try {
                    data = chunkData.write();
                }
                catch (Throwable thr) {
                    LOGGER.error("Failed to save chunk data (" + this.chunkX + "," + this.chunkZ + ") in world '" + WorldUtil.getWorldName((Level)this.world) + "'", thr);
                    completable.completeExceptionally(thr);
                    return;
                }
                completable.complete((Object)data);
            };
            if (unloading) {
                this.chunkDataUnload.toRun().setRunnable(run);
                task = this.chunkDataUnload.task();
            } else {
                task = this.scheduler.saveExecutor.createTask(run);
            }
            task.queue();
            MoonriseRegionFileIO.scheduleSave(this.world, this.chunkX, this.chunkZ, (Completable<CompoundTag>)completable, task, MoonriseRegionFileIO.RegionFileType.CHUNK_DATA, Priority.NORMAL);
            if (chunkSave != null) {
                chunkSave[0] = completable;
            }
        }
        catch (Throwable thr) {
            LOGGER.error("Failed to save chunk data (" + this.chunkX + "," + this.chunkZ + ") in world '" + WorldUtil.getWorldName((Level)this.world) + "'", thr);
        }
        return true;
    }

    private boolean saveEntities(ChunkEntitySlices entities, boolean unloading, Completable<CompoundTag>[] entitySave) {
        try {
            if (entities.isTransient()) {
                if (!unloading) {
                    return false;
                }
                CompoundTag save = entities.save();
                if (save == null) {
                    return false;
                }
                Completable<CompoundTag> mergeFrom = MoonriseRegionFileIO.loadDataAsync(this.world, this.chunkX, this.chunkZ, MoonriseRegionFileIO.RegionFileType.ENTITY_DATA, false, Priority.NORMAL);
                Completable toWrite = mergeFrom.handle((onDisk, thr) -> {
                    if (thr != null) {
                        LOGGER.error("Cannot merge transient entities for chunk (" + this.chunkX + "," + this.chunkZ + ") in world '" + WorldUtil.getWorldName((Level)this.world) + "', data on disk will be replaced", thr);
                        return save;
                    }
                    ChunkEntitySlices.copyEntities(onDisk, save);
                    return save;
                });
                MoonriseRegionFileIO.scheduleSave(this.world, this.chunkX, this.chunkZ, (Completable<CompoundTag>)toWrite, null, MoonriseRegionFileIO.RegionFileType.ENTITY_DATA, Priority.NORMAL);
                this.lastEntitySaveNull = false;
                if (entitySave != null) {
                    entitySave[0] = toWrite;
                }
                return true;
            }
            CompoundTag save = entities.save();
            if (save == null && this.lastEntitySaveNull) {
                return false;
            }
            MoonriseRegionFileIO.scheduleSave(this.world, this.chunkX, this.chunkZ, save, MoonriseRegionFileIO.RegionFileType.ENTITY_DATA);
            boolean bl = this.lastEntitySaveNull = save == null;
            if (entitySave != null) {
                entitySave[0] = Completable.completed((Object)save);
            }
        }
        catch (Throwable thr2) {
            LOGGER.error("Failed to save entity data (" + this.chunkX + "," + this.chunkZ + ") in world '" + WorldUtil.getWorldName((Level)this.world) + "'", thr2);
        }
        return true;
    }

    private boolean savePOI(PoiChunk poi, boolean unloading, Completable<CompoundTag>[] poiSave) {
        try {
            CompoundTag save = poi.save();
            poi.setDirty(false);
            if (save == null && this.lastPoiSaveNull) {
                return false;
            }
            MoonriseRegionFileIO.scheduleSave(this.world, this.chunkX, this.chunkZ, save, MoonriseRegionFileIO.RegionFileType.POI_DATA);
            boolean bl = this.lastPoiSaveNull = save == null;
            if (poiSave != null) {
                poiSave[0] = Completable.completed((Object)save);
            }
        }
        catch (Throwable thr) {
            LOGGER.error("Failed to save poi data (" + this.chunkX + "," + this.chunkZ + ") in world '" + WorldUtil.getWorldName((Level)this.world) + "'", thr);
        }
        return true;
    }

    public String toString() {
        ChunkCompletion lastCompletion = this.lastChunkCompletion;
        ChunkEntitySlices entityChunk = this.entityChunk;
        FullChunkStatus pendingFullStatus = this.pendingFullChunkStatus;
        FullChunkStatus currentFullStatus = this.currentFullChunkStatus;
        return "NewChunkHolder{world=" + WorldUtil.getWorldName((Level)this.world) + ", chunkX=" + this.chunkX + ", chunkZ=" + this.chunkZ + ", entityChunkFromDisk=" + (entityChunk != null && !entityChunk.isTransient()) + ", lastChunkCompletion={chunk_class=" + (lastCompletion == null || lastCompletion.chunk() == null ? "null" : lastCompletion.chunk().getClass().getName()) + ",status=" + String.valueOf(lastCompletion == null ? "null" : lastCompletion.genStatus()) + "}, currentGenStatus=" + String.valueOf(this.currentGenStatus) + ", requestedGenStatus=" + String.valueOf(this.requestedGenStatus) + ", generationTask=" + String.valueOf(this.generationTask) + ", generationTaskStatus=" + String.valueOf(this.generationTaskStatus) + ", priority=" + String.valueOf(this.priority) + ", priorityLocked=" + this.priorityLocked + ", neighbourRequestedPriority=" + String.valueOf(this.neighbourRequestedPriority) + ", effective_priority=" + String.valueOf(this.getEffectivePriority(null)) + ", oldTicketLevel=" + this.oldTicketLevel + ", currentTicketLevel=" + this.currentTicketLevel + ", totalNeighboursUsingThisChunk=" + this.totalNeighboursUsingThisChunk + ", fullNeighbourChunksLoadedBitset=" + this.fullNeighbourChunksLoadedBitset + ", currentChunkStatus=" + String.valueOf((Object)currentFullStatus) + ", pendingChunkStatus=" + String.valueOf((Object)pendingFullStatus) + ", is_unload_safe=" + this.isSafeToUnload() + ", killed=" + this.unloaded + "}";
    }

    private static JsonElement serializeStacktraceElement(StackTraceElement element) {
        return element == null ? JsonNull.INSTANCE : new JsonPrimitive(element.toString());
    }

    private static JsonObject serializeCompletable(CallbackCompletable<?> completable) {
        JsonObject ret = new JsonObject();
        if (completable == null) {
            return ret;
        }
        ret.addProperty("valid", Boolean.TRUE);
        boolean isCompleted = completable.isCompleted();
        ret.addProperty("completed", Boolean.valueOf(isCompleted));
        if (isCompleted) {
            Throwable throwable = completable.getThrowable();
            if (throwable != null) {
                JsonArray throwableJson = new JsonArray();
                ret.add("throwable", (JsonElement)throwableJson);
                for (StackTraceElement element : throwable.getStackTrace()) {
                    throwableJson.add(NewChunkHolder.serializeStacktraceElement(element));
                }
            } else {
                Object result = completable.getResult();
                ret.add("result_class", (JsonElement)(result == null ? JsonNull.INSTANCE : new JsonPrimitive(result.getClass().getName())));
            }
        }
        return ret;
    }

    public JsonObject getDebugJson() {
        PrioritisedExecutor.PrioritisedTask unloadTask;
        JsonObject ret = new JsonObject();
        ChunkCompletion lastCompletion = this.lastChunkCompletion;
        ChunkEntitySlices slices = this.entityChunk;
        PoiChunk poiChunk = this.poiChunk;
        ret.addProperty("chunkX", (Number)this.chunkX);
        ret.addProperty("chunkZ", (Number)this.chunkZ);
        ret.addProperty("entity_chunk", (String)(slices == null ? "null" : "transient=" + slices.isTransient()));
        ret.addProperty("poi_chunk", "null=" + (poiChunk == null));
        ret.addProperty("completed_chunk_class", lastCompletion == null ? "null" : lastCompletion.chunk().getClass().getName());
        ret.addProperty("completed_gen_status", lastCompletion == null ? "null" : lastCompletion.genStatus().toString());
        ret.addProperty("priority", Objects.toString(this.priority));
        ret.addProperty("neighbour_requested_priority", Objects.toString(this.neighbourRequestedPriority));
        ret.addProperty("generation_task", Objects.toString(this.generationTask));
        ret.addProperty("is_safe_unload", Objects.toString(this.isSafeToUnload()));
        ret.addProperty("old_ticket_level", (Number)this.oldTicketLevel);
        ret.addProperty("current_ticket_level", (Number)this.currentTicketLevel);
        ret.addProperty("neighbours_using_chunk", (Number)this.totalNeighboursUsingThisChunk);
        JsonObject neighbourWaitState = new JsonObject();
        ret.add("neighbour_state", (JsonElement)neighbourWaitState);
        JsonArray blockingGenNeighbours = new JsonArray();
        neighbourWaitState.add("blocking_gen_task", (JsonElement)blockingGenNeighbours);
        for (NewChunkHolder blockingGenNeighbour : this.neighboursBlockingGenTask) {
            JsonObject neighbour = new JsonObject();
            blockingGenNeighbours.add((JsonElement)neighbour);
            neighbour.addProperty("chunkX", (Number)blockingGenNeighbour.chunkX);
            neighbour.addProperty("chunkZ", (Number)blockingGenNeighbour.chunkZ);
        }
        JsonArray neighboursWaitingForUs = new JsonArray();
        neighbourWaitState.add("neighbours_waiting_on_us", (JsonElement)neighboursWaitingForUs);
        for (Reference2ObjectMap.Entry entry : this.neighboursWaitingForUs.reference2ObjectEntrySet()) {
            NewChunkHolder holder = (NewChunkHolder)entry.getKey();
            ChunkStatus status = (ChunkStatus)entry.getValue();
            JsonObject neighbour = new JsonObject();
            neighboursWaitingForUs.add((JsonElement)neighbour);
            neighbour.addProperty("chunkX", (Number)holder.chunkX);
            neighbour.addProperty("chunkZ", (Number)holder.chunkZ);
            neighbour.addProperty("waiting_for", Objects.toString(status));
        }
        ret.addProperty("pending_chunk_full_status", Objects.toString((Object)this.pendingFullChunkStatus));
        ret.addProperty("current_chunk_full_status", Objects.toString((Object)this.currentFullChunkStatus));
        ret.addProperty("generation_task", Objects.toString(this.generationTask));
        ret.addProperty("requested_generation", Objects.toString(this.requestedGenStatus));
        ret.addProperty("has_entity_load_task", Boolean.valueOf(this.entityDataLoadTask != null));
        ret.addProperty("has_poi_load_task", Boolean.valueOf(this.poiDataLoadTask != null));
        UnloadTask entityDataUnload = this.entityDataUnload;
        UnloadTask poiDataUnload = this.poiDataUnload;
        UnloadTask chunkDataUnload = this.chunkDataUnload;
        ret.add("entity_unload_completable", (JsonElement)NewChunkHolder.serializeCompletable(entityDataUnload == null ? null : entityDataUnload.completable()));
        ret.add("poi_unload_completable", (JsonElement)NewChunkHolder.serializeCompletable(poiDataUnload == null ? null : poiDataUnload.completable()));
        ret.add("chunk_unload_completable", (JsonElement)NewChunkHolder.serializeCompletable(chunkDataUnload == null ? null : chunkDataUnload.completable()));
        PrioritisedExecutor.PrioritisedTask prioritisedTask = unloadTask = chunkDataUnload == null ? null : chunkDataUnload.task();
        if (unloadTask == null) {
            ret.addProperty("unload_task_priority", "null");
            ret.addProperty("unload_task_suborder", (Number)0L);
        } else {
            ret.addProperty("unload_task_priority", Objects.toString(unloadTask.getPriority()));
            ret.addProperty("unload_task_suborder", (Number)unloadTask.getSubOrder());
        }
        ret.addProperty("killed", Boolean.valueOf(this.unloaded));
        return ret;
    }

    public static abstract class GenericDataLoadTaskCallback
    implements Cancellable {
        protected final Consumer<GenericDataLoadTask.TaskResult<?, Throwable>> consumer;
        protected final NewChunkHolder chunkHolder;
        protected boolean completed;
        protected GenericDataLoadTask<?, ?> schedule;
        protected final AtomicBoolean scheduled = new AtomicBoolean();

        public GenericDataLoadTaskCallback(Consumer<GenericDataLoadTask.TaskResult<?, Throwable>> consumer, NewChunkHolder chunkHolder) {
            this.consumer = consumer;
            this.chunkHolder = chunkHolder;
        }

        public void schedule() {
            if (this.scheduled.getAndSet(true)) {
                throw new IllegalStateException("Double calling schedule()");
            }
            if (this.schedule != null) {
                this.schedule.scheduleNow();
                this.schedule = null;
            }
        }

        boolean isCompleted() {
            return this.completed;
        }

        private boolean setCompleted() {
            if (this.completed) {
                return false;
            }
            this.completed = true;
            return true;
        }

        void markCompleted() {
            if (this.completed) {
                throw new IllegalStateException("May not be completed here");
            }
            this.completed = true;
        }

        void acceptCompleted(GenericDataLoadTask.TaskResult<?, Throwable> result) {
            if (result != null) {
                if (!this.completed) {
                    throw new IllegalStateException("Cannot be uncompleted at this point");
                }
            } else {
                throw new NullPointerException("Result cannot be null (cancelled)");
            }
            this.consumer.accept(result);
        }

        abstract void internalCancel();

        /*
         * WARNING - Removed try catching itself - possible behaviour change.
         */
        public boolean cancel() {
            NewChunkHolder holder = this.chunkHolder;
            ReentrantAreaLock.Node schedulingLock = holder.scheduler.schedulingLockArea.lock(holder.chunkX, holder.chunkZ);
            try {
                if (!this.completed) {
                    this.completed = true;
                    this.internalCancel();
                    boolean bl = true;
                    return bl;
                }
                boolean bl = false;
                return bl;
            }
            finally {
                holder.scheduler.schedulingLockArea.unlock(schedulingLock);
            }
        }
    }

    private static final class EntityDataLoadTaskCallback
    extends GenericDataLoadTaskCallback {
        public EntityDataLoadTaskCallback(Consumer<GenericDataLoadTask.TaskResult<?, Throwable>> consumer, NewChunkHolder chunkHolder) {
            super(consumer, chunkHolder);
        }

        @Override
        void internalCancel() {
            this.chunkHolder.entityDataLoadTaskWaiters.remove(this);
            this.chunkHolder.entityDataLoadTask.cancel();
        }
    }

    private static final class PoiDataLoadTaskCallback
    extends GenericDataLoadTaskCallback {
        public PoiDataLoadTaskCallback(Consumer<GenericDataLoadTask.TaskResult<?, Throwable>> consumer, NewChunkHolder chunkHolder) {
            super(consumer, chunkHolder);
        }

        @Override
        void internalCancel() {
            this.chunkHolder.poiDataLoadTaskWaiters.remove(this);
            this.chunkHolder.poiDataLoadTask.cancel();
        }
    }

    public record ChunkCompletion(ChunkAccess chunk, ChunkStatus genStatus) {
    }

    public record UnloadTask(CallbackCompletable<CompoundTag> completable, PrioritisedExecutor.PrioritisedTask task, LazyRunnable toRun) {
    }

    record UnloadState(NewChunkHolder holder, ChunkAccess chunk, ChunkEntitySlices entityChunk, PoiChunk poiChunk) {
    }

    public record SaveStat(boolean savedChunk, boolean savedEntityChunk, boolean savedPoiChunk) {
    }
}
