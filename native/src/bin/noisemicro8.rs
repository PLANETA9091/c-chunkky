use crussty_chunkgen::noise::*;
use crussty_chunkgen::rng::XoroshiroRandomSource;
use std::time::Instant;

fn main() {
    let seed: i64 = 3257840388504953787;
    let mut rng = XoroshiroRandomSource::new(seed);
    let pn = PerlinNoise::create(&mut rng, -9, vec![1.0; 10]);
    let nl = pn.noise_levels[0].as_ref().unwrap();
    let n = 100_000usize;
    let mut acc = 0u64;
    let mut x8 = [0f64; 8];
    let mut x4 = [0f64; 4];
    for i in 0..8 { x8[i] = (i as f64) * 0.37 + 1.0; }
    for i in 0..4 { x4[i] = (i as f64) * 0.37 + 1.0; }
    // warm
    for i in 0..2000 {
        let v = nl.noise_5_batch8(&x8, &x8, &x8, 0.0, &[0.0; 8]);
        acc ^= v[0].to_bits();
        let v4 = nl.noise_5_batch(&x4, &x4, &x4, 0.0, &[0.0; 4]);
        acc ^= v4[0].to_bits();
    }
    let t0 = Instant::now();
    for i in 0..n {
        let mut xx = x8; xx[0] = i as f64 * 0.13;
        let v = nl.noise_5_batch8(&xx, &xx, &xx, 0.0, &[0.0; 8]);
        acc ^= v[0].to_bits();
    }
    let d8 = t0.elapsed();
    let t1 = Instant::now();
    for i in 0..(n * 2) {
        let mut xx = x4; xx[0] = i as f64 * 0.13;
        let v = nl.noise_5_batch(&xx, &xx, &xx, 0.0, &[0.0; 4]);
        acc ^= v[0].to_bits();
    }
    let d4 = t1.elapsed();
    println!("8-wide avx512: {:.1} ns/sample", d8.as_nanos() as f64 / n as f64 / 8.0);
    println!("4-wide avx2:   {:.1} ns/sample", d4.as_nanos() as f64 / (n * 2) as f64 / 4.0);
    println!("avx512f={}", std::arch::is_x86_feature_detected!("avx512f"));
    println!("acc={acc}");
}
