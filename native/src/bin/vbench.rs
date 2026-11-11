//! Isolated native fill timing on the VANILLA (legacy nochunk) engine —
//! the 26.3-campaign production path (no datapacks). Mirrors ChunkGen::fill.
//!
//! Usage: vbench -- <n> [cx cz seed]
//! Prints per-fill ns p50/p90/min/max over the loop; CRUS_PHASES=1 prints
//! the jni_prof phase split (slice/cellfill/material/select).

use crussty_chunkgen::nochunk::{ChunkGen, OUT_LEN};
use crussty_chunkgen::density::Env;
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut pos = Vec::new();
    let mut seen = false;
    for a in &args[1..] {
        if a == "--" {
            seen = true;
            continue;
        }
        if seen {
            pos.push(a.clone());
        }
    }
    let _ = seen;
    let n: u32 = pos.first().and_then(|s| s.parse().ok()).unwrap_or(128);
    let cx: i32 = pos.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    let cz: i32 = pos.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
    let seed: i64 = pos
        .get(3)
        .and_then(|s| s.parse().ok())
        .unwrap_or(3257840388504953787);

    let env: &'static Env = Box::leak(Box::new(Env::new(seed)));
    let mut gen = ChunkGen::new(env);
    let mut buf = vec![0u16; OUT_LEN];

    // warm (caches/branch warmup) + sink to defeat DCE
    let mut sink: u64 = 0;
    for i in 0..16u32 {
        gen.fill(i as i32 - 8, 0, buf.as_mut_slice().try_into().unwrap());
        sink = sink.wrapping_add(buf[12345] as u64);
    }

    let mut samples: Vec<u128> = Vec::with_capacity(n as usize);
    for i in 0..n {
        let ccx = cx + (i as i32 % 17) - 8; // vary chunk to defeat plane-cache-only timing
        let t0 = Instant::now();
        gen.fill(ccx, cz, buf.as_mut_slice().try_into().unwrap());
        let el = t0.elapsed();
        samples.push(el.as_nanos());
        sink = sink.wrapping_add(buf[12345] as u64);
    }
    samples.sort();
    let p = |q: f64| -> u128 {
        let idx = ((samples.len() as f64 - 1.0) * q).round() as usize;
        samples[idx]
    };
    println!(
        "vanilla fill: n={} p50={}ns p90={}ns p99={}ns min={}ns max={}ns sink={:e}",
        n,
        p(0.50),
        p(0.90),
        p(0.99),
        samples[0],
        *samples.last().unwrap(),
        sink as f64 * 0.0
    );
    if std::env::var("CRUS_PHASES").as_deref() == Ok("1") {
        use crussty_chunkgen::nochunk::{
            JP_CELLFILL_NS, JP_FILLS, JP_MATERIAL_NS, JP_SELECT_NS, JP_SLICE_NS, JP_WALL_NS,
        };
        use std::sync::atomic::Ordering::Relaxed;
        for a in [&JP_FILLS, &JP_WALL_NS, &JP_SLICE_NS, &JP_CELLFILL_NS, &JP_MATERIAL_NS, &JP_SELECT_NS] {
            a.store(0, Relaxed);
        }
        for i in 0..n {
            gen.fill(cx + (i as i32 % 17) - 8, cz, buf.as_mut_slice().try_into().unwrap());
        }
        println!("{}", crussty_chunkgen::nochunk::jni_prof_snapshot());
    }
}
