// col-path microbench for BlendedNoise slice simulation.
// Mimics the density plane scan: 16 columns x 6 batch8 calls (48 y-samples,
// step 8, bottom-up), columns advance sequentially, TLS col cache persists
// across calls exactly like production. Reports median ns/call of
//   [1] compute_batch8_col with AVX-512 FORCED OFF (2x 4-wide AVX2 halves —
//       the AMD/CI-runner path),
//   [2] compute_batch8_col with AVX-512 forced ON (col512, if supported).
use crussty_chunkgen::noise::BlendedNoise;
use crussty_chunkgen::noise::set_avx512_selected;
use crussty_chunkgen::rng::XoroshiroRandomSource;

fn median(xs: &mut [f64]) -> f64 {
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    xs[xs.len() / 2]
}

fn main() {
    let mut rng = XoroshiroRandomSource::new(0x5EED_1234);
    // overworld terrain blended params
    let bn = BlendedNoise::create(&mut rng, 0.25, 0.4, 1.0, 2.0, 1.0);
    let col_base = bn.col_base;

    let has512 = std::arch::is_x86_feature_detected!("avx512f");
    println!("avx512f detected locally: {has512}");

    for mode in 0..2 {
        // warmup
        for rep in 0..3 {
            scan_once(&bn, col_base, rep, mode);
        }
        let mut samples_ns: Vec<f64> = Vec::new();
        for rep in 0..9 {
            let t0 = std::time::Instant::now();
            scan_once(&bn, col_base, 1000 + rep, mode);
            samples_ns.push(t0.elapsed().as_nanos() as f64);
        }
        let med = median(&mut samples_ns);
        let calls = 16.0 * 6.0;
        println!(
            "mode={mode} ({}): plane scan median {:.0} us => {:.0} ns/call (96 calls)",
            if mode == 0 { "AVX2 halves" } else { "col512" },
            med / 1000.0,
            med / calls
        );
    }

    // generic reference (no col memo)
    let mut samples_ns: Vec<f64> = Vec::new();
    for rep in 0..9 {
        let t0 = std::time::Instant::now();
        for c in 0..16 {
            let bx = (1000 + rep) * 16 + c;
            for j in 0..6 {
                let mut by = [0i32; 8];
                for l in 0..8 {
                    by[l] = -64 + ((j * 8 + l) * 8) as i32;
                }
                let _ = bn.compute_batch8(&[bx; 8], &by, &[7; 8]);
            }
        }
        samples_ns.push(t0.elapsed().as_nanos() as f64);
    }
    let med = median(&mut samples_ns);
    println!(
        "generic compute_batch8: plane scan median {:.0} us => {:.0} ns/call",
        med / 1000.0,
        med / 96.0
    );
}

#[inline(never)]
fn scan_once(bn: &BlendedNoise, col_base: usize, rep: usize, mode: usize) {
    set_avx512_selected(mode == 1);
    for c in 0..16 {
        let bx = (rep * 16 + c) as i32;
        for j in 0..6 {
            let mut by = [0i32; 8];
            for l in 0..8 {
                by[l] = -64 + ((j * 8 + l) * 8) as i32;
            }
            let _ = bn.compute_batch8_col(bx, &by, 7, col_base);
        }
    }
    set_avx512_selected(false);
}
