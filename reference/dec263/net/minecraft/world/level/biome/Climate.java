package net.minecraft.world.level.biome;

import ca.spottedleaf.moonrise.patches.chunk_gen.climate_rtree.ChunkGenRTree;
import ca.spottedleaf.moonrise.patches.chunk_gen.climate_rtree.RTreeLeafDistance;
import com.google.common.annotations.VisibleForTesting;
import com.google.common.collect.ImmutableList;
import com.google.common.collect.Lists;
import com.mojang.datafixers.util.Pair;
import com.mojang.serialization.Codec;
import com.mojang.serialization.DataResult;
import com.mojang.serialization.MapCodec;
import com.mojang.serialization.codecs.RecordCodecBuilder;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Comparator;
import java.util.Iterator;
import java.util.List;
import java.util.Locale;
import java.util.stream.Collectors;
import net.minecraft.core.QuartPos;
import net.minecraft.util.ExtraCodecs;
import net.minecraft.util.Mth;
import net.minecraft.world.level.levelgen.densityfunction.DensitySampler;
import org.jspecify.annotations.Nullable;

public class Climate {
   private static final boolean DEBUG_SLOW_BIOME_SEARCH = false;
   private static final float QUANTIZATION_FACTOR = 10000.0F;
   @VisibleForTesting
   protected static final int PARAMETER_COUNT = 7;

   public static Climate.TargetPoint target(float temperature, float humidity, float continentalness, float erosion, float depth, float weirdness) {
      return new Climate.TargetPoint(
         quantizeCoord(temperature),
         quantizeCoord(humidity),
         quantizeCoord(continentalness),
         quantizeCoord(erosion),
         quantizeCoord(depth),
         quantizeCoord(weirdness)
      );
   }

   public static Climate.ParameterPoint parameters(
      float temperature, float humidity, float continentalness, float erosion, float depth, float weirdness, float offset
   ) {
      return new Climate.ParameterPoint(
         Climate.Parameter.point(temperature),
         Climate.Parameter.point(humidity),
         Climate.Parameter.point(continentalness),
         Climate.Parameter.point(erosion),
         Climate.Parameter.point(depth),
         Climate.Parameter.point(weirdness),
         quantizeCoord(offset)
      );
   }

   public static Climate.ParameterPoint parameters(
      Climate.Parameter temperature,
      Climate.Parameter humidity,
      Climate.Parameter continentalness,
      Climate.Parameter erosion,
      Climate.Parameter depth,
      Climate.Parameter weirdness,
      float offset
   ) {
      return new Climate.ParameterPoint(temperature, humidity, continentalness, erosion, depth, weirdness, quantizeCoord(offset));
   }

   public static long quantizeCoord(float coord) {
      return (long)(coord * 10000.0F);
   }

   public static float unquantizeCoord(long coord) {
      return (float)coord / 10000.0F;
   }

   @VisibleForTesting
   interface DistanceMetric<T> {
      long distance(Climate.RTree.Node<T> var1, long[] var2);
   }

   public static record Parameter(long min, long max) {
      public static final Codec<Climate.Parameter> CODEC = ExtraCodecs.intervalCodec(
         Codec.floatRange(-2.0F, 2.0F),
         "min",
         "max",
         (min, max) -> min.compareTo(max) > 0
               ? DataResult.error(() -> "Cannon construct interval, min > max (" + min + " > " + max + ")")
               : DataResult.success(new Climate.Parameter(Climate.quantizeCoord(min), Climate.quantizeCoord(max))),
         p -> Climate.unquantizeCoord(p.min()),
         p -> Climate.unquantizeCoord(p.max())
      );

      public static Climate.Parameter point(float min) {
         return span(min, min);
      }

      public static Climate.Parameter span(float min, float max) {
         if (min > max) {
            throw new IllegalArgumentException("min > max: " + min + " " + max);
         } else {
            return new Climate.Parameter(Climate.quantizeCoord(min), Climate.quantizeCoord(max));
         }
      }

      public static Climate.Parameter span(Climate.Parameter min, Climate.Parameter max) {
         if (min.min() > max.max()) {
            throw new IllegalArgumentException("min > max: " + min + " " + max);
         } else {
            return new Climate.Parameter(min.min(), max.max());
         }
      }

