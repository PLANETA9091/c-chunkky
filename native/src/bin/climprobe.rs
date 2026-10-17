//! Climate probe: evaluate the six biome climate fields at one quart and
//! compare the RTree pick against a brute-force nearest scan.
//!
//! Usage: climprobe <dp_root...> -- <qx> <qy> <qz> [seed]

use crussty_chunkgen::biomes::{fill_biomes, BiomeSourceData, ClimateDFs};
use crussty_chunkgen::worldgen::Assets;
use std::path::Path;

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
        if seen {
            pos.push(a.clone());
        } else {
            roots.push(a.clone());
        }
    }
    let qx: i32 = pos.get(0).and_then(|s| s.parse().ok()).unwrap_or(0);
    let qy: i32 = pos.get(1).and_then(|s| s.parse().ok()).unwrap_or(-13);
    let qz: i32 = pos.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
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

    let env: &'static crussty_chunkgen::density::Env =
        Box::leak(Box::new(crussty_chunkgen::density::Env::new_assets(seed, &assets)));
    let compiled = env.compile();
    let src = BiomeSourceData::from_assets(&assets).expect("biome source");
    let climate = ClimateDFs::from_router(&compiled.router, env, &compiled).expect("climate");

    // 1) fill_biomes FIRST with a cold thread-local (reproduce biomefill)
    {
        let height = env.wg.height as usize;
        let sections = height / 16;
        let mut grid = vec![0i32; sections * 64];
        fill_biomes(&src, &climate, env, &compiled, qx >> 2, qz >> 2, &mut grid);
        let sy = ((qy - env.wg.min_y / 4) / 4) as usize;
        let lx = (qx & 3) as usize;
        let ly = ((qy - env.wg.min_y / 4) & 3) as usize;
        let lz = (qz & 3) as usize;
        let gi = sy * 64 + lx * 16 + ly * 4 + lz;
        println!("COLD-fill pick @ ({qx},{qy},{qz}) -> {}", src.biome_names[grid[gi] as usize]);
    }

    let pt = crussty_chunkgen::density::Pt {
        x: qx * 4,
        y: qy * 4,
        z: qz * 4,
    };
    let interp = crussty_chunkgen::density::InterpState::new(
        compiled.interp_slots,
        4,
        (env.wg.height / 8) as usize,
    );
    let mut memo = crussty_chunkgen::density::MemoCtx::new(compiled.arena.len())
        .with_cols(compiled.col_count as usize);
    memo.next_serial();
    let names = ["temperature", "vegetation", "continents", "erosion", "depth", "ridges"];
    let mut vals = [0i64; 7];
    for (fi, name) in names.iter().enumerate() {
        let v = env.eval(&compiled, climate.idx[fi], pt, crussty_chunkgen::density::Mode::Direct, &interp, Some(&memo));
        let q = crussty_chunkgen::biomes::quantize_pub(v);
        vals[fi] = q;
        println!("{name:14} = {v:.6} -> {q}");
    }
    vals[6] = 0; // offset sample

    // tree pick (cold start — fresh source instance, no warm start yet)
    let tree_leaf = src.find_value(&vals);
    println!("tree pick  : {} ({})", tree_leaf, src.biome_names[src.entries[tree_leaf].biome_idx as usize]);

    // brute force top-5
    let mut ranked: Vec<(i64, usize)> = (0..src.entries.len())
        .map(|i| {
            let mut l: i64 = 0;
            for d in 0..7 {
                let dd = src.entries[i].params[d].distance(vals[d]);
                l = l.wrapping_add(dd.wrapping_mul(dd));
            }
            (l, i)
        })
        .collect();
    ranked.sort_by_key(|(d, _)| *d);
    let mut k = 0usize;
    println!("brute-force top-5:");
    for (d, i) in &ranked {
        if k >= 5 { break }
        let i = *i as usize;
        println!("  dist={:>12} (f64={:.5}) {}", d, *d as f64 / 1e8, src.biome_names[src.entries[i].biome_idx as usize]);
        k += 1;
    }

    // context: what does fill_biomes produce for the chunk containing this quart?
    let cx = qx >> 2;
    let cz = qz >> 2;
    let height = env.wg.height as usize;
    let sections = height / 16;
    let mut grid = vec![0i32; sections * 64];
    fill_biomes(&src, &climate, env, &compiled, cx, cz, &mut grid);
    let sy = ((qy - env.wg.min_y / 4) / 4) as usize;
    let lx = (qx & 3) as usize;
    let ly = ((qy - env.wg.min_y / 4) & 3) as usize;
    let lz = (qz & 3) as usize;
    let gi = sy * 64 + lx * 16 + ly * 4 + lz;
    println!(
        "fill_biomes @ chunk ({cx},{cz}) quart ({qx},{qy},{qz}) -> {}",
        src.biome_names[grid[gi] as usize]
    );
}
