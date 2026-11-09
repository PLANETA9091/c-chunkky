//! Isolated native fill timing on the datapack path (noise + surface tail),
//! mirroring the production ffi::fill sequence exactly.
//!
//! Usage: dpperf <dp_root...> -- <n> [cx cz seed]
//! Prints per-fill ns and p50/p90/min over the loop.

use crussty_chunkgen::ffi::{fill, surface_assign_ids, surface_assign_temps, surface_states_of};
use crussty_chunkgen::nochunk::OUT_LEN;
use crussty_chunkgen::worldgen::Assets;
use std::path::Path;
use std::time::Instant;

fn biome_temp(assets: &Assets, name: &str) -> (f32, bool) {
    let (ns, file) = match name.split_once(':') {
        Some((ns, f)) => (ns, f),
        None => ("minecraft", name),
    };
    let key = format!("data/{ns}/worldgen/biome/{file}.json");
    let bytes = assets
        .get(&key)
        .map(|b| b.to_vec())
        .or_else(|| {
            crussty_chunkgen::asset(&format!("minecraft/worldgen/biome/{file}.json"))
                .map(|b| b.to_vec())
        });
    let Some(bytes) = bytes else {
        return (0.8, false);
    };
    if let Ok(j) = crussty_chunkgen::json::parse(std::str::from_utf8(&bytes).unwrap_or("")) {
        let t = j.get("temperature").and_then(|v| v.as_f64()).unwrap_or(0.8) as f32;
        let frozen = j
            .get("temperature_modifier")
            .and_then(|v| v.as_str())
            .map(|s| s.contains("frozen"))
            .unwrap_or(false);
        return (t, frozen);
    }
    (0.8, false)
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                collect(root, &p, out);
            } else if p.extension().map(|x| x == "json").unwrap_or(false) {
                if let Ok(rel) = p.strip_prefix(root) {
                    let rel = rel.to_string_lossy().replace('\\', "/");
                    let Some(stripped) = rel.strip_prefix("data/") else { continue };
                    if let Ok(bytes) = std::fs::read(&p) {
                        out.push((stripped.to_string(), bytes));
                    }
                }
            }
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut roots = Vec::new();
    let mut pos = Vec::new();
    let mut seen = false;
    for a in &args[1..] {
        if a == "--" {
            seen = true;
            continue;
        }
        if !seen {
            roots.push(a.clone());
        } else {
            pos.push(a.clone());
        }
    }
    let n: u32 = pos.first().and_then(|s| s.parse().ok()).unwrap_or(128);
    let cx: i32 = pos.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    let cz: i32 = pos.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
    let seed: i64 = pos
        .get(3)
        .and_then(|s| s.parse().ok())
        .unwrap_or(3257840388504953787);

    let mut assets = Assets::new();
    let mut expanded: Vec<String> = Vec::new();
    for r in &roots {
        expanded.push(r.clone());
        let ov = Path::new(r).join("overlay.datapack");
        if ov.is_dir() {
            expanded.push(ov.to_string_lossy().to_string());
        }
    }
    for root in &expanded {
        let root = Path::new(root);
        let mut files = Vec::new();
        collect(root, root, &mut files);
        files.sort();
        for (path, bytes) in files {
            let keep = path.contains("/worldgen/noise_settings/")
                || path.contains("/worldgen/density_function/")
                || path.contains("/worldgen/noise/")
                || path.contains("/worldgen/biome/")
                || path.ends_with("/dimension/overworld.json");
            if keep {
                assets.insert(path, bytes);
            }
        }
    }

    let names_pre: Vec<String> = {
        let src = crussty_chunkgen::biomes::BiomeSourceData::from_assets(&assets);
        match src {
            Some(src) => src.biome_names,
            None => Vec::new(),
        }
    };
    let temps_pre: Vec<(f32, bool)> = names_pre.iter().map(|n| biome_temp(&assets, n)).collect();

    let id = match crussty_chunkgen::ffi::ctx_new_assets(seed, 63, assets) {
        Ok(id) => id,
        Err(e) => {
            eprintln!("ctx_new_assets failed: {e}");
            std::process::exit(2);
        }
    };
    let states = match surface_states_of(id) {
        Some(s) => s,
        None => {
            eprintln!("no native surface engine in ctx");
            std::process::exit(2);
        }
    };
    let ids: Vec<u16> = (0..states.len() as u16).map(|i| 22 + i).collect();
    surface_assign_ids(id, &ids).expect("assign ids");
    let (temps, frozen): (Vec<f32>, Vec<bool>) = temps_pre.iter().map(|(t, f)| (*t, *f)).unzip();
    surface_assign_temps(id, &temps, &frozen).expect("assign temps");

    let mut buf = vec![0u16; OUT_LEN];

    let sprof = std::env::var("CRUS_SPROF").is_ok();
    if std::env::var("CRUS_DPROF").as_deref() == Ok("1") {
        crussty_chunkgen::density::PROF_ON
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
    if sprof {
        crussty_chunkgen::surface_fill::SR_PROF_ON
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }

    // warm (JIT-less but cache/branch warmup) + sink to defeat DCE
    let mut sink: u64 = 0;
    for i in 0..16 {
        fill(id, i - 8, 0, buf.as_mut_slice().try_into().unwrap());
        sink = sink.wrapping_add(buf[12345] as u64);
    }
    if sprof {
        // zero counters after warmup
        use crussty_chunkgen::surface_fill as sf;
        for c in [
            &sf::SR_TOTAL_NS,
            &sf::SR_PRELIM_NS,
            &sf::SR_PRELIM_EVALS,
            &sf::SR_TEST_CALLS,
            &sf::SR_COMPUTE_NS,
            &sf::SR_WALK_STEPS,
            &sf::SR_HEIGHTMAP_NS,
            &sf::SR_TEST_TOTAL,
            &sf::SR_C_BIOME,
            &sf::SR_C_NOISE_THR,
            &sf::SR_C_STONE_DEPTH,
            &sf::SR_C_WATER,
            &sf::SR_C_YABOVE,
            &sf::SR_C_VGRAD,
            &sf::SR_C_TEMP,
            &sf::SR_C_OTHER,
        ] {
            c.store(0, std::sync::atomic::Ordering::Relaxed);
        }
    }

    let mut samples: Vec<u128> = Vec::with_capacity(n as usize);
    for i in 0..n {
        let ccx = cx + (i as i32 % 17) - 8; // vary chunk to avoid same-addr cache
        let t0 = Instant::now();
        fill(id, ccx, cz, buf.as_mut_slice().try_into().unwrap());
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
        "native fill dp: n={} p50={}ns p90={}ns min={}ns max={}ns sink={:e}",
        n,
        p(0.50),
        p(0.90),
        samples[0],
        *samples.last().unwrap(),
        sink as f64 * 0.0
    );
    if std::env::var("CRUS_DPROF").as_deref() == Ok("1") {
        use crussty_chunkgen::density as d;
        let g = |c: &std::sync::atomic::AtomicU64| c.load(std::sync::atomic::Ordering::Relaxed);
        println!(
            "dprof: noise {:.2}ms/{} calls | blend {:.2}ms/{} calls | col hit/miss {}/{}",
            g(&d::PROF_NOISE_NS) as f64 / 1e6,
            g(&d::PROF_NOISE_CALLS),
            g(&d::PROF_BLEND_NS) as f64 / 1e6,
            g(&d::PROF_BLEND_CALLS),
            g(&d::COL_HITS),
            g(&d::COL_MISSES),
        );
    }
    if std::env::var("CRUS_PROFILE").as_deref() == Ok("1") {
        println!(
            "{}",
            crussty_chunkgen::nochunk::jni_prof_snapshot()
        );
    }
    if sprof {
        use crussty_chunkgen::surface_fill as sf;
        let g = |c: &std::sync::atomic::AtomicU64| c.load(std::sync::atomic::Ordering::Relaxed);
        let per = |v: u64| v as f64 / n as f64 / 1e6;
        let pern = |v: u64| v as f64 / n as f64;
        println!(
            "sprof/chunk: total {:.2}ms | heightmap {:.3} | prelim {:.2}ms ({} evals) | compute {:.2}ms ({} tests) | walk_steps {}",
            per(g(&sf::SR_TOTAL_NS)),
            per(g(&sf::SR_HEIGHTMAP_NS)),
            per(g(&sf::SR_PRELIM_NS)),
            pern(g(&sf::SR_PRELIM_EVALS)),
            per(g(&sf::SR_COMPUTE_NS)),
            pern(g(&sf::SR_TEST_CALLS)),
            pern(g(&sf::SR_WALK_STEPS)),
        );
        println!(
            "sprof kinds: test_total {} | biome {} nthr {} stone_depth {} water {} y_above {} vgrad {} temp/other {}",
            pern(g(&sf::SR_TEST_TOTAL)),
            pern(g(&sf::SR_C_BIOME)),
            pern(g(&sf::SR_C_NOISE_THR)),
            pern(g(&sf::SR_C_STONE_DEPTH)),
            pern(g(&sf::SR_C_WATER)),
            pern(g(&sf::SR_C_YABOVE)),
            pern(g(&sf::SR_C_VGRAD)),
            pern(g(&sf::SR_C_TEMP) + g(&sf::SR_C_OTHER)),
        );
    }
}

// --- sub-phase probe: eval-only timing of slot0 plane (CRUS_EVALPROF=1) ---
