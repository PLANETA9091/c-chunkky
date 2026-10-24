package net.minecraft.world.level.levelgen;

import net.minecraft.core.BlockPos;
import net.minecraft.core.BlockPos.MutableBlockPos;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.server.level.ServerPlayer;
import net.minecraft.stats.Stats;
import net.minecraft.util.RandomSource;
import net.minecraft.world.attribute.EnvironmentAttributes;
import net.minecraft.world.entity.EntitySpawnReason;
import net.minecraft.world.entity.EntityTypes;
import net.minecraft.world.entity.monster.PatrollingMonster;
import net.minecraft.world.level.CustomSpawner;
import net.minecraft.world.level.NaturalSpawner;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.gamerules.GameRules;
import net.minecraft.world.timeline.Timelines;
import org.bukkit.event.entity.CreatureSpawnEvent.SpawnReason;

public class PatrolSpawner implements CustomSpawner {
   private int nextTick;

   @Override
   public void tick(ServerLevel level, boolean spawnEnemies) {
      if (!level.paperConfig().entities.behavior.pillagerPatrols.disable && level.paperConfig().entities.behavior.pillagerPatrols.spawnChance != 0.0) {
         if (spawnEnemies && level.getGameRules().get(GameRules.SPAWN_PATROLS)) {
            RandomSource random = level.getRandom();
            int playerCount = level.players().size();
            if (playerCount < 1) {
               return;
            }

            ServerPlayer player = level.players().get(random.nextInt(playerCount));
            if (player.isSpectator()) {
               return;
            }

            int patrolSpawnDelay;
            if (level.paperConfig().entities.behavior.pillagerPatrols.spawnDelay.perPlayer) {
               player.patrolSpawnDelay--;
               patrolSpawnDelay = player.patrolSpawnDelay;
            } else {
               this.nextTick--;
               patrolSpawnDelay = this.nextTick;
            }

            if (patrolSpawnDelay <= 0) {
               long dayCount;
               if (level.paperConfig().entities.behavior.pillagerPatrols.start.perPlayer) {
                  dayCount = (long)(player.getStats().getValue(Stats.CUSTOM.get(Stats.PLAY_TIME)) / 24000);
               } else {
                  dayCount = (long)level.registryAccess()
                     .get(Timelines.OVERWORLD_DAY)
                     .map(timeline -> timeline.value().getPeriodCount(level.clockManager()))
                     .orElse(0)
                     .intValue();
               }

               if (level.paperConfig().entities.behavior.pillagerPatrols.spawnDelay.perPlayer) {
                  player.patrolSpawnDelay = player.patrolSpawnDelay
                     + level.paperConfig().entities.behavior.pillagerPatrols.spawnDelay.ticks
                     + random.nextInt(1200);
               } else {
                  this.nextTick = this.nextTick + level.paperConfig().entities.behavior.pillagerPatrols.spawnDelay.ticks + random.nextInt(1200);
               }

               if (dayCount >= (long)level.paperConfig().entities.behavior.pillagerPatrols.start.day
                  && level.isBrightOutside()
                  && random.nextDouble() < level.paperConfig().entities.behavior.pillagerPatrols.spawnChance
                  && playerCount >= 1
                  && !player.isSpectator()
                  && !level.isCloseToVillage(player.blockPosition(), 2)) {
                  int x = (24 + random.nextInt(24)) * (random.nextBoolean() ? -1 : 1);
                  int z = (24 + random.nextInt(24)) * (random.nextBoolean() ? -1 : 1);
                  MutableBlockPos spawnPos = player.blockPosition().mutable().move(x, 0, z);
                  int delta = 10;
                  if (level.hasChunksAt(spawnPos.getX() - 10, spawnPos.getZ() - 10, spawnPos.getX() + 10, spawnPos.getZ() + 10)
                     && level.environmentAttributes().getValue(EnvironmentAttributes.CAN_PILLAGER_PATROL_SPAWN, spawnPos)) {
                     int groupSize = (int)Math.ceil((double)level.getCurrentDifficultyAt(spawnPos).getEffectiveDifficulty()) + 1;

                     for (int i = 0; i < groupSize; i++) {
                        spawnPos.setY(level.getHeightmapPos(Heightmap.Types.MOTION_BLOCKING_NO_LEAVES, spawnPos).getY());
                        if (i == 0) {
                           if (!this.spawnPatrolMember(level, spawnPos, random, true)) {
                              break;
                           }
                        } else {
                           this.spawnPatrolMember(level, spawnPos, random, false);
                        }

                        spawnPos.setX(spawnPos.getX() + random.nextInt(5) - random.nextInt(5));
                        spawnPos.setZ(spawnPos.getZ() + random.nextInt(5) - random.nextInt(5));
                     }
                  }
               }
            }
         }
      }
   }

   private boolean spawnPatrolMember(ServerLevel level, BlockPos pos, RandomSource random, boolean isLeader) {
      BlockState state = level.getBlockState(pos);
      if (!NaturalSpawner.isValidEmptySpawnBlock(level, pos, state, state.getFluidState(), EntityTypes.PILLAGER)) {
         return false;
      } else if (!PatrollingMonster.checkPatrollingMonsterSpawnRules(EntityTypes.PILLAGER, level, EntitySpawnReason.PATROL, pos, random)) {
         return false;
      } else {
         PatrollingMonster mob = EntityTypes.PILLAGER.create(level, EntitySpawnReason.PATROL);
         if (mob != null) {
            if (isLeader) {
               mob.setPatrolLeader(true);
               mob.findPatrolTarget();
            }

            mob.setPos((double)pos.getX(), (double)pos.getY(), (double)pos.getZ());
            mob.finalizeSpawn(level, level.getCurrentDifficultyAt(pos), EntitySpawnReason.PATROL, null);
            level.addFreshEntityWithPassengers(mob, SpawnReason.PATROL);
            return true;
         } else {
            return false;
         }
      }
   }
}
