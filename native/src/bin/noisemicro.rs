use crussty_chunkgen::noise::*;
use crussty_chunkgen::rng::XoroshiroRandomSource;
use std::time::Instant;

fn main() {
    let seed: i64 = 3257840388504953787;
    let mut rng = XoroshiroRandomSource::new(seed);
    let pn = PerlinNoise::create(&mut rng, -9, vec![1.0; 10]);
    let n = 200_000usize; // batch calls
    let mut acc = 0u64;
    // warm
    for i in 0..1000 {
        let x = [(i as f64) * 0.1, 1.3, 2.7, 3.9];
        let v = pn.noise_levels[0].as_ref().unwrap().noise_batch(&x, &x, &x);
        acc ^= v[0].to_bits();
    }
    let t0 = Instant::now();
    for i in 0..n {
        let x = [(i as f64) * 0.1, 1.3, 2.7, 3.9];
        let v = pn.noise_levels[0].as_ref().unwrap().noise_batch(&x, &x, &x);
        acc ^= v[0].to_bits();
    }
    let d_batch = t0.elapsed();
    // scalar
    let t1 = Instant::now();
    for i in 0..n {
        let x0 = (i as f64) * 0.1; let x1 = 1.3; let x2 = 2.7; let x3 = 3.9;
        let nl = pn.noise_levels[0].as_ref().unwrap();
        let a = nl.noise(x0, x1, x2); let b = nl.noise(x1, x2, x3);
        let c = nl.noise(x2, x3, x0); let d = nl.noise(x3, x0, x1);
        acc ^= (a.to_bits() ^ b.to_bits()) ^ (c.to_bits() ^ d.to_bits());
    }
    let d_scalar = t1.elapsed();
    println!("batch(simd) 4 lanes: {:?} => {:.1} ns/sample", d_batch, d_batch.as_nanos() as f64 / n as f64 / 4.0);
    println!("scalar 4 samples:    {:?} => {:.1} ns/sample", d_scalar, d_scalar.as_nanos() as f64 / n as f64 / 4.0);
    println!("acc={acc}");
}
