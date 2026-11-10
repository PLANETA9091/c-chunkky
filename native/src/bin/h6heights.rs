//! h6heights — dump native base_height over a block grid for parent-vs-branch
//! dump diffing (the cont-133 `heightcmp` bin was lost with the local-only
//! stageD branch; the gate returns as a byte-diff of two dumps: any value
//! change in the height walk = REJECT).
//!
//! The dump format matches ProbePlugin.crusbase ("x z world ocean"), so the
//! output can also be fed to baseref directly.
//!
//! Usage: h6heights <dp_root...> -- <x0> <z0> <x1> <z1> [step] [seed] [outfile]

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
    let x0: i32 = pos.first().and_then(|s| s.parse().ok()).unwrap_or(-64);
    let z0: i32 = pos.get(1).and_then(|s| s.parse().ok()).unwrap_or(-64);
    let x1: i32 = pos.get(2).and_then(|s| s.parse().ok()).unwrap_or(63);
    let z1: i32 = pos.get(3).and_then(|s| s.parse().ok()).unwrap_or(63);
    let step: i32 = pos.get(4).and_then(|s| s.parse().ok()).unwrap_or(2);
    let seed: i64 = pos
        .get(5)
        .and_then(|s| s.parse().ok())
        .unwrap_or(3257840388504953787);
    let outfile = pos
        .get(6)
        .cloned()
        .unwrap_or_else(|| "h6heights.txt".into());
    let step = if step >= 1 { step } else { 1 };

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
    let id = match ffi::ctx_new_assets(seed, 63, assets) {
        Ok(id) => id,
        Err(e) => {
            eprintln!("ctx_new_assets failed: {e}");
            std::process::exit(2);
        }
    };

    use std::fmt::Write as _;
    let mut buf = String::new();
    let mut n = 0usize;
    let mut z = z0;
    while z <= z1 {
        let mut x = x0;
        while x <= x1 {
            let w = ffi::base_height(id, x, z, 0);
            let o = ffi::base_height(id, x, z, 1);
            let _ = writeln!(buf, "{} {} {} {}", x, z, w, o);
            n += 1;
            x += step;
        }
        z += step;
    }
    buf.push_str("DONE\n");
    std::fs::write(&outfile, buf).expect("write dump");
    println!("h6heights: {} columns -> {} (step {})", n, outfile, step);
}
