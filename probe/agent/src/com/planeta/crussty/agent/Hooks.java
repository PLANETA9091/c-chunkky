package com.planeta.crussty.agent;

/**
 * Static print hooks for the vanilla-lane trace agent. Called from woven NMS
 * bytecode; NMS types are passed as Object to avoid link-time deps. Assumes
 * the agent jar is on the same classloader as the NMS classes (paperclip
 * repacks NMS into the system loader — verify fast at boot).
 */
public final class Hooks {

    public static void hgt(int ret, Object types, int x, int z) {
        System.out.println("[hgt] ty=" + name(types) + " x=" + x + " z=" + z + " -> " + ret);
    }

    public static void hupd(int ret, Object hm, int x, int y, int z, Object state) {
        System.out.println("[hupd] id=" + System.identityHashCode(hm) + " r=" + (ret != 0)
                + " (" + x + "," + y + "," + z + ") " + state);
    }

    public static void hmNew(Object hm, Object types) {
        System.out.println("[hm-new] id=" + System.identityHashCode(hm) + " type=" + name(types));
    }

    private static String name(Object types) {
        try {
            return (String) types.getClass().getMethod("getSerializedName").invoke(types);
        } catch (Throwable t) {
            return String.valueOf(types);
        }
    }

    private Hooks() {
    }
}
