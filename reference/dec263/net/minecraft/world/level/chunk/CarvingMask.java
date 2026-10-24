package net.minecraft.world.level.chunk;

import java.util.BitSet;

public class CarvingMask implements CarverOutput {
   private final int minY;
   private final int maxY;
   private final int height;
   private final BitSet mask;

   public CarvingMask(int minY, int maxY) {
      this.minY = minY;
      this.maxY = maxY;
      this.height = maxY - minY + 1;
      this.mask = new BitSet(256 * this.height);
   }

   private int getIndex(int x, int y, int z) {
      return y - this.minY + (z + (x << 4)) * this.height;
   }

   @Override
   public int minY() {
      return this.minY;
   }

   @Override
   public int maxY() {
      return this.maxY;
   }

   @Override
   public void carve(int x, int y, int z) {
      this.mask.set(this.getIndex(x, y, z));
   }

   public void visit(CarvingMask.Visitor visitor) {
      int startIndex = this.mask.nextSetBit(0);

      while (startIndex != -1) {
         int endIndex = this.mask.nextClearBit(startIndex) - 1;
         this.visitSegment(visitor, startIndex, endIndex);
         startIndex = this.mask.nextSetBit(endIndex + 1);
      }
   }

   private void visitSegment(CarvingMask.Visitor visitor, int startIndex, int endIndex) {
      int startColumn = startIndex / this.height;
      int endColumn = endIndex / this.height;

      for (int column = startColumn; column <= endColumn; column++) {
         int columnX = column >> 4 & 15;
         int columnZ = column & 15;
         int columnBaseIndex = column * this.height;
         int bottomY = Math.max(startIndex - columnBaseIndex, 0) + this.minY;
         int topY = Math.min(endIndex - columnBaseIndex, this.height - 1) + this.minY;
         visitor.visitColumn(columnX, columnZ, bottomY, topY);
      }
   }

   public boolean isEmpty() {
      return this.mask.isEmpty();
   }

   @FunctionalInterface
   public interface Filter {
      boolean test(int var1, int var2, int var3);
   }

   @FunctionalInterface
   public interface Visitor {
      void visitColumn(int var1, int var2, int var3, int var4);
   }
}
