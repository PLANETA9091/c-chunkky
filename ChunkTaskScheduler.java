/*
 * Decompiled with CFR 0.152.
 * 
 * Could not load the following classes:
 *  ca.spottedleaf.concurrentutil.executor.PrioritisedExecutor
 *  ca.spottedleaf.concurrentutil.executor.PrioritisedExecutor$PrioritisedTask
 *  ca.spottedleaf.concurrentutil.executor.queue.AreaDependentQueue
 *  ca.spottedleaf.concurrentutil.executor.queue.PrioritisedTaskQueue
 *  ca.spottedleaf.concurrentutil.executor.thread.BalancedPrioritisedThreadPool$OrderedStreamGroup$Queue
 *  ca.spottedleaf.concurrentutil.lock.ReentrantAreaLock
 *  ca.spottedleaf.concurrentutil.lock.ReentrantAreaLock$Node
 *  ca.spottedleaf.concurrentutil.util.ConcurrentUtil
 *  ca.spottedleaf.concurrentutil.util.Priority
 *  ca.spottedleaf.moonrise.common.util.CoordinateUtils
 *  ca.spottedleaf.moonrise.common.util.JsonUtil
 *  ca.spottedleaf.moonrise.common.util.MoonriseCommon
 *  ca.spottedleaf.moonrise.common.util.TickThread
 *  ca.spottedleaf.moonrise.common.util.WorldUtil
 *  com.google.gson.JsonArray
 *  com.google.gson.JsonElement
 *  com.google.gson.JsonObject
 *  com.mojang.logging.LogUtils
 *  net.minecraft.CrashReport
 *  net.minecraft.CrashReportCategory
 *  net.minecraft.ReportedException
 *  net.minecraft.server.MinecraftServer
 *  net.minecraft.util.StaticCache2D
 *  net.minecraft.world.entity.Entity$RemovalReason
 *  net.minecraft.world.level.ChunkPos
 *  net.minecraft.world.level.Level
 *  net.minecraft.world.level.chunk.ChunkAccess
 *  net.minecraft.world.level.chunk.LevelChunk
 *  net.minecraft.world.phys.Vec3
 *  org.slf4j.Logger
 */
package ca.spottedleaf.moonrise.patches.chunk_system.scheduling;

import ca.spottedleaf.concurrentutil.executor.PrioritisedExecutor;
import ca.spottedleaf.concurrentutil.executor.queue.AreaDependentQueue;
import ca.spottedleaf.concurrentutil.executor.queue.PrioritisedTaskQueue;
import ca.spottedleaf.concurrentutil.executor.thread.BalancedPrioritisedThreadPool;
import ca.spottedleaf.concurrentutil.lock.ReentrantAreaLock;
import ca.spottedleaf.concurrentutil.util.ConcurrentUtil;
import ca.spottedleaf.concurrentutil.util.Priority;
import ca.spottedleaf.moonrise.common.util.CoordinateUtils;
import ca.spottedleaf.moonrise.common.util.JsonUtil;
import ca.spottedleaf.moonrise.common.util.MoonriseCommon;
import ca.spottedleaf.moonrise.common.util.TickThread;
import ca.spottedleaf.moonrise.common.util.WorldUtil;
import ca.spottedleaf.moonrise.patches.chunk_system.scheduling.ChunkHolderManager;
import ca.spottedleaf.moonrise.patches.chunk_system.scheduling.NewChunkHolder;
import ca.spottedleaf.moonrise.patches.chunk_system.scheduling.task.ChunkFullTask;
import ca.spottedleaf.moonrise.patches.chunk_system.scheduling.task.ChunkLightTask;
import ca.spottedleaf.moonrise.patches.chunk_system.scheduling.task.ChunkLoadTask;
import ca.spottedleaf.moonrise.patches.chunk_system.scheduling.task.ChunkProgressionTask;
import ca.spottedleaf.moonrise.patches.chunk_system.scheduling.task.ChunkUpgradeGenericStatusTask;
import ca.spottedleaf.moonrise.patches.chunk_system.status.ChunkSystemChunkStep;
import ca.spottedleaf.moonrise.patches.chunk_system.ticket.ChunkSystemTicketType;
import ca.spottedleaf.moonrise.patches.chunk_system.util.ParallelSearchRadiusIteration;
import com.google.gson.JsonArray;
import com.google.gson.JsonElement;
import com.google.gson.JsonObject;
import com.mojang.logging.LogUtils;
import java.io.File;
import java.time.LocalDateTime;
import java.time.format.DateTimeFormatter;
import java.util.ArrayDeque;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;
import java.util.Map;
import java.util.Objects;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicLong;
import java.util.function.Consumer;
import net.minecraft.CrashReport;
import net.minecraft.CrashReportCategory;
import net.minecraft.ReportedException;
import net.minecraft.server.MinecraftServer;
import net.minecraft.server.level.ChunkLevel;
import net.minecraft.server.level.ChunkMap;
import net.minecraft.server.level.FullChunkStatus;
import net.minecraft.server.level.GenerationChunkHolder;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.server.level.ServerPlayer;
import net.minecraft.server.level.TicketType;
import net.minecraft.util.StaticCache2D;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.chunk.ChunkAccess;
import net.minecraft.world.level.chunk.LevelChunk;
import net.minecraft.world.level.chunk.status.ChunkPyramid;
import net.minecraft.world.level.chunk.status.ChunkStatus;
import net.minecraft.world.level.chunk.status.ChunkStep;
import net.minecraft.world.phys.Vec3;
import org.slf4j.Logger;

