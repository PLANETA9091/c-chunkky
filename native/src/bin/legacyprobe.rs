// RNG parity probe vs /tmp LegacyProbe.java (paper-26.3-257 jar): LegacyRandom
// next_f64 (lossy l2f/fmul/f2d) + Marsaglia next_gaussian, 4 seeds + the
// set_large_feature_seed re-seed pattern. Bit-for-bit f64 patterns must match.

use crussty_chunkgen::rng::LegacyRandom;

fn f64bits(v: f64) -> i64 {
    v.to_bits() as i64
}

fn main() {
    let mut out = String::new();
    for seed in [3257840388504953787i64, 12345, -987654321098, 1] {
        let mut r = LegacyRandom::new(seed);
        out.push_str(&format!("seed {seed} f64:"));
        for _ in 0..8 {
            out.push_str(&format!(" {}", f64bits(r.next_f64())));
        }
        out.push_str(" gauss:");
        for _ in 0..4 {
            out.push_str(&format!(" {}", f64bits(r.next_gaussian())));
        }
        out.push('\n');
    }
    // carver re-seed pattern: setSeed(base); l0=nextLong(); l1=nextLong();
    // setSeed(x*l0 ^ z*l1 ^ base); f64; g; g  (x=0, z=1)
    let base = 3257840388504953787i64;
    for i3 in 0..3i64 {
        let mut r = LegacyRandom::new(0);
        r.set_seed(base + i3);
        let l0 = r.next_long();
        let l1 = r.next_long();
        r.set_seed((0i64.wrapping_mul(l0)) ^ (1i64.wrapping_mul(l1)) ^ (base + i3));
        out.push_str(&format!(
            "carver{i3} f64 {} g {} g {}\n",
            f64bits(r.next_f64()),
            f64bits(r.next_gaussian()),
            f64bits(r.next_gaussian())
        ));
    }
    print!("{out}");
}