      @Override
      public String toString() {
         return this.min == this.max ? String.format(Locale.ROOT, "%d", this.min) : String.format(Locale.ROOT, "[%d-%d]", this.min, this.max);
      }

      public long distance(long target) {
         long above = target - this.max;
         long below = this.min - target;
         return above > 0L ? above : Math.max(below, 0L);
      }

      public Climate.Parameter span(@Nullable Climate.Parameter other) {
         return other == null ? this : new Climate.Parameter(Math.min(this.min, other.min()), Math.max(this.max, other.max()));
      }
   }

   public static class ParameterList<T> {
      private final List<Pair<Climate.ParameterPoint, T>> values;
      private final Climate.RTree<T> index;
      private static final boolean DEBUG_PARAM_LOOKUP = false;

      public static <T> Codec<Climate.ParameterList<T>> codec(MapCodec<T> valueCodec) {
         return ExtraCodecs.nonEmptyList(
               RecordCodecBuilder.create(
                     i -> i.group(Climate.ParameterPoint.CODEC.fieldOf("parameters").forGetter(Pair::getFirst), valueCodec.forGetter(Pair::getSecond))
                           .apply(i, Pair::of)
                  )
                  .listOf()
            )
            .xmap(Climate.ParameterList::new, Climate.ParameterList::values);
      }

      public ParameterList(List<Pair<Climate.ParameterPoint, T>> values) {
         this(values, 19);
      }

      private ParameterList(List<Pair<Climate.ParameterPoint, T>> values, int childrenPerNode) {
         this.values = values;
         this.index = Climate.RTree.create(values, childrenPerNode);
      }

      @VisibleForTesting
      public Climate.ParameterList<T> rebuildWithChildrenPerNode(int childrenPerNode) {
         return new Climate.ParameterList<>(this.values, childrenPerNode);
      }

      public List<Pair<Climate.ParameterPoint, T>> values() {
         return this.values;
      }

      public T findValue(Climate.TargetPoint target) {
         return this.index.moonrise$findNearest(target);
      }

      @VisibleForTesting
      public T findValueBruteForce(Climate.TargetPoint target) {
         Iterator<Pair<Climate.ParameterPoint, T>> iterator = this.values().iterator();
         Pair<Climate.ParameterPoint, T> first = iterator.next();
         long bestFitness = ((Climate.ParameterPoint)first.getFirst()).fitness(target);
         T best = (T)first.getSecond();

         while (iterator.hasNext()) {
            Pair<Climate.ParameterPoint, T> parameter = iterator.next();
            long fitness = ((Climate.ParameterPoint)parameter.getFirst()).fitness(target);
            if (fitness < bestFitness) {
               bestFitness = fitness;
               best = (T)parameter.getSecond();
            }
         }

         return best;
      }

      public T findValueIndex(Climate.TargetPoint target) {
         return this.findValueIndex(target, Climate.RTree.Node::distance);
      }

      protected T findValueIndex(Climate.TargetPoint target, Climate.DistanceMetric<T> distanceMetric) {
         return this.index.search(target, distanceMetric);
      }
   }

   public static record ParameterPoint(
      Climate.Parameter temperature,
      Climate.Parameter humidity,
      Climate.Parameter continentalness,
      Climate.Parameter erosion,
      Climate.Parameter depth,
      Climate.Parameter weirdness,
      long offset
   ) {
      public static final Codec<Climate.ParameterPoint> CODEC = RecordCodecBuilder.create(
         i -> i.group(
                  Climate.Parameter.CODEC.fieldOf("temperature").forGetter(p -> p.temperature),
                  Climate.Parameter.CODEC.fieldOf("humidity").forGetter(p -> p.humidity),
                  Climate.Parameter.CODEC.fieldOf("continentalness").forGetter(p -> p.continentalness),
                  Climate.Parameter.CODEC.fieldOf("erosion").forGetter(p -> p.erosion),
                  Climate.Parameter.CODEC.fieldOf("depth").forGetter(p -> p.depth),
                  Climate.Parameter.CODEC.fieldOf("weirdness").forGetter(p -> p.weirdness),
                  Codec.floatRange(0.0F, 1.0F).fieldOf("offset").xmap(Climate::quantizeCoord, Climate::unquantizeCoord).forGetter(p -> p.offset)
               )
               .apply(i, Climate.ParameterPoint::new)
      );

      public long fitness(Climate.TargetPoint target) {
         return Mth.square(this.temperature.distance(target.temperature))
            + Mth.square(this.humidity.distance(target.humidity))
            + Mth.square(this.continentalness.distance(target.continentalness))
            + Mth.square(this.erosion.distance(target.erosion))
            + Mth.square(this.depth.distance(target.depth))
            + Mth.square(this.weirdness.distance(target.weirdness))
            + Mth.square(this.offset);
      }

      List<Climate.Parameter> parameterSpace() {
         return ImmutableList.of(
            this.temperature, this.humidity, this.continentalness, this.erosion, this.depth, this.weirdness, new Climate.Parameter(this.offset, this.offset)
         );
      }
   }

