//! Isolated native fill benchmark with datapack assets — the exact fill path
//! the plugin's JNI uses (noise + surface tail after assign_ids/temps).
//!
//! Usage: dptbench <dp_root...> [n_fills] [seed]
//!
//! Reports mean/p50/p90/max us per fill over a rotating 16x16 coord tile so
//! caches stay honest (same chunk coords repeat every 256 fills).

use crussty_chunkgen::ffi::{
    bench_fill_biomes, ctx_new_assets, fill, surface_assign_ids, surface_assign_temps,
    surface_states_of,
};
use crussty_chunkgen::worldgen::Assets;
use std::path::Path;
use std::time::Instant;

#[allow(dead_code)]
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
    let Some(bytes) = bytes else { return (0.8, false) };
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

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut roots = Vec::new();
    let mut rest = Vec::new();
    let mut i = 1;
    while i < args.len() {
        if args[i].starts_with('/') || args[i].starts_with('.') {
            roots.push(args[i].clone());
        } else {
            rest.push(args[i].clone());
        }
        i += 1;
    }
    let n: usize = rest.first().and_then(|s| s.parse().ok()).unwrap_or(256);
    let seed: i64 = rest
        .get(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(3257840388504953787);
    assert!(!roots.is_empty(), "usage: dptbench <dp_root...> [n] [seed]");

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
    println!("assets: {} files", assets.len());

    let names_pre: Vec<String> = {
        match crussty_chunkgen::biomes::BiomeSourceData::from_assets(&assets) {
            Some(src) => src.biome_names,
            None => Vec::new(),
        }
    };
    let temps_pre: Vec<(f32, bool)> = names_pre.iter().map(|nm| biome_temp(&assets, nm)).collect();

    let id = match ctx_new_assets(seed, 63, assets) {
        Ok(id) => id,
        Err(e) => {
            eprintln!("ctx_new_assets failed: {e}");
            std::process::exit(2);
        }
    };
    let states = if std::env::var("CRUS_NO_SURFACE").ok().as_deref() == Some("1") {
        println!("surface: DISABLED (CRUS_NO_SURFACE=1) — noise-only fill\n");
        None
    } else {
        Some(surface_states_of(id).expect("no native surface engine in ctx"))
    };
    if let Some(states) = &states {
        let ids: Vec<u16> = (0..states.len() as u16).map(|i| 1000 + i).collect();
        surface_assign_ids(id, &ids).expect("assign ids");
        let temps: Vec<f32> = temps_pre.iter().map(|t| t.0).collect();
        let frozen: Vec<bool> = temps_pre.iter().map(|t| t.1).collect();
        surface_assign_temps(id, &temps, &frozen).expect("assign temps");
        println!(
            "surface states: {} | biomes: {}",
            states.len(),
            temps_pre.len()
        );
    }

    let mut buf = vec![0u16; crussty_chunkgen::nochunk::OUT_LEN];
    let arr: &mut [u16; crussty_chunkgen::nochunk::OUT_LEN] =
        buf.as_mut_slice().try_into().unwrap();
    // biome quart grid (plugin's fillBiomes output — feeds BiomeSwap AND the
    // surface tail via grid_cache)
    let mut bgrid: Vec<i32> = vec![0; (384 / 16) * 64];

    // warmup 16 (plugin order: fillBiomes then fill)
    for k in 0..16 {
        let cx = 32 + (k % 4) as i32;
        let cz = 32 + (k / 4) as i32;
        bench_fill_biomes(id, cx, cz, &mut bgrid);
        fill(id, cx, cz, arr);
    }

    // biome-grid (fillBiomes) cost alone
    let mut bio_samples: Vec<u128> = Vec::with_capacity(n);
    for k in 0..n {
        let cx = 32 + (k % 16) as i32;
        let cz = 32 + ((k / 16) % 16) as i32;
        let t0 = Instant::now();
        bench_fill_biomes(id, cx, cz, &mut bgrid);
        bio_samples.push(t0.elapsed().as_micros());
    }
    bio_samples.sort();

    let mut samples: Vec<u128> = Vec::with_capacity(n);
    for k in 0..n {
        let cx = 32 + (k % 16) as i32;
        let cz = 32 + ((k / 16) % 16) as i32;
        // plugin order per chunk: BIOMES (fillBiomes) then NOISE+SURFACE (fill)
        bench_fill_biomes(id, cx, cz, &mut bgrid);
        let t0 = Instant::now();
        fill(id, cx, cz, arr);
        samples.push(t0.elapsed().as_micros());
    }
    samples.sort();
    let mean = samples.iter().sum::<u128>() / samples.len() as u128;
    let p50 = samples[samples.len() / 2];
    let p90 = samples[samples.len() * 9 / 10];
    let p99 = samples[samples.len() * 99 / 100];
    let bmean = bio_samples.iter().sum::<u128>() / bio_samples.len() as u128;
    println!(
        "fills={} fill mean={}us p50={}us p90={}us p99={}us min={}us max={}us",
        n,
        mean,
        p50,
        p90,
        p99,
        samples[0],
        samples[n - 1]
    );
    println!(
        "fillBiomes mean={}us p50={}us p90={}us",
        bmean,
        bio_samples[bio_samples.len() / 2],
        bio_samples[bio_samples.len() * 9 / 10]
    );
    println!("jni_prof: {}", crussty_chunkgen::nochunk::jni_prof_snapshot());

    // ---- deep-zone fast-path bit-parity self-check vs full walk ----------
    std::env::set_var("CRUS_SF_NOFAST", "1");
    let mut ref_buf = vec![0u16; crussty_chunkgen::nochunk::OUT_LEN];
    let ref_arr: &mut [u16; crussty_chunkgen::nochunk::OUT_LEN] =
        ref_buf.as_mut_slice().try_into().unwrap();
    let mut bad = 0usize;
    for k in 0..12 {
        let cx = 60 + (k % 4) as i32;
        let cz = 60 + (k / 4) as i32;
        bench_fill_biomes(id, cx, cz, &mut bgrid);
        fill(id, cx, cz, ref_arr); // fallback full-walk path
        std::env::remove_var("CRUS_SF_NOFAST");
        fill(id, cx, cz, arr); // fast zones
        std::env::set_var("CRUS_SF_NOFAST", "1");
        for i in 0..crussty_chunkgen::nochunk::OUT_LEN {
            if ref_arr[i] != arr[i] {
                bad += 1;
                if bad <= 4 {
                    let y = i as i32 / 256 - 64;
                    eprintln!("FASTPATH DIVERGE ({cx},{cz}) y={y} slow={} fast={}", ref_arr[i], arr[i]);
                }
            }
        }
    }
    std::env::remove_var("CRUS_SF_NOFAST");
    println!(
        "fastpath self-check: 12 chunks, {bad} divergent blocks {}",
        if bad == 0 { "(BIT-EXACT)" } else { "(BROKEN!)" }
    );
}
