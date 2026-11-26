// f32x16 vs f64x8 ImprovedNoise head-to-head (cont-153 gate).
use crussty_chunkgen::noise::*;
use crussty_chunkgen::rng::XoroshiroRandomSource;
use std::time::Instant;

fn main() {
    let seed: i64 = 3257840388504953787;
    let mut rng = XoroshiroRandomSource::new(seed);
    let pn = PerlinNoise::create(&mut rng, -9, vec![1.0; 10]);
    let nl = pn.noise_levels[0].as_ref().unwrap();
    let n = 200_000usize;
    let mut x16 = [0f32; 16];
    let mut y16 = [0f32; 16];
    let mut z16 = [0f32; 16];
    for i in 0..16 { x16[i] = (i as f32) * 0.37 + 1.0; y16[i] = 64.0 + i as f32 * 0.11; z16[i] = i as f32 * 0.23; }
    let mut x8 = [0f64; 8];
    let mut y8 = [0f64; 8];
    let mut z8 = [0f64; 8];
    for i in 0..8 { x8[i] = x16[i] as f64; y8[i] = y16[i] as f64; z8[i] = z16[i] as f64; }
    let mut acc = 0u64;
    for _ in 0..2000 {
        let v = unsafe { nl.noise_5_batch16_f32_avx512(&x16, &y16, &z16) };
        acc ^= v[0].to_bits() as u64;
        let v8 = nl.noise_5_batch8(&x8, &y8, &z8, 0.0, &[0.0; 8]);
        acc ^= v8[0].to_bits();
    }
    let reps = (n / 16) as i32;
    let t0 = Instant::now();
    for i in 0..reps {
        let mut xx = x16; xx[0] = i as f32 * 0.13;
        let v = unsafe { nl.noise_5_batch16_f32_avx512(&xx, &y16, &z16) };
        acc ^= v[0].to_bits() as u64;
    }
    let d16 = t0.elapsed();
    let reps8 = (n / 8) as i32;
    let t1 = Instant::now();
    for i in 0..reps8 {
        let mut xx = x8; xx[0] = i as f64 * 0.13;
        let v = nl.noise_5_batch8(&xx, &y8, &z8, 0.0, &[0.0; 8]);
        acc ^= v[0].to_bits() as u64;
    }
    let d8 = t1.elapsed();
    println!("f32x16: {:.2} ns/sample | f64x8: {:.2} ns/sample | acc={acc}",
        d16.as_nanos() as f64 / n as f64,
        d8.as_nanos() as f64 / n as f64);
}
