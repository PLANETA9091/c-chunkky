package net.minecraft.world.level.levelgen.densityfunction.op;

import com.mojang.serialization.MapCodec;
import com.mojang.serialization.codecs.RecordCodecBuilder;
import net.minecraft.util.Interval;
import net.minecraft.world.level.levelgen.densityfunction.DensityBuffer;
import net.minecraft.world.level.levelgen.densityfunction.DensityFunction;
import net.minecraft.world.level.levelgen.densityfunction.DensitySampler;
import net.minecraft.world.level.levelgen.densityfunction.DensityVolume;
import net.minecraft.world.level.levelgen.densityfunction.DfRewriteRule;
import net.minecraft.world.level.levelgen.densityfunction.SamplerContext;
import net.minecraft.world.level.levelgen.densityfunction.ScopedDensityBuffer;
import net.minecraft.world.level.levelgen.densityfunction.generator.ConstantFunction;

public record PowFunction(DensityFunction base, DensityFunction exponent) implements DensityFunction {
   public static final MapCodec<PowFunction> CODEC = RecordCodecBuilder.mapCodec(
      i -> i.group(
               DensityFunction.CODEC.fieldOf("base").forGetter(PowFunction::base), DensityFunction.CODEC.fieldOf("exponent").forGetter(PowFunction::exponent)
            )
            .apply(i, PowFunction::new)
   );

   // $VF: Inserted dummy exception handlers to handle obfuscated exceptions
   @Override
   public DensitySampler compileSampler(DensityFunction.CompileContext context) {
      Throwable var15;
      label47: {
         DensitySampler base = this.base.compileSampler(context);
         DensitySampler exponent = this.exponent.compileSampler(context);
         if (this.base instanceof ConstantFunction var4) {
            ConstantFunction var10000 = var4;

            try {
               var16 = var10000.value();
            } catch (Throwable var12) {
               var15 = var12;
               boolean var10001 = false;
               break label47;
            }

            float var7 = var16;
            if (true) {
               return new PowFunction.ConstBaseSampler((double)var7, exponent);
            }
         }

         if (this.exponent instanceof ConstantFunction var14) {
            ConstantFunction var17 = var14;

            try {
               var18 = var17.value();
            } catch (Throwable var11) {
               var15 = var11;
               boolean var20 = false;
               break label47;
            }

            float var9 = var18;
            if (true) {
               return compileConstExponent(base, var9);
            }
         }

         return new PowFunction.Sampler(base, exponent);
      }

      Throwable var13 = var15;
      throw new MatchException(var13.toString(), var13);
   }

   private static DensitySampler compileConstExponent(DensitySampler base, float exponent) {
      float absExponent = Math.abs(exponent);
      DensitySampler specialSampler;
      if (absExponent == 0.5F) {
         specialSampler = new UnaryFunction.SqrtSampler(base);
      } else if (absExponent == 1.0F) {
         specialSampler = base;
      } else if (absExponent == 2.0F) {
         specialSampler = new UnaryFunction.SquareSampler(base);
      } else {
         if (absExponent != 3.0F) {
            return new PowFunction.ConstExponentSampler(base, (double)exponent);
         }

         specialSampler = new UnaryFunction.CubeSampler(base);
      }

      return (DensitySampler)(exponent >= 0.0F ? specialSampler : new UnaryFunction.ReciprocalSampler(specialSampler));
   }

   @Override
   public DensityFunction rewriteChildren(DfRewriteRule rule) {
      DensityFunction base = rule.rewrite(this.base);
      DensityFunction exponent = rule.rewrite(this.exponent);
      return base == this.base && exponent == this.exponent ? this : new PowFunction(base, exponent);
   }

   @Override
   public Interval range() {
      return Interval.pow(this.base.range(), this.exponent.range());
   }

   @DensityFunction.Axes
   @Override
   public int domainAxes() {
      return this.base.domainAxes() | this.exponent.domainAxes();
   }

   @Override
   public MapCodec<PowFunction> codec() {
      return CODEC;
   }

   private static record ConstBaseSampler(double base, DensitySampler exponent) implements DensitySampler {
      @Override
      public void sampleVolume(SamplerContext context, DensityBuffer outputBuffer, DensityVolume volume) {
         this.exponent.sampleVolume(context, outputBuffer, volume);

         for (int i = 0; i < outputBuffer.size(); i++) {
            outputBuffer.set(i, (float)Math.pow(this.base, (double)outputBuffer.get(i)));
         }
      }

      @Override
      public float sampleValue(SamplerContext context, int blockX, int blockY, int blockZ) {
         return (float)Math.pow(this.base, (double)this.exponent.sampleValue(context, blockX, blockY, blockZ));
      }
   }

   private static record ConstExponentSampler(DensitySampler base, double exponent) implements DensitySampler {
      @Override
      public void sampleVolume(SamplerContext context, DensityBuffer outputBuffer, DensityVolume volume) {
         this.base.sampleVolume(context, outputBuffer, volume);

         for (int i = 0; i < outputBuffer.size(); i++) {
            outputBuffer.set(i, (float)Math.pow((double)outputBuffer.get(i), this.exponent));
         }
      }

      @Override
      public float sampleValue(SamplerContext context, int blockX, int blockY, int blockZ) {
         return (float)Math.pow((double)this.base.sampleValue(context, blockX, blockY, blockZ), this.exponent);
      }
   }

   private static record Sampler(DensitySampler base, DensitySampler exponent) implements DensitySampler {
      @Override
      public void sampleVolume(SamplerContext context, DensityBuffer outputBuffer, DensityVolume volume) {
         this.base.sampleVolume(context, outputBuffer, volume);

         try (ScopedDensityBuffer exponentBuffer = context.acquireBuffer(volume)) {
            this.exponent.sampleVolume(context, exponentBuffer, volume);

            for (int i = 0; i < outputBuffer.size(); i++) {
               float base = outputBuffer.get(i);
               float exponent = exponentBuffer.get(i);
               outputBuffer.set(i, (float)Math.pow((double)base, (double)exponent));
            }
         }
      }

      @Override
      public float sampleValue(SamplerContext context, int blockX, int blockY, int blockZ) {
         return (float)Math.pow(
            (double)this.base.sampleValue(context, blockX, blockY, blockZ), (double)this.exponent.sampleValue(context, blockX, blockY, blockZ)
         );
      }
   }
}
