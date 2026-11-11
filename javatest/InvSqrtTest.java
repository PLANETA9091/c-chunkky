public class InvSqrtTest {
    public static void main(String[] a) {
        long mism = 0, n = 0;
        // domain: distSqr (0..3*16^2=768ish) + pointOffset (1..2), plus thresholds 1/sqrt(layer+adj)
        for (double x = 0.000001; x < 20000; x = x * 1.0000001 + 1e-9) {
            n++;
            double a1 = Math.invsqrt(x);
            double a2 = 1.0 / Math.sqrt(x);
            if (Double.doubleToRawLongBits(a1) != Double.doubleToRawLongBits(a2)) {
                if (mism < 5) System.out.println("MISMATCH x=" + x + " invsqrt=" + Double.toHexString(a1) + " 1/sqrt=" + Double.toHexString(a2));
                mism++;
            }
        }
        System.out.println("n=" + n + " mismatches=" + mism);
    }
}
