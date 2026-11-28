//! cont-158: carve-in-fill tail cost probe — mask pass vs apply_carvers over
//! a chunk sequence, per-chunk ns (vanilla 26.3 trees).
//! Usage: benchcarve [cx cz seed]
use crussty_chunkgen::carver263::generate_carver_mask;
use crussty_chunkgen::fill263::Gen263;
use crussty_chunkgen::nochunk::OUT_LEN;
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let cx: i32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(1028);
    let cz: i32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(1028);
    let seed: i64 = args
        .get(3)
        .and_then(|s| s.parse().ok())
        .unwrap_or(3257840388504953787);

    // warmup
    for i in 0..4i32 {
        let _ = generate_carver_mask(seed, cx + i, cz, -64, 384);
    }
    let n = 16u32;
    let t0 = Instant::now();
    let mut bits_total = 0u64;
    for i in 0..n {
        let mask = generate_carver_mask(seed, cx + (i % 4) as i32, cz + (i / 4) as i32, -64, 384);
        mask.visit(|_x, _z, bottom_y, top_y| {
            bits_total += (top_y - bottom_y + 1) as u64;
            true
        });
    }
    let dt = t0.elapsed();
    println!(
        "mask pass: {:.2} ms/ch (bits/ch {})",
        dt.as_nanos() as f64 / n as f64 / 1e6,
        bits_total / n as u64
    );

    // apply tail (fill first, then apply_carvers per chunk)
    let g = Gen263::new(seed, -64, 384, 63);
    let mut out = [0u16; OUT_LEN];
    let mut marks = Vec::new();
    let mut fill_ns = 0u128;
    let mut apply_ns = 0u128;
    let mut changed_total = 0u64;
    for i in 0..n {
        let a = Instant::now();
        g.fill_chunk(cx + (i % 4) as i32, cz + (i / 4) as i32, &[], &mut out, &mut marks);
        let b = Instant::now();
        fill_ns += (b - a).as_nanos();
        let pre = out;
        let mut ops: Vec<i64> = Vec::new();
        let top_ids = crussty_chunkgen::surface263::TopMatIds::build(None);
        let cm = g.apply_carvers(
            cx + (i % 4) as i32,
            cz + (i / 4) as i32,
            seed,
            &mut out,
            &mut ops,
            None,
            None,
            None,
            &top_ids,
        );
        let c = Instant::now();
        apply_ns += (c - b).as_nanos();
        changed_total += out
            .iter()
            .zip(pre.iter())
            .filter(|(a, b)| a != b)
            .count() as u64;
        let _ = cm;
    }
    println!(
        "fill_chunk: {:.2} ms/ch | apply tail: {:.2} ms/ch (changed/ch {})",
        fill_ns as f64 / n as f64 / 1e6,
        apply_ns as f64 / n as f64 / 1e6,
        changed_total / n as u64
    );
}
