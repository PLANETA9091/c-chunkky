package com.planeta.crussty;

import net.minecraft.core.Holder;
import net.minecraft.core.Registry;
import net.minecraft.core.registries.Registries;
import net.minecraft.resources.Identifier;
import net.minecraft.world.level.biome.Biome;
import net.minecraft.world.level.biome.BiomeResolver;
import net.minecraft.world.level.biome.BiomeSource;
import net.minecraft.world.level.biome.Climate;
import net.minecraft.world.level.biome.MultiNoiseBiomeSource;

import java.lang.reflect.Field;
import java.lang.reflect.Method;
import java.util.Arrays;
import java.util.stream.Stream;

/**
 * Grid-backed BiomeSource: the chunk's biome quart grid comes from the NATIVE
 * engine (JNI fillBiomes — the same fill_biomes263 the hood/terrain paths
 * already use), so the BIOMES chunk status stops sampling the Java climate
 * splines + noise + RTree search (~1.5-2ms/chunk of Paper-side work that the
 * JFR window attributed to MultiNoiseBiomeSource.createResolverForChunk).
 *
 * Correctness: cont-53/54 proved the native point resolution bit-equal to the
 * preset MultiNoiseBiomeSource; this source makes the live pipeline agree BY
 * CONSTRUCTION (grid answers; vanilla RTree stays as the out-of-range
 * fallback).
 *
 * Serialization: a PresetCodec registered as "crussty:grid" in
 * BuiltInRegistries.BIOME_SOURCE. ENCODE delegates to the held vanilla
 * MultiNoiseBiomeSource — the JSON written is exactly what the preset source
 * writes (bit-equal level.dat). DECODE delegates to MultiNoiseBiomeSource.CODEC
 * (a reload rebuilds the preset source; BiomeSwap re-wraps at world load).
 *
 * WHY not return MultiNoiseBiomeSource.CODEC from codec() directly: its
 * RecordCodecBuilder getters are typed to MultiNoiseBiomeSource, so encoding a
 * NativeBiomeSource instance throws ClassCastException inside the
 * WorldGenSettings save (CI 41075573643: server died at boot,
 * initServer -> saveEverything -> KeyDispatchCodec -> CCE). Registration makes
 * the dispatch write OUR type tag and route the value encode through us.
 *
 * Subclassing note: MultiNoiseBiomeSource has only a private ctor, so we
 * extend BiomeSource directly and compose the preset source (its protected
 * collectPossibleBiomes is NOT callable cross-instance; we rebuild the set
 * from the native name table, which is the same biome list by definition).
 *
 * Kill switch: CRUS_BIOME_GRID=0 in BiomeSwap keeps the plain preset source.
 */
public final class NativeBiomeSource extends BiomeSource {

    private final MultiNoiseBiomeSource vanilla;
    private final CrusstyChunkGenerator.CrusstyFiller filler;
    private final Holder<Biome>[] byIndex;
    private final int minQuartY;
    private final int sections;

    @SuppressWarnings("unchecked")
    private NativeBiomeSource(MultiNoiseBiomeSource vanilla,
                              CrusstyChunkGenerator.CrusstyFiller filler,
                              String[] names, int sections, int minQuartY,
                              Registry<Biome> biomeReg) {
        super();
        this.vanilla = vanilla;
        this.filler = filler;
        this.sections = sections;
        this.minQuartY = minQuartY;
        this.byIndex = new Holder[names.length];
        int misses = 0;
        for (int i = 0; i < names.length; i++) {
            Holder<Biome> h = null;
            try {
                h = biomeReg.get(Identifier.parse(names[i])).orElse(null);
            } catch (Throwable t) {
                // fall through to the miss path
            }
            if (h == null) {
                misses++;
                h = biomeReg.get(Identifier.withDefaultNamespace("plains")).orElse(null);
            }
            byIndex[i] = h;
        }
        if (misses > 0) {
            java.util.logging.Logger.getLogger("CrusstyGen").warning(
                    "NativeBiomeSource: " + misses + "/" + names.length
                            + " biome names unresolved (plains substituted)");
        }
    }

