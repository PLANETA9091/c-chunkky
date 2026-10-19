package com.planeta.crussty.agent;

import java.lang.instrument.ClassFileTransformer;
import java.lang.instrument.Instrumentation;
import java.security.ProtectionDomain;

/**
 * Vanilla-lane trace agent: instruments two NMS methods so the reference run
 * prints the exact heightmap call/update stream during decoration:
 *   - WorldGenRegion.getHeight(Heightmap$Types,II)I  -> "[hgt] ty=.. x=.. z=.. -> .."
 *   - Heightmap.update(IIILBlockState;)Z             -> "[hupd] ..."
 * Activated only on the vanilla reference lane via -javaagent.
 */
public final class VanTraceAgent {

    public static void premain(String args, Instrumentation inst) {
        inst.addTransformer(new ClassFileTransformer() {
            @Override
            public byte[] transform(ClassLoader loader, String className, Class<?> beingDefined,
                                    ProtectionDomain pd, byte[] classfileBuffer) {
                try {
                    if ("net/minecraft/server/level/WorldGenRegion".equals(className)) {
                        return WorldGenRegionWeaver.weave(classfileBuffer);
                    }
                    if ("net/minecraft/world/level/levelgen/Heightmap".equals(className)) {
                        return HeightmapWeaver.weave(classfileBuffer);
                    }
                } catch (Throwable t) {
                    System.err.println("[vantrace] weave failed for " + className + ": " + t);
                }
                return null;
            }
        }, false);
    }
}
