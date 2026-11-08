//! signfoldprobe — engagement + bit-neutrality of the sign-op run fold
//! (cont-128) on REAL pack worldgen: unzip datapacks, build the merged
//! Assets (later wins), compile with fold on/off, report arena shrinkage
//! and spot-check bit parity on every router root.
//!
//! Usage: signfoldprobe <extracted_root>... (each root = datapack, later
//! overrides earlier) or signfoldprobe --zip <a.zip> <b.zip> ...

use crussty_chunkgen::density::Env;
use std::path::{Path, PathBuf};
use std::process::Command;

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
    let seed = 3257840388504953787i64;
    let args: Vec<String> = std::env::args().skip(1).collect();
    let tmp = std::env::temp_dir().join("signfoldprobe_packs");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let mut roots: Vec<std::path::PathBuf> = Vec::new();
    let mut zips: Vec<String> = Vec::new();
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        if a == "--zip" {
            zips = it.collect();
            break;
        } else {
            roots.push(PathBuf::from(a));
        }
    }
    for (i, z) in zips.iter().enumerate() {
        let dst = tmp.join(format!("p{i}"));
        let st = Command::new("unzip")
            .args(["-oq", z.as_str(), "-d", dst.to_str().unwrap()])
            .status()
            .expect("unzip");
        assert!(st.success(), "unzip {z}");
        roots.push(dst);
    }
    let mut assets = crussty_chunkgen::worldgen::Assets::new();
    for root in &roots {
        let mut files = Vec::new();
        collect(root, root, &mut files);
        files.sort();
        for (path, bytes) in files {
            if path.contains("/worldgen/") {
                assets.insert(path, bytes);
            }
        }
    }
    println!("assets: {} entries", assets.len());
    let env: &'static Env = Box::leak(Box::new(Env::new_assets(seed, &assets)));
    let on = env.compile_opt(true);
    let off = env.compile_opt(false);
    println!(
        "arena: fold-on {} vs fold-off {} nodes ({} saved, {:.2}%)",
        on.arena.len(),
        off.arena.len(),
        off.arena.len() - on.arena.len(),
        100.0 * (off.arena.len() - on.arena.len()) as f64 / off.arena.len() as f64
    );
    // bit parity across every router root on a point grid
    let mut bad = 0usize;
    let mut checked = 0usize;
    let mut keys: Vec<&String> = on.router.keys().collect();
    keys.sort();
    for k in &keys {
        let (ron, roff) = (on.router[k.as_str()], off.router[k.as_str()]);
        let interp_on = crussty_chunkgen::density::InterpState::new(on.interp_slots, 1, 48);
        let interp_off = crussty_chunkgen::density::InterpState::new(off.interp_slots, 1, 48);
        for i in 0..4i32 {
            for j in 0..3i32 {
                for y in (0..48i32).step_by(7) {
                    let pt = crussty_chunkgen::density::Pt {
                        x: 40 + i * 8,
                        y: (y - 8) * 8,
                        z: -24 + j * 8,
                    };
                    let a = env.eval(&on, ron, pt, crussty_chunkgen::density::Mode::Slice, &interp_on, None);
                    let b = env.eval(&off, roff, pt, crussty_chunkgen::density::Mode::Slice, &interp_off, None);
                    if a.to_bits() != b.to_bits() {
                        bad += 1;
                        eprintln!("BIT DIFF at {k} ({},{},{}) bits {} vs {}", pt.x, pt.y, pt.z, a.to_bits(), b.to_bits());
                    }
                    checked += 1;
                }
            }
        }
    }
    println!("bit parity: {checked} points, {bad} diffs");
    assert!(bad == 0, "sign fold is NOT bit-neutral");
    println!("SIGNFOLDPROBE OK");
}