public final class ChunkTaskScheduler {
    private static final Logger LOGGER = LogUtils.getClassLogger();
    public static final TicketType CHUNK_LOAD = ChunkSystemTicketType.create("chunk_system:chunk_load", Long::compareTo);
    private static final AtomicLong CHUNK_LOAD_IDS = new AtomicLong();
    public static final TicketType NON_FULL_CHUNK_LOAD = ChunkSystemTicketType.create("chunk_system:non_full_load", Long::compareTo);
    private static final AtomicLong NON_FULL_CHUNK_LOAD_IDS = new AtomicLong();
    public static final TicketType ENTITY_LOAD = ChunkSystemTicketType.create("chunk_system:entity_load", Long::compareTo);
    private static final AtomicLong ENTITY_LOAD_IDS = new AtomicLong();
    public static final TicketType POI_LOAD = ChunkSystemTicketType.create("chunk_system:poi_load", Long::compareTo);
    private static final AtomicLong POI_LOAD_IDS = new AtomicLong();
    public static final TicketType CHUNK_RELIGHT = ChunkSystemTicketType.create("starlight:chunk_relight", Long::compareTo);
    private static final AtomicLong CHUNK_RELIGHT_IDS = new AtomicLong();
    public final ServerLevel world;
    public final AreaDependentQueue radiusAwareScheduler;
    public final BalancedPrioritisedThreadPool.OrderedStreamGroup.Queue parallelGenExecutor;
    public final BalancedPrioritisedThreadPool.OrderedStreamGroup.Queue loadExecutor;
    public final BalancedPrioritisedThreadPool.OrderedStreamGroup.Queue ioExecutor;
    public final BalancedPrioritisedThreadPool.OrderedStreamGroup.Queue compressionExecutor;
    public final BalancedPrioritisedThreadPool.OrderedStreamGroup.Queue saveExecutor;
    private final PrioritisedTaskQueue mainThreadExecutor = new PrioritisedTaskQueue();
    public final ChunkHolderManager chunkHolderManager;
    private static final int[] ACCESS_RADIUS_TABLE_LOAD;
    private static final int[] ACCESS_RADIUS_TABLE_GEN;
    private static final int[] ACCESS_RADIUS_TABLE;
    private static final int MAX_ACCESS_RADIUS;
    public final ReentrantAreaLock schedulingLockArea;
    private final int lockShift;
    private volatile boolean shutdown;
    private final AtomicBoolean failedChunkSystem = new AtomicBoolean();
    public static final ArrayDeque<ChunkInfo> WAITING_CHUNKS;

    public static Long getNextChunkLoadId() {
        return CHUNK_LOAD_IDS.getAndIncrement();
    }

    public static Long getNextNonFullLoadId() {
        return NON_FULL_CHUNK_LOAD_IDS.getAndIncrement();
    }

    public static Long getNextEntityLoadId() {
        return ENTITY_LOAD_IDS.getAndIncrement();
    }

    public static Long getNextPoiLoadId() {
        return POI_LOAD_IDS.getAndIncrement();
    }

    public static Long getNextChunkRelightId() {
        return CHUNK_RELIGHT_IDS.getAndIncrement();
    }

    public static int getTicketLevel(ChunkStatus status) {
        return ChunkLevel.byStatus(status);
    }

    private static int getAccessRadius0(ChunkStatus toStatus, ChunkPyramid pyramid) {
        int radius;
        if (toStatus == ChunkStatus.EMPTY) {
            return 0;
        }
        ChunkStep chunkStep = pyramid.getStepTo(toStatus);
        int maxRange = radius = chunkStep.getAccumulatedRadiusOf(ChunkStatus.EMPTY);
        for (int dist = 0; dist <= radius; ++dist) {
            ChunkStatus requiredNeighbourStatus = ((ChunkSystemChunkStep)chunkStep).moonrise$getRequiredStatusAtRadius(dist);
            int rad = ACCESS_RADIUS_TABLE[requiredNeighbourStatus.getIndex()];
            if (rad == -1) {
                throw new IllegalStateException();
            }
            maxRange = Math.max(maxRange, dist + rad);
        }
        return maxRange;
    }

    public static int getMaxAccessRadius() {
        return MAX_ACCESS_RADIUS;
    }

    public static int getAccessRadius(ChunkStatus genStatus) {
        return ACCESS_RADIUS_TABLE[genStatus.getIndex()];
    }

    public static int getAccessRadius(FullChunkStatus status) {
        return status.ordinal() - 1 + ChunkTaskScheduler.getAccessRadius(ChunkStatus.FULL);
    }

    public final int getChunkSystemLockShift() {
        return this.lockShift;
    }

    public boolean hasShutdown() {
        return this.shutdown;
    }

    public void setShutdown(boolean shutdown) {
        this.shutdown = shutdown;
    }

    public ChunkTaskScheduler(ServerLevel world) {
        this.world = world;
        this.lockShift = Math.max(world.moonrise$getRegionChunkShift(), 6);
        this.schedulingLockArea = new ReentrantAreaLock(this.getChunkSystemLockShift());
        this.parallelGenExecutor = MoonriseCommon.SERVER_GROUP.createExecutor();
        this.loadExecutor = MoonriseCommon.SERVER_GROUP.createExecutor();
        this.radiusAwareScheduler = new AreaDependentQueue((PrioritisedExecutor)this.parallelGenExecutor, 4);
        this.ioExecutor = MoonriseCommon.SERVER_IO_GROUP.createExecutor();
        this.compressionExecutor = MoonriseCommon.SERVER_GROUP.createExecutor();
        this.saveExecutor = MoonriseCommon.SERVER_GROUP.createExecutor();
        this.chunkHolderManager = new ChunkHolderManager(world, this);
    }

    public static Object stringIfNull(Object obj) {
        return obj == null ? "null" : obj;
    }

