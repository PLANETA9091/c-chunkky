package com.planeta.crussty.oracle;

import org.bukkit.Bukkit;
import org.bukkit.craftbukkit.CraftServer;
import org.bukkit.plugin.java.JavaPlugin;

import java.io.BufferedWriter;
import java.io.IOException;
import java.lang.reflect.Field;
import java.nio.file.Files;
import java.nio.file.Path;

/**
 * Dumps the full internal state of the live BlendedNoise (terrain) instance:
 * min/max/main PerlinNoise octaves with xo/yo/zo and the full p[256] table.
 * Line format (native bench "blended" mode mirrors it):
 *   B <which> <octaveIndex> xo=<f64bits> yo=<f64bits> zo=<f64bits> p=<512 hex>
 */
public final class OracleBlendPlugin extends JavaPlugin {

    @Override
    public void onEnable() {
        try {
            run();
        } catch (Exception e) {
            throw new RuntimeException(e);
        }
        getLogger().info("OracleBlend done");
    }

    private void run() throws Exception {
        CraftServer cs = (CraftServer) Bukkit.getServer();
        Object level = cs.getServer().getClass().getMethod("overworld").invoke(cs.getServer());
        Object cache = level.getClass().getMethod("getChunkSource").invoke(level);
        Object state = cache.getClass().getMethod("randomState").invoke(cache);
        Object router = state.getClass().getMethod("router").invoke(state);
        Object fd = router.getClass().getMethod("finalDensity").invoke(router);

        Object blended = findBlended(fd, 0);
        if (blended == null) throw new IllegalStateException("BlendedNoise not found in final_density");

        Path out = Path.of("oracle_blend.txt");
        try (BufferedWriter w = Files.newBufferedWriter(out)) {
            for (String which : new String[] {"minLimitNoise", "maxLimitNoise", "mainNoise"}) {
                Field f = blended.getClass().getDeclaredField(which);
                f.setAccessible(true);
                Object perlin = f.get(blended);
                Field levelsF = perlin.getClass().getDeclaredField("noiseLevels");
                levelsF.setAccessible(true);
                Object[] levels = (Object[]) levelsF.get(perlin);
                for (int i = 0; i < levels.length; i++) {
                    Object oct = levels[i];
                    if (oct == null) {
                        w.write(String.format("B %s %d null%n", which, i));
                        continue;
                    }
                    w.write(stateLine(which, i, oct));
                }
            }
        }
    }

    private String stateLine(String which, int i, Object oct) throws Exception {
        double xo = getD(oct, "xo");
        double yo = getD(oct, "yo");
        double zo = getD(oct, "zo");
        Field pf = oct.getClass().getDeclaredField("p");
        pf.setAccessible(true);
        byte[] p = (byte[]) pf.get(oct);
        StringBuilder hex = new StringBuilder(512);
        for (byte b : p) {
            hex.append(String.format("%02x", b));
        }
        return String.format("B %s %d xo=%016x yo=%016x zo=%016x p=%s%n",
                which, i, Double.doubleToLongBits(xo), Double.doubleToLongBits(yo),
                Double.doubleToLongBits(zo), hex);
    }

    private static double getD(Object o, String name) throws Exception {
        Field f = o.getClass().getDeclaredField(name);
        f.setAccessible(true);
        return f.getDouble(o);
    }

    private Object findBlended(Object node, int depth) throws Exception {
        if (node == null || depth > 40) return null;
        if (node.getClass().getSimpleName().equals("BlendedNoise")) return node;
        for (String m : new String[] {"wrapped", "input", "argument1", "argument2", "whenInRange",
                "whenOutOfRange", "shiftX", "shiftY", "shiftZ", "function"}) {
            try {
                java.lang.reflect.Method meth = node.getClass().getMethod(m);
                meth.setAccessible(true);
                Object child = meth.invoke(node);
                if (child instanceof net.minecraft.core.Holder<?> holder) {
                    child = holder.value();
                }
                Object r = findBlended(child, depth + 1);
                if (r != null) return r;
            } catch (NoSuchMethodException ignored) {
            }
        }
        return null;
    }
}
