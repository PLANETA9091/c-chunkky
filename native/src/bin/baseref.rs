//! baseref — native base_height vs vanilla delegate dump (crusbase probe).
//!
//! Usage: baseref <dp_root...> -- <seed> <sea_level> <dump_file>
//! The dump lines: "x z world ocean" (from ProbePlugin.crusbase). We rebuild
//! the ctx from the same pack roots (same filter as the plugin: noise_settings,
//! density_function, noise, biome, dimension/overworld) and diff every point.

use crussty_chunkgen::ffi;
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
        if seen {
            pos.push(a.clone());
        } else {
            roots.push(a.clone());
        }
    }
    if pos.len() < 3 {
        eprintln!("usage: baseref <dp_root...> -- <seed> <sea_level> <dump_file>");
        std::process::exit(2);
    }
    let seed: i64 = pos[0].parse().expect("seed");
    let sea: i32 = pos[1].parse().expect("sea");
    let dump = pos[2].clone();

    // Same asset filter the plugin uses (packRel): noise_settings,
    // density_function, noise, biome, dimension/overworld.
    let mut assets = crussty_chunkgen::worldgen::Assets::new();
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
    let id = match ffi::ctx_new_assets(seed, sea, assets) {
        Ok(id) => id,
        Err(e) => {
            eprintln!("ctx_new_assets failed: {e}");
            std::process::exit(2);
        }
    };

    let text = std::fs::read_to_string(&dump).expect("dump file");
    let mut total = 0usize;
    let mut world_diff = 0usize;
    let mut ocean_diff = 0usize;
    let mut first_diffs: Vec<String> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with("DONE") {
            continue;
        }
        if line.starts_with("FAILED") {
            eprintln!("probe dump failed: {line}");
            std::process::exit(3);
        }
        let p: Vec<&str> = line.split_whitespace().collect();
        if p.len() != 4 {
            continue;
        }
        let x: i32 = p[0].parse().unwrap();
        let z: i32 = p[1].parse().unwrap();
        let v_world: i32 = p[2].parse().unwrap();
        let v_ocean: i32 = p[3].parse().unwrap();
        let n_world = ffi::base_height(id, x, z, 0);
        let n_ocean = ffi::base_height(id, x, z, 1);
        total += 1;
        let mut bad = false;
        if n_world != v_world {
            world_diff += 1;
            bad = true;
        }
        if n_ocean != v_ocean {
            ocean_diff += 1;
            bad = true;
        }
        if bad && first_diffs.len() < 12 {
            first_diffs.push(format!(
                "({x},{z}) world {v_world} vs {n_world} | ocean {v_ocean} vs {n_ocean}"
            ));
        }
    }
    println!("points={total} world_diff={world_diff} ocean_diff={ocean_diff}");
    for d in &first_diffs {
        println!("  DIFF {d}");
    }
    if world_diff == 0 && ocean_diff == 0 {
        println!("BASEHEIGHT PARITY OK");
    } else {
        println!("BASEHEIGHT PARITY FAILED");
        std::process::exit(1);
    }
}