    public void unrecoverableChunkSystemFailure(int chunkX, int chunkZ, Map<String, Object> objectsOfInterest, Throwable thr) {
        NewChunkHolder holder = this.chunkHolderManager.getChunkHolder(chunkX, chunkZ);
        LOGGER.error("Chunk system error at chunk (" + chunkX + "," + chunkZ + "), holder: " + String.valueOf(holder) + ", exception:", new Throwable(thr));
        if (this.failedChunkSystem.getAndSet(true)) {
            return;
        }
        ReportedException reportedException = thr instanceof ReportedException ? (ReportedException)thr : new ReportedException(new CrashReport("Chunk system error", thr));
        CrashReportCategory crashReportCategory = reportedException.getReport().addCategory("Chunk system details");
        crashReportCategory.setDetail("Chunk coordinate", (Object)new ChunkPos(chunkX, chunkZ).toString());
        crashReportCategory.setDetail("ChunkHolder", (Object)Objects.toString(holder));
        crashReportCategory.setDetail("unrecoverableChunkSystemFailure caller thread", (Object)Thread.currentThread().getName());
        crashReportCategory = reportedException.getReport().addCategory("Chunk System Objects of Interest");
        for (Map.Entry<String, Object> entry : objectsOfInterest.entrySet()) {
            Object object = entry.getValue();
            if (object instanceof Throwable) {
                Throwable thrObject = (Throwable)object;
                crashReportCategory.setDetailError(Objects.toString(entry.getKey()), thrObject);
                continue;
            }
            crashReportCategory.setDetail(Objects.toString(entry.getKey()), (Object)Objects.toString(entry.getValue()));
        }
        Runnable crash = () -> {
            throw new RuntimeException("Chunk system crash propagated from unrecoverableChunkSystemFailure", (Throwable)reportedException);
        };
        this.scheduleChunkTask(chunkX, chunkZ, crash, Priority.BLOCKING);
        this.world.getServer().moonrise$setChunkSystemCrash(new RuntimeException("Chunk system crash propagated from unrecoverableChunkSystemFailure", (Throwable)reportedException));
    }

    public boolean executeMainThreadTask() {
        TickThread.ensureTickThread((String)"Cannot execute main thread task off-main");
        return this.mainThreadExecutor.executeTask();
    }

    public void executeAllRecentlyQueuedMainThreadTasks() {
        long executed = this.mainThreadExecutor.getTotalTasksExecuted();
        long scheduled = this.mainThreadExecutor.getTotalTasksScheduled();
        long left = scheduled - executed;
        for (long i = 0L; i < left && this.mainThreadExecutor.executeTask(); ++i) {
        }
    }

    public void raisePriority(int x, int z, Priority priority) {
        this.chunkHolderManager.raisePriority(x, z, priority);
    }

    public void setPriority(int x, int z, Priority priority) {
        this.chunkHolderManager.setPriority(x, z, priority);
    }

    public void lowerPriority(int x, int z, Priority priority) {
        this.chunkHolderManager.lowerPriority(x, z, priority);
    }

    /*
     * WARNING - Removed try catching itself - possible behaviour change.
     */
    public void scheduleTickingState(int chunkX, int chunkZ, FullChunkStatus toStatus, boolean addTicket, Priority priority, Consumer<LevelChunk> onComplete) {
        LevelChunk chunk2;
        boolean scheduled;
        int radius = toStatus.ordinal() - 1;
        if (!TickThread.isTickThreadFor((Level)this.world, (int)chunkX, (int)chunkZ, (int)Math.max(0, radius))) {
            this.scheduleChunkTask(chunkX, chunkZ, () -> this.scheduleTickingState(chunkX, chunkZ, toStatus, addTicket, priority, onComplete), priority);
            return;
        }
        int accessRadius = ChunkTaskScheduler.getAccessRadius(toStatus);
        if (this.chunkHolderManager.ticketLockArea.isHeldByCurrentThread(chunkX, chunkZ, accessRadius)) {
            throw new IllegalStateException("Cannot schedule chunk load during ticket level update");
        }
        if (this.schedulingLockArea.isHeldByCurrentThread(chunkX, chunkZ, accessRadius)) {
            throw new IllegalStateException("Cannot schedule chunk loading recursively");
        }
        if (toStatus == FullChunkStatus.INACCESSIBLE) {
            throw new IllegalArgumentException("Cannot wait for INACCESSIBLE status");
        }
        int minLevel = ChunkLevel.byStatus(toStatus);
        Long chunkReference = addTicket ? ChunkTaskScheduler.getNextChunkLoadId() : null;
        long chunkKey = CoordinateUtils.getChunkKey((int)chunkX, (int)chunkZ);
        if (addTicket) {
            this.chunkHolderManager.addTicketAtLevel(CHUNK_LOAD, chunkKey, minLevel, chunkReference);
            this.chunkHolderManager.processTicketUpdates();
        }
        Consumer<LevelChunk> loadCallback = onComplete == null && !addTicket ? null : chunk -> {
            try {
                if (onComplete != null) {
                    onComplete.accept((LevelChunk)chunk);
                }
            }
            finally {
                if (addTicket) {
                    this.chunkHolderManager.removeTicketAtLevel(CHUNK_LOAD, chunkKey, minLevel, chunkReference);
                }
            }
        };
        ReentrantAreaLock.Node ticketLock = this.chunkHolderManager.ticketLockArea.lock(chunkX, chunkZ, accessRadius);
        try {
            ReentrantAreaLock.Node schedulingLock = this.schedulingLockArea.lock(chunkX, chunkZ, accessRadius);
            try {
                NewChunkHolder chunkHolder = this.chunkHolderManager.getChunkHolder(chunkKey);
                if (chunkHolder == null || chunkHolder.getTicketLevel() > minLevel) {
                    scheduled = false;
                    chunk2 = null;
                } else {
                    FullChunkStatus currStatus = chunkHolder.getChunkStatus();
                    if (currStatus.isOrAfter(toStatus)) {
                        scheduled = false;
                        chunk2 = (LevelChunk)chunkHolder.getCurrentChunk();
                    } else {
                        scheduled = true;
                        chunk2 = null;
                        for (int dz = -radius; dz <= radius; ++dz) {
                            for (int dx = -radius; dx <= radius; ++dx) {
                                NewChunkHolder neighbour;
                                NewChunkHolder newChunkHolder = neighbour = (dx | dz) == 0 ? chunkHolder : this.chunkHolderManager.getChunkHolder(dx + chunkX, dz + chunkZ);
                                if (neighbour == null) continue;
                                neighbour.raisePriority(priority);
                            }
                        }
                        if (loadCallback != null) {
                            chunkHolder.addFullStatusConsumer(toStatus, loadCallback);
                        }
                    }
                }
            }
            finally {
                this.schedulingLockArea.unlock(schedulingLock);
            }
        }
        finally {
            this.chunkHolderManager.ticketLockArea.unlock(ticketLock);
        }
        if (loadCallback != null && !scheduled) {
            try {
                loadCallback.accept(chunk2);
            }
            catch (Throwable thr) {
                LOGGER.error("Failed to process chunk full status callback", thr);
            }
        }
    }

