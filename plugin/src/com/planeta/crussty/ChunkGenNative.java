package com.planeta.crussty;

/**
 * JNI bridge to the native chunk generator (libcrussty_chunkgen.so).
 * Frozen ABI contract (see native/src/lib.rs):
 *   newCtx(seed, minY, height) -> ctx pointer (0 on failure)
 *   freeCtx(ctx)
 *   fill(ctx, chunkX, chunkZ, short[16*16*height]) — zero-copy write into out[],
 *   layout idx = (y - minY)*256 + z*16 + x, u16 block ids per block_registry.txt.
 */
public final class ChunkGenNative {
    static {
        System.loadLibrary("crussty_chunkgen");
    }

    public static native long newCtx(long seed, int minY, int height);

    public static native void freeCtx(long ctx);

    public static native void fill(long ctx, int chunkX, int chunkZ, short[] out);

    private ChunkGenNative() {
    }
}
