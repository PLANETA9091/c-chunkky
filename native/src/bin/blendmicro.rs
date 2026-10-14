//! BlendedNoise micro-benchmark: per-call cost of compute_batch (4-lane),
//! split into main (8 octaves) vs min/max (16+16) loops, plus the col
//! variant. Mirrors the slice-phase call pattern (corner grid, fixed x/z
//! per column, y stepping by cell_height).
use crussty_chunkgen::noise::BlendedNoise;
use crussty_chunkgen::rng::XoroshiroRandomSource;
use std::time::Instant;

fn main() {
    let seed: i64 = 3257840388504953787;
    let mut rng = XoroshiroRandomSource::new(seed);
    // vanilla overworld-ish blended params
    let b = BlendedNoise::create(&mut rng, 1.0, 1.0, 1.0, 1.0, 1.0);
    let n = 100_000usize;

    // realistic corner coords: fixed (x,z) column, y descending by 8
    let mut acc = 0u64;
    let mk = |i: usize| -> [[i32; 4]; 3] {
        let y = 32 - ((i % 40) as i32) * 8;
        [[i as i32 * 4, 4, 8, 12], [y, y - 8, y - 16, y - 24], [64, 68, 72, 76]]
    };
    for i in 0..2000 {
        let [bx, by, bz] = mk(i);
        let v = b.compute_batch(&bx, &by, &bz);
        acc ^= v[0].to_bits();
    }
    let t0 = Instant::now();
    for i in 0..n {
        let [bx, by, bz] = mk(i);
        let v = b.compute_batch(&bx, &by, &bz);
        acc ^= v[0].to_bits();
    }
    let d4 = t0.elapsed();

    // min/max loops only (approximate: measure compute of a clone with main
    // octaves intact is not separable without code changes — instead time
    // the main loop cost by calling with max amplitude skew). Just report
    // the total per-call cost; the split comes from the phases diag.
    let t1 = Instant::now();
    for i in 0..n {
        let [bx, by, bz] = mk(i);
        let v = b.compute_batch8(&[bx[0], bx[1], bx[2], bx[3], bx[0], bx[1], bx[2], bx[3]],
                                 &[by[0], by[1], by[2], by[3], by[0], by[1], by[2], by[3]],
                                 &[bz[0], bz[1], bz[2], bz[3], bz[0], bz[1], bz[2], bz[3]]);
        acc ^= v[0].to_bits();
    }
    let d8 = t1.elapsed();

    println!(
        "compute_batch4: {:.1} ns/call ({:.2} us per 4-lane point-set)",
        d4.as_nanos() as f64 / n as f64,
        d4.as_nanos() as f64 / n as f64 / 1000.0
    );
    println!(
        "compute_batch8 (duplicated lanes): {:.1} ns/call",
        d8.as_nanos() as f64 / n as f64
    );
    println!("acc={acc}");
}
