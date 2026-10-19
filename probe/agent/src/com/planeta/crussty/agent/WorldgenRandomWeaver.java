package com.planeta.crussty.agent;

import org.objectweb.asm.ClassReader;
import org.objectweb.asm.ClassVisitor;
import org.objectweb.asm.ClassWriter;
import org.objectweb.asm.MethodVisitor;
import org.objectweb.asm.Opcodes;

/**
 * Instruments WorldgenRandom seeding (vanilla-lane RNG slot proof):
 *   - setDecorationSeed(JII)J prints "[dseed] lvl=<l> x=<x> z=<z> -> <ret>"
 *   - setFeatureSeed(JII)V          prints "[fseed] deco=<d> idx=<i> step=<s>"
 */
public final class WorldgenRandomWeaver {

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
            if ("setDecorationSeed".equals(name) && "(JII)J".equals(desc)) {
                return new DSeedMV(mv);
            }
            if ("setFeatureSeed".equals(name) && "(JII)V".equals(desc)) {
                return new FSeedMV(mv);
            }
            return mv;
        }
    }

    /** setDecorationSeed: wrap LRETURN. slots: 0=this, 1=levelSeed, 2=x, 3=z. */
    private static final class DSeedMV extends MethodVisitor implements Opcodes {
        DSeedMV(MethodVisitor next) {
            super(ASM9, next);
        }

        @Override
        public void visitInsn(int opcode) {
            if (opcode == LRETURN) {
                super.visitInsn(DUP2);
                super.visitVarInsn(LLOAD, 1);
                super.visitVarInsn(ILOAD, 3);
                super.visitVarInsn(ILOAD, 4);
                super.visitMethodInsn(INVOKESTATIC,
                        "com/planeta/crussty/agent/Hooks",
                        "dseed", "(JJII)V", false);
            }
            super.visitInsn(opcode);
        }
    }

    /** setFeatureSeed: prefix print (VOID method: print at head, before the body). */
    private static final class FSeedMV extends MethodVisitor implements Opcodes {
        FSeedMV(MethodVisitor next) {
            super(ASM9, next);
        }

        @Override
        public void visitCode() {
            super.visitCode();
            super.visitVarInsn(LLOAD, 1);
            super.visitVarInsn(ILOAD, 3);
            super.visitVarInsn(ILOAD, 4);
            super.visitMethodInsn(INVOKESTATIC,
                    "com/planeta/crussty/agent/Hooks",
                    "fseed", "(JII)V", false);
        }
    }
}
