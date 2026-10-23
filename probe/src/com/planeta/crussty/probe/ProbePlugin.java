package com.planeta.crussty.probe;

import org.bukkit.Bukkit;
import org.bukkit.Chunk;
import org.bukkit.ChunkSnapshot;
import org.bukkit.World;
import org.bukkit.block.data.BlockData;
import org.bukkit.command.Command;
import org.bukkit.command.CommandSender;
import org.bukkit.plugin.java.JavaPlugin;

import java.io.BufferedOutputStream;
import java.io.ByteArrayOutputStream;
import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.PrintWriter;
import java.lang.reflect.Method;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/**
 * CrusstyProbe — vanilla reference chunk dumper for project c-faster.
 *
 * Bukkit-only (no NMS). Dumps fully-loaded chunks to the frozen .crsf format:
 *   u32 magic 0x46534243 ("CRSF" LE) | u32 version=1 | i32 chunkX | i32 chunkZ | i32 minY | i32 height
 *   u32 palette_len; palette_len entries of (u16 len + utf8 block-state string)
 *   16*16*height u16 LE palette indices, layout idx=(y-minY)*256 + z*16 + x
 *
 * Block state strings come from ChunkSnapshot.getBlockData(x,y,z).getAsString()
 * (e.g. "minecraft:water[level=0]") — deterministic, self-describing.
 */
public final class ProbePlugin extends JavaPlugin {

    private static final int MAGIC = 0x46534243; // bytes C,R,S,F in LE order
    private static final int VERSION = 1;
    // status/dump region: 12x12 chunks; CRUS_REGION_MIN lets the f1 probe move
    // the measured region away from spawn AND cover every margin chunk whose
    // features can spill into the compared inner 8x8 (see f1_probe.sh).
    private static final int REGION_MIN = Integer.getInteger("crus.region.min", -6);
    private static final int REGION_MAX = REGION_MIN + 11;

    @Override
    public void onEnable() {
        getLogger().info("CrusstyProbe enabled — vanilla reference dumper (region " + REGION_MIN + ".." + REGION_MAX + ")");
    }

    @Override
    public boolean onCommand(CommandSender sender, Command cmd, String label, String[] args) {
        switch (cmd.getName()) {
            case "crusstatus": return status(sender, args);
            case "crustick": return tick(sender);
            case "crusfull": return fullCount(sender, args);
            case "crusstydump": return dumpOne(sender, args);
            case "crusstydumpregion": return dumpRegion(sender, args);
            case "crusbase": return baseDump(sender, args);
            default: return false;
        }
    }

    /** crusfull <blockX1> <blockZ1> <blockX2> <blockZ2> [world] — exact count of
     *  FULLY loaded chunks inside the rect. Completion signal for the big-radius
     *  pregen: the global /crusgenstats chunks= counter also ticks for partial
     *  neighbour chunks (noise stage runs on ring chunks), so it cannot be used
     *  for per-batch completion arithmetic. */
    private boolean fullCount(CommandSender sender, String[] args) {
        if (args.length < 4) {
            sender.sendMessage("usage: crusfull <x1> <z1> <x2> <z2> [world]");
            return true;
        }
        int x1, z1, x2, z2;
        try {
            x1 = Integer.parseInt(args[0]);
            z1 = Integer.parseInt(args[1]);
            x2 = Integer.parseInt(args[2]);
            z2 = Integer.parseInt(args[3]);
        } catch (NumberFormatException e) {
            sender.sendMessage("BAD_COORDS");
            return true;
        }
        World w = resolveWorld(sender, args.length >= 5 ? args[4] : null);
        if (w == null) return true;
        int cxMin = Math.min(x1, x2) >> 4, cxMax = Math.max(x1, x2) >> 4;
        int czMin = Math.min(z1, z2) >> 4, czMax = Math.max(z1, z2) >> 4;
        int expect = (cxMax - cxMin + 1) * (czMax - czMin + 1);
        int count = 0;
        for (Chunk c : w.getLoadedChunks()) {
            int cx = c.getX(), cz = c.getZ();
            if (cx >= cxMin && cx <= cxMax && cz >= czMin && cz <= czMax) count++;
        }
        sender.sendMessage("FULL=" + count + "/" + expect
                + " cx[" + cxMin + ".." + cxMax + "] cz[" + czMin + ".." + czMax + "]");
        return true;
    }

    private World resolveWorld(CommandSender sender, String name) {
        if (name != null) {
            World w = Bukkit.getWorld(name);
            if (w == null) sender.sendMessage("WORLD_NOT_FOUND " + name);
            return w;
        }
        return Bukkit.getWorlds().get(0);
    }

