/*
 * Decompiled with CFR 0.152.
 * 
 * Could not load the following classes:
 *  com.mojang.datafixers.kinds.App
 *  com.mojang.datafixers.kinds.Applicative
 *  com.mojang.serialization.Codec
 *  com.mojang.serialization.codecs.RecordCodecBuilder
 *  net.minecraft.core.Holder
 *  net.minecraft.core.HolderSet
 *  net.minecraft.core.registries.Registries
 *  net.minecraft.core.registries.codec.RegistryCodecs
 *  net.minecraft.resources.ResourceKey
 *  net.minecraft.util.ExtraCodecs
 *  net.minecraft.world.level.block.Block
 *  net.minecraft.world.level.block.state.BlockState
 *  net.minecraft.world.level.levelgen.feature.stateproviders.BlockStateProvider
 */
package net.minecraft.world.level.levelgen;

import com.mojang.datafixers.kinds.App;
import com.mojang.datafixers.kinds.Applicative;
import com.mojang.serialization.Codec;
import com.mojang.serialization.codecs.RecordCodecBuilder;
import java.util.List;
import net.minecraft.core.Holder;
import net.minecraft.core.HolderSet;
import net.minecraft.core.registries.Registries;
import net.minecraft.core.registries.codec.RegistryCodecs;
import net.minecraft.resources.ResourceKey;
import net.minecraft.util.ExtraCodecs;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.levelgen.feature.stateproviders.BlockStateProvider;

public record GeodeBlockSettings(Holder<BlockStateProvider> fillingProvider, Holder<BlockStateProvider> innerLayerProvider, Holder<BlockStateProvider> alternateInnerLayerProvider, Holder<BlockStateProvider> middleLayerProvider, Holder<BlockStateProvider> outerLayerProvider, List<BlockState> innerPlacements, HolderSet<Block> cannotReplace, HolderSet<Block> invalidBlocks) {
    public static final Codec<GeodeBlockSettings> CODEC = RecordCodecBuilder.create(i -> i.group((App)BlockStateProvider.CODEC.fieldOf("filling_provider").forGetter(GeodeBlockSettings::fillingProvider), (App)BlockStateProvider.CODEC.fieldOf("inner_layer_provider").forGetter(GeodeBlockSettings::innerLayerProvider), (App)BlockStateProvider.CODEC.fieldOf("alternate_inner_layer_provider").forGetter(GeodeBlockSettings::alternateInnerLayerProvider), (App)BlockStateProvider.CODEC.fieldOf("middle_layer_provider").forGetter(GeodeBlockSettings::middleLayerProvider), (App)BlockStateProvider.CODEC.fieldOf("outer_layer_provider").forGetter(GeodeBlockSettings::outerLayerProvider), (App)ExtraCodecs.nonEmptyList((Codec)BlockState.CODEC.listOf()).fieldOf("inner_placements").forGetter(GeodeBlockSettings::innerPlacements), (App)RegistryCodecs.holderSet((ResourceKey)Registries.BLOCK).fieldOf("cannot_replace").forGetter(GeodeBlockSettings::cannotReplace), (App)RegistryCodecs.holderSet((ResourceKey)Registries.BLOCK).fieldOf("invalid_blocks").forGetter(GeodeBlockSettings::invalidBlocks)).apply((Applicative)i, GeodeBlockSettings::new));
}
