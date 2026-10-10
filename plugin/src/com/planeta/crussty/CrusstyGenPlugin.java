package com.planeta.crussty;

import org.bukkit.command.Command;
import org.bukkit.command.CommandSender;
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
            return true;
        }
        return false;
    }
}
