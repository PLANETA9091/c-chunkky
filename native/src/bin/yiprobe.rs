//! Probe: print actual yi patterns inside the noise col8 path.
use crussty_chunkgen::noise::{NormalNoise, NN_COL_ENGAGED, NN_COL_BLOCKED, NNPROF_ON};
use crussty_chunkgen::noise::OctColState;
use std::time::Instant;
use crussty_chunkgen::rng::XoroshiroRandomSource;

fn main() {
    crussty_chunkgen::noise::probe_avx512_width();
    iso_same_kernel();
    iso_kernel();
    NNPROF_ON.store(true, std::sync::atomic::Ordering::Relaxed);
    let mut rng = XoroshiroRandomSource::new(0x0DEC5101);
    let nn = NormalNoise::create(&mut rng, 3, vec![1.0]);
    let mut y = [0f64; 8];
    for l in 0..8 {
        y[l] = -64.0 + l as f64 * 8.0;
    }
    // manually drive one column: get_value_batch8 with uniform x/z
    let v = nn.get_value_batch8(&[64.0; 8], &y, &[128.0; 8]);
    println!("vals {:?}", v);

    // and again at a different z (new column)
    let v2 = nn.get_value_batch8(&[64.0; 8], &y, &[132.0; 8]);
    println!("vals2 {:?}", v2);

    println!("col_engaged {}", NN_COL_ENGAGED.load(std::sync::atomic::Ordering::Relaxed));
    println!("col_blocked {}", NN_COL_BLOCKED.load(std::sync::atomic::Ordering::Relaxed));
}

#[cfg(target_arch = "x86_64")]
fn iso_same_kernel() {
    use std::arch::x86_64::*;
    let mut rng = XoroshiroRandomSource::new(0x0DEC5101);
    let nn = NormalNoise::create(&mut rng, 3, vec![1.0]);
    let pn = &nn.first;
    let nl = pn.noise_levels[0].as_ref().unwrap();
    let mut st = OctColState::INVALID;
    let kx = 64.0 * 8.0;
    let kz = 128.0 * 8.0;
    let wx = crussty_chunkgen::noise::wrap(kx);
    let wz = crussty_chunkgen::noise::wrap(kz);
    nl.col_init(&mut st, wx, wz);
    st.valid = true;
    st.key_x = kx.to_bits();
    st.key_z = kz.to_bits();
    let dy = [0.25f64, 0.5, 0.75, 0.125, 0.375, 0.625, 0.875, 0.0];
    let mut sink = 0u64;
    for _ in 0..100 {
        let r = nl.noise_col8_same_pub(&st, &dy);
        for q in r { sink ^= q.to_bits(); }
    }
    let t0 = Instant::now();
    const REPS: usize = 400000;
    for _ in 0..REPS {
        let r = nl.noise_col8_same_pub(&st, &dy);
        for q in r { sink ^= q.to_bits(); }
    }
    let el = t0.elapsed().as_nanos() as f64;
    println!("isolated noise_col8_same: {:.1}ns/call {:.2}ns/pt sink={sink}", el / REPS as f64, el / REPS as f64 / 8.0);
}

#[cfg(target_arch = "x86_64")]
fn iso_kernel() {
    use std::arch::x86_64::*;
    let mut rng = XoroshiroRandomSource::new(0x0DEC5101);
    let nn = NormalNoise::create(&mut rng, 3, vec![1.0]);
    let pn = &nn.first;
    let nl = pn.noise_levels[0].as_ref().unwrap();
    let mut st = OctColState::INVALID;
    // init column state once
    let kx = 64.0 * 8.0; // x * input_factor
    let kz = 128.0 * 8.0;
    let wx = crussty_chunkgen::noise::wrap(kx);
    let wz = crussty_chunkgen::noise::wrap(kz);
    nl.col_init(&mut st, wx, wz);
    st.valid = true;
    st.key_x = kx.to_bits();
    st.key_z = kz.to_bits();
    let mut sink = 0u64;
    // y corners step 8 (like slice): wy = wrap(y * 8)
    let mut wy = [0f64; 8];
    for l in 0..8 {
        wy[l] = crussty_chunkgen::noise::wrap((-64.0 + (l * 8) as f64) * 8.0);
    }
    for _ in 0..100 {
        let r = nl.noise_col8_vec(&mut st, &wy);
        for q in r { sink ^= q.to_bits(); }
    }
    let t0 = Instant::now();
    const REPS: usize = 200000;
    for _ in 0..REPS {
        let r = nl.noise_col8_vec(&mut st, &wy);
        for q in r { sink ^= q.to_bits(); }
    }
    let el = t0.elapsed().as_nanos() as f64;
    println!("isolated noise_col8_vec: {:.1}ns/call {:.2}ns/pt sink={sink}", el / REPS as f64, el / REPS as f64 / 8.0);
}
