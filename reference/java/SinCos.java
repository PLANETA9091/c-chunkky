/** Exhaustive Java Math.sin/cos over the REAL ore domain:
 *  f = (next(24) * 5.9604645E-8F) * (float)Math.PI, k in [0, 2^24). */
public class SinCos {
    public static void main(String[] args) {
        int lo = Integer.parseInt(args[0]);
        int hi = Integer.parseInt(args[1]);
        StringBuilder sb = new StringBuilder();
        for (int k = lo; k < hi; k++) {
            float f = (float) k * 5.9604645E-8F * (float) Math.PI;
            double d = f;
            long s = Double.doubleToRawLongBits(Math.sin(d));
            long c = Double.doubleToRawLongBits(Math.cos(d));
            sb.append(Integer.toHexString(k)).append(' ')
              .append(Long.toHexString(s)).append(' ')
              .append(Long.toHexString(c)).append('\n');
            if (sb.length() > 1 << 20) { System.out.print(sb); sb.setLength(0); }
        }
        System.out.print(sb);
    }
}
