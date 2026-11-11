/*
 * Decompiled with CFR 0.152.
 * 
 * Could not load the following classes:
 *  net.minecraft.core.BlockPos
 *  net.minecraft.core.Direction
 *  net.minecraft.server.level.ServerLevel
 *  net.minecraft.util.RandomSource
 *  net.minecraft.world.level.Level
 *  net.minecraft.world.level.LevelAccessor
 *  net.minecraft.world.level.block.AmethystBlock
 *  net.minecraft.world.level.block.AmethystClusterBlock
 *  net.minecraft.world.level.block.Block
 *  net.minecraft.world.level.block.Blocks
 *  net.minecraft.world.level.block.state.BlockBehaviour$Properties
 *  net.minecraft.world.level.block.state.BlockState
 *  net.minecraft.world.level.block.state.properties.Property
 *  net.minecraft.world.level.material.Fluid
 *  net.minecraft.world.level.material.Fluids
 *  org.bukkit.craftbukkit.event.CraftEventFactory
 */
package net.minecraft.world.level.block;

import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.LevelAccessor;
import net.minecraft.world.level.block.AmethystBlock;
import net.minecraft.world.level.block.AmethystClusterBlock;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockBehaviour;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.Property;
import net.minecraft.world.level.material.Fluid;
import net.minecraft.world.level.material.Fluids;
import org.bukkit.craftbukkit.event.CraftEventFactory;

public class BuddingAmethystBlock
extends AmethystBlock {
    public static final int GROWTH_CHANCE = 5;
    private static final Direction[] DIRECTIONS = Direction.values();

    public BuddingAmethystBlock(BlockBehaviour.Properties properties) {
        super(properties);
    }

    protected void randomTick(BlockState state, ServerLevel level, BlockPos pos, RandomSource random) {
        if (random.nextInt(5) == 0) {
            Direction growDirection = DIRECTIONS[random.nextInt(DIRECTIONS.length)];
            BlockPos growPos = pos.relative(growDirection);
            BlockState relativeState = level.getBlockState(growPos);
            Block nextStage = null;
            if (BuddingAmethystBlock.canClusterGrowAtState(relativeState)) {
                nextStage = Blocks.SMALL_AMETHYST_BUD;
            } else if (relativeState.is(Blocks.SMALL_AMETHYST_BUD) && relativeState.getValue((Property)AmethystClusterBlock.FACING) == growDirection) {
                nextStage = Blocks.MEDIUM_AMETHYST_BUD;
            } else if (relativeState.is(Blocks.MEDIUM_AMETHYST_BUD) && relativeState.getValue((Property)AmethystClusterBlock.FACING) == growDirection) {
                nextStage = Blocks.LARGE_AMETHYST_BUD;
            } else if (relativeState.is(Blocks.LARGE_AMETHYST_BUD) && relativeState.getValue((Property)AmethystClusterBlock.FACING) == growDirection) {
                nextStage = Blocks.AMETHYST_CLUSTER;
            }
            if (nextStage != null) {
                BlockState targetState = (BlockState)((BlockState)nextStage.defaultBlockState().setValue((Property)AmethystClusterBlock.FACING, (Comparable)growDirection)).setValue((Property)AmethystClusterBlock.WATERLOGGED, (Comparable)Boolean.valueOf(relativeState.getFluidState().is((Fluid)Fluids.WATER)));
                if (nextStage == Blocks.SMALL_AMETHYST_BUD) {
                    CraftEventFactory.handleBlockSpreadEvent((LevelAccessor)level, (BlockPos)pos, (BlockPos)growPos, (BlockState)targetState, (int)3);
                } else {
                    CraftEventFactory.handleBlockGrowEvent((Level)level, (BlockPos)growPos, (BlockState)targetState, (int)3);
                }
            }
        }
    }

    public static boolean canClusterGrowAtState(BlockState state) {
        return state.isAir() || state.is(Blocks.WATER) && state.getFluidState().isFull();
    }
}
