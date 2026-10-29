package com.planeta.crussty.agent;

import org.objectweb.asm.ClassReader;
import org.objectweb.asm.ClassVisitor;
import org.objectweb.asm.ClassWriter;
import org.objectweb.asm.MethodVisitor;
import org.objectweb.asm.Opcodes;

/**
 * Instruments ConfiguredFeature.place(WorldGenLevel,ChunkGenerator,RandomSource,BlockPos)Z:
 * prints "[cf] <configuredName> origin=(x,y,z)" at method head, then delegates
 * to the original body. Tells us exactly which origins vanilla runs features at.
 */
public final class ConfiguredFeatureWeaver {

    static byte[] weave(byte[] src) {
        ClassReader cr = new ClassReader(src);
        ClassWriter cw = new ClassWriter(cr, ClassWriter.COMPUTE_MAXS);
        cr.accept(new CV(cw), 0);
        return cw.toByteArray();
    }

    private static final class CV extends ClassVisitor implements Opcodes {
        CV(ClassVisitor next) {
            super(ASM9, next);
        }

        @Override
        public MethodVisitor visitMethod(int access, String name, String desc, String sig, String[] exs) {
            MethodVisitor mv = super.visitMethod(access, name, desc, sig, exs);
            if ("place".equals(name)
                    && "(Lnet/minecraft/world/level/WorldGenLevel;Lnet/minecraft/world/level/chunk/ChunkGenerator;Lnet/minecraft/util/RandomSource;Lnet/minecraft/core/BlockPos;)Z".equals(desc)) {
                return new MV(mv);
            }
            return mv;
        }
    }

    /** head: Hooks.cf(this, pos) then fall through to the body. slots: 0=this, 4=pos. */
    private static final class MV extends MethodVisitor implements Opcodes {
        MV(MethodVisitor next) {
            super(ASM9, next);
        }

        @Override
        public void visitCode() {
            super.visitCode();
            super.visitVarInsn(ALOAD, 0);
            super.visitVarInsn(ALOAD, 4);
            super.visitMethodInsn(INVOKESTATIC,
                    "com/planeta/crussty/agent/Hooks",
                    "cf", "(Ljava/lang/Object;Ljava/lang/Object;)V", false);
        }
    }
}
