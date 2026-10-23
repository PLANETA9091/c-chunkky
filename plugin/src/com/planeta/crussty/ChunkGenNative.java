package com.planeta.crussty;

/**
 * JNI bridge to the native chunk generator (libcrussty_chunkgen.so).
 * ABI contract (see native/src/ffi.rs):
 *   newCtx(seed, minY, height, seaLevel) -> ctx pointer (0 on failure)
 *   freeCtx(ctx)
 *   fill(ctx, chunkX, chunkZ, short[16*16*height]) — zero-copy write into out[],
 *   layout idx = (y - minY)*256 + z*16 + x, u16 block ids per block_registry.txt.
 *   fillMarked(ctx, cx, cz, short[] out, long[] marks) -> int — fill + packed
 *   BlockPos list of aquifer fluid-update marks (vanilla
 *   shouldScheduleFluidUpdate positions whose written block holds a fluid);
 *   returns the count. The plugin feeds these into ProtoChunk
 *   markPosForPostprocessing so Paper runs the exact vanilla fluid ticks
 *   (flowing water, obsidian, bubble columns) after chunk conversion.
 */
public final class ChunkGenNative {
    static {
        // CRUSSTY module mode first: the module's cplugin_init writes
        // plugins/CrusstyGen.crussty-native (absolute path of the module .so,
        // which carries BOTH cplugin_init and every JNI native). System.load
        // binds the library to THIS class's loader — no java.library.path.
        // Standalone harness/probe runs (no marker) keep the classic
        // System.loadLibrary via -Djava.library.path.
        Throwable first = null;
        String marker = null;
        try {
            marker = moduleNativePath();
        } catch (Throwable ignored) {
        }
        boolean loaded = false;
        if (marker != null) {
            try {
                System.load(marker);
                loaded = true;
                System.err.println("[CrusstyGen] natives bound via CRUSSTY module: " + marker);
            } catch (Throwable t) {
                first = t;
            }
        }
        if (!loaded) {
            try {
                System.loadLibrary("crussty_chunkgen");
            } catch (Throwable t) {
                if (first != null) t.addSuppressed(first);
                throw t;
            }
        }
    }

    /** Marker file next to this plugin jar -> absolute path of the module
     *  .so (written by the CRUSSTY module bootstrap at boot). Null when
     *  absent — standalone deployment. */
    private static String moduleNativePath() {
        try {
            var cs = ChunkGenNative.class.getProtectionDomain().getCodeSource();
            if (cs == null || cs.getLocation() == null) return null;
            var jar = java.nio.file.Path.of(cs.getLocation().toURI());
            var marker = jar.getParent() == null ? null
                    : jar.getParent().resolve("CrusstyGen.crussty-native");
            if (marker != null && java.nio.file.Files.isRegularFile(marker)) {
                String p = java.nio.file.Files.readString(marker).trim();
                return p.isEmpty() ? null : p;
            }
        } catch (Throwable ignored) {
        }
        return null;
    }

    public static native long newCtx(long seed, int minY, int height, int seaLevel);

    /**
     * newCtxAssets(seed, seaLevel, paths, blobs) -> ctx id (0 on parse
     * failure). Builds the worldgen registry from world-datapack asset
     * overrides: paths are repo-relative "<ns>/worldgen/..." (the leading
     * "data/" of datapack entries is stripped by the caller), later entries
     * must already be merged (last write wins by the caller). On 0 the caller
     * falls back to newCtx (embedded vanilla).
     */
    public static native long newCtxAssets(long seed, int seaLevel, String[] paths, byte[][] blobs);

    public static native void freeCtx(long ctx);

    public static native void fill(long ctx, int chunkX, int chunkZ, short[] out);

    /**
     * ctxBiomeNames(ctx) -> String[] — deduplicated biome names of the native
     * multi_noise source (stable index order used by fillBiomes), or null
     * when the ctx has no native biome source (embedded-vanilla worldgen).
     */
    public static native String[] ctxBiomeNames(long ctx);

    /**
     * fillBiomes(ctx, cx, cz, out) -> int — writes the chunk's biome quart
     * indexes (indexes into the ctxBiomeNames table, NOT registry ids) in
     * the vanilla fillBiomesFromNoise order: out[s*64 + x*16 + y*4 + z],
     * sections ascending (height/16 sections x 4x4x4 quarts). Returns the
     * count written, or -1 when the ctx has no native biome source.
     */
    public static native int fillBiomes(long ctx, int cx, int cz, int[] out);

    /** surfaceStates(ctx) -> String[] — block state strings of the parsed
     *  surface rule tree (first-appearance order), or null when the ctx has
     *  no native surface engine. */
    public static native String[] surfaceStates(long ctx);

    /** surfaceSetIds(ctx, ids) -> 0 on success — assigns the JVM material id
     *  for every state string (append order = string order) and builds the
     *  native bandlands table. */
    public static native int surfaceSetIds(long ctx, short[] ids);

    /** surfaceSetTemps(ctx, temps, frozen) -> 0 on success — per native
     *  biome index: base temperature + FROZEN modifier flag. */
    public static native int surfaceSetTemps(long ctx, float[] temps, boolean[] frozen);

