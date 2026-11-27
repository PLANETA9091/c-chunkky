//! Streaming fill bench: a spiral of DISTINCT chunks (real generation order),
//! column-cache on by default (CRUS_COL_CACHE=0 disables). Reports wall mean
//! and jni_prof phase split.
//!
//! Usage: colbench -- <n> <cx0> <cz0> [seed]
use crussty_chunkgen::nochunk::{ChunkGen, OUT_LEN};
use crussty_chunkgen::density::Env;
use std::time::Instant;

fn main() {
    let mut pos = Vec::new();
    let mut seen = false;
    for a in std::env::args().skip(1) {
        if a == "--" {
            seen = true;
            continue;
        }
        if seen {
            pos.push(a.clone());
        }
    }
    let n: usize = pos.first().map(|v| v.parse().unwrap_or(100)).unwrap_or(100);
    let cx0: i32 = pos.get(1).map(|v| v.parse().unwrap_or(0)).unwrap_or(0);
    let cz0: i32 = pos.get(2).map(|v| v.parse().unwrap_or(0)).unwrap_or(0);
    let seed: i64 = pos.get(3).and_then(|v| v.parse::<i64>().ok()).unwrap_or(3257840388504953787);

    let env: &'static Env = Box::leak(Box::new(Env::new(seed)));
    let mut gen = ChunkGen::new(env);
    let mut out = [0u16; OUT_LEN];

    // spiral order: +x, +z, -x, -z expanding (Paper-ish region sweep)
    let mut coords = Vec::with_capacity(n);
    let (mut x, mut z) = (cx0, cz0);
    let (mut dx, mut dz) = (1i32, 0i32);
    let mut leg = 1usize;
    let mut step = 0usize;
    for _ in 0..n {
        coords.push((x, z));
        x += dx;
        z += dz;
        step += 1;
        if step == leg {
            step = 0;
            // turn left
            (dx, dz) = (-dz, dx);
            if dz == 0 && dx == 1 || dz == 0 && dx == -1 {
                leg += 1;
            }
        }
    }

    // warmup: 8 chunks (ctx pools, lanes calibration)
    for i in 0..8.min(coords.len()) {
        let (cx, cz) = coords[i];
        gen.fill(cx, cz, &mut out);
    }
    crussty_chunkgen::nochunk::reset_jni_prof();

    let t0 = Instant::now();
    let mut sink = 0u64;
    for &(cx, cz) in &coords {
        gen.fill(cx, cz, &mut out);
        sink ^= out[0] as u64;
    }
    let el = t0.elapsed();
    let n = coords.len() as f64;
    println!(
        "colbench: {} chunks, total {:.1}ms, mean {:.2} ms/chunk (sink {:x})",
        coords.len(),
        el.as_secs_f64() * 1000.0,
        el.as_secs_f64() * 1000.0 / n,
        sink & 0xffff
    );
    println!("{}", crussty_chunkgen::nochunk::jni_prof_snapshot());
}