   public static final class RTree<T> implements ChunkGenRTree<T> {
      private static final int CHILDREN_PER_NODE = 19;
      private final Climate.RTree.Node<T> root;
      private final ThreadLocal<Climate.RTree.Leaf<T>> lastResult = new ThreadLocal<>();
      private final ThreadLocal<RTreeLeafDistance<T>> lastReturned = ThreadLocal.withInitial(() -> new RTreeLeafDistance<>(null, Long.MAX_VALUE));

      private void findNearest0(long[] params, RTreeLeafDistance<T> suggested, Climate.RTree.SubTree<T> node) {
         for (Climate.RTree.Node<T> child : node.children) {
            long childDist = child.distance(params);
            if (childDist < suggested.distance) {
               if (child instanceof Climate.RTree.Leaf<T> leaf) {
                  suggested.leaf = leaf;
                  suggested.distance = childDist;
               } else {
                  this.findNearest0(params, suggested, (Climate.RTree.SubTree<T>)child);
               }
            }
         }
      }

      private static void toParameterArray(Climate.TargetPoint target, long[] params) {
         params[0] = target.temperature();
         params[1] = target.humidity();
         params[2] = target.continentalness();
         params[3] = target.erosion();
         params[4] = target.depth();
         params[5] = target.weirdness();
         params[6] = 0L;
      }

      @Override
      public final T moonrise$findNearest(Climate.TargetPoint target) {
         if (this.root == null) {
            return null;
         } else if (this.root instanceof Climate.RTree.Leaf<T> ret) {
            return ret.value;
         } else {
            RTreeLeafDistance<T> ret = this.lastReturned.get();
            long[] params = ret.parameterArray;
            toParameterArray(target, params);
            if (ret.leaf != null) {
               ret.distance = ret.leaf.distance(params);
            }

            this.findNearest0(params, ret, (Climate.RTree.SubTree<T>)this.root);
            return ret.leaf.value;
         }
      }

      private RTree(Climate.RTree.Node<T> root) {
         this.root = root;
      }

      public static <T> Climate.RTree<T> create(List<Pair<Climate.ParameterPoint, T>> values) {
         return create(values, 19);
      }

      public static <T> Climate.RTree<T> create(List<Pair<Climate.ParameterPoint, T>> values, int childrenPerNode) {
         if (values.isEmpty()) {
            throw new IllegalArgumentException("Need at least one value to build the search tree.");
         } else {
            int dimensions = ((Climate.ParameterPoint)values.get(0).getFirst()).parameterSpace().size();
            if (dimensions != 7) {
               throw new IllegalStateException("Expecting parameter space to be 7, got " + dimensions);
            } else {
               List<Climate.RTree.Leaf<T>> leaves = values.stream()
                  .map(p -> new Climate.RTree.Leaf<>((Climate.ParameterPoint)p.getFirst(), p.getSecond()))
                  .collect(Collectors.toCollection(ArrayList::new));
               return new Climate.RTree<>(build(dimensions, leaves, childrenPerNode));
            }
         }
      }

