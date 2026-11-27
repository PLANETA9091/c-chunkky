//! Streaming PACK-path fill bench: spiral of distinct chunks through the
//! production ffi::fill entry (noise + surface tail), mirroring the server.
//! Usage: packcolbench <dp_root...> -- <n> <cx0> <cz0> [seed]
use crussty_chunkgen::ffi::{ctx_new_assets, fill, surface_assign_ids, surface_assign_temps, surface_states_of};
use crussty_chunkgen::nochunk::{reset_jni_prof, OUT_LEN};
use crussty_chunkgen::worldgen::Assets;
use std::path::Path;
use std::time::Instant;

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
    let n: usize = pos.first().and_then(|s| s.parse().ok()).unwrap_or(100);
    let cx0: i32 = pos.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    let cz0: i32 = pos.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
    let seed: i64 = pos.get(3).and_then(|s| s.parse().ok()).unwrap_or(3257840388504953787);

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
    let id = match ctx_new_assets(seed, 63, assets) {
        Ok(id) => id,
        Err(e) => {
            eprintln!("ctx_new_assets failed: {e}");
            std::process::exit(2);
        }
    };
    let _ = crussty_chunkgen::density::h6_diag_init();
    let states = match surface_states_of(id) {
        Some(s) => s,
        None => {
            eprintln!("no native surface engine in ctx");
            std::process::exit(2);
        }
    };
    let ids: Vec<u16> = (0..states.len() as u16).map(|i| 22 + i).collect();
    surface_assign_ids(id, &ids).expect("assign ids");
    let temps: Vec<f32> = (0..states.len()).map(|_| 0.8).collect();
    let frozen: Vec<bool> = (0..states.len()).map(|_| false).collect();
    surface_assign_temps(id, &temps, &frozen).expect("assign temps");

    // spiral coords
    let mut coords = Vec::with_capacity(n);
    let (mut x, mut z) = (cx0, cz0);
    let (mut dx, mut dz) = (1i32, 0i32);
    let mut leg = 1usize;
    let mut step = 0usize;
    for _ in 0..n {
        coords.push((x, z));
        x += dx;
        z += dz;
        step += 1;
        if step == leg {
            step = 0;
            (dx, dz) = (-dz, dx);
            if dz == 0 {
                leg += 1;
            }
        }
    }

    let mut buf = [0u16; OUT_LEN];
    for i in 0..8.min(coords.len()) {
        let (cx, cz) = coords[i];
        fill(id, cx as i32, cz as i32, &mut buf);
    }
    reset_jni_prof();

    let t0 = Instant::now();
    let mut sink = 0u64;
    for &(cx, cz) in &coords {
        fill(id, cx as i32, cz as i32, &mut buf);
        sink ^= buf[0] as u64;
    }
    let el = t0.elapsed();
    let cnt = coords.len() as f64;
    println!(
        "packcolbench: {} chunks, mean {:.2} ms/chunk (sink {:x})",
        coords.len(),
        el.as_secs_f64() * 1000.0 / cnt,
        sink & 0xffff
    );
    println!("{}", crussty_chunkgen::nochunk::jni_prof_snapshot());
    if std::env::var("CRUS_H6_DIAG").as_deref() == Ok("1") {
        println!("{}", crussty_chunkgen::density::h6_diag_snapshot());
    }
}
