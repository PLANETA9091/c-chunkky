package com.planeta.crussty.agent;

import org.objectweb.asm.ClassReader;
import org.objectweb.asm.ClassVisitor;
import org.objectweb.asm.ClassWriter;
import org.objectweb.asm.MethodVisitor;
import org.objectweb.asm.Opcodes;

/**
 * Instruments WorldGenRegion.getHeight(Heightmap$Types,II)I:
 * every ireturn calls Hooks.hgt(ret, types, x, z).
 */
public final class WorldGenRegionWeaver {

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
            if ("getHeight".equals(name)
                    && "(Lnet/minecraft/world/level/levelgen/Heightmap$Types;II)I".equals(desc)) {
                return new MV(mv);
            }
            return mv;
        }
    }

    private static final class MV extends MethodVisitor implements Opcodes {
        MV(MethodVisitor next) {
            super(ASM9, next);
        }

        @Override
        public void visitInsn(int opcode) {
            if (opcode == IRETURN) {
                // stack: ret. Call Hooks.hgt(ret, types, x, z) then return.
                super.visitInsn(DUP);
                super.visitVarInsn(ALOAD, 1);
                super.visitVarInsn(ILOAD, 2);
                super.visitVarInsn(ILOAD, 3);
                super.visitMethodInsn(INVOKESTATIC,
                        "com/planeta/crussty/agent/Hooks",
                        "hgt", "(ILjava/lang/Object;II)V", false);
            }
            super.visitInsn(opcode);
        }
    }
}
