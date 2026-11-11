/*
 * Decompiled with CFR 0.152.
 * 
 * Could not load the following classes:
 *  com.google.common.base.Preconditions
 *  com.mojang.serialization.MapCodec
 *  org.bukkit.World
 *  org.bukkit.block.Biome
 *  org.bukkit.generator.ChunkGenerator
 *  org.bukkit.generator.ChunkGenerator$BiomeGrid
 *  org.bukkit.generator.ChunkGenerator$ChunkData
 *  org.bukkit.generator.WorldInfo
 *  org.jspecify.annotations.Nullable
 */
package org.bukkit.craftbukkit.generator;

import com.google.common.base.Preconditions;
import com.mojang.serialization.MapCodec;
import io.papermc.paper.FeatureHooks;
import java.util.List;
import java.util.Objects;
import java.util.Random;
import java.util.Set;
import java.util.concurrent.CompletableFuture;
import java.util.function.Function;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Holder;
import net.minecraft.core.QuartPos;
import net.minecraft.core.RegistryAccess;
import net.minecraft.core.SectionPos;
import net.minecraft.resources.ResourceKey;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.server.level.WorldGenRegion;
import net.minecraft.util.Mth;
import net.minecraft.util.random.WeightedList;
import net.minecraft.world.entity.MobCategory;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.LevelHeightAccessor;
import net.minecraft.world.level.NoiseColumn;
import net.minecraft.world.level.StructureManager;
import net.minecraft.world.level.WorldGenLevel;
import net.minecraft.world.level.biome.BiomeManager;
import net.minecraft.world.level.biome.BiomeSource;
import net.minecraft.world.level.biome.MobSpawnSettings;
import net.minecraft.world.level.block.EntityBlock;
import net.minecraft.world.level.block.entity.BlockEntity;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.ChunkAccess;
import net.minecraft.world.level.chunk.ChunkGenerator;
import net.minecraft.world.level.chunk.ChunkGeneratorStructureState;
import net.minecraft.world.level.chunk.LevelChunkSection;
import net.minecraft.world.level.levelgen.Heightmap;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.level.levelgen.NoiseBasedChunkGenerator;
import net.minecraft.world.level.levelgen.RandomState;
import net.minecraft.world.level.levelgen.WorldgenRandom;
import net.minecraft.world.level.levelgen.blending.Blender;
import net.minecraft.world.level.levelgen.densityfunction.SamplerContext;
import net.minecraft.world.level.levelgen.structure.templatesystem.StructureTemplateManager;
import org.bukkit.World;
import org.bukkit.block.Biome;
import org.bukkit.craftbukkit.CraftHeightMap;
import org.bukkit.craftbukkit.block.CraftBiome;
import org.bukkit.craftbukkit.generator.CraftChunkData;
import org.bukkit.craftbukkit.generator.InternalChunkGenerator;
import org.bukkit.craftbukkit.generator.OldCraftChunkData;
import org.bukkit.craftbukkit.util.RandomSourceWrapper;
import org.bukkit.generator.ChunkGenerator;
import org.bukkit.generator.WorldInfo;
import org.jspecify.annotations.Nullable;

