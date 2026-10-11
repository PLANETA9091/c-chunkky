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

    public static native void freeCtx(long ctx);

    public static native void fill(long ctx, int chunkX, int chunkZ, short[] out);

    public static native int fillMarked(long ctx, int chunkX, int chunkZ, short[] out, long[] marks);

    private ChunkGenNative() {
    }
}