    /** crustick — the overworld game time (tick counter) — the deterministic
     *  settle signal: the judge settles by TICKS (300), not wall seconds; the
     *  post-gen simulation (fluid spreads, growth) is tick-deterministic and
     *  the CI runner wall-clock drifts (kelp/amethyst random-tick growth was
     *  the 0.556% CI-only divergence). */
    private boolean tick(CommandSender sender) {
        World w = Bukkit.getWorlds().get(0);
        sender.sendMessage("gametime=" + w.getGameTime());
        return true;
    }

    /** crusstatus — loaded chunk count + test-region summary, one line per world. */
    private boolean status(CommandSender sender, String[] args) {
        // optional rect override: crusstatus [minCx minCz maxCx maxCz] — the
        // reference capture (bench.sh) counts its own 8x8 (-4..3) while the
        // judge pregen counts the 12x12 (-6..5)
        int rmin = REGION_MIN, rmax = REGION_MAX;
        if (args != null && args.length >= 4) {
            try {
                rmin = Integer.parseInt(args[0]);
                rmax = Integer.parseInt(args[2]);
            } catch (NumberFormatException ignored) {
            }
        }
        for (World w : Bukkit.getWorlds()) {
            int total = w.getLoadedChunks().length;
            int region = 0;
            int forced = 0;
            StringBuilder flags = new StringBuilder();
            for (int cx = rmin; cx <= rmax; cx++) {
                for (int cz = rmin; cz <= rmax; cz++) {
                    if (w.isChunkLoaded(cx, cz)) region++;
                    if (w.isChunkForceLoaded(cx, cz)) forced++;
                }
            }
            flags.append(" f(0,-4)=").append(w.isChunkForceLoaded(0, -4))
                 .append(" f(0,0)=").append(w.isChunkForceLoaded(0, 0))
                 .append(" l(0,-4)=").append(w.isChunkLoaded(0, -4));
            sender.sendMessage(w.getName() + ": loaded=" + total
                    + " region[" + rmin + ".." + rmax + "," + rmin + ".." + rmax + "]=" + region + "/64"
                    + " forced=" + forced
                    + flags
                    + " srvpid=" + ProcessHandle.current().pid());
        }
        return true;
    }

