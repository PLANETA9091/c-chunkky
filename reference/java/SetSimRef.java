import java.util.*;

/** Replays an insert/pop trace through java.util.HashSet and prints the
 *  resulting pop order — validates the Rust JSetSim HashMap replication. */
public class SetSimRef {
    public static void main(String[] args) throws Exception {
        // stdin: lines "A x y z" (add) or "P" (pop-first) — prints pops
        java.io.BufferedReader r = new java.io.BufferedReader(new java.io.InputStreamReader(System.in));
        HashSet<long[]> set = new HashSet<>();  // encode pos as long[3]? need BlockPos-like hash — use custom
        // emulate Vec3i hashCode via a wrapper
        class P implements Comparable<P> {
            final int x, y, z;
            P(int x, int y, int z) { this.x = x; this.y = y; this.z = z; }
            public int hashCode() { return (y + z * 31) * 31 + x; }
            public boolean equals(Object o) { P p = (P) o; return x == p.x && y == p.y && z == p.z; }
            public int compareTo(P o) {
                if (y != o.y) return Integer.compare(y, o.y);
                if (z != o.z) return Integer.compare(z, o.z);
                return Integer.compare(x, o.x);
            }
            public String toString() { return x + "," + y + "," + z; }
        }
        HashSet<P> set2 = new HashSet<>();
        String line;
        StringBuilder pops = new StringBuilder();
        while ((line = r.readLine()) != null) {
            line = line.trim();
            if (line.isEmpty()) continue;
            if (line.equals("P")) {
                java.util.Iterator<P> it = set2.iterator();
                if (it.hasNext()) { P p = it.next(); it.remove(); pops.append(p).append(' '); }
            } else {
                String[] f = line.split("\\s+");
                set2.add(new P(Integer.parseInt(f[1]), Integer.parseInt(f[2]), Integer.parseInt(f[3])));
            }
        }
        System.out.println(pops);
    }
}