    /** Build over the level's biome registry; null on any failure (caller keeps vanilla). */
    public static NativeBiomeSource create(MultiNoiseBiomeSource vanilla,
                                           CrusstyChunkGenerator.CrusstyFiller filler,
                                           String[] names, int sections, int minQuartY,
                                           Object nmsWorld) {
        if (codecRegistered() == null) {
            java.util.logging.Logger.getLogger("CrusstyGen").warning(
                    "NativeBiomeSource: codec not registered — vanilla biome source stays");
            return null;
        }
        try {
            Object regAccess = nmsWorld.getClass().getMethod("registryAccess").invoke(nmsWorld);
            Object biomeRegistry = regAccess.getClass()
                    .getMethod("lookupOrThrow", net.minecraft.resources.ResourceKey.class)
                    .invoke(regAccess, Registries.BIOME);
            @SuppressWarnings("unchecked")
            Registry<Biome> biomeReg = (Registry<Biome>) biomeRegistry;
            return new NativeBiomeSource(vanilla, filler, names, sections, minQuartY, biomeReg);
        } catch (Throwable t) {
            java.util.logging.Logger.getLogger("CrusstyGen").warning(
                    "NativeBiomeSource create failed: " + t);
            return null;
        }
    }

    /** Saved level.dat round-trips as the vanilla preset source (bit-equal). */
    @Override
    protected com.mojang.serialization.MapCodec<? extends BiomeSource> codec() {
        return codecRegistered();
    }

    /** Registered codec (null until tryRegisterCodec succeeds once). */
    private static volatile MapCodec<BiomeSource> REGISTERED;
    private static volatile boolean TRIED;

    private static MapCodec<BiomeSource> codecRegistered() {
        if (TRIED) {
            return REGISTERED;
        }
        synchronized (NativeBiomeSource.class) {
            if (!TRIED) {
                REGISTERED = tryRegisterCodec();
                TRIED = true;
            }
            return REGISTERED;
        }
    }

    /**
     * Register PresetCodec in BuiltInRegistries.BIOME_SOURCE so the
     * BiomeSource dispatch (a) writes our type tag and (b) routes the value
     * encode through us. Reflective with fallback chains — names are
     * cross-checked by the CI javap lane (nms-biome-signatures.txt); any
     * failure returns null and create() keeps the vanilla source.
     */
    @SuppressWarnings({"unchecked", "rawtypes"})
    private static MapCodec<BiomeSource> tryRegisterCodec() {
        try {
            Object reg = Class.forName("net.minecraft.core.registries.BuiltInRegistries")
                    .getField("BIOME_SOURCE").get(null);
            Object unfreeze = null;
            Object freeze = null;
            Field frozenField = null;
            try {
                unfreeze = reg.getClass().getMethod("unfreeze");
                freeze = reg.getClass().getMethod("freeze");
            } catch (Throwable ignored) {
                // fall through to the boolean-field path
            }
            if (unfreeze == null) {
                for (Class<?> k = reg.getClass(); k != null && frozenField == null; k = k.getSuperclass()) {
                    for (Field f : k.getDeclaredFields()) {
                        if (f.getType() == boolean.class && f.getName().equals("frozen")) {
                            f.setAccessible(true);
                            frozenField = f;
                            break;
                        }
                    }
                }
            }
            if (unfreeze != null) {
                unfreeze.invoke(reg);
            } else if (frozenField != null) {
                frozenField.set(reg, false);
            } else {
                throw new IllegalStateException("no unfreeze/frozen on " + reg.getClass().getName());
            }
            Object id;
            try {
                id = Identifier.class.getMethod("fromNamespaceAndPath", String.class, String.class)
                        .invoke(null, "crussty", "grid");
            } catch (Throwable e1) {
                id = Identifier.parse("crussty:grid");
            }
            Method register = null;
            for (Method m : Class.forName("net.minecraft.core.Registry").getMethods()) {
                if (!m.getName().equals("register") || !java.lang.reflect.Modifier.isStatic(m.getModifiers())) {
                    continue;
                }
                Class<?>[] p = m.getParameterTypes();
                if (p.length == 3 && p[0].isInstance(reg) && p[1] == Identifier.class) {
                    register = m;
                    break;
                }
            }
            if (register == null) {
                throw new IllegalStateException("no Registry.register(Registry, Identifier, T)");
            }
            Object registered = register.invoke(null, reg, id, PresetCodec.INSTANCE);
            if (freeze != null) {
                freeze.invoke(reg);
            } else if (frozenField != null) {
                frozenField.set(reg, true);
            }
            java.util.logging.Logger.getLogger("CrusstyGen").info(
                    "NativeBiomeSource: codec registered as crussty:grid -> " + registered);
            return PresetCodec.INSTANCE;
        } catch (Throwable t) {
            java.util.logging.Logger.getLogger("CrusstyGen").warning(
                    "NativeBiomeSource codec registration failed (vanilla source stays): " + t);
            return null;
        }
    }