    /** crusbase <blockX1> <blockZ1> <blockX2> <blockZ2> <step> <outfile> [world]
     *  Dumps the VANILLA delegate's getBaseHeight (pure density column walk,
     *  beard-less) for a grid of points, both heightmap types, via full
     *  reflection (this plugin compiles against paper-api only). One line per
     *  point: "x z world ocean". Runs async — writes DONE as the last line. */
    private boolean baseDump(CommandSender sender, String[] args) {
        if (args.length < 6) {
            sender.sendMessage("usage: crusbase <x1> <z1> <x2> <z2> <step> <outfile> [world]");
            return true;
        }
        int x1, z1, x2, z2, step;
        try {
            x1 = Integer.parseInt(args[0]);
            z1 = Integer.parseInt(args[1]);
            x2 = Integer.parseInt(args[2]);
            z2 = Integer.parseInt(args[3]);
            step = Integer.parseInt(args[4]);
        } catch (NumberFormatException e) {
            sender.sendMessage("BAD_ARGS");
            return true;
        }
        World w = resolveWorld(sender, args.length >= 7 ? args[6] : null);
        if (w == null) return true;
        File out = new File(args[5]).getAbsoluteFile();
        sender.sendMessage("BASE_DUMP_STARTED " + out);
        new Thread(() -> {
            PrintWriter pw = null;
            try {
                pw = new PrintWriter(
                        new BufferedOutputStream(new FileOutputStream(out), 1 << 16));
                Object nmsWorld = w.getClass().getMethod("getHandle").invoke(w);
                Object chunkSource = nmsWorld.getClass().getMethod("getChunkSource").invoke(nmsWorld);
                Object gen = chunkSource.getClass().getMethod("getGenerator").invoke(chunkSource);
                // CustomChunkGenerator.delegate -> vanilla NoiseBasedChunkGenerator
                Object delegate = null;
                for (Class<?> k = gen.getClass(); k != null && delegate == null; k = k.getSuperclass()) {
                    try {
                        java.lang.reflect.Field f = k.getDeclaredField("delegate");
                        f.setAccessible(true);
                        delegate = f.get(gen);
                        break;
                    } catch (NoSuchFieldException ignored) { }
                }
                if (delegate == null) {
                    // not wrapped (vanilla world): use the generator itself
                    delegate = gen;
                }
                pw.println("# delegate=" + delegate.getClass().getName());
                Class<?> types = Class.forName("net.minecraft.world.level.levelgen.Heightmap$Types");
                Object wswg = types.getField("WORLD_SURFACE_WG").get(null);
                Object ofwg = types.getField("OCEAN_FLOOR_WG").get(null);
                Class<?> rngState = Class.forName("net.minecraft.world.level.levelgen.RandomState");
                java.lang.reflect.Field fSettings = null;                for (Class<?> k = delegate.getClass(); k != null; k = k.getSuperclass()) {
                    try {
                        fSettings = k.getDeclaredField("settings");
                        fSettings.setAccessible(true);
                        break;
                    } catch (NoSuchFieldException ignored) { }
                }
                Object settings = fSettings.get(delegate);
                Object regAccess = nmsWorld.getClass().getMethod("registryAccess").invoke(nmsWorld);
                // 1.21.8: RandomState.create(NoiseGeneratorSettings,
                //   HolderGetter<NoiseParameters>, long seed)
                Object settingsVal = settings.getClass().getMethod("value").invoke(settings);
                Object noiseKey = Class.forName("net.minecraft.core.registries.Registries")
                        .getField("NOISE").get(null);
                Object noiseGetter = regAccess.getClass()
                        .getMethod("lookupOrThrow", noiseKey.getClass())
                        .invoke(regAccess, noiseKey);
                Method mCreate = null;
                for (Method m : rngState.getMethods()) {
                    if (m.getName().equals("create") && m.getParameterCount() == 3
                            && m.getParameterTypes()[2] == long.class
                            && m.getParameterTypes()[0].isInstance(settingsVal)) {
                        mCreate = m;
                        break;
                    }
                }
                Object rs = mCreate.invoke(null, settingsVal, noiseGetter, w.getSeed());
                Method mBase = delegate.getClass().getMethod("getBaseHeight",
                        int.class, int.class, types,
                        Class.forName("net.minecraft.world.level.LevelHeightAccessor"),
                        rngState);
                long t0 = System.nanoTime();
                for (int x = x1; x <= x2; x += step) {
                    for (int z = z1; z <= z2; z += step) {
                        int world = (Integer) mBase.invoke(delegate, x, z, wswg, nmsWorld, rs);
                        int ocean = (Integer) mBase.invoke(delegate, x, z, ofwg, nmsWorld, rs);
                        pw.println(x + " " + z + " " + world + " " + ocean);
                    }
                }
                pw.println("DONE ms=" + ((System.nanoTime() - t0) / 1_000_000L));
                pw.close();
            } catch (Throwable t) {
                if (pw != null) pw.println("FAILED " + t);
                t.printStackTrace();
                if (pw != null) pw.close();
            }
        }, "crusbase-dump").start();
        return true;
    }

    /** crusstydump <cx> <cz> <outfile> [world] */
    private boolean dumpOne(CommandSender sender, String[] args) {
        if (args.length < 3) {
            sender.sendMessage("usage: crusstydump <cx> <cz> <outfile> [world]");
            return true;
        }
        int cx, cz;
        try {
            cx = Integer.parseInt(args[0]);
            cz = Integer.parseInt(args[1]);
        } catch (NumberFormatException e) {
            sender.sendMessage("BAD_COORDS");
            return true;
        }
        World w = resolveWorld(sender, args.length >= 4 ? args[3] : null);
        if (w == null) return true;
        if (!w.isChunkLoaded(cx, cz)) {
            sender.sendMessage("SKIP_UNLOADED " + cx + " " + cz);
            return true;
        }
        long t0 = System.nanoTime();
        try {
            writeDump(w, cx, cz, new File(args[2]));
        } catch (Exception e) {
            sender.sendMessage("DUMP_FAILED " + cx + " " + cz + " "
                    + e.getClass().getSimpleName() + ": " + e.getMessage());
            return true;
        }
        long ms = (System.nanoTime() - t0) / 1_000_000L;
        sender.sendMessage("DUMPED " + cx + " " + cz + " -> " + args[2] + " ms=" + ms);
        return true;
    }

