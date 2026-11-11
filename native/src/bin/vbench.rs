//! Isolated native fill timing on the VANILLA (legacy nochunk) engine —
//! the 26.3-campaign production path (no datapacks). Mirrors ChunkGen::fill.
//!
//! Usage: vbench -- <n> [cx cz seed]
//! Prints per-fill ns p50/p90/p99/min over the loop; CRUS_PHASES=1 prints
//! the jni_prof phase split (slice/cellfill/material/select).
//! CRUS_DETAIL=1 additionally prints the cont-142 blend/material
//! micro-profiler counters (blend path selection, per-batch8-call ns,
//! main/minmax octave split, material row4 vs fallback split).
//! CRUS_H6_DIAG=1 prints the per-noise-name census.
//! CRUS_BLENDPROF=1 (set implicitly by CRUS_DETAIL) enables the blend
//! internal timers; CRUS_PROFILE=1 enables the jni_prof timers.

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

    let detail = std::env::var("CRUS_DETAIL").as_deref() == Ok("1");
    if detail {
        crussty_chunkgen::noise::BP_ON.store(true, std::sync::atomic::Ordering::Relaxed);
        // enable the density dispatch timers (blend/noise ns inside fill)
        crussty_chunkgen::density::PROF_ON.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    let h6 = crussty_chunkgen::density::h6_diag_init();
    let env: &'static Env = Box::leak(Box::new(Env::new(seed)));
    let mut gen = ChunkGen::new(env);
    let mut buf = vec![0u16; OUT_LEN];

    if detail {
        println!("{}", crussty_chunkgen::noise::blend_path_snapshot());
    }

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
        samples.push(el.as_nanos() as u64 as u128);
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
    if detail {
        use crussty_chunkgen::noise::{
            BP_CALLS, BP_GENERIC_CALLS, BP_MAIN_KERNEL_CALLS, BP_MAIN_NS, BP_MINMAX_KERNEL_CALLS,
            BP_MINMAX_NS, BP_TOTAL_NS,
        };
        use crussty_chunkgen::nochunk::{MT_FB_ROWS, MT_ROW4_NS, MT_ROW4_ROWS, MT_ROW4_TAKEN};
        use crussty_chunkgen::density::{PROF_BLEND_CALLS, PROF_BLEND_NS, PROF_NOISE_CALLS, PROF_NOISE_NS};
        use std::sync::atomic::Ordering::Relaxed;
        let g = |c: &std::sync::atomic::AtomicU64| c.load(Relaxed);
        let calls = g(&BP_CALLS).max(1) as f64;
        println!(
            "blend142: b8col calls={} total={:.2}ms | b8generic={} | b4col={:.2}ms/{} ({:.2}us/c) | b4generic={:.2}ms/{} ({:.2}us/c)",
            g(&BP_CALLS),
            g(&BP_TOTAL_NS) as f64 / 1e6,
            g(&BP_GENERIC_CALLS),
            g(&crussty_chunkgen::noise::BP4_COL_NS) as f64 / 1e6,
            g(&crussty_chunkgen::noise::BP4_COL_CALLS),
            g(&crussty_chunkgen::noise::BP4_COL_NS) as f64
                / g(&crussty_chunkgen::noise::BP4_COL_CALLS).max(1) as f64
                / 1e3,
            g(&crussty_chunkgen::noise::BP4_GENERIC_NS) as f64 / 1e6,
            g(&crussty_chunkgen::noise::BP4_GENERIC_CALLS),
            g(&crussty_chunkgen::noise::BP4_GENERIC_NS) as f64
                / g(&crussty_chunkgen::noise::BP4_GENERIC_CALLS).max(1) as f64
                / 1e3,
        );
        println!(
            "blend142(internals): main={:.2}ms/{}k-calls | minmax={:.2}ms/{}k-calls (b8col calls={})",
            g(&BP_MAIN_NS) as f64 / 1e6,
            g(&BP_MAIN_KERNEL_CALLS),
            g(&BP_MINMAX_NS) as f64 / 1e6,
            g(&BP_MINMAX_KERNEL_CALLS),
            g(&BP_CALLS),
        );
        println!(
            "blend142(dispatch): dispatch_blended={:.2}ms/{} calls (incl. col-memo dispatch + lane check)",
            g(&PROF_BLEND_NS) as f64 / 1e6,
            g(&PROF_BLEND_CALLS),
        );
        println!(
            "noise142: normalnoise={:.2}ms/{} calls",
            g(&PROF_NOISE_NS) as f64 / 1e6,
            g(&PROF_NOISE_CALLS),
        );
        println!(
            "mat142: row4={:.2}ms/{} rows (taken={} fallback={}) | subst_pos={} subst_neg={} ore_reached={} aqu_air={}",
            g(&MT_ROW4_NS) as f64 / 1e6,
            g(&MT_ROW4_ROWS),
            g(&MT_ROW4_TAKEN),
            g(&MT_FB_ROWS),
            gen.prof.blocks_subst_pos,
            gen.prof.blocks_subst_neg,
            gen.prof.blocks_ore_reached,
            gen.prof.blocks_aquifer_air,
        );
        println!(
            "cellverdict: mixed={} v1_lava={} v1_air={} v1_cellmat={} v1_fallback={} v2_stone={} v2_fallback={}",
            g(&crussty_chunkgen::nochunk::CELL_V_MIXED),
            g(&crussty_chunkgen::nochunk::CELL_V1_LAVA),
            g(&crussty_chunkgen::nochunk::CELL_V1_AIR),
            g(&crussty_chunkgen::nochunk::CELL_V1_CELLMAT),
            g(&crussty_chunkgen::nochunk::CELL_V1_FALLBACK),
            g(&crussty_chunkgen::nochunk::CELL_V2_STONE),
            g(&crussty_chunkgen::nochunk::CELL_V2_FALLBACK),
        );
        let _ = MT_FB_ROWS;
        let _ = MT_ROW4_TAKEN;
        let _ = MT_ROW4_ROWS;
        let _ = MT_ROW4_NS;
    }
    if std::env::var("CRUS_PHASES").as_deref() == Ok("1") {
        use crussty_chunkgen::nochunk::{
            JP_CELLFILL_NS, JP_FILLS, JP_MATERIAL_NS, JP_SELECT_NS, JP_SLICE_NS, JP_WALL_NS,
            MT_ROW4_NS,
        };
        use std::sync::atomic::Ordering::Relaxed;
        for a in [&JP_FILLS, &JP_WALL_NS, &JP_SLICE_NS, &JP_CELLFILL_NS, &JP_MATERIAL_NS, &JP_SELECT_NS] {
            a.store(0, Relaxed);
        }
        crussty_chunkgen::nochunk::MT_ROW4_NS.store(0, Relaxed);
        for i in 0..n {
            gen.fill(cx + (i as i32 % 17) - 8, cz, buf.as_mut_slice().try_into().unwrap());
        }
        println!("{}", crussty_chunkgen::nochunk::jni_prof_snapshot());
        println!(
            "mat142split: row4={:.2}ms perblock-fallback={:.2}ms (of material total)",
            crussty_chunkgen::nochunk::MT_ROW4_NS.load(Relaxed) as f64 / 1e6,
            (JP_MATERIAL_NS.load(Relaxed).saturating_sub(crussty_chunkgen::nochunk::MT_ROW4_NS.load(Relaxed))) as f64 / 1e6,
        );
        let _ = MT_ROW4_NS;
    }
    if h6 {
        // H6 per-noise census (third layer; only when CRUS_H6_DIAG=1)
        crussty_chunkgen::density::h6_diag_reset();
        for i in 0..8u32 {
            gen.fill(cx + (i as i32 % 17) - 8, cz, buf.as_mut_slice().try_into().unwrap());
        }
        println!("{}", crussty_chunkgen::density::h6_diag_snapshot());
    }
}
