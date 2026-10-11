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
 * Byte-exact octave-by-octave trace of BlendedNoise.compute for the points in
 * oracle_misses.txt. Mirrors the decompiled compute exactly; prints every
 * intermediate as f64 bits.
 */
public final class OracleBlendCalc extends JavaPlugin {

    private Object blended;

    @Override
    public void onEnable() {
        try {
            CraftServer cs = (CraftServer) Bukkit.getServer();
            Object level = cs.getServer().getClass().getMethod("overworld").invoke(cs.getServer());
            Object cache = level.getClass().getMethod("getChunkSource").invoke(level);
            Object state = cache.getClass().getMethod("randomState").invoke(cache);
            Object router = state.getClass().getMethod("router").invoke(state);
            Object fd = router.getClass().getMethod("finalDensity").invoke(router);
            blended = find(fd, 0);
            if (blended == null) throw new IllegalStateException("no BlendedNoise");
            run();
        } catch (Exception e) {
            throw new RuntimeException(e);
        }
        getLogger().info("OracleBlendCalc done");
    }

    private void run() throws Exception {
        double xzMultiplier = getD(blended, "xzMultiplier");
        double yMultiplier = getD(blended, "yMultiplier");
        double xzFactor = getD(blended, "xzFactor");
        double yFactor = getD(blended, "yFactor");
        double smear = getD(blended, "smearScaleMultiplier");
        Object minN = getO(blended, "minLimitNoise");
        Object maxN = getO(blended, "maxLimitNoise");
        Object mainN = getO(blended, "mainNoise");

        Path out = Path.of("oracle_blendcalc.txt");
        try (BufferedWriter w = Files.newBufferedWriter(out)) {
            for (String line : Files.readAllLines(Path.of("oracle_misses.txt"))) {
                if (line.isBlank()) continue;
                String[] p = line.trim().split("\\s+");
                int bx = Integer.parseInt(p[0]), by = Integer.parseInt(p[1]), bz = Integer.parseInt(p[2]);
                w.write(String.format("PT %d %d %d%n", bx, by, bz));
                trace(w, bx, by, bz, xzMultiplier, yMultiplier, xzFactor, yFactor, smear, minN, maxN, mainN);
            }
        }
    }

    private static double wrap(double value) {
        long l = (long) (value / 3.3554432E7 + 0.5); // Mth.lfloor
        return value - l * 3.3554432E7;
    }

    private static long lfloor(double value) {
        long l = (long) value;
        return value < l ? l - 1L : l;
    }

    private void trace(BufferedWriter w, int blockX, int blockY, int blockZ,
                       double xzM, double yM, double xzF, double yF, double smear,
                       Object minN, Object maxN, Object mainN) throws Exception {
        double d = blockX * xzM;
        double d1 = blockY * yM;
        double d2 = blockZ * xzM;
        double d3 = d / xzF;
        double d4 = d1 / yF;
        double d5 = d2 / xzF;
        double d6 = yM * smear;
        double d7 = d6 / yF;
        w.write(String.format("d %016x d1 %016x d2 %016x d3 %016x d4 %016x d5 %016x d6 %016x d7 %016x%n",
                Double.doubleToLongBits(d), Double.doubleToLongBits(d1), Double.doubleToLongBits(d2),
                Double.doubleToLongBits(d3), Double.doubleToLongBits(d4), Double.doubleToLongBits(d5),
                Double.doubleToLongBits(d6), Double.doubleToLongBits(d7)));
        double d8 = 0.0, d9 = 0.0, d10 = 0.0, d11 = 1.0;
        for (int i = 0; i < 8; i++) {
            Object oct = octave(mainN, i);
            if (oct != null) {
                double v = noise5(oct, wrap(d3 * d11), wrap(d4 * d11), wrap(d5 * d11), d7 * d11, d4 * d11);
                d10 += v / d11;
                w.write(String.format("main %d %016x%n", i, Double.doubleToLongBits(v)));
            } else {
                w.write(String.format("main %d null%n", i));
            }
            d11 /= 2.0;
        }
        double d12 = (d10 / 10.0 + 1.0) / 2.0;
        boolean flag1 = d12 >= 1.0;
        boolean flag2 = d12 <= 0.0;
        w.write(String.format("d10 %016x d12 %016x flag1 %d flag2 %d%n",
                Double.doubleToLongBits(d10), Double.doubleToLongBits(d12), flag1 ? 1 : 0, flag2 ? 1 : 0));
        d11 = 1.0;
        for (int i1 = 0; i1 < 16; i1++) {
            double d13 = wrap(d * d11);
            double d14 = wrap(d1 * d11);
            double d15 = wrap(d2 * d11);
            double d16 = d6 * d11;
            if (!flag1) {
                Object oct = octave(minN, i1);
                if (oct != null) {
                    double v = noise5(oct, d13, d14, d15, d16, d1 * d11);
                    d8 += v / d11;
                    w.write(String.format("min %d %016x%n", i1, Double.doubleToLongBits(v)));
                } else {
                    w.write(String.format("min %d null%n", i1));
                }
            }
            if (!flag2) {
                Object oct = octave(maxN, i1);
                if (oct != null) {
                    double v = noise5(oct, d13, d14, d15, d16, d1 * d11);
                    d9 += v / d11;
                    w.write(String.format("max %d %016x%n", i1, Double.doubleToLongBits(v)));
                } else {
                    w.write(String.format("max %d null%n", i1));
                }
            }
            d11 /= 2.0;
        }
        w.write(String.format("d8 %016x d9 %016x result %016x%n",
                Double.doubleToLongBits(d8), Double.doubleToLongBits(d9),
                Double.doubleToLongBits(clampedLerp(d8 / 512.0, d9 / 512.0, d12) / 128.0)));
    }