    /** crusstydumpregion <cx1> <cz1> <cx2> <cz2> <outdir> <prefix> [world] */
    private boolean dumpRegion(CommandSender sender, String[] args) {
        if (args.length < 6) {
            sender.sendMessage("usage: crusstydumpregion <cx1> <cz1> <cx2> <cz2> <outdir> <prefix> [world]");
            return true;
        }
        int cx1, cz1, cx2, cz2;
        try {
            cx1 = Integer.parseInt(args[0]);
            cz1 = Integer.parseInt(args[1]);
            cx2 = Integer.parseInt(args[2]);
            cz2 = Integer.parseInt(args[3]);
        } catch (NumberFormatException e) {
            sender.sendMessage("BAD_COORDS");
            return true;
        }
        int cxMin = Math.min(cx1, cx2), cxMax = Math.max(cx1, cx2);
        int czMin = Math.min(cz1, cz2), czMax = Math.max(cz1, cz2);
        World w = resolveWorld(sender, args.length >= 7 ? args[6] : null);
        if (w == null) return true;
        File dir = new File(args[4]).getAbsoluteFile();
        if (!dir.exists() && !dir.mkdirs()) {
            sender.sendMessage("MKDIR_FAILED " + dir);
            return true;
        }
        String prefix = args[5];
        int dumped = 0, skipped = 0;
        List<String> skippedList = new ArrayList<>();
        long t0 = System.nanoTime();
        for (int cx = cxMin; cx <= cxMax; cx++) {
            for (int cz = czMin; cz <= czMax; cz++) {
                if (!w.isChunkLoaded(cx, cz)) {
                    skipped++;
                    if (skippedList.size() < 128) skippedList.add(cx + "," + cz);
                    continue;
                }
                try {
                    writeDump(w, cx, cz, new File(dir, prefix + "_" + cx + "_" + cz + ".crsf"));
                    dumped++;
                } catch (Exception e) {
                    sender.sendMessage("DUMP_FAILED " + cx + " " + cz + " "
                            + e.getClass().getSimpleName() + ": " + e.getMessage());
                }
            }
        }
        long ms = (System.nanoTime() - t0) / 1_000_000L;
        StringBuilder sb = new StringBuilder();
        sb.append("REGION_DUMP done=").append(dumped)
          .append(" skipped=").append(skipped)
          .append(" dir=").append(dir)
          .append(" prefix=").append(prefix)
          .append(" ms=").append(ms);
        if (!skippedList.isEmpty()) {
            sb.append(" skipped_chunks=");
            for (String s : skippedList) sb.append(s).append(';');
        }
        sender.sendMessage(sb.toString());
        return true;
    }

    /** Serialise one fully-loaded chunk to .crsf (frozen format, little-endian). */
    private static void writeDump(World world, int cx, int cz, File out) throws IOException {
        Chunk chunk = world.getChunkAt(cx, cz); // loaded already; no generation triggered
        ChunkSnapshot snap = chunk.getChunkSnapshot(false, false, false);
        int minY = world.getMinHeight();
        int height = world.getMaxHeight() - minY;
        int nBlocks = 16 * 16 * height;

        Map<String, Integer> palette = new LinkedHashMap<>();
        short[] idx = new short[nBlocks];
        int i = 0;
        for (int y = minY; y < minY + height; y++) {
            for (int z = 0; z < 16; z++) {
                for (int x = 0; x < 16; x++) {
                    BlockData bd = snap.getBlockData(x, y, z);
                    String s = bd.getAsString();
                    Integer id = palette.get(s);
                    if (id == null) {
                        id = palette.size();
                        palette.put(s, id);
                    }
                    idx[i++] = id.shortValue();
                }
            }
        }

        ByteArrayOutputStream bytes = new ByteArrayOutputStream(nBlocks * 2 + 8192);
        ByteBuffer head = ByteBuffer.allocate(24).order(ByteOrder.LITTLE_ENDIAN);
        head.putInt(MAGIC).putInt(VERSION).putInt(cx).putInt(cz).putInt(minY).putInt(height);
        bytes.write(head.array());

        ByteBuffer pc = ByteBuffer.allocate(4).order(ByteOrder.LITTLE_ENDIAN);
        pc.putInt(palette.size());
        bytes.write(pc.array());
        for (Map.Entry<String, Integer> e : palette.entrySet()) {
            byte[] sb2 = e.getKey().getBytes(StandardCharsets.UTF_8);
            ByteBuffer pe = ByteBuffer.allocate(2 + sb2.length).order(ByteOrder.LITTLE_ENDIAN);
            pe.putShort((short) sb2.length).put(sb2);
            bytes.write(pe.array());
        }

        ByteBuffer body = ByteBuffer.allocate(nBlocks * 2).order(ByteOrder.LITTLE_ENDIAN);
        for (short v : idx) body.putShort(v);
        bytes.write(body.array());

        try (BufferedOutputStream bos = new BufferedOutputStream(new FileOutputStream(out), 1 << 16)) {
            bytes.writeTo(bos);
        }
    }
}
