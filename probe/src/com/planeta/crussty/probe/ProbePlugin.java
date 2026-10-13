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
    private static final int REGION_MIN = -4;    // test region: 8x8 chunks around origin
    private static final int REGION_MAX = 3;

    @Override
    public void onEnable() {
        getLogger().info("CrusstyProbe enabled — vanilla reference dumper");
    }

    @Override
    public boolean onCommand(CommandSender sender, Command cmd, String label, String[] args) {
        switch (cmd.getName()) {
            case "crusstatus": return status(sender);
            case "crusstydump": return dumpOne(sender, args);
            case "crusstydumpregion": return dumpRegion(sender, args);
            default: return false;
        }
    }

    private World resolveWorld(CommandSender sender, String name) {
        if (name != null) {
            World w = Bukkit.getWorld(name);
            if (w == null) sender.sendMessage("WORLD_NOT_FOUND " + name);
            return w;
        }
        return Bukkit.getWorlds().get(0);
    }

    /** crusstatus — loaded chunk count + test-region summary, one line per world. */
    private boolean status(CommandSender sender) {
        for (World w : Bukkit.getWorlds()) {
            int total = w.getLoadedChunks().length;
            int region = 0;
            int forced = 0;
            StringBuilder flags = new StringBuilder();
            for (int cx = REGION_MIN; cx <= REGION_MAX; cx++) {
                for (int cz = REGION_MIN; cz <= REGION_MAX; cz++) {
                    if (w.isChunkLoaded(cx, cz)) region++;
                    if (w.isChunkForceLoaded(cx, cz)) forced++;
                }
            }
            flags.append(" f(0,-4)=").append(w.isChunkForceLoaded(0, -4))
                 .append(" f(0,0)=").append(w.isChunkForceLoaded(0, 0))
                 .append(" l(0,-4)=").append(w.isChunkLoaded(0, -4));
            sender.sendMessage(w.getName() + ": loaded=" + total
                    + " region[-4..3,-4..3]=" + region + "/64"
                    + " forced=" + forced
                    + flags
                    + " srvpid=" + ProcessHandle.current().pid());
        }
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
