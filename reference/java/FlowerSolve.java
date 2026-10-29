public class FlowerSolve {
    static long GOLDEN = -7046029254386353131L, SILVER = 7640891576956012809L;
    static long lo, hi;
    static long mixStafford13(long s) { s=(s^s>>>30)*-4658895280553007687L; s=(s^s>>>27)*-7723592293110705685L; return s^s>>>31; }
    static void setSeed(long seed) { long l=seed^SILVER, l1=l+GOLDEN; lo=mixStafford13(l); hi=mixStafford13(l1); if((lo|hi)==0L){lo=GOLDEN;hi=SILVER;} }
    static long xnext() { long l=lo,l1=hi,l2=Long.rotateLeft(l+l1,17)+l; l1^=l; lo=Long.rotateLeft(l,49)^l1^(l1<<21); hi=Long.rotateLeft(l1,28); return l2; }
    static int nextInt() { return (int)xnext(); }
    static int nextBits(int b){ return (int)(xnext()>>>64-b); }
    static long wrNextLong(){ return ((long)nextBits(32)<<32)+nextBits(32); }
    static long setDecorationSeed(long levelSeed,int minX,int minZ){ setSeed(levelSeed); long a=wrNextLong()|1L,b=wrNextLong()|1L; long s=(long)minX*a+(long)minZ*b^levelSeed; setSeed(s); return s; }
    static int nextIntBound(int bound) {
        long u = nextInt() & 0xFFFFFFFFL; long m = u * bound; long low = m & 0xFFFFFFFFL;
        if (low < bound) { long rej = Long.remainderUnsigned(~bound + 1L, bound);
            while (low < rej) { u = nextInt() & 0xFFFFFFFFL; m = u * bound; low = m & 0xFFFFFFFFL; } }
        return (int)(m >> 32);
    }
    public static void main(String[] a) {
        long seed = Long.parseLong(a[0]);
        // chunk (cx,cz) with observed poppy (x,z) pairs passed as cx,cz,x,z,cx,cz,x,z...
        java.util.List<int[]> obs = new java.util.ArrayList<>();
        for (int i = 1; i + 3 < a.length; i += 4) {
            obs.add(new int[]{Integer.parseInt(a[i]), Integer.parseInt(a[i+1]), Integer.parseInt(a[i+2]), Integer.parseInt(a[i+3])});
        }
        for (int step = 0; step <= 10; step++) {
            for (int gid = 0; gid <= 15; gid++) {
                boolean all = true;
                StringBuilder sb = new StringBuilder();
                for (int[] o : obs) {
                    int cx=o[0], cz=o[1];
                    long deco = setDecorationSeed(seed, cx*16, cz*16);
                    setSeed(deco + (long)gid + (long)(10000*step));
                    boolean hit = false;
                    for (int k = 0; k < 6 && !hit; k++) {
                        int x = cx*16 + nextIntBound(16);
                        int z = cz*16 + nextIntBound(16);
                        if (x==o[2] && z==o[3]) hit = true;
                        if (k==0) sb.append("(").append(x).append(",").append(z).append(")");
                    }
                    if (!hit) { all = false; break; }
                }
                if (all) System.out.println("MATCH step=" + step + " gid=" + gid + " first=" + sb);
            }
        }
    }
}
