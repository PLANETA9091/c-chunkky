package com.planeta.crussty.agent;

import org.objectweb.asm.ClassReader;
import org.objectweb.asm.ClassVisitor;
import org.objectweb.asm.ClassWriter;
import org.objectweb.asm.MethodVisitor;
import org.objectweb.asm.Opcodes;

/**
 * Instruments Heightmap:
 *   - constructor calls Hooks.hmNew(this, types) at RETURN
 *   - update(IIILBlockState;)Z calls Hooks.hupd(ret, this, x, y, z, state) at each IRETURN
 */
public final class HeightmapWeaver {

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
            if ("<init>".equals(name)
                    && "(Lnet/minecraft/world/level/chunk/ChunkAccess;Lnet/minecraft/world/level/levelgen/Heightmap$Types;)V".equals(desc)) {
                return new CtorMV(mv);
            }
            if ("update".equals(name) && "(IIILnet/minecraft/world/level/block/state/BlockState;)Z".equals(desc)) {
                return new UpdateMV(mv);
            }
            return mv;
        }
    }

    private static final class CtorMV extends MethodVisitor implements Opcodes {
        CtorMV(MethodVisitor next) {
            super(ASM9, next);
        }

        @Override
        public void visitInsn(int opcode) {
            if (opcode == RETURN) {
                super.visitVarInsn(ALOAD, 0);
                super.visitVarInsn(ALOAD, 2);
                super.visitMethodInsn(INVOKESTATIC,
                        "com/planeta/crussty/agent/Hooks",
                        "hmNew", "(Ljava/lang/Object;Ljava/lang/Object;)V", false);
            }
            super.visitInsn(opcode);
        }
    }

    private static final class UpdateMV extends MethodVisitor implements Opcodes {
        UpdateMV(MethodVisitor next) {
            super(ASM9, next);
        }

        @Override
        public void visitInsn(int opcode) {
            if (opcode == IRETURN) {
                super.visitInsn(DUP);
                super.visitVarInsn(ALOAD, 0);
                super.visitVarInsn(ILOAD, 1);
                super.visitVarInsn(ILOAD, 2);
                super.visitVarInsn(ILOAD, 3);
                super.visitVarInsn(ALOAD, 4);
                super.visitMethodInsn(INVOKESTATIC,
                        "com/planeta/crussty/agent/Hooks",
                        "hupd", "(ILjava/lang/Object;IIILjava/lang/Object;)V", false);
            }
            super.visitInsn(opcode);
        }
    }
}