public class CustomChunkGenerator
extends InternalChunkGenerator {
    private static final long INITIAL_SEED = 0L;
    private final ChunkGenerator delegate;
    private final org.bukkit.generator.ChunkGenerator generator;
    private final ServerLevel world;
    private final Random random = new Random();
    private boolean newApi;
    private boolean implementBaseHeight = true;

    public CustomChunkGenerator(ServerLevel world, ChunkGenerator delegate, org.bukkit.generator.ChunkGenerator generator) {
        super(delegate.getBiomeSource(), delegate.generationSettingsGetter);
        this.world = world;
        this.delegate = delegate;
        this.generator = generator;
    }

    public ChunkGenerator getDelegate() {
        return this.delegate;
    }

    private static WorldgenRandom getSeededRandom() {
        return new WorldgenRandom(new LegacyRandomSource(0L));
    }

    @Override
    public BiomeSource getBiomeSource() {
        return this.delegate.getBiomeSource();
    }

    @Override
    public int getMinY() {
        return this.delegate.getMinY();
    }

    @Override
    public int getSeaLevel() {
        return this.delegate.getSeaLevel();
    }

    @Override
    public void createStructures(RegistryAccess registryAccess, ChunkGeneratorStructureState state, StructureManager structureManager, ChunkAccess centerChunk, StructureTemplateManager structureTemplateManager, ResourceKey<Level> level) {
        WorldgenRandom random = CustomChunkGenerator.getSeededRandom();
        int x = centerChunk.getPos().x();
        int z = centerChunk.getPos().z();
        random.setSeed(Mth.getSeed(x, "should-structures".hashCode(), z) ^ this.world.getSeed());
        if (this.generator.shouldGenerateStructures((WorldInfo)this.world.getWorld(), (Random)new RandomSourceWrapper.RandomWrapper(random), x, z)) {
            super.createStructures(registryAccess, state, structureManager, centerChunk, structureTemplateManager, level);
        }
    }

    public boolean shouldGenerateSurface(ChunkAccess protoChunk) {
        WorldgenRandom random = CustomChunkGenerator.getSeededRandom();
        int x = protoChunk.getPos().x();
        int z = protoChunk.getPos().z();
        random.setSeed(Mth.getSeed(x, "should-surface".hashCode(), z) ^ this.world.getSeed());
        return this.generator.shouldGenerateSurface((WorldInfo)this.world.getWorld(), (Random)new RandomSourceWrapper.RandomWrapper(random), x, z);
    }

    /*
     * WARNING - Removed try catching itself - possible behaviour change.
     */
    public void buildSurface(ChunkAccess protoChunk) {
        ChunkGenerator.ChunkData data;
        int z;
        int x;
        block13: {
            WorldgenRandom random = CustomChunkGenerator.getSeededRandom();
            x = protoChunk.getPos().x();
            z = protoChunk.getPos().z();
            CraftChunkData chunkData = new CraftChunkData(this.world.getWorld(), protoChunk);
            random.setLargeFeatureWithSalt(0L, x, z, 0);
            this.generator.generateSurface((WorldInfo)this.world.getWorld(), (Random)new RandomSourceWrapper.RandomWrapper(random), x, z, (ChunkGenerator.ChunkData)chunkData);
            random = CustomChunkGenerator.getSeededRandom();
            random.setLargeFeatureWithSalt(0L, x, z, 0);
            this.generator.generateBedrock((WorldInfo)this.world.getWorld(), (Random)new RandomSourceWrapper.RandomWrapper(random), x, z, (ChunkGenerator.ChunkData)chunkData);
            chunkData.breakLink();
            if (this.newApi) {
                return;
            }
            this.random.setSeed((long)x * 341873128712L + (long)z * 132897987541L);
            CustomBiomeGrid biomeGrid = new CustomBiomeGrid(this, protoChunk);
            try {
                if (this.generator.isParallelCapable()) {
                    data = this.generator.generateChunkData((World)this.world.getWorld(), this.random, x, z, (ChunkGenerator.BiomeGrid)biomeGrid);
                    break block13;
                }
                CustomChunkGenerator customChunkGenerator = this;
                synchronized (customChunkGenerator) {
                    data = this.generator.generateChunkData((World)this.world.getWorld(), this.random, x, z, (ChunkGenerator.BiomeGrid)biomeGrid);
                }
            }
            catch (UnsupportedOperationException exception) {
                this.newApi = true;
                return;
            }
        }
        Preconditions.checkArgument((boolean)(data instanceof OldCraftChunkData), (String)"Plugins must use createChunkData(World) rather than implementing ChunkData: %s", (Object)data);
        OldCraftChunkData craftData = (OldCraftChunkData)data;
        LevelChunkSection[] sections = craftData.getRawChunkData();
        LevelChunkSection[] protoSections = protoChunk.getSections();
        int sectionCount = Math.min(protoSections.length, sections.length);
        for (int sectionIndex = 0; sectionIndex < sectionCount; ++sectionIndex) {
            if (sections[sectionIndex] == null) continue;
            LevelChunkSection section = sections[sectionIndex];
            LevelChunkSection oldSection = protoSections[sectionIndex];
            for (int biomeX = 0; biomeX < 4; ++biomeX) {
                for (int biomeY = 0; biomeY < 4; ++biomeY) {
                    for (int biomeZ = 0; biomeZ < 4; ++biomeZ) {
                        section.setNoiseBiome(biomeX, biomeY, biomeZ, oldSection.getNoiseBiome(biomeX, biomeY, biomeZ));
                    }
                }
            }
            protoSections[sectionIndex] = section;
        }
        if (craftData.getTiles() != null) {
            for (BlockPos pos : craftData.getTiles()) {
                BlockEntity blockEntity;
                int tz;
                int ty;
                int tx = pos.getX();
                BlockState state = craftData.getBlockState(tx, ty = pos.getY(), tz = pos.getZ());
                if (!state.hasBlockEntity() || (blockEntity = ((EntityBlock)((Object)state.getBlock())).newBlockEntity(new BlockPos((x << 4) + tx, ty, (z << 4) + tz), state)) == null) continue;
                protoChunk.setBlockEntity(blockEntity);
            }
        }
    }

    public boolean shouldGenerateCaves(ChunkAccess chunk) {
        WorldgenRandom random = CustomChunkGenerator.getSeededRandom();
        int x = chunk.getPos().x();
        int z = chunk.getPos().z();
        random.setSeed(Mth.getSeed(x, "should-caves".hashCode(), z) ^ this.world.getSeed());
        return this.generator.shouldGenerateCaves((WorldInfo)this.world.getWorld(), (Random)new RandomSourceWrapper.RandomWrapper(random), x, z);
    }

    public void applyCarvers(ChunkAccess chunk) {
        WorldgenRandom random = CustomChunkGenerator.getSeededRandom();
        int x = chunk.getPos().x();
        int z = chunk.getPos().z();
        CraftChunkData chunkData = new CraftChunkData(this.world.getWorld(), chunk);
        random.setDecorationSeed(this.world.getSeed(), 0, 0);
        this.generator.generateCaves((WorldInfo)this.world.getWorld(), (Random)new RandomSourceWrapper.RandomWrapper(random), x, z, (ChunkGenerator.ChunkData)chunkData);
        chunkData.breakLink();
    }

    @Override
    public CompletableFuture<ChunkAccess> buildTerrain(ChunkAccess chunk, Blender blender, RandomState randomState, StructureManager structureManager, BiomeManager biomeManager, @Nullable WorldGenRegion carverBiomeRegion, Set<Holder<net.minecraft.world.level.biome.Biome>> possibleBiomes) {
        ChunkGenerator chunkGenerator = this.delegate;
        if (chunkGenerator instanceof NoiseBasedChunkGenerator) {
            NoiseBasedChunkGenerator noiseBasedChunkGenerator = (NoiseBasedChunkGenerator)chunkGenerator;
            return noiseBasedChunkGenerator.buildTerrain(chunk, blender, randomState, structureManager, biomeManager, carverBiomeRegion, possibleBiomes, this);
        }
        CompletableFuture<ChunkAccess> future = null;
        if (this.shouldGenerateNoise(chunk)) {
            future = this.delegate.buildTerrain(chunk, blender, randomState, structureManager, biomeManager, carverBiomeRegion, possibleBiomes);
        }
        Function<ChunkAccess, ChunkAccess> work = c -> {
            this.fillFromNoise((ChunkAccess)c);
            this.buildSurface((ChunkAccess)c);
            this.applyCarvers((ChunkAccess)c);
            return c;
        };
        return future == null ? CompletableFuture.supplyAsync(() -> (ChunkAccess)work.apply(chunk), FeatureHooks.getWorldgenExecutor()) : future.thenApply(work);
    }

    public boolean shouldGenerateNoise(ChunkAccess chunk) {
        WorldgenRandom random = CustomChunkGenerator.getSeededRandom();
        int x = chunk.getPos().x();
        int z = chunk.getPos().z();
        random.setSeed(Mth.getSeed(x, "should-noise".hashCode(), z) ^ this.world.getSeed());
        return this.generator.shouldGenerateNoise((WorldInfo)this.world.getWorld(), (Random)new RandomSourceWrapper.RandomWrapper(random), x, z);
    }

    public void fillFromNoise(ChunkAccess chunk) {
        WorldgenRandom random = CustomChunkGenerator.getSeededRandom();
        int x = chunk.getPos().x();
        int z = chunk.getPos().z();
        CraftChunkData chunkData = new CraftChunkData(this.world.getWorld(), chunk);
        random.setLargeFeatureWithSalt(0L, x, z, 0);
        this.generator.generateNoise((WorldInfo)this.world.getWorld(), (Random)new RandomSourceWrapper.RandomWrapper(random), x, z, (ChunkGenerator.ChunkData)chunkData);
        chunkData.breakLink();
    }

    @Override
    public int getBaseHeight(int x, int z, Heightmap.Types type, LevelHeightAccessor heightAccessor, RandomState randomState) {
        if (this.implementBaseHeight) {
            try {
                WorldgenRandom random = CustomChunkGenerator.getSeededRandom();
                random.setLargeFeatureWithSalt(0L, SectionPos.blockToSectionCoord(x), SectionPos.blockToSectionCoord(z), 0);
                return this.generator.getBaseHeight((WorldInfo)this.world.getWorld(), (Random)new RandomSourceWrapper.RandomWrapper(random), x, z, CraftHeightMap.fromNMS(type));
            }
            catch (UnsupportedOperationException exception) {
                this.implementBaseHeight = false;
            }
        }
        return this.delegate.getBaseHeight(x, z, type, heightAccessor, randomState);
    }

    @Override
    public WeightedList<MobSpawnSettings.SpawnerData> getMobsAt(Level level, StructureManager structureManager, MobCategory mobCategory, BlockPos pos) {
        return this.delegate.getMobsAt(level, structureManager, mobCategory, pos);
    }

    @Override
    public void applyBiomeDecoration(WorldGenLevel level, ChunkAccess chunk, StructureManager structureManager) {
        WorldgenRandom random = CustomChunkGenerator.getSeededRandom();
        int x = chunk.getPos().x();
        int z = chunk.getPos().z();
        random.setSeed(Mth.getSeed(x, "should-decoration".hashCode(), z) ^ level.getSeed());
        super.applyBiomeDecoration(level, chunk, structureManager, this.generator.shouldGenerateDecorations((WorldInfo)this.world.getWorld(), (Random)new RandomSourceWrapper.RandomWrapper(random), x, z));
    }

    @Override
    public void addDebugScreenInfo(List<String> result, RandomState randomState, BlockPos feetPos, SamplerContext samplerContext) {
        this.delegate.addDebugScreenInfo(result, randomState, feetPos, samplerContext);
    }

    @Override
    public void spawnOriginalMobs(WorldGenRegion worldGenRegion) {
        WorldgenRandom random = CustomChunkGenerator.getSeededRandom();
        int x = worldGenRegion.getCenter().x();
        int z = worldGenRegion.getCenter().z();
        random.setSeed(Mth.getSeed(x, "should-mobs".hashCode(), z) ^ worldGenRegion.getSeed());
        if (this.generator.shouldGenerateMobs((WorldInfo)this.world.getWorld(), (Random)new RandomSourceWrapper.RandomWrapper(random), x, z)) {
            this.delegate.spawnOriginalMobs(worldGenRegion);
        }
    }

    @Override
    public int getSpawnHeight(LevelHeightAccessor heightAccessor) {
        return this.delegate.getSpawnHeight(heightAccessor);
    }

    @Override
    public int getGenDepth() {
        return this.delegate.getGenDepth();
    }

    @Override
    public NoiseColumn getBaseColumn(int x, int z, LevelHeightAccessor heightAccessor, RandomState randomState) {
        return this.delegate.getBaseColumn(x, z, heightAccessor, randomState);
    }

    @Override
    protected MapCodec<? extends ChunkGenerator> codec() {
        return MapCodec.unit(null);
    }

    @Deprecated
    private class CustomBiomeGrid
    implements ChunkGenerator.BiomeGrid {
        private final ChunkAccess chunk;
        final /* synthetic */ CustomChunkGenerator this$0;

        public CustomBiomeGrid(CustomChunkGenerator customChunkGenerator, ChunkAccess chunk) {
            CustomChunkGenerator customChunkGenerator2 = customChunkGenerator;
            Objects.requireNonNull(customChunkGenerator2);
            this.this$0 = customChunkGenerator2;
            this.chunk = chunk;
        }

        public Biome getBiome(int x, int z) {
            return this.getBiome(x, 0, z);
        }

        public void setBiome(int x, int z, Biome biome) {
            for (int y = this.this$0.world.getWorld().getMinHeight(); y < this.this$0.world.getWorld().getMaxHeight(); y += 4) {
                this.setBiome(x, y, z, biome);
            }
        }

        public Biome getBiome(int x, int y, int z) {
            return CraftBiome.minecraftHolderToBukkit(this.chunk.getNoiseBiome(QuartPos.fromBlock(x), QuartPos.fromBlock(y), QuartPos.fromBlock(z)));
        }

        public void setBiome(int x, int y, int z, Biome biome) {
            Holder<net.minecraft.world.level.biome.Biome> b = CraftBiome.bukkitToMinecraftHolder(biome);
            Preconditions.checkArgument((b != null ? 1 : 0) != 0, (String)"Cannot set the biome to %s", (Object)biome);
            this.chunk.setNoiseBiome(QuartPos.fromBlock(x), QuartPos.fromBlock(y), QuartPos.fromBlock(z), b);
        }
    }
}

