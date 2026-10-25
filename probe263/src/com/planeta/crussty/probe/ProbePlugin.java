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
 * 26.3 biome parity probe: dumps a stable per-chunk biome fingerprint for
 * every chunk currently loaded in a region (paper 26.3, Bukkit API only —
 * ChunkSnapshot.getBiome covers the full column).
 *
 * Console/rcon:  biomedump <outfile>
 * Lines: "cx cz biomeCount hash" — hash = FNV-1a over the biome ordinal of
 * every (x, y, z) block in the chunk in a fixed order.
 *
 * A/B protocol: boot with the native biome stage ON, forceload a region,
 * biomedump file_on; boot with CRUS_NO_NATIVE_BIOMES_263=1, same region,
 * biomedump file_off; diff.
 */
public final class ProbePlugin extends JavaPlugin {

    @Override
    public void onEnable() {
        var cmd = getCommand("biomedump");
        if (cmd != null) {
            cmd.setExecutor(this);
        }
        getLogger().info("ProbePlugin enabled (biomedump)");
    }

    @Override
    public boolean onCommand(CommandSender sender, Command command, String label, String[] args) {
        if (!command.getName().equals("biomedump")) {
            return false;
        }
        String out = args.length >= 1 ? args[0] : "biomedump.txt";
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
                java.util.Set<Biome> seen = new java.util.HashSet<>();
                int minY = w.getMinHeight();
                int height = w.getMaxHeight() - w.getMinHeight();
                for (int x = 0; x < 16; x++) {
                    for (int z = 0; z < 16; z++) {
                        for (int y = 0; y < height; y++) {
                            Biome b = snap.getBiome(x, minY + y, z);
                            if (seen.add(b)) {
                                distinct++;
                            }
                            hash = (hash ^ (long) b.ordinal()) * 0x100000001b3L;
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
            sender.sendMessage("biomedump write failed: " + e);
            return true;
        }
        sender.sendMessage("biomedump: " + n + " chunks -> " + out);
        return true;
    }
}
