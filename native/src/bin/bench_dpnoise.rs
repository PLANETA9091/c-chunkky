//! Micro-bench of the pack-path noise kernels on a realistic slice walk:
//! NormalNoise col path (two octave counts) + BlendedNoise col path.
//! Pattern: 49-corner y-walk (step 8) at one (x,z), then next z-column —
//! mirrors the production slice batch shape.
use crussty_chunkgen::noise::{BlendedNoise, NormalNoise};
use crussty_chunkgen::rng::XoroshiroRandomSource;
use std::time::Instant;

const YC: usize = 49; // corners per z-row (cell_count_y+1)
const ZCOLS: usize = 20; // z-rows per plane
const REPS: usize = 40;

fn walk_y(step: f64) -> [f64; 8] {
    let mut y = [0f64; 8];
    for l in 0..8 {
        y[l] = (-64.0 + ((l) as f64) * step) * 1.0;
    }
    y
}

fn bench_nn(name: &str, amps: Vec<f64>) {
    let mut rng = XoroshiroRandomSource::new(0x0DEC5101);
    let nn = NormalNoise::create(&mut rng, -5, amps);
    let oct = nn.first.noise_levels.len() + nn.second.noise_levels.len();
    let mut sink = 0u64;
    // warm
    for z in 0..4 {
        for g in 0..(YC / 8 + 1) {
            let mut y = [0f64; 8];
            for l in 0..8 {
                let p = (g * 8 + l).min(YC - 1);
                y[l] = (-64.0 + p as f64 * 8.0) * 1.0;
            }
            let v = nn.get_value_batch8(&[64.0; 8], &y, &[z as f64 * 4.0; 8]);
            for x in v {
                sink ^= x.to_bits();
            }
        }
    }
    let t0 = Instant::now();
    for _ in 0..REPS {
        for z in 0..ZCOLS {
            for g in 0..(YC / 8 + 1) {
                let mut y = [0f64; 8];
                for l in 0..8 {
                    let p = (g * 8 + l).min(YC - 1);
                    y[l] = (-64.0 + p as f64 * 8.0) * 1.0;
                }
                let v = nn.get_value_batch8(&[64.0; 8], &y, &[z as f64 * 4.0; 8]);
                for x in v {
                    sink ^= x.to_bits();
                }
            }
        }
    }
    let el = t0.elapsed().as_nanos() as f64;
    let pts = (REPS * ZCOLS * ((YC / 8 + 1) * 8)) as f64;
    println!(
        "{name}: {sink} {:.1}ns/pt {:.2}ns/oct/pt (octaves {oct})",
        el / pts,
        el / pts / oct as f64
    );
}

fn bench_blend() {
    let mut rng = XoroshiroRandomSource::new(0x0DEC5103);
    let bl = BlendedNoise::create(
        &mut rng,
        684.412, // xz multiplier
        8.555,   // y multiplier
        1.0,     // xz factor
        1.0,     // y factor
        80.0,    // smear scale
    );
    let mut sink = 0u64;
    for z in 0..4 {
        for g in 0..(YC / 8 + 1) {
            let mut y = [0i32; 8];
            for l in 0..8 {
                let p = (g * 8 + l).min(YC - 1);
                y[l] = -64 + p as i32 * 8;
            }
            let v = bl.compute_batch8_col(64, &y, z as i32 * 4, bl.col_base);
            for x in v {
                sink ^= x.to_bits();
            }
        }
    }
    let t0 = Instant::now();
    for _ in 0..REPS {
        for z in 0..ZCOLS {
            for g in 0..(YC / 8 + 1) {
                let mut y = [0i32; 8];
                for l in 0..8 {
                    let p = (g * 8 + l).min(YC - 1);
                    y[l] = -64 + p as i32 * 8;
                }
                let v = bl.compute_batch8_col(64, &y, z as i32 * 4, bl.col_base);
                for x in v {
                    sink ^= x.to_bits();
                }
            }
        }
    }
    let el = t0.elapsed().as_nanos() as f64;
    let pts = (REPS * ZCOLS * ((YC / 8 + 1) * 8)) as f64;
    // octaves: main 8 + min up to 16 + max up to 16
    let main_oct = (0..8).filter(|i| bl.main_noise.get_octave_noise(*i).is_some()).count();
    let min_oct = (0..16).filter(|i| bl.min_noise.get_octave_noise(*i).is_some()).count();
    let max_oct = (0..16).filter(|i| bl.max_noise.get_octave_noise(*i).is_some()).count();
    let oct = main_oct + min_oct + max_oct;
    println!(
        "blend: {sink} {:.1}ns/pt {:.2}ns/oct/pt (main {main_oct}+min {min_oct}+max {max_oct})",
        el / pts,
        el / pts / oct as f64
    );
}