    /**
     * Encode = the vanilla preset source's own JSON; decode = the vanilla
     * preset codec (reload rebuilds the preset source; BiomeSwap re-wraps).
     */
    private static final class PresetCodec extends com.mojang.serialization.MapCodec<BiomeSource> {
        static final PresetCodec INSTANCE = new PresetCodec();

        @Override
        public <T> com.mojang.serialization.RecordBuilder<T> encode(BiomeSource input,
                com.mojang.serialization.DynamicOps<T> ops,
                com.mojang.serialization.RecordBuilder<T> prefix) {
            MultiNoiseBiomeSource v = input instanceof NativeBiomeSource n ? n.vanilla : null;
            if (v == null) {
                throw new IllegalStateException("PresetCodec.encode: not a NativeBiomeSource: " + input);
            }
            return MultiNoiseBiomeSource.CODEC.encode(v, ops, prefix);
        }

        @Override
        public <T> com.mojang.serialization.DataResult<BiomeSource> decode(
                com.mojang.serialization.DynamicOps<T> ops, com.mojang.serialization.MapLike<T> input) {
            return MultiNoiseBiomeSource.CODEC.decode(ops, input).map(m -> (BiomeSource) m);
        }

        @Override
        public <T> java.util.stream.Stream<T> keys(com.mojang.serialization.DynamicOps<T> ops) {
            return MultiNoiseBiomeSource.CODEC.keys(ops);
        }
    }

    /** The native name table IS the parameter list's biome set. */
    @Override
    protected Stream<Holder<Biome>> collectPossibleBiomes() {
        return Arrays.stream(byIndex).distinct();
    }

    @Override
    public BiomeResolver createResolver(Climate.Sampler sampler) {
        return new GridResolver(sampler);
    }

    @Override
    public BiomeResolver createResolverForChunk(Climate.Sampler sampler,
                                                int x0, int y0, int z0,
                                                int x1, int y1, int z1) {
        // bounds units are undocumented (quart vs block) — the resolver derives
        // the chunk lazily from the first query (qx>>2), unit-independent.
        return new GridResolver(sampler);
    }

    /** Per-chunk grid cache; one JNI fill (~0.15ms) then pure array lookups. */
    private final class GridResolver implements BiomeResolver {
        private final Climate.Sampler fallback;
        // volatile publication: grid is fully written before cxz lands
        private volatile long cxz = Long.MIN_VALUE;
        private volatile int[] grid;

        GridResolver(Climate.Sampler fallback) {
            this.fallback = fallback;
        }

        @Override
        public Holder<Biome> getNoiseBiome(int qx, int qy, int qz) {
            int cx = qx >> 2, cz = qz >> 2;
            long key = ((long) cx << 32) ^ (cz & 0xffffffffL);
            if (key != cxz) {
                synchronized (this) {
                    if (key != cxz) {
                        int[] g = new int[sections * 64];
                        // the filler closure pins its own pool ctx — the long
                        // param is unused by the production lambda
                        filler.fill(0L, cx, cz, g);
                        grid = g;
                        cxz = key;
                    }
                }
            }
            int s = (qy - minQuartY) >> 2;
            if (s < 0 || s >= sections) {
                // out of the world's quart range: vanilla RTree answers (same
                // semantics as today's pipeline)
                return vanilla.getNoiseBiome(fallback.sample(qx, qy, qz));
            }
            int idx = s * 64 + (qx & 3) * 16 + (qy & 3) * 4 + (qz & 3);
            Holder<Biome> h = byIndex[grid[idx]];
            return h != null ? h
                    : vanilla.getNoiseBiome(fallback.sample(qx, qy, qz));
        }
    }
}
