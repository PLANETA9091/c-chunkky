package net.minecraft.world.level.levelgen.densityfunction;

import net.minecraft.core.Holder;
import net.minecraft.core.Direction.Axis;
import net.minecraft.world.level.levelgen.densityfunction.generator.ConstantFunction;
import net.minecraft.world.level.levelgen.densityfunction.generator.GradientFunction;
import net.minecraft.world.level.levelgen.densityfunction.op.SliceFunction;

@FunctionalInterface
public interface DfRewriteRule {
   DfRewriteRule INLINE_REFERENCE = function -> {
      DensityFunction var6;
      if (function instanceof DensityFunctions.HolderHolder $b$0) {
         DensityFunctions.HolderHolder var10000 = $b$0;

         try {
            var5 = var10000.function();
         } catch (Throwable var4) {
            throw new MatchException(var4.toString(), var4);
         }

         Holder patt1$temp = var5;
         var6 = (DensityFunction)patt1$temp.value();
      } else {
         var6 = function;
      }

      return var6;
   };
   DfRewriteRule SLICE_UNIFORM_AXES = new DfRewriteRule.SliceUniformAxes(7);

   static DfRewriteRule sequence(DfRewriteRule... rules) {
      return function -> {
         for (DfRewriteRule rule : rules) {
            function = rule.rewrite(function);
         }

         return function;
      };
   }

   DensityFunction rewrite(DensityFunction var1);

   public static record SliceUniformAxes(@DensityFunction.Axes int parentDomainAxes) implements DfRewriteRule {
      @Override
      public DensityFunction rewrite(DensityFunction function) {
         if (shouldSkip(function)) {
            return function;
         } else {
            int domainAxes = function.domainAxes();
            if (this.parentDomainAxes == domainAxes) {
               return function.rewriteChildren(this);
            } else {
               DensityFunction newFunction = function.rewriteChildren(new DfRewriteRule.SliceUniformAxes(domainAxes));
               int removedAxes = this.parentDomainAxes & ~domainAxes;
               return this.removeAxes(newFunction, removedAxes);
            }
         }
      }

      private DensityFunction removeAxes(DensityFunction function, @DensityFunction.Axes int axes) {
         int filteredAxes = axes & ~getExistingRemovedAxes(function);
         if ((filteredAxes & 1) != 0) {
            function = new SliceFunction(Axis.X, 0, function);
         }

         if ((filteredAxes & 4) != 0) {
            function = new SliceFunction(Axis.Z, 0, function);
         }

         if ((filteredAxes & 2) != 0) {
            function = new SliceFunction(Axis.Y, 0, function);
         }

         return function;
      }

      @DensityFunction.Axes
      private static int getExistingRemovedAxes(DensityFunction function) {
         int axes = 0;

         while (function instanceof SliceFunction) {
            SliceFunction slice = (SliceFunction)function;
            axes |= DensityFunction.axesFrom(slice.axis());
            function = slice.input();
         }

         return axes;
      }

      private static boolean shouldSkip(DensityFunction function) {
         return function instanceof ConstantFunction || function instanceof GradientFunction;
      }
   }
}