    public void scheduleChunkLoad(int chunkX, int chunkZ, boolean gen, ChunkStatus toStatus, boolean addTicket, Priority priority, Consumer<ChunkAccess> onComplete) {
        if (gen) {
            this.scheduleChunkLoad(chunkX, chunkZ, toStatus, addTicket, priority, onComplete);
            return;
        }
        this.scheduleChunkLoad(chunkX, chunkZ, ChunkStatus.EMPTY, addTicket, priority, chunk -> {
            if (chunk == null) {
                if (onComplete != null) {
                    onComplete.accept(null);
                }
            } else if (chunk.getPersistedStatus().isOrAfter(toStatus)) {
                this.scheduleChunkLoad(chunkX, chunkZ, toStatus, addTicket, priority, onComplete);
            } else if (onComplete != null) {
                onComplete.accept(null);
            }
        });
    }

    /*
     * WARNING - Removed try catching itself - possible behaviour change.
     */
    public boolean beginChunkLoadForNonFullSync(int chunkX, int chunkZ, ChunkStatus toStatus, Priority priority) {
        int accessRadius = ChunkTaskScheduler.getAccessRadius(toStatus);
        long chunkKey = CoordinateUtils.getChunkKey((int)chunkX, (int)chunkZ);
        int minLevel = ChunkTaskScheduler.getTicketLevel(toStatus);
        ArrayList<ChunkProgressionTask> tasks = new ArrayList<ChunkProgressionTask>();
        ReentrantAreaLock.Node ticketLock = this.chunkHolderManager.ticketLockArea.lock(chunkX, chunkZ, accessRadius);
        try {
            ReentrantAreaLock.Node schedulingLock = this.schedulingLockArea.lock(chunkX, chunkZ, accessRadius);
            try {
                NewChunkHolder chunkHolder = this.chunkHolderManager.getChunkHolder(chunkKey);
                if (chunkHolder == null || chunkHolder.getTicketLevel() > minLevel) {
                    boolean bl = false;
                    return bl;
                }
                ChunkStatus genStatus = chunkHolder.getCurrentGenStatus();
                if (genStatus != null && genStatus.isOrAfter(toStatus)) {
                    boolean bl = true;
                    return bl;
                }
                chunkHolder.raisePriority(priority);
                if (!chunkHolder.upgradeGenTarget(toStatus)) {
                    this.schedule(chunkX, chunkZ, toStatus, chunkHolder, tasks);
                }
            }
            finally {
                this.schedulingLockArea.unlock(schedulingLock);
            }
        }
        finally {
            this.chunkHolderManager.ticketLockArea.unlock(ticketLock);
        }
        int len = tasks.size();
        for (int i = 0; i < len; ++i) {
            ((ChunkProgressionTask)tasks.get(i)).schedule();
        }
        return true;
    }

    public ChunkAccess syncLoadNonFull(int chunkX, int chunkZ, ChunkStatus status) {
        if (status == null || status.isOrAfter(ChunkStatus.FULL)) {
            throw new IllegalArgumentException("Status: " + String.valueOf(status));
        }
        if (!TickThread.isTickThread()) {
            return this.world.getChunkSource().getChunk(chunkX, chunkZ, status, true);
        }
        ChunkAccess loaded = this.world.moonrise$getSpecificChunkIfLoaded(chunkX, chunkZ, status);
        if (loaded != null) {
            return loaded;
        }
        if (this.hasShutdown()) {
            throw new IllegalStateException("Chunk system has shut down, cannot process chunk requests in world '" + WorldUtil.getWorldName((Level)this.world) + "' at (" + chunkX + "," + chunkZ + ") status: " + String.valueOf(status));
        }
        Long ticketId = ChunkTaskScheduler.getNextNonFullLoadId();
        int ticketLevel = ChunkTaskScheduler.getTicketLevel(status);
        this.chunkHolderManager.addTicketAtLevel(NON_FULL_CHUNK_LOAD, chunkX, chunkZ, ticketLevel, ticketId);
        this.chunkHolderManager.processTicketUpdates();
        this.beginChunkLoadForNonFullSync(chunkX, chunkZ, status, Priority.BLOCKING);
        this.world.getChunkSource().mainThreadProcessor.managedBlock(() -> this.world.moonrise$getSpecificChunkIfLoaded(chunkX, chunkZ, status) != null);
        loaded = this.world.moonrise$getSpecificChunkIfLoaded(chunkX, chunkZ, status);
        this.chunkHolderManager.removeTicketAtLevel(NON_FULL_CHUNK_LOAD, chunkX, chunkZ, ticketLevel, ticketId);
        if (loaded == null) {
            throw new IllegalStateException("Expected chunk to be loaded for status " + String.valueOf(status));
        }
        return loaded;
    }

