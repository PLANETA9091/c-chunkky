//! Evaluate every noise_router DF at given block positions (datapack assets).
//! Pairs with the oracle plugin's -Doracledfprobe mode.
//!
//! Usage: dfprobe <extracted_root>... -- "x,y,z;x,y,z;..." [seed]

use crussty_chunkgen::density::{Env, InterpState, Mode, Pt};
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

const FIELDS: &[&str] = &[
    "barrier", "fluid_level_floodedness", "fluid_level_spread", "lava",
    "temperature", "vegetation", "continents", "erosion", "depth", "ridges",
    "initial_density_without_jaggedness", "final_density",
    "vein_toggle", "vein_ridged", "vein_gap",
];

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut roots: Vec<String> = Vec::new();
    let mut pos: Vec<String> = Vec::new();
    let mut seen_dash = false;
    for a in &args[1..] {
        if a == "--" {
            seen_dash = true;
            continue;
        }
        if !seen_dash {
            roots.push(a.clone());
        } else {
            pos.push(a.clone());
        }
    }
    let spec = pos.get(0).cloned().unwrap_or_default();
    let seed: i64 = pos.get(1).and_then(|s| s.parse().ok()).unwrap_or(3257840388504953787);

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
    let env: &'static Env = Box::leak(Box::new(Env::new_assets(seed, &assets)));
    let compiled = env.compile();

    for part in spec.split(';') {
        let c: Vec<&str> = part.trim().split(',').collect();
        if c.len() != 3 {
            continue;
        }
        let (x, y, z): (i32, i32, i32) = (
            c[0].trim().parse().unwrap(),
            c[1].trim().parse().unwrap(),
            c[2].trim().parse().unwrap(),
        );
        let pt = Pt { x, y, z };
        let interp = InterpState::new(compiled.interp_slots, 4, 48);
        for f in FIELDS {
            let Some(&idx) = compiled.router.get(*f) else {
                println!("{x} {y} {z} {f} MISSING");
                continue;
            };
            let v = env.eval(&compiled, idx, pt, Mode::Direct, &interp, None);
            println!("{x} {y} {z} {f} {} {v:.17e}", v.to_bits() as i64);
        }
    }
}
