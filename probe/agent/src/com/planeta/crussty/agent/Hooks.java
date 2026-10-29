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


    public static void dseed(long ret, long levelSeed, int x, int z) {
        System.out.println("[dseed] lvl=" + levelSeed + " x=" + x + " z=" + z + " -> " + ret);
    }

    public static void fseed(long deco, int index, int step) {
        System.out.println("[fseed] deco=" + deco + " idx=" + index + " step=" + step);
    }


    public static void cf(Object configured, Object pos) {
        // ConfiguredFeature.toString() = "Configured: <feature class>: <config>"
        // BlockPos.toString() = "BlockPos{x=.., y=.., z=..}"
        String p = String.valueOf(pos);
        // keep only x/y/z to limit log size
        int a = p.indexOf('{');
        int b = p.indexOf('}');
        System.out.println("[cf] " + String.valueOf(configured).substring(0, Math.min(60, String.valueOf(configured).length()))
                + " origin=" + (a >= 0 && b > a ? p.substring(a, b + 1) : p));
    }

    private Hooks() {
    }
}