fn bench_generic(nn: &NormalNoise, name: &str, oct: usize) {
    // lanes NOT uniform in x/z -> generic batch8 path (no column memo)
    let mut sink = 0u64;
    for _ in 0..4 {
        for z in 0..4 {
            for g in 0..(YC / 8 + 1) {
                let mut y = [0f64; 8];
                let mut x = [0f64; 8];
                for l in 0..8 {
                    let p = (g * 8 + l).min(YC - 1);
                    y[l] = -64.0 + p as f64 * 8.0;
                    x[l] = 64.0 + l as f64 * 4.0;
                }
                let v = nn.get_value_batch8(&x, &y, &[z as f64 * 4.0; 8]);
                for q in v {
                    sink ^= q.to_bits();
                }
            }
        }
    }
    let t0 = Instant::now();
    for _ in 0..REPS {
        for z in 0..ZCOLS {
            for g in 0..(YC / 8 + 1) {
                let mut y = [0f64; 8];
                let mut x = [0f64; 8];
                for l in 0..8 {
                    let p = (g * 8 + l).min(YC - 1);
                    y[l] = -64.0 + p as f64 * 8.0;
                    x[l] = 64.0 + l as f64 * 4.0;
                }
                let v = nn.get_value_batch8(&x, &y, &[z as f64 * 4.0; 8]);
                for q in v {
                    sink ^= q.to_bits();
                }
            }
        }
    }
    let el = t0.elapsed().as_nanos() as f64;
    let pts = (REPS * ZCOLS * ((YC / 8 + 1) * 8)) as f64;
    println!(
        "{name}: {sink} {:.1}ns/pt {:.2}ns/oct/pt",
        el / pts,
        el / pts / oct as f64
    );
}

fn bench_single_oct(first_octave: i32) {
    let _ = &first_octave;
    let name = format!("oct f=2^{}", first_octave);
    let mut rng = XoroshiroRandomSource::new(0x0DEC5101);
    let nn = NormalNoise::create(&mut rng, first_octave, vec![1.0]);
    let mut sink = 0u64;
    for z in 0..4 {
        for g in 0..(YC / 8 + 1) {
            let mut y = [0f64; 8];
            for l in 0..8 {
                let p = (g * 8 + l).min(YC - 1);
                y[l] = -64.0 + p as f64 * 8.0;
            }
            let v = nn.get_value_batch8(&[64.0; 8], &y, &[z as f64 * 4.0; 8]);
            for x in v {
                sink ^= x.to_bits();
            }
        }
    }
    let t0 = Instant::now();
    for _ in 0..(REPS * 4) {
        for z in 0..ZCOLS {
            for g in 0..(YC / 8 + 1) {
                let mut y = [0f64; 8];
                for l in 0..8 {
                    let p = (g * 8 + l).min(YC - 1);
                    y[l] = -64.0 + p as f64 * 8.0;
                }
                let v = nn.get_value_batch8(&[64.0; 8], &y, &[z as f64 * 4.0; 8]);
                for x in v {
                    sink ^= x.to_bits();
                }
            }
        }
    }
    let el = t0.elapsed().as_nanos() as f64;
    let pts = (REPS * 4 * ZCOLS * ((YC / 8 + 1) * 8)) as f64;
    println!("{name}: {sink} {:.2}ns/pt", el / pts);
}

fn bench_hop() {
    // 8 NormalNoise instances hopping per group like the production slice
    // walk (8 nodes x first/second per group), same column pattern.
    let mut rng = XoroshiroRandomSource::new(0x0DEC5109);
    let nodes: Vec<NormalNoise> = (0..8)
        .map(|k| {
            NormalNoise::create(
                &mut rng,
                -5 + (k % 3) as i32,
                vec![1.5, 2.0, 2.0, 2.0, 2.0, 2.0, 2.0, 2.0, 2.0],
            )
        })
        .collect();
    let mut sink = 0u64;
    for _ in 0..4 {
        for z in 0..4 {
            for g in 0..(YC / 8 + 1) {
                let mut y = [0f64; 8];
                for l in 0..8 {
                    let p = (g * 8 + l).min(YC - 1);
                    y[l] = -64.0 + p as f64 * 8.0;
                }
                for n in &nodes {
                    let v = n.get_value_batch8(&[64.0; 8], &y, &[z as f64 * 4.0; 8]);
                    for x in v {
                        sink ^= x.to_bits();
                    }
                }
            }
        }
    }
    let t0 = Instant::now();
    for _ in 0..REPS {
        for z in 0..ZCOLS {
            for g in 0..(YC / 8 + 1) {
                let mut y = [0f64; 8];
                for l in 0..8 {
                    let p = (g * 8 + l).min(YC - 1);
                    y[l] = -64.0 + p as f64 * 8.0;
                }
                for n in &nodes {
                    let v = n.get_value_batch8(&[64.0; 8], &y, &[z as f64 * 4.0; 8]);
                    for x in v {
                        sink ^= x.to_bits();
                    }
                }
            }
        }
    }
    let el = t0.elapsed().as_nanos() as f64;
    let pts = (REPS * ZCOLS * ((YC / 8 + 1) * 8) * 8) as f64; // x8 nodes
    println!(
        "nn-9oct x8 HOPPING: {sink} {:.1}ns/pt ({:.1}ns/pt/node)",
        el / pts,
        el / pts / 8.0
    );
}

fn main() {
    crussty_chunkgen::noise::probe_avx512_width();
    bench_hop();
    bench_single_oct(-5);
    bench_single_oct(-3);
    bench_single_oct(-1);
    bench_single_oct(0);
    bench_single_oct(1);
    bench_single_oct(3);
    bench_nn("nn-9oct (continentalness)", vec![
        1.5, 2.0, 2.0, 2.0, 2.0, 2.0, 2.0, 2.0, 2.0,
    ]);
    bench_nn("nn-4oct (simple)", vec![1.0, 1.0, 1.0, 1.0]);
    bench_nn("nn-16oct (detail)", vec![
        1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0,
    ]);
    {
        let mut rng = XoroshiroRandomSource::new(0x0DEC5102);
        let nn = NormalNoise::create(&mut rng, -5, vec![1.5, 2.0, 2.0, 2.0, 2.0, 2.0, 2.0, 2.0, 2.0]);
        bench_generic(&nn, "nn-9oct GENERIC", 18);
    }
    bench_blend();
}
