public class FlowerRef {
    static long GOLDEN = -7046029254386353131L, SILVER = 7640891576956012809L;
    static long lo, hi;
    static long mixStafford13(long s) { s=(s^s>>>30)*-4658895280553007687L; s=(s^s>>>27)*-7723592293110705685L; return s^s>>>31; }
    static void setSeed(long seed) { long l=seed^SILVER, l1=l+GOLDEN; lo=mixStafford13(l); hi=mixStafford13(l1); if((lo|hi)==0L){lo=GOLDEN;hi=SILVER;} }
    static long xnext() { long l=lo,l1=hi,l2=Long.rotateLeft(l+l1,17)+l; l1^=l; lo=Long.rotateLeft(l,49)^l1^(l1<<21); hi=Long.rotateLeft(l1,28); return l2; }
    static int nextInt() { return (int)xnext(); } // XoroshiroRandomSource.nextInt = (int)nextLong
    static int nextBits(int b){ return (int)(xnext()>>>64-b); }
    static long wrNextLong(){ return ((long)nextBits(32)<<32)+nextBits(32); }
    static long setDecorationSeed(long levelSeed,int minX,int minZ){ setSeed(levelSeed); long a=wrNextLong()|1L,b=wrNextLong()|1L; long s=(long)minX*a+(long)minZ*b^levelSeed; setSeed(s); return s; }
    // XoroshiroRandomSource.nextInt(bound): Lemire with rejection on nextInt()
    static int nextIntBound(int bound) {
        long u = nextInt() & 0xFFFFFFFFL;
        long m = u * bound;
        long low = m & 0xFFFFFFFFL;
        if (low < bound) {
            long reject = Long.remainderUnsigned(~bound + 1L, bound);
            while (low < reject) {
                u = nextInt() & 0xFFFFFFFFL;
                m = u * bound;
                low = m & 0xFFFFFFFFL;
            }
        }
        return (int)(m >> 32);
    }
    public static void main(String[] a) {
        long seed = Long.parseLong(a[0]);
        int ox = Integer.parseInt(a[1]), oz = Integer.parseInt(a[2]);
        int gid = Integer.parseInt(a[3]), step = Integer.parseInt(a[4]);
        long deco = setDecorationSeed(seed, ox, oz);
        setSeed(deco + (long) gid + (long)(10000 * step));
        for (int i = 0; i < 6; i++) {
            int x = ox + nextIntBound(16);
            int z = oz + nextIntBound(16);
            System.out.println("P " + x + " " + z);
        }
    }
}
