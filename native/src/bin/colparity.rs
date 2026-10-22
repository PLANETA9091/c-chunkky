//! colparity: bit-exactness sweep between the 4-wide AVX2 col kernels and the
//! 8-wide AVX-512 col kernels (get_value_batch_col with avx512_selected
//! forced off/on). The judge residual hypothesis: the two widths diverge on
//! some lane pattern; local CPUs (avx512f=true) always ran 8-wide while the
//! CI Turin/Milan pool (avx512f=false) runs 4-wide.
//!
//! Run: cargo run --release --bin colparity [-- --sweep2]
//! Exit code 1 on any bit mismatch.

use crussty_chunkgen::noise::{NormalNoise, avx512_selected, set_avx512_selected};
use crussty_chunkgen::rng::XoroshiroRandomSource;

struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0
    }
    fn f64_range(&mut self, lo: f64, hi: f64) -> f64 {
        let u = (self.next() >> 11) as f64 / (1u64 << 53) as f64;
        lo + u * (hi - lo)
    }
    fn i64_range(&mut self, lo: i64, hi: i64) -> i64 {
        lo + (self.next() % (hi - lo + 1) as u64) as i64
    }
}

fn compare_one(nn: &NormalNoise, tag: &str, x_in: f64, z_in: f64, y: &[f64; 8]) -> usize {
    // 4-wide pass (two col4 calls under the hood)
    set_avx512_selected(false);
    let v4 = nn.first.get_value_batch_col::<8>(x_in, z_in, y, nn.col_base);
    // 8-wide pass (one col8 call)
    set_avx512_selected(true);
    let v8 = nn.first.get_value_batch_col::<8>(x_in, z_in, y, nn.col_base);
    let mut bad = 0;
    for l in 0..8 {
        if v4[l].to_bits() != v8[l].to_bits() {
            bad += 1;
            if bad <= 4 {
                println!(
                    "MISMATCH {tag} x={x_in:.6} z={z_in:.6} lane={l} y={:e} v4={:e}({:016x}) v8={:e}({:016x})",
                    y[l], v4[l], v4[l].to_bits(), v8[l], v8[l].to_bits()
                );
            }
        }
    }
    bad
}

fn compare_blended(tag: &str, bx: i32, by: &[i32; 8], bz: i32, bl: &crussty_chunkgen::noise::BlendedNoise) -> usize {
    use crussty_chunkgen::noise::avx2_available;
    // generic batch (no col memo, no width dispatch)
    let vg = bl.compute_batch8(by, by, by); // placeholder to keep signature warm
    let _ = vg;
    // 4-wide col path (2x col4 avx2) — avx512 forced OFF
    set_avx512_selected(false);
    let v4 = bl.compute_batch8_col(bx, by, bz, bl.col_base);
    // 8-wide col512 path
    set_avx512_selected(true);
    let v8 = bl.compute_batch8_col(bx, by, bz, bl.col_base);
    // generic full batch (the CRUS_SLICE_GENERIC dispatch target)
    let mut bxi = [bx; 8];
    let mut bzi = [bz; 8];
    for l in 0..8 {
        bxi[l] = bx;
        bzi[l] = bz;
    }
    let vgen = if avx2_available() {
        bl.compute_batch8(&bxi, by, &bzi)
    } else {
        v4
    };
    let mut bad = 0;
    for l in 0..8 {
        let bits = [v4[l].to_bits(), v8[l].to_bits(), vgen[l].to_bits()];
        if bits[0] != bits[1] || bits[0] != bits[2] {
            bad += 1;
            if bad <= 4 {
                println!(
                    "BLENDED MISMATCH {tag} bx={bx} bz={bz} lane={l} by={} v4={:e}({:016x}) v8={:e}({:016x}) vgen={:e}({:016x})",
                    by[l], v4[l], bits[0], v8[l], bits[1], vgen[l], bits[2]
                );
            }
        }
    }
    bad
}

