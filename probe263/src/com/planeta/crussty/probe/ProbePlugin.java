package com.planeta.crussty.probe;

import org.bukkit.Bukkit;
import org.bukkit.ChunkSnapshot;
import org.bukkit.World;
import org.bukkit.block.Biome;
import org.bukkit.command.Command;
import org.bukkit.command.CommandSender;
import org.bukkit.plugin.java.JavaPlugin;

import java.io.PrintWriter;


/**
 * 26.3 parity probe: dumps stable per-chunk fingerprints for every chunk
 * currently loaded in a region (paper 26.3, Bukkit API only).
 *
 * Console/rcon:
 *   biomedump <outfile> — FNV-1a over the biome ordinal of every block.
 *   blockdump <outfile> — FNV-1a over the Material ordinal of every block
 *                         (the surface/carve parity fingerprint).
 *
 * Lines: "cx cz distinct hash".
 *
 * A/B protocol: boot with the native stage ON, forceload a region, dump
 * file_on; boot with the CRUS_NO_NATIVE_*_263=1 kill-switch, same region,
 * dump file_off; diff.
 */
public final class ProbePlugin extends JavaPlugin {

    @Override
    public void onEnable() {
        for (String name : new String[] {"biomedump", "blockdump", "chunkdump", "statdump"}) {
            var cmd = getCommand(name);
            if (cmd != null) {
                cmd.setExecutor(this);
            }
        }
        getLogger().info("ProbePlugin enabled (biomedump, blockdump, chunkdump)");
    }

    @Override
    public boolean onCommand(CommandSender sender, Command command, String label, String[] args) {
        if (command.getName().equals("chunkdump")) {
            // chunkdump <cx> <cz> <outfile> — "x y z material" per non-air
            // block of one loaded chunk (bisection aid).
            if (args.length < 3) {
                sender.sendMessage("usage: chunkdump <cx> <cz> <outfile>");
                return true;
            }
            int tcx = Integer.parseInt(args[0]);
            int tcz = Integer.parseInt(args[1]);
            World w = Bukkit.getWorld("world");
            if (w == null || !w.isChunkLoaded(tcx, tcz)) {
                sender.sendMessage("chunk not loaded: " + tcx + "," + tcz);
                return true;
            }
            org.bukkit.Chunk c = w.getChunkAt(tcx, tcz);
            ChunkSnapshot snap = c.getChunkSnapshot(false, true, false);
            int minY = w.getMinHeight();
            int height = w.getMaxHeight() - w.getMinHeight();
            int n = 0;
            StringBuilder sb = new StringBuilder(1 << 22);
            for (int x = 0; x < 16; x++) {
                for (int z = 0; z < 16; z++) {
                    for (int y = 0; y < height; y++) {
                        org.bukkit.Material m = snap.getBlockType(x, minY + y, z);
                        if (m.isAir()) {
                            continue;
                        }
                        sb.append(x).append(' ').append(minY + y).append(' ').append(z)
                                .append(' ').append(m.name()).append('\n');
                        n++;
                    }
                }
            }
            try {
                PrintWriter pw = new PrintWriter(args[2], "UTF-8");
                pw.print(sb);
                pw.close();
            } catch (Exception e) {
                sender.sendMessage("chunkdump write failed: " + e);
                return true;
            }
            sender.sendMessage("chunkdump: " + n + " blocks -> " + args[2]);
            return true;
        }
        if (command.getName().equals("statdump")) {
            // statdump <outfile> — "cx cz material:count ..." per chunk.
            String out = args.length >= 1 ? args[0] : "statdump.txt";
            int n = 0;
            StringBuilder sb = new StringBuilder(1 << 22);
            for (World w : Bukkit.getWorlds()) {
                if (!w.getName().equals("world")) continue;
                for (org.bukkit.Chunk c : w.getLoadedChunks()) {
                    ChunkSnapshot snap = c.getChunkSnapshot(false, true, false);
                    int minY = w.getMinHeight();
                    int height = w.getMaxHeight() - w.getMinHeight();
                    java.util.Map<String, Integer> hist = new java.util.TreeMap<>();
                    for (int x = 0; x < 16; x++)
                        for (int z = 0; z < 16; z++)
                            for (int y = 0; y < height; y++) {
                                String m = snap.getBlockType(x, minY + y, z).name();
                                hist.merge(m, 1, Integer::sum);
                            }
                    sb.append(c.getX()).append(' ').append(c.getZ());
                    for (var e : hist.entrySet()) sb.append(' ').append(e.getKey()).append(':').append(e.getValue());
                    sb.append('\n');
                    n++;
                }
            }
            try {
                PrintWriter pw = new PrintWriter(out, "UTF-8");
                pw.print(sb); pw.close();
            } catch (Exception e) { sender.sendMessage("statdump failed: " + e); return true; }
            sender.sendMessage("statdump: " + n + " chunks -> " + out);
            return true;
        }
        boolean biomeMode = command.getName().equals("biomedump");
        if (!biomeMode && !command.getName().equals("blockdump")) {
            return false;
        }
        String out = args.length >= 1 ? args[0] : (biomeMode ? "biomedump.txt" : "blockdump.txt");
        int n = 0;
        StringBuilder sb = new StringBuilder(1 << 20);
        for (World w : Bukkit.getWorlds()) {
            if (!w.getName().equals("world")) {
                continue;
            }
            for (org.bukkit.Chunk c : w.getLoadedChunks()) {
                ChunkSnapshot snap = c.getChunkSnapshot(false, true, false);
                long hash = 0xcbf29ce484222325L;
                int distinct = 0;
                java.util.Set<Integer> seen = new java.util.HashSet<>();
                int minY = w.getMinHeight();
                int height = w.getMaxHeight() - w.getMinHeight();
                for (int x = 0; x < 16; x++) {
                    for (int z = 0; z < 16; z++) {
                        for (int y = 0; y < height; y++) {
                            int v = biomeMode
                                    ? snap.getBiome(x, minY + y, z).ordinal()
                                    : snap.getBlockType(x, minY + y, z).ordinal();
                            if (seen.add(v)) {
                                distinct++;
                            }
                            hash = (hash ^ (long) v) * 0x100000001b3L;
                        }
                    }
                }
                sb.append(c.getX()).append(' ').append(c.getZ()).append(' ')
                        .append(distinct).append(' ').append(hash).append('\n');
                n++;
            }
        }
        try {
            PrintWriter pw = new PrintWriter(out, "UTF-8");
            pw.print(sb);
            pw.close();
        } catch (Exception e) {
            sender.sendMessage((biomeMode ? "biomedump" : "blockdump") + " write failed: " + e);
            return true;
        }
        sender.sendMessage((biomeMode ? "biomedump: " : "blockdump: ") + n + " chunks -> " + out);
        return true;
    }
}
