//! bhdebug — compare native fill blocks vs base_height walk at one column.

use crussty_chunkgen::ffi;

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

use std::path::Path;

fn main() {
    let seed: i64 = 3257840388504953787;
    let mut assets = crussty_chunkgen::worldgen::Assets::new();
    for r in ["workdir/dpext/merged/01", "workdir/dpext/merged/02"] {
        let root = Path::new(r);
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
    let id = ffi::ctx_new_assets(seed, 63, assets).expect("ctx");
    for (cx, cz) in [(0, 0), (32, 32)] {
        let mut buf = vec![0u16; crussty_chunkgen::nochunk::OUT_LEN];
        ffi::fill(id, cx, cz, buf.as_mut_slice().try_into().unwrap());
        let mut out = String::new();
        for y in [3i32, 15, 21, 47, 48, 61, 62] {
            let b = buf[((y + 64) * 256) as usize];
            out.push_str(&format!(" y{y}={b}"));
        }
        println!("fill({cx},{cz}):{out}");
    }
    let w = ffi::base_height(id, 512, 512, 0);
    let o = ffi::base_height(id, 512, 512, 1);
    println!("base_height world={w} ocean={o}");
    let w2 = ffi::base_height(id, 0, 0, 0);
    let o2 = ffi::base_height(id, 0, 0, 1);
    println!("base_height(0,0) world={w2} ocean={o2}");
}