    /*
     * WARNING - Removed try catching itself - possible behaviour change.
     */
    public void scheduleChunkLoad(int chunkX, int chunkZ, ChunkStatus toStatus, boolean addTicket, Priority priority, Consumer<ChunkAccess> onComplete) {
        ChunkAccess chunk2;
        boolean scheduled;
        if (!TickThread.isTickThreadFor((Level)this.world, (int)chunkX, (int)chunkZ)) {
            this.scheduleChunkTask(chunkX, chunkZ, () -> this.scheduleChunkLoad(chunkX, chunkZ, toStatus, addTicket, priority, onComplete), priority);
            return;
        }
        int accessRadius = ChunkTaskScheduler.getAccessRadius(toStatus);
        if (this.chunkHolderManager.ticketLockArea.isHeldByCurrentThread(chunkX, chunkZ, accessRadius)) {
            throw new IllegalStateException("Cannot schedule chunk load during ticket level update");
        }
        if (this.schedulingLockArea.isHeldByCurrentThread(chunkX, chunkZ, accessRadius)) {
            throw new IllegalStateException("Cannot schedule chunk loading recursively");
        }
        if (toStatus == ChunkStatus.FULL) {
            this.scheduleTickingState(chunkX, chunkZ, FullChunkStatus.FULL, addTicket, priority, onComplete);
            return;
        }
        int minLevel = ChunkTaskScheduler.getTicketLevel(toStatus);
        Long chunkReference = addTicket ? ChunkTaskScheduler.getNextChunkLoadId() : null;
        long chunkKey = CoordinateUtils.getChunkKey((int)chunkX, (int)chunkZ);
        if (addTicket) {
            this.chunkHolderManager.addTicketAtLevel(CHUNK_LOAD, chunkKey, minLevel, chunkReference);
            this.chunkHolderManager.processTicketUpdates();
        }
        Consumer<ChunkAccess> loadCallback = onComplete == null && !addTicket ? null : chunk -> {
            try {
                if (onComplete != null) {
                    onComplete.accept((ChunkAccess)chunk);
                }
            }
            finally {
                if (addTicket) {
                    this.chunkHolderManager.removeTicketAtLevel(CHUNK_LOAD, chunkKey, minLevel, chunkReference);
                }
            }
        };
        ArrayList<ChunkProgressionTask> tasks = new ArrayList<ChunkProgressionTask>();
        ReentrantAreaLock.Node ticketLock = this.chunkHolderManager.ticketLockArea.lock(chunkX, chunkZ, accessRadius);
        try {
            ReentrantAreaLock.Node schedulingLock = this.schedulingLockArea.lock(chunkX, chunkZ, accessRadius);
            try {
                NewChunkHolder chunkHolder = this.chunkHolderManager.getChunkHolder(chunkKey);
                if (chunkHolder == null || chunkHolder.getTicketLevel() > minLevel) {
                    scheduled = false;
                    chunk2 = null;
                } else {
                    ChunkStatus genStatus = chunkHolder.getCurrentGenStatus();
                    if (genStatus != null && genStatus.isOrAfter(toStatus)) {
                        scheduled = false;
                        chunk2 = chunkHolder.getCurrentChunk();
                    } else {
                        scheduled = true;
                        chunk2 = null;
                        chunkHolder.raisePriority(priority);
                        if (!chunkHolder.upgradeGenTarget(toStatus)) {
                            this.schedule(chunkX, chunkZ, toStatus, chunkHolder, tasks);
                        }
                        if (loadCallback != null) {
                            chunkHolder.addStatusConsumer(toStatus, loadCallback);
                        }
                    }
                }
            }
            finally {
                this.schedulingLockArea.unlock(schedulingLock);
            }
        }
        finally {
            this.chunkHolderManager.ticketLockArea.unlock(ticketLock);
        }
        int len = tasks.size();
        for (int i = 0; i < len; ++i) {
            ((ChunkProgressionTask)tasks.get(i)).schedule();
        }
        if (loadCallback != null && !scheduled) {
            try {
                loadCallback.accept(chunk2);
            }
            catch (Throwable thr) {
                LOGGER.error("Failed to process chunk status callback", thr);
            }
        }
    }

    private ChunkProgressionTask createTask(int chunkX, int chunkZ, ChunkAccess chunk, NewChunkHolder chunkHolder, StaticCache2D<GenerationChunkHolder> neighbours, ChunkStatus toStatus, Priority initialPriority) {
        if (toStatus == ChunkStatus.EMPTY) {
            return new ChunkLoadTask(this, this.world, chunkX, chunkZ, chunkHolder, initialPriority);
        }
        if (toStatus == ChunkStatus.LIGHT) {
            return new ChunkLightTask(this, this.world, chunkX, chunkZ, chunk, initialPriority);
        }
        if (toStatus == ChunkStatus.FULL) {
            return new ChunkFullTask(this, this.world, chunkX, chunkZ, chunkHolder, chunk, initialPriority);
        }
        return new ChunkUpgradeGenericStatusTask(this, this.world, chunkX, chunkZ, chunk, neighbours, toStatus, initialPriority);
    }

    ChunkProgressionTask schedule(int chunkX, int chunkZ, ChunkStatus targetStatus, NewChunkHolder chunkHolder, List<ChunkProgressionTask> allTasks) {
        return this.schedule(chunkX, chunkZ, targetStatus, chunkHolder, allTasks, chunkHolder.getEffectivePriority(Priority.NORMAL));
    }

