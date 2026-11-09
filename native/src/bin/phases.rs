//! Per-phase fill timings (slice / cellfill / material) using CRUS_PROFILE=1.
use crussty_chunkgen::density::Env;
use crussty_chunkgen::nochunk::{ChunkGen, OUT_LEN};

fn collect(root: &std::path::Path, dir: &std::path::Path, out: &mut Vec<(String, Vec<u8>)>) {
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                collect(root, &p, out);
            } else if p.extension().map(|x| x == "json").unwrap_or(false) {
                if let Ok(rel) = p.strip_prefix(root) {
                    let rel = rel.to_string_lossy().replace('\\', "/");
                    // roots come in two layouts: raw pack trees (data/<ns>/...)
                    // or pre-merged trees (<ns>/... with data/ already stripped)
                    let stripped = match rel.strip_prefix("data/") {
                        Some(s) => s.to_string(),
                        None => rel.clone(),
                    };
                    if let Ok(bytes) = std::fs::read(&p) {
                        out.push((stripped, bytes));
                    }
                }
            }
        }
    }
}

fn main() {
    std::env::set_var("CRUS_PROFILE", "1");
    // Diagnostic counters (Instant::now in the noise hot path) are opt-in:
    // CRUS_DIAG=1. Without it the measured window is clean.
    let diag = std::env::var("CRUS_DIAG").ok().as_deref() == Some("1");
    crussty_chunkgen::density::PROF_ON
        .store(diag, std::sync::atomic::Ordering::Relaxed);
    let args: Vec<String> = std::env::args().collect();
    let seed: i64 = std::env::var("CRUS_BENCH_SEED").ok().and_then(|v| v.parse().ok()).unwrap_or(3257840388504953787);
    let env: &'static Env = if args.len() > 1 {
        // pack mode: merge extraction roots in order (later wins)
        let mut assets = crussty_chunkgen::worldgen::Assets::new();
        for root in &args[1..] {
            let root = std::path::Path::new(root);
            let mut files = Vec::new();
            collect(root, root, &mut files);
            files.sort();
            for (path, bytes) in files {
                let keep = path.contains("/worldgen/noise_settings/")
                    || path.contains("/worldgen/density_function/")
                    || path.contains("/worldgen/noise/")
                    || path.ends_with("/dimension/overworld.json");
                if keep { assets.insert(path, bytes); }
            }
        }
        Box::leak(Box::new(Env::new_assets(seed, &assets)))
    } else {
        Box::leak(Box::new(Env::new(seed)))
    };
    let mut gen = ChunkGen::new(env);
    let mut out = [0u16; OUT_LEN];
    for cz in 0..2 {
        for cx in 0..2 {
            gen.fill(cx, cz, &mut out);
        }
    }
    // zero diagnostic counters before the measured window
    crussty_chunkgen::density::PROF_NOISE_NS.store(0, std::sync::atomic::Ordering::Relaxed);
    crussty_chunkgen::density::PROF_BLEND_NS.store(0, std::sync::atomic::Ordering::Relaxed);
    crussty_chunkgen::density::PROF_NOISE_CALLS.store(0, std::sync::atomic::Ordering::Relaxed);
    crussty_chunkgen::density::PROF_BLEND_CALLS.store(0, std::sync::atomic::Ordering::Relaxed);
    gen.prof.reset_phases();
    let n: i32 = std::env::var("CRUS_PHASES_N")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(16);
    let fill_diag = std::env::var("CRUS_FILL_DIAG").is_ok();
    let mut ftimes: Vec<f64> = Vec::with_capacity(n as usize);
    let t0 = std::time::Instant::now();
    for i in 0..n {
        let tf = std::time::Instant::now();
        gen.fill((i % 4) - 2, (i / 4) - 2, &mut out);
        ftimes.push(tf.elapsed().as_secs_f64() * 1e3);
    }
    let wall = t0.elapsed().as_nanos() as f64 / n as f64 / 1e6;
    if fill_diag {
        let mut s = ftimes.clone();
        s.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let pick = |q: f64| s[((s.len() as f64 - 1.0) * q) as usize];
        eprintln!(
            "[perf] n={} min={:.3} p10={:.3} p50={:.3} p90={:.3} max={:.3}",
            n,
            s[0],
            pick(0.1),
            pick(0.5),
            pick(0.9),
            s[s.len() - 1]
        );
    }
    if std::env::var("CRUS_CELL_STATS").is_ok() {
        use crussty_chunkgen::nochunk::{
            CELL_V1_AIR, CELL_V1_CELLMAT, CELL_V1_FALLBACK, CELL_V1_LAVA, CELL_V2_FALLBACK,
            CELL_V2_STONE, CELL_V_MIXED,
        };
        let r = |a: &std::sync::atomic::AtomicU64| a.load(std::sync::atomic::Ordering::Relaxed) / n as u64;
        println!(
            "cells/chunk: mixed {} | v1: lava {} air {} cellmat {} fallback {} | v2: stone {} fallback {}",
            r(&CELL_V_MIXED),
            r(&CELL_V1_LAVA),
            r(&CELL_V1_AIR),
            r(&CELL_V1_CELLMAT),
            r(&CELL_V1_FALLBACK),
            r(&CELL_V2_STONE),
            r(&CELL_V2_FALLBACK)
        );
    }
    let p = gen.prof;
    let f = |v: u64| v as f64 / n as f64 / 1e6;
    println!(
        "wall {:.2} ms | slice {:.2} | cellfill {:.2} | material {:.2} | other {:.2} | select {:.2}",
        wall,
        f(p.fill_slice_ns),
        f(p.cellfill_ns),
        f(p.material_ns),
        wall - f(p.fill_slice_ns) - f(p.cellfill_ns) - f(p.material_ns),
        f(p.select_ns),
    );
    println!(
        "blocks: subst+ {} subst- {} ore_reached {} aquifer_air {}",
        p.blocks_subst_pos, p.blocks_subst_neg, p.blocks_ore_reached, p.blocks_aquifer_air,
    );
    println!(
        "diag: normalnoise-batch {:.2} ms ({} calls) | blended-batch {:.2} ms ({} calls) per chunk",
        f(crussty_chunkgen::density::PROF_NOISE_NS.load(
            std::sync::atomic::Ordering::Relaxed
        )),
        crussty_chunkgen::density::PROF_NOISE_CALLS.load(std::sync::atomic::Ordering::Relaxed) / 16,
        f(crussty_chunkgen::density::PROF_BLEND_NS.load(
            std::sync::atomic::Ordering::Relaxed
        )),
        crussty_chunkgen::density::PROF_BLEND_CALLS.load(std::sync::atomic::Ordering::Relaxed) / 16,
    );
    if std::env::var("CRUS_SLOT_PROF").ok().as_deref() == Some("1") {
        gen.profile_slice_children(-2, -2);
    }
    let g = |v: u64| v as f64 / n as f64;
    println!(
        "aq: substance_calls {} fluid_calls {} summary_hits {} prelim_scans {} prelim_steps {}",
        g(crussty_chunkgen::nochunk::AQ_SUBSTANCE_CALLS.load(std::sync::atomic::Ordering::Relaxed)),
        g(crussty_chunkgen::nochunk::AQ_FLUID_CALLS.load(std::sync::atomic::Ordering::Relaxed)),
        g(crussty_chunkgen::nochunk::AQ_SUMMARY_HITS.load(std::sync::atomic::Ordering::Relaxed)),
        g(crussty_chunkgen::nochunk::PRELIM_SCANS.load(std::sync::atomic::Ordering::Relaxed)),
        g(crussty_chunkgen::nochunk::PRELIM_SCAN_STEPS.load(std::sync::atomic::Ordering::Relaxed)),
    );
    let gm = |v: u64| v as f64 / n as f64 / 1e6;
    println!(
        "ore: toggle {:.3}ms ({} calls) | finish {} | ridged {:.3}ms ({}) | gap {:.3}ms ({})",
        gm(crussty_chunkgen::nochunk::ORE_TOGGLE_NS.load(std::sync::atomic::Ordering::Relaxed)),
        g(crussty_chunkgen::nochunk::ORE_TOGGLE_CALLS.load(std::sync::atomic::Ordering::Relaxed)),
        g(crussty_chunkgen::nochunk::ORE_FINISH_CALLS.load(std::sync::atomic::Ordering::Relaxed)),
        gm(crussty_chunkgen::nochunk::ORE_RIDGED_NS.load(std::sync::atomic::Ordering::Relaxed)),
        g(crussty_chunkgen::nochunk::ORE_RIDGED_CALLS.load(std::sync::atomic::Ordering::Relaxed)),
        gm(crussty_chunkgen::nochunk::ORE_GAP_NS.load(std::sync::atomic::Ordering::Relaxed)),
        g(crussty_chunkgen::nochunk::ORE_GAP_CALLS.load(std::sync::atomic::Ordering::Relaxed)),
    );
    print_slot_anatomy(&gen);
}

/// Dump per-slot child node + column-cache/y-invariance verdict.
fn print_slot_anatomy(gen: &ChunkGen<'static>) {
    let c = &gen.compiled;
    for (slot, &child) in c.interp_children.iter().enumerate() {
        let col = c.col_idx[child as usize];
        println!(
            "slot {slot}: child {} col_cache={}",
            child,
            if col == u32::MAX { "NO".into() } else { format!("yes({col})") }
        );
    }
}
