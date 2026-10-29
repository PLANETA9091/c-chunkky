package net.minecraft.world.level.chunk;

public interface CarverOutput {
   void carve(int var1, int var2, int var3);

   int minY();

   int maxY();
}
