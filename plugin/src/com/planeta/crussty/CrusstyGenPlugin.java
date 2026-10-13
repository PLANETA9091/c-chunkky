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
        getLogger().info("CrusstyGen enabled — generator: CrusstyChunkGenerator (noise stage takeover)");
    }

    @Override
    public void onDisable() {
        CrusstyChunkGenerator.freeAll();
        getLogger().info("CrusstyGen disabled — native ctx freed");
    }

    @Override
    public boolean onCommand(CommandSender sender, Command command, String label, String[] args) {
        if (command.getName().equals("crusgenstats")) {
            sender.sendMessage("[CrusstyGen] native noise stage: " + GenStats.snapshot());
            try {
                sender.sendMessage("[CrusstyGen] " + ChunkGenNative.profStatsGlobal());
            } catch (Throwable ignored) {
            }
            return true;
        }
        return false;
    }
}
