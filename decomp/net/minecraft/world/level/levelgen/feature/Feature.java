/*
 * Decompiled with CFR 0.152.
 * 
 * Could not load the following classes:
 *  com.mojang.serialization.Codec
 *  com.mojang.serialization.MapCodec
 *  net.minecraft.core.BlockPos
 *  net.minecraft.core.BlockPos$MutableBlockPos
 *  net.minecraft.core.Direction
 *  net.minecraft.core.Holder
 *  net.minecraft.core.HolderSet
 *  net.minecraft.core.registries.BuiltInRegistries
 *  net.minecraft.core.registries.Registries
 *  net.minecraft.core.registries.codec.RegistryCodecs
 *  net.minecraft.resources.ResourceKey
 *  net.minecraft.util.RandomSource
 *  net.minecraft.world.level.LevelWriter
 *  net.minecraft.world.level.WorldGenLevel
 *  net.minecraft.world.level.block.state.BlockState
 *  net.minecraft.world.level.chunk.ChunkGenerator
 */
package net.minecraft.world.level.levelgen.feature;

import com.mojang.serialization.Codec;
import com.mojang.serialization.MapCodec;
import java.util.function.Predicate;
import java.util.stream.Stream;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.core.Holder;
import net.minecraft.core.HolderSet;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.core.registries.Registries;
import net.minecraft.core.registries.codec.RegistryCodecs;
import net.minecraft.resources.ResourceKey;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.LevelWriter;
import net.minecraft.world.level.WorldGenLevel;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.ChunkGenerator;

public interface Feature {
    public static final Codec<Feature> DIRECT_CODEC = BuiltInRegistries.FEATURE_TYPE.byNameCodec().dispatch(Feature::codec, t -> t);
    public static final Codec<Holder<Feature>> CODEC = RegistryCodecs.holder((ResourceKey)Registries.FEATURE, DIRECT_CODEC);
    public static final Codec<HolderSet<Feature>> LIST_CODEC = RegistryCodecs.holderSet((ResourceKey)Registries.FEATURE, DIRECT_CODEC);

    public MapCodec<? extends Feature> codec();

    public boolean place(WorldGenLevel var1, ChunkGenerator var2, RandomSource var3, BlockPos var4);

    default public Stream<Holder<Feature>> getSubFeatures() {
        return Stream.empty();
    }

    default public void setBlock(LevelWriter level, BlockPos pos, BlockState blockState) {
        level.setBlockAndUpdate(pos, blockState);
    }

    default public void safeSetBlock(WorldGenLevel level, BlockPos pos, BlockState state, Predicate<BlockState> canReplace) {
        if (canReplace.test(level.getBlockState(pos))) {
            level.setBlock(pos, state, 2);
        }
    }

    default public void markAboveForPostProcessing(WorldGenLevel level, BlockPos placePos) {
        BlockPos.MutableBlockPos pos = placePos.mutable();
        for (int i = 0; i < 2; ++i) {
            pos.move(Direction.UP);
            if (level.getBlockState((BlockPos)pos).isAir()) {
                return;
            }
            level.getChunk((BlockPos)pos).markPosForPostProcessing((BlockPos)pos);
        }
    }
}
