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
        System.loadLibrary("crussty_chunkgen");
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

    /** Global per-fill phase snapshot from the native (JNI diagnostics). */
    public static native String profStatsGlobal();

    private ChunkGenNative() {
    }
}
