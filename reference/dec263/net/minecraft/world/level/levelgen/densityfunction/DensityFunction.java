package net.minecraft.world.level.levelgen.densityfunction;

import com.mojang.serialization.Codec;
import com.mojang.serialization.MapCodec;
import java.lang.annotation.ElementType;
import java.lang.annotation.Retention;
import java.lang.annotation.RetentionPolicy;
import java.lang.annotation.Target;
import java.util.Objects;
import net.minecraft.core.Holder;
import net.minecraft.core.Direction.Axis;
import net.minecraft.core.Holder.Direct;
import net.minecraft.core.Holder.Reference;
import net.minecraft.core.registries.Registries;
import net.minecraft.core.registries.codec.RegistryCodecs;
import net.minecraft.resources.Identifier;
import net.minecraft.util.Interval;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.levelgen.synth.Noise;
import net.minecraft.world.level.levelgen.synth.NormalNoise;

public interface DensityFunction {
   Codec<Holder<DensityFunction>> REFERENCE_CODEC = RegistryCodecs.holder(Registries.DENSITY_FUNCTION);
   Codec<DensityFunction> CODEC = RegistryCodecs.holder(Registries.DENSITY_FUNCTION, DensityFunctions.DIRECT_CODEC).xmap(holder -> {
      Objects.requireNonNull(holder);

      return (DensityFunction)(switch (holder) {
         case Direct direct -> (DensityFunction)direct.value();
         case Reference reference -> new DensityFunctions.HolderHolder(reference);
         default -> throw new MatchException(null, null);
      });
   }, value -> {
      Objects.requireNonNull(value);

      return switch (value) {
         case DensityFunctions.HolderHolder $b$0 -> {
            DensityFunctions.HolderHolder var7 = $b$0;

            try {
               var8 = var7.function();
            } catch (Throwable var6) {
               throw new MatchException(var6.toString(), var6);
            }

            Holder patt3$temp = var8;
            yield patt3$temp;
         }
         default -> Holder.direct(value);
      };
   });
   int AXIS_X = 1;
   int AXIS_Y = 2;
   int AXIS_Z = 4;
   @DensityFunction.Axes
   int NO_AXES = 0;
   @DensityFunction.Axes
   int ALL_AXES = 7;

   @DensityFunction.Axes
   static int axesFrom(Axis axis) {
      return switch (axis) {
         case X -> 1;
         case Y -> 2;
         case Z -> 4;
      };
   }

   DensitySampler compileSampler(DensityFunction.CompileContext var1);

   DensityFunction rewriteChildren(DfRewriteRule var1);

   Interval range();

   @DensityFunction.Axes
   int domainAxes();

   MapCodec<? extends DensityFunction> codec();

   default DensityFunction clamp(float min, float max) {
      return DensityFunctions.clamp(this, min, max);
   }

   default DensityFunction abs() {
      return DensityFunctions.abs(this);
   }

   default DensityFunction square() {
      return DensityFunctions.square(this);
   }

   default DensityFunction cube() {
      return DensityFunctions.cube(this);
   }

   default DensityFunction sqrt() {
      return DensityFunctions.sqrt(this);
   }

   default DensityFunction halfNegative() {
      return DensityFunctions.halfNegative(this);
   }

   default DensityFunction quarterNegative() {
      return DensityFunctions.quarterNegative(this);
   }

   default DensityFunction reciprocal() {
      return DensityFunctions.reciprocal(this);
   }

   default DensityFunction negate() {
      return DensityFunctions.negate(this);
   }

   default DensityFunction squeeze() {
      return DensityFunctions.squeeze(this);
   }

   default DensityFunction log() {
      return DensityFunctions.log(this);
   }

   default DensityFunction sign() {
      return DensityFunctions.sign(this);
   }

   default DensityFunction add(DensityFunction right) {
      return DensityFunctions.add(this, right);
   }

   default DensityFunction add(float right) {
      return DensityFunctions.add(this, DensityFunctions.constant(right));
   }

   default DensityFunction sub(DensityFunction right) {
      return DensityFunctions.sub(this, right);
   }

   default DensityFunction sub(float right) {
      return DensityFunctions.sub(this, DensityFunctions.constant(right));
   }

   default DensityFunction mul(DensityFunction right) {
      return DensityFunctions.mul(this, right);
   }

   default DensityFunction mul(float right) {
      return DensityFunctions.mul(this, DensityFunctions.constant(right));
   }

   default DensityFunction div(DensityFunction right) {
      return DensityFunctions.div(this, right);
   }

   default DensityFunction div(float right) {
      return DensityFunctions.div(this, DensityFunctions.constant(right));
   }

   default DensityFunction pow(DensityFunction exponent) {
      return DensityFunctions.pow(this, exponent);
   }

   default DensityFunction pow(float exponent) {
      if (exponent == 0.5F) {
         return this.sqrt();
      } else if (exponent == 2.0F) {
         return this.square();
      } else {
         return exponent == 3.0F ? this.cube() : DensityFunctions.pow(this, DensityFunctions.constant(exponent));
      }
   }

   @Retention(RetentionPolicy.CLASS)
   @Target({ElementType.TYPE_USE})
   public @interface Axes {
   }

   public interface CompileContext {
      Noise createNoiseSampler(Holder<NormalNoise> var1);

      RandomSource createRandom(Identifier var1);

      @Deprecated
      RandomSource createEndIslandRandom();
   }
}
