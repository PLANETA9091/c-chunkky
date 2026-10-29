package net.minecraft.world.level.levelgen.structure.templatesystem;

import com.mojang.serialization.MapCodec;
import java.util.List;
import net.minecraft.core.BlockPos;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.block.state.BlockState;

public class AllOfRuleTest extends RuleTest {
   public static final MapCodec<AllOfRuleTest> CODEC = RuleTest.CODEC.listOf().fieldOf("rules").xmap(AllOfRuleTest::new, t -> t.rules);
   private final List<RuleTest> rules;
   private final RuleTest[] rulesArray;

   public AllOfRuleTest(List<RuleTest> rules) {
      this.rules = rules;
      this.rulesArray = rules.toArray(new RuleTest[0]);
   }

   @Override
   public boolean test(BlockState blockState, BlockPos pos, RandomSource random) {
      for (RuleTest rule : this.rulesArray) {
         if (!rule.test(blockState, pos, random)) {
            return false;
         }
      }

      return true;
   }

   @Override
   protected RuleTestType<?> getType() {
      return RuleTestType.ALL_OF_TEST;
   }
}