      private static <T> Climate.RTree.Node<T> build(int dimensions, List<? extends Climate.RTree.Node<T>> children, int childrenPerNode) {
         if (children.isEmpty()) {
            throw new IllegalStateException("Need at least one child to build a node");
         } else if (children.size() == 1) {
            return (Climate.RTree.Node<T>)children.get(0);
         } else if (children.size() <= childrenPerNode) {
            children.sort(Comparator.comparingLong(leaf -> {
               long totalMagnitude = 0L;

               for (int dx = 0; dx < dimensions; dx++) {
                  Climate.Parameter parameter = leaf.parameterSpace[dx];
                  totalMagnitude += Math.abs((parameter.min() + parameter.max()) / 2L);
               }

               return totalMagnitude;
            }));
            return new Climate.RTree.SubTree<>(children);
         } else {
            long minCost = Long.MAX_VALUE;
            int minDimension = -1;
            List<Climate.RTree.SubTree<T>> minBuckets = null;

            for (int d = 0; d < dimensions; d++) {
               sort(children, dimensions, d, false);
               List<Climate.RTree.SubTree<T>> buckets = bucketize(children, childrenPerNode);
               long totalCost = 0L;

               for (Climate.RTree.SubTree<T> bucket : buckets) {
                  totalCost += cost(bucket.parameterSpace);
               }

               if (minCost > totalCost) {
                  minCost = totalCost;
                  minDimension = d;
                  minBuckets = buckets;
               }
            }

            sort(minBuckets, dimensions, minDimension, true);
            return new Climate.RTree.SubTree<>(
               minBuckets.stream().map(b -> build(dimensions, Arrays.asList(b.children), childrenPerNode)).collect(Collectors.toList())
            );
         }
      }

      private static <T> void sort(List<? extends Climate.RTree.Node<T>> children, int dimensions, int dimension, boolean absolute) {
         Comparator<Climate.RTree.Node<T>> comparator = comparator(dimension, absolute);

         for (int d = 1; d < dimensions; d++) {
            comparator = comparator.thenComparing(comparator((dimension + d) % dimensions, absolute));
         }

         children.sort(comparator);
      }

      private static <T> Comparator<Climate.RTree.Node<T>> comparator(int dimension, boolean absolute) {
         return Comparator.comparingLong(leaf -> {
            Climate.Parameter parameter = leaf.parameterSpace[dimension];
            long center = (parameter.min() + parameter.max()) / 2L;
            return absolute ? Math.abs(center) : center;
         });
      }

      private static <T> List<Climate.RTree.SubTree<T>> bucketize(List<? extends Climate.RTree.Node<T>> nodes, int childrenPerNode) {
         List<Climate.RTree.SubTree<T>> buckets = Lists.newArrayList();
         List<Climate.RTree.Node<T>> children = Lists.newArrayList();
         int expectedChildrenCount = (int)Math.pow(
            (double)childrenPerNode, Math.floor(Math.log((double)nodes.size() - 0.01) / Math.log((double)childrenPerNode))
         );

         for (Climate.RTree.Node<T> child : nodes) {
            children.add(child);
            if (children.size() >= expectedChildrenCount) {
               buckets.add(new Climate.RTree.SubTree<>(children));
               children = Lists.newArrayList();
            }
         }

         if (!children.isEmpty()) {
            buckets.add(new Climate.RTree.SubTree<>(children));
         }

         return buckets;
      }

      private static long cost(Climate.Parameter[] parameterSpace) {
         long result = 0L;

         for (Climate.Parameter parameter : parameterSpace) {
            result += Math.abs(parameter.max() - parameter.min());
         }

         return result;
      }

      private static <T> List<Climate.Parameter> buildParameterSpace(List<? extends Climate.RTree.Node<T>> children) {
         if (children.isEmpty()) {
            throw new IllegalArgumentException("SubTree needs at least one child");
         } else {
            int dimensions = 7;
            List<Climate.Parameter> bounds = Lists.newArrayList();

            for (int d = 0; d < 7; d++) {
               bounds.add(null);
            }

            for (Climate.RTree.Node<T> child : children) {
               for (int d = 0; d < 7; d++) {
                  bounds.set(d, child.parameterSpace[d].span(bounds.get(d)));
               }
            }

            return bounds;
         }
      }

      public T search(Climate.TargetPoint target, Climate.DistanceMetric<T> distanceMetric) {
         long[] targetArray = target.toParameterArray();
         Climate.RTree.Leaf<T> leaf = this.root.search(targetArray, this.lastResult.get(), distanceMetric);
         this.lastResult.set(leaf);
         return leaf.value;
      }