    private ChunkProgressionTask schedule(int chunkX, int chunkZ, ChunkStatus targetStatus, NewChunkHolder chunkHolder, List<ChunkProgressionTask> allTasks, Priority minPriority) {
        if (!this.schedulingLockArea.isHeldByCurrentThread(chunkX, chunkZ, ChunkTaskScheduler.getAccessRadius(targetStatus))) {
            throw new IllegalStateException("Not holding scheduling lock");
        }
        if (chunkHolder.hasGenerationTask()) {
            chunkHolder.upgradeGenTarget(targetStatus);
            return null;
        }
        Priority requestedPriority = Priority.max((Priority)minPriority, (Priority)chunkHolder.getEffectivePriority(Priority.NORMAL));
        ChunkStatus currentGenStatus = chunkHolder.getCurrentGenStatus();
        ChunkAccess chunk = chunkHolder.getCurrentChunk();
        if (currentGenStatus == null) {
            ChunkProgressionTask task = this.createTask(chunkX, chunkZ, chunk, chunkHolder, null, ChunkStatus.EMPTY, requestedPriority);
            allTasks.add(task);
            ArrayList<NewChunkHolder> chunkHolderNeighbours = new ArrayList<NewChunkHolder>(1);
            chunkHolderNeighbours.add(chunkHolder);
            chunkHolder.setGenerationTarget(targetStatus);
            chunkHolder.setGenerationTask(task, ChunkStatus.EMPTY, chunkHolderNeighbours);
            return task;
        }
        if (currentGenStatus.isOrAfter(targetStatus)) {
            return null;
        }
        chunkHolder.setGenerationTarget(targetStatus);
        ChunkStatus chunkRealStatus = chunk.getPersistedStatus();
        ChunkStatus toStatus = currentGenStatus.moonrise$getNextStatus();
        ChunkPyramid chunkPyramid = chunkRealStatus.isOrAfter(toStatus) ? ChunkPyramid.LOADING_PYRAMID : ChunkPyramid.GENERATION_PYRAMID;
        ChunkStep chunkStep = chunkPyramid.getStepTo(toStatus);
        int neighbourReadRadius = Math.max(0, chunkStep.getAccumulatedRadiusOf(ChunkStatus.EMPTY));
        boolean unGeneratedNeighbours = false;
        if (neighbourReadRadius > 0) {
            ChunkMap chunkMap = this.world.getChunkSource().chunkMap;
            for (long pos : ParallelSearchRadiusIteration.getSearchIteration(neighbourReadRadius)) {
                int x = CoordinateUtils.getChunkX((long)pos);
                int z = CoordinateUtils.getChunkZ((long)pos);
                int radius = Math.max(Math.abs(x), Math.abs(z));
                ChunkStatus requiredNeighbourStatus = ((ChunkSystemChunkStep)chunkStep).moonrise$getRequiredStatusAtRadius(radius);
                unGeneratedNeighbours |= this.checkNeighbour(chunkX + x, chunkZ + z, requiredNeighbourStatus, chunkHolder, allTasks, requestedPriority);
            }
        }
        if (unGeneratedNeighbours) {
            chunkHolder.recalculateNeighbourPriorities();
            return null;
        }
        ArrayList<NewChunkHolder> chunkHolderNeighbours = new ArrayList<NewChunkHolder>((2 * neighbourReadRadius + 1) * (2 * neighbourReadRadius + 1));
        StaticCache2D neighbours = StaticCache2D.create((int)chunkX, (int)chunkZ, (int)neighbourReadRadius, (nx, nz) -> {
            NewChunkHolder holder = nx == chunkX && nz == chunkZ ? chunkHolder : this.chunkHolderManager.getChunkHolder(nx, nz);
            chunkHolderNeighbours.add(holder);
            return holder.vanillaChunkHolder;
        });
        ChunkProgressionTask task = this.createTask(chunkX, chunkZ, chunk, chunkHolder, (StaticCache2D<GenerationChunkHolder>)neighbours, toStatus, chunkHolder.getEffectivePriority(Priority.NORMAL));
        allTasks.add(task);
        chunkHolder.setGenerationTask(task, toStatus, chunkHolderNeighbours);
        return task;
    }

    private boolean checkNeighbour(int chunkX, int chunkZ, ChunkStatus requiredStatus, NewChunkHolder center, List<ChunkProgressionTask> tasks, Priority minPriority) {
        NewChunkHolder chunkHolder = this.chunkHolderManager.getChunkHolder(chunkX, chunkZ);
        if (chunkHolder == null) {
            throw new IllegalStateException("Missing chunkholder when required");
        }
        ChunkStatus holderStatus = chunkHolder.getCurrentGenStatus();
        if (holderStatus != null && holderStatus.isOrAfter(requiredStatus)) {
            return false;
        }
        if (chunkHolder.hasFailedGeneration()) {
            return true;
        }
        center.addGenerationBlockingNeighbour(chunkHolder);
        chunkHolder.addWaitingNeighbour(center, requiredStatus);
        if (chunkHolder.upgradeGenTarget(requiredStatus)) {
            return true;
        }
        this.schedule(chunkX, chunkZ, requiredStatus, chunkHolder, tasks, minPriority);
        return true;
    }

    @Deprecated
    public PrioritisedExecutor.PrioritisedTask scheduleChunkTask(Runnable run) {
        return this.scheduleChunkTask(run, Priority.NORMAL);
    }

    @Deprecated
    public PrioritisedExecutor.PrioritisedTask scheduleChunkTask(Runnable run, Priority priority) {
        return this.mainThreadExecutor.queueTask(run, priority);
    }

    public PrioritisedExecutor.PrioritisedTask createChunkTask(int chunkX, int chunkZ, Runnable run) {
        return this.createChunkTask(chunkX, chunkZ, run, Priority.NORMAL);
    }

    public PrioritisedExecutor.PrioritisedTask createChunkTask(int chunkX, int chunkZ, Runnable run, Priority priority) {
        return this.mainThreadExecutor.createTask(run, priority);
    }

