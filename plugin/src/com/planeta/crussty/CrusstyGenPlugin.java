package com.planeta.crussty;

import org.bukkit.command.Command;
import org.bukkit.command.CommandSender;
import org.bukkit.generator.ChunkGenerator;
import org.bukkit.plugin.java.JavaPlugin;

/**
 * CrusstyGen: native noise-stage generator for Paper 1.21.8.
 * Worlds are wired via bukkit.yml:
 *   worlds:
 *     vanilla:
 *       generator: CrusstyGen
 * Only generateNoise is overridden — vanilla surface/carvers/features/lighting
 * run on top, preserving parity with the vanilla reference dumps.
 */
public final class CrusstyGenPlugin extends JavaPlugin {

    private final ChunkGenerator generator = new CrusstyChunkGenerator();

    /** bukkit.yml "generator: CrusstyGen" resolves here. */
    @Override
    public ChunkGenerator getDefaultWorldGenerator(String worldName, String id) {
        getLogger().info("providing CrusstyChunkGenerator for world '" + worldName + "'");
        return generator;
    }

    @Override
    public void onEnable() {
        var cmd = getCommand("crusgenstats");
        if (cmd != null) {
            cmd.setExecutor(this);
        }
        // Eager datapack pre-resolve + native biome source swap per world.
        getServer().getPluginManager().registerEvents(listener(), this);
        getLogger().info("CrusstyGen enabled — generator: CrusstyChunkGenerator (noise stage takeover)");
    }

    private org.bukkit.event.Listener listener() {
        return new org.bukkit.event.Listener() {
            @org.bukkit.event.EventHandler
            public void onWorldLoad(org.bukkit.event.world.WorldLoadEvent e) {
                CrusstyChunkGenerator.onWorldLoad(e.getWorld());
            }
        };
    }

    @Override
    public void onDisable() {
        CrusstyChunkGenerator.freeAll();
        getLogger().info("CrusstyGen disabled — native ctx freed");
    }

    @Override
    public boolean onCommand(CommandSender sender, Command command, String label, String[] args) {
        if (command.getName().equals("crusgenstats")) {
            if (args.length >= 1 && args[0].equalsIgnoreCase("reset")) {
                GenStats.reset();
                DecoStats.reset();
                try {
                    ChunkGenNative.fill263ProfReset();
                } catch (Throwable ignored) {
                }
                sender.sendMessage("[CrusstyGen] stats window reset");
                return true;
            }
            // Region-bounded honest window: arm the rectangle (chunk coords)
            // and zero ONLY the window counters. Poll via "wincount", final
            // numbers in the full snapshot's window line.
            if (args.length >= 4 && args[0].equalsIgnoreCase("window")) {
                try {
                    int rx = Integer.parseInt(args[1]);
                    int rz = Integer.parseInt(args[2]);
                    int n = Integer.parseInt(args[3]);
                    GenStats.window(rx, rz, n);
                    sender.sendMessage("[CrusstyGen] window armed [" + rx + ".."
                            + (rx + n) + "," + rz + ".." + (rz + n) + "]");
                } catch (NumberFormatException e) {
                    sender.sendMessage("[CrusstyGen] window: bad coords");
                }
                return true;
            }
            if (args.length >= 1 && args[0].equalsIgnoreCase("wincount")) {
                sender.sendMessage("window_inside=" + GenStats.winInside());
                return true;
            }
            // Paper RCON delivers only ONE sendMessage per command — join the
            // three diagnostic lines into a single message or rcon consumers
            // lose [cpu] and jni_prof (observed 2026-10-15, prof sessions).
            StringBuilder sb = new StringBuilder(cpuLine())
                    .append(" | [CrusstyGen] native noise stage: ").append(GenStats.snapshot())
                    .append(" | [CrusstyGen] honest window: ").append(GenStats.windowSnapshot());
            try {
                sb.append(" | [CrusstyGen] native deco stage: ").append(DecoStats.snapshot())
                        .append(DecoStats.delegateNames()).append(DecoStats.surviveHist()).append(DecoStats.hTyHist());
            } catch (Throwable ignored) {
            }
            try {
                sb.append(" | [CrusstyGen] fill263 prof: ").append(ChunkGenNative.fill263Prof());
            } catch (Throwable ignored) {
            }
            try {
                sb.append(" | [CrusstyGen] ").append(ChunkGenNative.profStatsGlobal());
            } catch (Throwable ignored) {
            }
            sender.sendMessage(sb.toString());
            return true;
        }
        return false;
    }

    /** One-time runner identification line: CPU model, cores, AVX flags.
     * The CI runner pool is heterogeneous (Milan / Granite Rapids / Turin,
     * documented 2.1x window dispersion on identical code) — every judge
     * log must record the hardware for cross-run window comparisons. */
    private static volatile String CPU_LINE;
    private static String cpuLine() {
        String s = CPU_LINE;
        if (s != null) return s;
        String model = "unknown";
        String flags = "";
        try (java.util.Scanner sc = new java.util.Scanner(new java.io.File("/proc/cpuinfo"))) {
            sc.useDelimiter("\n");
            while (sc.hasNext()) {
                String l = sc.next();
                if (l.startsWith("model name") && model.equals("unknown")) {
                    int i = l.indexOf(':');
                    if (i >= 0) model = l.substring(i + 1).trim();
                }
                if (l.startsWith("flags") && flags.isEmpty()) {
                    int i = l.indexOf(':');
                    if (i >= 0) flags = l.substring(i + 1);
                }
            }
        } catch (Throwable ignored) {
        }
        boolean avx2 = flags.contains(" avx2");
        boolean avx512 = flags.contains(" avx512f");
        s = "[CrusstyGen] [cpu] " + model
                + " cores=" + java.lang.Runtime.getRuntime().availableProcessors()
                + " avx2=" + avx2 + " avx512f=" + avx512;
        CPU_LINE = s;
        return s;
    }
}