fn main() {
    println!(
        "hw: avx2={} avx512f={} selected(=true)",
        std::arch::is_x86_feature_detected!("avx2"),
        std::arch::is_x86_feature_detected!("avx512f")
    );
    let mut rng = Lcg(0x0DEC_5102_C0FF_EE01);
    let mut total_bad = 0usize;
    let mut total_calls = 0usize;

    // amplitude shapes: 1..=12 octaves (vanilla OverworldNoise uses 10/16-tap
    // shapes; input=value factor pairs differ per constructor)
    for octaves in 1..=12usize {
        let amps: Vec<f64> = (0..octaves).map(|i| 1.0 + (i % 3) as f64).collect();
        let mut r = XoroshiroRandomSource::new((0x5EED_0000u64 + octaves as u64) as i64);
        let nn = NormalNoise::create(&mut r, 0, amps);
        for trial in 0..400 {
            let x_in = rng.f64_range(-1.0e5, 1.0e5);
            let z_in = rng.f64_range(-1.0e5, 1.0e5);
            let mut y = [0f64; 8];
            // realistic worldgen y band plus dense same-cell clusters
            let base = rng.i64_range(-96, 384) as f64;
            if trial % 4 == 0 {
                // dense cluster: same-cell lanes stress the col fast path
                for l in 0..8 {
                    y[l] = base + (l as f64) * 0.125;
                }
            } else if trial % 4 == 1 {
                for l in 0..8 {
                    y[l] = base + (l as f64) * 8.0;
                }
            } else {
                for l in 0..8 {
                    y[l] = rng.f64_range(-96.0, 384.0);
                }
            }
            total_bad += compare_one(&nn, &format!("oct{octaves}"), x_in, z_in, &y);
            total_calls += 1;
            if total_bad > 40 {
                println!("...aborting sweep, >40 mismatches");
                std::process::exit(1);
            }
        }
        println!("octaves={octaves}: cumulative_bad={total_bad} over {total_calls} calls");
    }
    set_avx512_selected(avx512_selected()); // restore = no-op, explicit
    // BlendedNoise sweep (density slice kernels: compute_batch8_col512 vs
    // 2x compute_batch_col avx2 vs generic compute_batch8)
    let mut bl_bad = 0usize;
    let mut bl_calls = 0usize;
    for shape in 0..4 {
        let (xz_scale, y_scale, xz_factor, y_factor, smear) = match shape {
            0 => (1.0, 1.0, 80.0, 160.0, 4.0),      // overworld final_density-ish
            1 => (0.25, 0.125, 80.0, 160.0, 4.0),
            2 => (1.0, 2.0, 160.0, 320.0, 4.0),
            _ => (3.0, 4.0, 40.0, 80.0, 1.0),
        };
        let mut r = XoroshiroRandomSource::new((0xB1E9_DE00u64 + shape as u64) as i64);
        let bl = crussty_chunkgen::noise::BlendedNoise::create(&mut r, xz_scale, y_scale, xz_factor, y_factor, smear);
        for trial in 0..500 {
            let bx = rng.i64_range(-3_000_000, 3_000_000) as i32 / 4 * 4;
            let bz = rng.i64_range(-3_000_000, 3_000_000) as i32 / 4 * 4;
            let mut by = [0i32; 8];
            let ybase = rng.i64_range(-64, 320);
            match trial % 3 {
                0 => {
                    for l in 0..8 { by[l] = (ybase + l as i64) as i32; }
                }
                1 => {
                    for l in 0..8 { by[l] = (ybase + l as i64 * 4) as i32; }
                }
                _ => {
                    for l in 0..8 { by[l] = rng.i64_range(-64, 320) as i32; }
                }
            }
            bl_bad += compare_blended(&format!("shape{shape}"), bx, &by, bz, &bl);
            bl_calls += 1;
            if bl_bad > 40 {
                println!("...aborting blended sweep, >40 mismatches");
                std::process::exit(1);
            }
        }
        println!("blended shape{shape}: cumulative_bad={bl_bad} over {bl_calls} calls");
    }
    set_avx512_selected(false);
    println!(
        "DONE: perlin {total_calls} calls/{total_bad} bad; blended {bl_calls} calls/{bl_bad} bad (4-wide AVX2 vs 8-wide AVX-512 vs generic)"
    );
    if total_bad + bl_bad > 0 {
        std::process::exit(1);
    }
}
