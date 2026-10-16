//! Native biome fill test: dump the native biome resolution as
//! "BIOME <qx> <qy> <qz> <biome-name>" in the vanilla fill order.
//!
//! Usage: biomefill <extracted_root>... -- cx cz [seed]

use crussty_chunkgen::biomes::{fill_biomes, BiomeSourceData, ClimateDFs};
use crussty_chunkgen::worldgen::Assets;
use std::path::Path;

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
    let cx: i32 = pos.get(0).and_then(|s| s.parse().ok()).unwrap_or(0);
    let cz: i32 = pos.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    let seed: i64 = pos.get(2).and_then(|s| s.parse().ok()).unwrap_or(3257840388504953787);

    let mut assets = Assets::new();
    for root in &roots {
        let root = Path::new(root);
        let mut files = Vec::new();
        collect(root, root, &mut files);
        files.sort();
        for (path, bytes) in files {
            let keep = path.contains("/worldgen/noise_settings/")
                || path.contains("/worldgen/density_function/")
                || path.contains("/worldgen/noise/")
                || path.ends_with("/dimension/overworld.json");
            if keep {
                assets.insert(path, bytes);
            }
        }
    }
    let env: &'static crussty_chunkgen::density::Env =
        Box::leak(Box::new(crussty_chunkgen::density::Env::new_assets(seed, &assets)));
    let compiled = env.compile();
    let src = BiomeSourceData::from_assets(&assets).expect("multi_noise biome source");
    eprintln!(
        "biome source: {} entries, {} unique biomes; yinv flags: {:?}",
        src.entries.len(),
        src.biome_names.len(),
        ClimateDFs::from_router(&compiled.router, env, &compiled)
            .map(|c| c.y_invariant)
            .unwrap_or_default()
    );
    let climate = ClimateDFs::from_router(&compiled.router, env, &compiled).expect("climate dfs");
    if std::env::var("CRUS_BIOME_DEBUG").is_ok() {
        for (fi, name) in ["temperature", "vegetation", "continents", "erosion", "depth", "ridges"]
            .iter()
            .enumerate()
        {
            let idx = climate.idx[fi];
            let mut stack = vec![idx];
            let mut seen = std::collections::HashSet::new();
            let mut kinds: Vec<String> = Vec::new();
            while let Some(i) = stack.pop() {
                if !seen.insert(i) {
                    continue;
                }
                match &compiled.arena[i as usize] {
                    crussty_chunkgen::density::WKind::Noise { y_scale, .. } => {
                        kinds.push(format!("noise(ys={y_scale})"))
                    }
                    crussty_chunkgen::density::WKind::ShiftedNoise { y_scale, .. } => {
                        kinds.push(format!("shifted(ys={y_scale})"))
                    }
                    _ => {}
                }
                for c in compiled.arena[i as usize].children() {
                    stack.push(c);
                }
            }
            eprintln!(
                "router {name} yinv={} tree: {}",
                climate.y_invariant[fi],
                kinds.join(", ")
            );
        }
    }

    let height = env.wg.height as usize;
    let sections = height / 16;
    let mut out = vec![0i32; sections * 64];
    let t0 = std::time::Instant::now();
    fill_biomes(&src, &climate, env, &compiled, cx, cz, &mut out);
    let dt = t0.elapsed();
    use crussty_chunkgen::biomes::{SEARCH_NS, EVAL_NS};
    eprintln!(
        "fill_biomes: {dt:?} ({:.2}us/quart) | search {:.2}ms | eval {:.2}ms",
        dt.as_nanos() as f64 / (sections * 64) as f64 / 1e3,
        SEARCH_NS.load(std::sync::atomic::Ordering::Relaxed) as f64 / 1e6,
        EVAL_NS.load(std::sync::atomic::Ordering::Relaxed) as f64 / 1e6
    );
    eprintln!(
        "col cache: hits={} misses={}",
        crussty_chunkgen::density::COL_HITS.load(std::sync::atomic::Ordering::Relaxed),
        crussty_chunkgen::density::COL_MISSES.load(std::sync::atomic::Ordering::Relaxed)
    );

    let quart_x0 = cx * 4;
    let quart_z0 = cz * 4;
    let quart_y0 = env.wg.min_y / 4;
    let mut s = String::new();
    for sy in 0..sections {
        for x in 0..4usize {
            for y in 0..4usize {
                for z in 0..4usize {
                    let idx = src.biome_names[out[sy * 64 + x * 16 + y * 4 + z] as usize].clone();
                    s.push_str(&format!(
                        "BIOME {} {} {} {}\n",
                        quart_x0 + x as i32,
                        quart_y0 + sy as i32 * 4 + y as i32,
                        quart_z0 + z as i32,
                        idx
                    ));
                }
            }
        }
    }
    print!("{s}");
}