    // ------------------------------------------------------------------
    // Stage F1 — native feature decoration engine (see features_jni.rs).
    // Protocol: decoStates/decoPlan at world load; per chunk decoDecoSeed
    // once + decoChunkPlan for the (step, index) runs; per run decoRunFeature
    // with DecoLevelBridge upcalls, ops written with setBlock(pos, state, 2).
    // ------------------------------------------------------------------

    /** decoStates(ctx) -> String[] — distinct state strings of the decoded
     *  feature registry (ops reference them by index), or null when the ctx
     *  has no native deco engine. */
    public static native String[] decoStates(long ctx);

    /** decoPlan(ctx) -> String[][] — featuresPerStep in FeatureSorter order
     *  (index within a step = the vanilla per-step feature seed index i3). */
    public static native String[][] decoPlan(long ctx);

    /** decoChunkPlan(ctx, regionBiomes, out) -> int runs — union of the
     *  region biomes' features mapped to (step, index) runs, sorted per
     *  step. out[2k] = step, out[2k+1] = index. Returns -1 when out is too
     *  small (size it to sum(plan[step].length) * 2 + 16). */
    public static native int decoChunkPlan(long ctx, String[] regionBiomes, int[] out);

    /** decoRunFeature(ctx, decoSeed, paperSeed, originX, originY, originZ,
     *  maxY, step, gid, bridge, ops) -> int opCount. Expands one placed
     *  feature with the exact vanilla seeding (paperSeed -1 = no Paper
     *  featureSeeds override). ops[5k..+5] = xRel, yAbs, zRel, stateIdx,
     *  flags (bit0 schedule_tick, bit1 rotated + axis in bits 2-3). Level
     *  queries upcall into the bridge object (DecoDriver). */
    public static native int decoRunFeature(long ctx, long decoSeed, long paperSeed,
            int originX, int originY, int originZ, int maxY, int step, int gid,
            Object bridge, int[] ops);

    /** decoDecoSeed(levelSeed, originX, originZ) -> long — the vanilla chunk
     *  decoration seed `l` (setDecorationSeed; every feature re-seeds from it). */
    public static native long decoDecoSeed(long levelSeed, int originX, int originZ);

    /** decoChunkBegin(ctx, cx, cz, minSectionY) — reset the chunk-scoped
     *  feature-write overlay + lazy 3x3 section snapshot before the feature
     *  loop. Must precede the decoRunFeature calls of this chunk on the same
     *  ctx (the populate threads hold exclusive ctx locks). */
    public static native void decoChunkBegin(long ctx, int cx, int cz, int minSectionY,
            long biomeZoomSeed);

    /** decoOpsDropped(ctx, coords, count) — ops the Java apply loop DROPPED
     *  (postproc marks, unparseable states, out-of-range sections): remove
     *  their pending-overlay entries so the chunk overlay stays exact. */
    public static native void decoOpsDropped(long ctx, int[] coords, int count);

    /** decoDelegateMark(ctx) — a vanilla delegate feature ran and wrote via
     *  the region directly; bumps the snapshot epoch (stale sections reload)
     *  AND drops the cached WG heightmaps (the region maps moved — re-handshake). */
    public static native void decoDelegateMark(long ctx);

    /** decoSetFlags(ctx, flags) — one byte per decoStates entry
     *  (bit0=isAir, bit1=blocksMotion). Feeds the native WG heightmap
     *  update-on-write (vanilla Heightmap.update bookkeeping on buffered ops). */
    public static native void decoSetFlags(long ctx, byte[] flags);

    /** decoBiomeFeaturesFull(ctx, biomeNames, joinedRefs) — the FULL pack
     *  biome -> placed-refs union (BiomeFilter native path). joinedRefs[i]
     *  is \u0001-separated. Must run after decoStates/decoPlan at world load. */
    public static native void decoBiomeFeaturesFull(long ctx, String[] biomeNames, String[] joinedRefs);

    public static native int fillMarked(long ctx, int chunkX, int chunkZ, short[] out, long[] marks);

    /**
     * fillBeard(ctx, cx, cz, short[] out, long[] marks, int[] beard) -> int.
     * Atomic set_beard + fillMarked: `beard` is the plugin-serialized vanilla
     * Beardifier list for THIS chunk:
     * [nPieces, (minX,minY,minZ,maxX,maxY,maxZ,adj,delta)*, nJunctions, (x,y,z)*].
     * Pass [0,0] for no structures (must never be null — the ctx pool reuses
     * generators and a stale beard would leak into the next chunk).
     */
    public static native int fillBeard(long ctx, int chunkX, int chunkZ, short[] out, long[] marks, int[] beard);

    /**
     * baseHeight(ctx, x, z, mode) -> int — native replica of vanilla
     * NoiseBasedChunkGenerator.getBaseHeight (iterateNoiseColumn, 1-cell
     * NoiseChunk walk, always beard-less). mode 0 = WORLD_SURFACE_WG /
     * WORLD_SURFACE (stop at the first non-air state), mode 1 =
     * OCEAN_FLOOR_WG / OCEAN_FLOOR (stop at the first blocksMotion state).
     * Returns Integer.MIN_VALUE when the ctx is unknown or the walk failed —
     * the caller must fall back to the NMS delegate (vanilla) path.
     */
    public static native int baseHeight(long ctx, int x, int z, int mode);

    /** Global per-fill phase snapshot from the native (JNI diagnostics). */
    public static native String profStatsGlobal();

    private ChunkGenNative() {
    }
}
