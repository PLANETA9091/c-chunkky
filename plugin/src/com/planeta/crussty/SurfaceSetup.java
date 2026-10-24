package com.planeta.crussty;

import net.minecraft.core.Holder;
import net.minecraft.core.Registry;
import net.minecraft.core.registries.Registries;
import net.minecraft.resources.ResourceKey;
import net.minecraft.resources.Identifier;
import net.minecraft.world.level.biome.Biome;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.state.BlockState;

/**
 * Mojmap-compiled companion: completes the native surface engine handshake.
 * resolveBlockState + extendPalette need the mojmap server jar (the block
 * state registry + property system), so they live here — compiled together
 * with NativeBiomeSource/BiomeSwap by plugin/build.sh when the server jar is
 * present.
 */
public final class SurfaceSetup {

    public static void apply(long[] ctxs, org.bukkit.World world, String[] states) throws Exception {
        short[] ids = CrusstyChunkGenerator.extendPalette(states, (String st) -> {
            try {
                return resolveBlockState(st);
            } catch (Exception e) {
                throw new RuntimeException(e);
            }
        });
        if (ids == null) {
            throw new IllegalStateException("palette unavailable (bulk init failed)");
        }
        String[] names = ChunkGenNative.ctxBiomeNames(ctxs[0]);
        if (names == null || names.length == 0) {
            throw new IllegalStateException("no native biome names");
        }
        Object nmsWorld = world.getClass().getMethod("getHandle").invoke(world);
        Object regAccess = nmsWorld.getClass().getMethod("registryAccess").invoke(nmsWorld);
        Object biomeRegistry = regAccess.getClass()
                .getMethod("lookupOrThrow", ResourceKey.class)
                .invoke(regAccess, Registries.BIOME);
        @SuppressWarnings("unchecked")
        Registry<Biome> biomeReg = (Registry<Biome>) biomeRegistry;
        float[] temps = new float[names.length];
        boolean[] frozen = new boolean[names.length];
        int misses = 0;
        for (int i = 0; i < names.length; i++) {
            java.util.Optional<Holder.Reference<Biome>> hOpt =
                    biomeReg.get(Identifier.parse(names[i]));
            if (hOpt.isEmpty()) {
                misses++;
                continue;
            }
            Biome b = hOpt.get().value();
            temps[i] = b.getBaseTemperature();
            java.lang.reflect.Field fCs = Biome.class.getDeclaredField("climateSettings");
            fCs.setAccessible(true);
            Object cs = fCs.get(b);
            java.lang.reflect.Field fMod =
                    cs.getClass().getDeclaredField("temperatureModifier");
            fMod.setAccessible(true);
            Object mod = fMod.get(cs);
            frozen[i] = mod != null && "FROZEN".equals(mod.toString());
        }
        if (misses > 0) {
            throw new IllegalStateException("biome names unresolved: " + misses);
        }
        // every pool ctx must carry the ids+temps — fills acquire arbitrary
        // ctxs and a bare ctx silently skips the native surface tail
        for (long c : ctxs) {
            int rc = ChunkGenNative.surfaceSetIds(c, ids);
            if (rc != 0) {
                throw new IllegalStateException("surfaceSetIds rc=" + rc);
            }
            rc = ChunkGenNative.surfaceSetTemps(c, temps, frozen);
            if (rc != 0) {
                throw new IllegalStateException("surfaceSetTemps rc=" + rc);
            }
        }
    }

    /** Parses "minecraft:grass_block[snowy=false]" into the NMS BlockState
     *  (default state + property assignments via the state definition). */
    private static Object resolveBlockState(String s) throws Exception {
        String name = s;
        String props = null;
        int b = s.indexOf('[');
        if (b >= 0) {
            name = s.substring(0, b);
            props = s.substring(b + 1, s.length() - 1);
        }
        Registry<Block> reg = net.minecraft.core.registries.BuiltInRegistries.BLOCK;
        final String fname = name;
        Block blk = reg.getOptional(Identifier.parse(name))
                .orElseThrow(() -> new IllegalStateException("block not found: " + fname));
        BlockState st = blk.defaultBlockState();
        if (props != null && !props.isEmpty()) {
            BlockState cur = st;
            var def = blk.getStateDefinition();
            for (String kv : props.split(",")) {
                int eq = kv.indexOf('=');
                if (eq <= 0) {
                    throw new IllegalStateException("bad prop " + kv + " in " + s);
                }
                String pk = kv.substring(0, eq);
                String pv = kv.substring(eq + 1);
                var prop = def.getProperty(pk);
                if (prop == null) {
                    throw new IllegalStateException("no prop " + pk + " on " + name);
                }
                BlockState st2 = setProp(cur, prop, pv);
                if (st2 == null) {
                    throw new IllegalStateException("no value " + pv + " for prop " + pk + " on " + name);
                }
                cur = st2;
            }
            st = cur;
        }
        return st;
    }

    @SuppressWarnings({"unchecked", "rawtypes"})
    private static BlockState setProp(BlockState st, net.minecraft.world.level.block.state.properties.Property prop, String pv) {
        for (Object o : prop.getPossibleValues()) {
            Comparable v = (Comparable) o;
            if (prop.getName(v).equals(pv)) {
                return st.setValue((net.minecraft.world.level.block.state.properties.Property) prop, v);
            }
        }
        return null;
    }
}