    private static double clampedLerp(double a, double b, double t) {
        if (t < 0.0) return a;
        return t > 1.0 ? b : a + t * (b - a);
    }

    private double noise5(Object oct, double x, double y, double z, double yScale, double yMax) throws Exception {
        double xo = getD(oct, "xo"), yo = getD(oct, "yo"), zo = getD(oct, "zo");
        Field pf = oct.getClass().getDeclaredField("p");
        pf.setAccessible(true);
        byte[] p = (byte[]) pf.get(oct);
        double d0 = x + xo, d1 = y + yo, d2 = z + zo;
        int fx = floor(d0), fy = floor(d1), fz = floor(d2);
        double d3 = d0 - fx, d4 = d1 - fy, d5 = d2 - fz;
        double d7;
        if (yScale != 0.0) {
            double d6 = (yMax >= 0.0 && yMax < d4) ? yMax : d4;
            d7 = floor(d6 / yScale + 1.0E-7F) * yScale;
        } else {
            d7 = 0.0;
        }
        return sampleAndLerp(p, fx, fy, fz, d3, d4 - d7, d5, d4);
    }

    private double sampleAndLerp(byte[] p, int gx, int gy, int gz, double dx, double wdy, double dz, double dy) {
        int i = p(gx, p), i1 = p(gx + 1, p);
        int i2 = p(i + gy, p), i3 = p(i + gy + 1, p), i4 = p(i1 + gy, p), i5 = p(i1 + gy + 1, p);
        double d0 = grad(p, i2 + gz, dx, wdy, dz);
        double d1 = grad(p, i4 + gz, dx - 1, wdy, dz);
        double d2 = grad(p, i3 + gz, dx, wdy - 1, dz);
        double d3 = grad(p, i5 + gz, dx - 1, wdy - 1, dz);
        double d4 = grad(p, i2 + gz + 1, dx, wdy, dz - 1);
        double d5 = grad(p, i4 + gz + 1, dx - 1, wdy, dz - 1);
        double d6 = grad(p, i3 + gz + 1, dx, wdy - 1, dz - 1);
        double d7 = grad(p, i5 + gz + 1, dx - 1, wdy - 1, dz - 1);
        double d8 = smooth(dx), d9 = smooth(dy), d10 = smooth(dz);
        return lerp(d10, lerp(d9, lerp(d8, d0, d1), lerp(d8, d2, d3)), lerp(d9, lerp(d8, d4, d5), lerp(d8, d6, d7)));
    }

    private static int p(int idx, byte[] p) {
        return p[idx & 0xFF] & 0xFF;
    }

    private static final int[][] GRAD = {
            {1, 1, 0}, {-1, 1, 0}, {1, -1, 0}, {-1, -1, 0},
            {1, 0, 1}, {-1, 0, 1}, {1, 0, -1}, {-1, 0, -1},
            {0, 1, 1}, {0, -1, 1}, {0, 1, -1}, {0, -1, -1},
            {1, 1, 0}, {0, -1, 1}, {-1, 1, 0}, {0, -1, -1}};

    private static double grad(byte[] p, int idx, double x, double y, double z) {
        int[] g = GRAD[p(idx, p) & 15];
        return g[0] * x + g[1] * y + g[2] * z;
    }

    private static double smooth(double v) {
        return v * v * v * (v * (v * 6.0 - 15.0) + 10.0);
    }

    private static double lerp(double t, double a, double b) {
        return a + t * (b - a);
    }

    private static int floor(double value) {
        int i = (int) value;
        return value < i ? i - 1 : i;
    }

    private Object octave(Object perlin, int idx) throws Exception {
        Field lf = perlin.getClass().getDeclaredField("noiseLevels");
        lf.setAccessible(true);
        Object[] levels = (Object[]) lf.get(perlin);
        return levels[levels.length - 1 - idx];
    }

    private Object find(Object node, int depth) throws Exception {
        if (node == null || depth > 40) return null;
        if (node.getClass().getSimpleName().equals("BlendedNoise")) return node;
        for (String m : new String[] {"wrapped", "input", "argument1", "argument2", "whenInRange",
                "whenOutOfRange", "shiftX", "shiftY", "shiftZ", "function"}) {
            try {
                java.lang.reflect.Method meth = node.getClass().getMethod(m);
                meth.setAccessible(true);
                Object child = meth.invoke(node);
                if (child instanceof net.minecraft.core.Holder<?> holder) child = holder.value();
                Object r = find(child, depth + 1);
                if (r != null) return r;
            } catch (NoSuchMethodException ignored) {
            }
        }
        return null;
    }

    private static double getD(Object o, String name) throws Exception {
        Field f = o.getClass().getDeclaredField(name);
        f.setAccessible(true);
        return f.getDouble(o);
    }

    private static Object getO(Object o, String name) throws Exception {
        Field f = o.getClass().getDeclaredField(name);
        f.setAccessible(true);
        return f.get(o);
    }
}