      public static final class Leaf<T> extends Climate.RTree.Node<T> {
         private final T value;

         private Leaf(Climate.ParameterPoint parameterPoint, T value) {
            super(parameterPoint.parameterSpace());
            this.value = value;
         }

         @Override
         protected Climate.RTree.Leaf<T> search(long[] target, @Nullable Climate.RTree.Leaf<T> candidate, Climate.DistanceMetric<T> distanceMetric) {
            return this;
         }
      }

      @VisibleForTesting
      abstract static class Node<T> {
         protected final Climate.Parameter[] parameterSpace;

         protected Node(List<Climate.Parameter> parameterSpace) {
            this.parameterSpace = parameterSpace.toArray(new Climate.Parameter[0]);
         }

         protected abstract Climate.RTree.Leaf<T> search(long[] var1, @Nullable Climate.RTree.Leaf<T> var2, Climate.DistanceMetric<T> var3);

         protected long distance(long[] target) {
            Climate.Parameter[] params = this.parameterSpace;
            if (params.length >= 7 && target.length >= 7) {
               long distance = 0L;

               for (int i = 0; i < 7; i++) {
                  distance += Mth.square(params[i].distance(target[i]));
               }

               return distance;
            } else {
               throw new IndexOutOfBoundsException();
            }
         }

         @Override
         public String toString() {
            return Arrays.toString((Object[])this.parameterSpace);
         }
      }

      private static final class SubTree<T> extends Climate.RTree.Node<T> {
         private final Climate.RTree.Node<T>[] children;

         public SubTree(List<? extends Climate.RTree.Node<T>> children) {
            this(Climate.RTree.buildParameterSpace(children), children);
         }

         public SubTree(List<Climate.Parameter> parameterSpace, List<? extends Climate.RTree.Node<T>> children) {
            super(parameterSpace);
            this.children = children.toArray(new Climate.RTree.Node[0]);
         }

         @Override
         protected Climate.RTree.Leaf<T> search(long[] target, @Nullable Climate.RTree.Leaf<T> candidate, Climate.DistanceMetric<T> distanceMetric) {
            long minDistance = candidate == null ? Long.MAX_VALUE : distanceMetric.distance(candidate, target);
            Climate.RTree.Leaf<T> closestLeaf = candidate;

            for (Climate.RTree.Node<T> child : this.children) {
               long childDistance = distanceMetric.distance(child, target);
               if (minDistance > childDistance) {
                  Climate.RTree.Leaf<T> leaf = child.search(target, closestLeaf, distanceMetric);
                  long leafDistance = child == leaf ? childDistance : distanceMetric.distance(leaf, target);
                  if (minDistance > leafDistance) {
                     minDistance = leafDistance;
                     closestLeaf = leaf;
                  }
               }
            }

            return closestLeaf;
         }
      }
   }

   public static record Sampler(
      DensitySampler.Bound temperature,
      DensitySampler.Bound humidity,
      DensitySampler.Bound continentalness,
      DensitySampler.Bound erosion,
      DensitySampler.Bound depth,
      DensitySampler.Bound weirdness
   ) {
      public Climate.TargetPoint sample(int quartX, int quartY, int quartZ) {
         int blockX = QuartPos.toBlock(quartX);
         int blockY = QuartPos.toBlock(quartY);
         int blockZ = QuartPos.toBlock(quartZ);
         return Climate.target(
            this.temperature.sampleValue(blockX, blockY, blockZ),
            this.humidity.sampleValue(blockX, blockY, blockZ),
            this.continentalness.sampleValue(blockX, blockY, blockZ),
            this.erosion.sampleValue(blockX, blockY, blockZ),
            this.depth.sampleValue(blockX, blockY, blockZ),
            this.weirdness.sampleValue(blockX, blockY, blockZ)
         );
      }
   }

   public static record TargetPoint(long temperature, long humidity, long continentalness, long erosion, long depth, long weirdness) {
      @VisibleForTesting
      long[] toParameterArray() {
         return new long[]{this.temperature, this.humidity, this.continentalness, this.erosion, this.depth, this.weirdness, 0L};
      }
   }
}