    public PrioritisedExecutor.PrioritisedTask scheduleChunkTask(int chunkX, int chunkZ, Runnable run) {
        return this.scheduleChunkTask(chunkX, chunkZ, run, Priority.NORMAL);
    }

    public PrioritisedExecutor.PrioritisedTask scheduleChunkTask(int chunkX, int chunkZ, Runnable run, Priority priority) {
        return this.mainThreadExecutor.queueTask(run, priority);
    }

    public boolean halt(boolean sync, long maxWaitNS) {
        this.parallelGenExecutor.halt();
        this.loadExecutor.halt();
        if (sync) {
            long time = System.nanoTime();
            long failures = 9L;
            while (true) {
                if (!this.parallelGenExecutor.isActive() && !this.loadExecutor.isActive()) {
                    return true;
                }
                if (System.nanoTime() - time >= maxWaitNS) {
                    return false;
                }
                failures = ConcurrentUtil.linearLongBackoff((long)failures, (long)500000L, (long)50000000L);
            }
        }
        return true;
    }

    public boolean haltIO(boolean sync, long maxWaitNS) {
        this.ioExecutor.halt();
        this.saveExecutor.halt();
        this.compressionExecutor.halt();
        if (sync) {
            long time = System.nanoTime();
            long failures = 9L;
            while (true) {
                if (!(this.ioExecutor.isActive() || this.saveExecutor.isActive() || this.compressionExecutor.isActive())) {
                    return true;
                }
                if (System.nanoTime() - time >= maxWaitNS) {
                    return false;
                }
                failures = ConcurrentUtil.linearLongBackoff((long)failures, (long)500000L, (long)50000000L);
            }
        }
        return true;
    }

    /*
     * WARNING - Removed try catching itself - possible behaviour change.
     */
    public static void pushChunkWait(ServerLevel world, int chunkX, int chunkZ) {
        ArrayDeque<ChunkInfo> arrayDeque = WAITING_CHUNKS;
        synchronized (arrayDeque) {
            WAITING_CHUNKS.push(new ChunkInfo(chunkX, chunkZ, world));
        }
    }

    /*
     * WARNING - Removed try catching itself - possible behaviour change.
     */
    public static void popChunkWait() {
        ArrayDeque<ChunkInfo> arrayDeque = WAITING_CHUNKS;
        synchronized (arrayDeque) {
            WAITING_CHUNKS.pop();
        }
    }

    /*
     * WARNING - Removed try catching itself - possible behaviour change.
     */
    public static ChunkInfo[] getChunkInfos() {
        ArrayDeque<ChunkInfo> arrayDeque = WAITING_CHUNKS;
        synchronized (arrayDeque) {
            return WAITING_CHUNKS.toArray(new ChunkInfo[0]);
        }
    }

    private static JsonObject debugPlayer(ServerPlayer player) {
        ServerLevel world = player.level();
        JsonObject ret = new JsonObject();
        ret.addProperty("name", player.getScoreboardName());
        ret.addProperty("uuid", player.getUUID().toString());
        ret.addProperty("real", Boolean.valueOf(player.moonrise$isRealPlayer()));
        ret.addProperty("world-name", WorldUtil.getWorldName((Level)world));
        Vec3 pos = player.position();
        ret.addProperty("x", (Number)pos.x);
        ret.addProperty("y", (Number)pos.y);
        ret.addProperty("z", (Number)pos.z);
        Entity.RemovalReason removalReason = player.getRemovalReason();
        ret.addProperty("removal-reason", removalReason == null ? "null" : removalReason.name());
        ret.add("view-distances", (JsonElement)player.moonrise$getViewDistanceHolder().toJson());
        return ret;
    }

    public JsonObject getDebugJson() {
        JsonObject ret = new JsonObject();
        ret.addProperty("lock_shift", (Number)this.getChunkSystemLockShift());
        ret.addProperty("ticket_shift", (Number)6);
        ret.addProperty("region_shift", (Number)this.world.moonrise$getRegionChunkShift());
        ret.addProperty("name", WorldUtil.getWorldName((Level)this.world));
        ret.addProperty("view-distance", (Number)this.world.moonrise$getPlayerChunkLoader().getAPIViewDistance());
        ret.addProperty("tick-distance", (Number)this.world.moonrise$getPlayerChunkLoader().getAPITickDistance());
        ret.addProperty("send-distance", (Number)this.world.moonrise$getPlayerChunkLoader().getAPISendViewDistance());
        JsonArray players = new JsonArray();
        ret.add("players", (JsonElement)players);
        for (ServerPlayer player : this.world.players()) {
            players.add((JsonElement)ChunkTaskScheduler.debugPlayer(player));
        }
        ret.add("chunk-holder-manager", (JsonElement)this.chunkHolderManager.getDebugJson());
        return ret;
    }

    public static JsonObject debugAllWorlds(MinecraftServer server) {
        JsonObject ret = new JsonObject();
        ret.addProperty("data-version", (Number)2);
        JsonArray allPlayers = new JsonArray();
        ret.add("all-players", (JsonElement)allPlayers);
        for (ChunkInfo[] player : server.getPlayerList().getPlayers()) {
            allPlayers.add((JsonElement)ChunkTaskScheduler.debugPlayer((ServerPlayer)player));
        }
        JsonArray chunkWaitInfos = new JsonArray();
        ret.add("chunk-wait-infos", (JsonElement)chunkWaitInfos);
        for (ChunkInfo info : ChunkTaskScheduler.getChunkInfos()) {
            chunkWaitInfos.add((JsonElement)info.toJson());
        }
        JsonArray worlds = new JsonArray();
        ret.add("worlds", (JsonElement)worlds);
        for (ServerLevel world : server.getAllLevels()) {
            worlds.add((JsonElement)world.moonrise$getChunkTaskScheduler().getDebugJson());
        }
        return ret;
    }

