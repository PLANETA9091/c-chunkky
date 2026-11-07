//! cont-125: clean per-fill stage profile — reset prof counters, run exactly
//! N fill_chunk calls (fresh frontier chunks like the server), print
//! prof263_snapshot with the true fill count. No server sections, no SIMD
//! parity — pure noise-stage accounting.
use crussty_chunkgen::df263;
use crussty_chunkgen::fill263::Gen263;
use crussty_chunkgen::nochunk::OUT_LEN;

fn main() {
    let seed = 3257840388504953787i64;
    let n_fills: usize = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(60);
    let g = Gen263::new(seed, -64, 384, 63);
    let mut out = [0u16; OUT_LEN];
    let mut marks = Vec::new();

    // warmup (3 fills, prof reset AFTER warmup)
    for i in 0..3 {
        let cx = (i % 2) as i32;
        let cz = (i / 2) as i32;
        g.fill_chunk(cx, cz, &[0, 0], &mut out, &mut marks);
    }
    df263::prof263_reset();

    let mut t = Vec::with_capacity(n_fills);
    for i in 0..n_fills {
        // frontier walk: fresh chunk each fill (honest single-touch state)
        let cx = (i as i32 % 12) - 6;
        let cz = (i as i32 / 12) - 2;
        let t0 = std::time::Instant::now();
        g.fill_chunk(cx, cz, &[0, 0], &mut out, &mut marks);
        t.push(t0.elapsed().as_secs_f64() * 1e3);
    }
    t.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p50 = t[t.len() / 2];
    println!(
        "fills={} fill p50={:.3}ms min={:.3} max={:.3}",
        n_fills,
        p50,
        t[0],
        t[t.len() - 1]
    );
    println!("{}", df263::prof263_snapshot());
}