    public static File getChunkDebugFile() {
        return new File(new File(new File("."), "debug"), "chunks-" + DateTimeFormatter.ofPattern("yyyy-MM-dd_HH.mm.ss").format(LocalDateTime.now()) + ".txt");
    }

    public static void dumpAllChunkLoadInfo(MinecraftServer server, boolean writeDebugInfo) {
        ChunkInfo[] chunkInfos = ChunkTaskScheduler.getChunkInfos();
        if (chunkInfos.length > 0) {
            LOGGER.error("Chunk wait task info below: ");
            for (ChunkInfo chunkInfo : chunkInfos) {
                NewChunkHolder holder = chunkInfo.world.moonrise$getChunkTaskScheduler().chunkHolderManager.getChunkHolder(chunkInfo.chunkX, chunkInfo.chunkZ);
                LOGGER.error("Chunk wait: " + String.valueOf(chunkInfo));
                LOGGER.error("Chunk holder: " + String.valueOf(holder));
            }
            if (writeDebugInfo) {
                File file = ChunkTaskScheduler.getChunkDebugFile();
                LOGGER.error("Writing chunk information dump to " + String.valueOf(file));
                try {
                    JsonUtil.writeJson((JsonElement)ChunkTaskScheduler.debugAllWorlds(server), (File)file);
                    LOGGER.error("Successfully written chunk information!");
                }
                catch (Throwable thr) {
                    LOGGER.error("Failed to dump chunk information to file " + file.toString(), thr);
                }
            }
        }
    }

    static {
        ChunkStatus.EMPTY.moonrise$setWriteRadius(0);
        ChunkStatus.STRUCTURE_STARTS.moonrise$setWriteRadius(0);
        ChunkStatus.STRUCTURE_REFERENCES.moonrise$setWriteRadius(0);
        ChunkStatus.BIOMES.moonrise$setWriteRadius(0);
        ChunkStatus.TERRAIN.moonrise$setWriteRadius(0);
        ChunkStatus.FEATURES.moonrise$setWriteRadius(1);
        ChunkStatus.INITIALIZE_LIGHT.moonrise$setWriteRadius(0);
        ChunkStatus.LIGHT.moonrise$setWriteRadius(2);
        ChunkStatus.SPAWN.moonrise$setWriteRadius(0);
        ChunkStatus.FULL.moonrise$setWriteRadius(0);
        ChunkStatus.EMPTY.moonrise$setEmptyLoadStatus(true);
        ChunkStatus.STRUCTURE_REFERENCES.moonrise$setEmptyLoadStatus(true);
        ChunkStatus.BIOMES.moonrise$setEmptyLoadStatus(true);
        ChunkStatus.TERRAIN.moonrise$setEmptyLoadStatus(true);
        ChunkStatus.FEATURES.moonrise$setEmptyLoadStatus(true);
        ChunkStatus.SPAWN.moonrise$setEmptyLoadStatus(true);
        List<ChunkStatus> parallelCapableStatus = Arrays.asList(ChunkStatus.EMPTY, ChunkStatus.STRUCTURE_STARTS, ChunkStatus.STRUCTURE_REFERENCES, ChunkStatus.BIOMES, ChunkStatus.TERRAIN, ChunkStatus.INITIALIZE_LIGHT);
        for (ChunkStatus status : parallelCapableStatus) {
            status.moonrise$setParallelCapable(true);
        }
        ACCESS_RADIUS_TABLE_LOAD = new int[ChunkStatus.getStatusList().size()];
        ACCESS_RADIUS_TABLE_GEN = new int[ChunkStatus.getStatusList().size()];
        ACCESS_RADIUS_TABLE = new int[ChunkStatus.getStatusList().size()];
        Arrays.fill(ACCESS_RADIUS_TABLE_LOAD, -1);
        Arrays.fill(ACCESS_RADIUS_TABLE_GEN, -1);
        Arrays.fill(ACCESS_RADIUS_TABLE, -1);
        List<ChunkStatus> statuses = ChunkStatus.getStatusList();
        int len = statuses.size();
        for (int i = 0; i < len; ++i) {
            ChunkStatus status = statuses.get(i);
            ChunkTaskScheduler.ACCESS_RADIUS_TABLE_LOAD[i] = ChunkTaskScheduler.getAccessRadius0(status, ChunkPyramid.LOADING_PYRAMID);
            ChunkTaskScheduler.ACCESS_RADIUS_TABLE_GEN[i] = ChunkTaskScheduler.getAccessRadius0(status, ChunkPyramid.GENERATION_PYRAMID);
            ChunkTaskScheduler.ACCESS_RADIUS_TABLE[i] = Math.max(ACCESS_RADIUS_TABLE_LOAD[i], ACCESS_RADIUS_TABLE_GEN[i]);
        }
        MAX_ACCESS_RADIUS = ACCESS_RADIUS_TABLE[ACCESS_RADIUS_TABLE.length - 1];
        WAITING_CHUNKS = new ArrayDeque();
    }

    public static final class ChunkInfo {
        public final int chunkX;
        public final int chunkZ;
        public final ServerLevel world;

        public ChunkInfo(int chunkX, int chunkZ, ServerLevel world) {
            this.chunkX = chunkX;
            this.chunkZ = chunkZ;
            this.world = world;
        }

        public JsonObject toJson() {
            JsonObject ret = new JsonObject();
            ret.addProperty("chunk-x", (Number)this.chunkX);
            ret.addProperty("chunk-z", (Number)this.chunkZ);
            ret.addProperty("world-name", WorldUtil.getWorldName((Level)this.world));
            return ret;
        }

        public String toString() {
            return "[( " + this.chunkX + "," + this.chunkZ + ") in '" + WorldUtil.getWorldName((Level)this.world) + "']";
        }
    }
}
