//! Probe: which FlatCache/Cache2d children in the compiled arena are
//! y-DEPENDENT (value at (x,10,z) != value at (x,200,z)) — the nodes that
//! make vanilla's y=0 FlatCache grid semantics diverge from transparent.
//!
//! Usage: yprobe <extracted_root>... [seed]

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

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let seed: i64 = args
        .get(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(3257840388504953787);
    let mut assets = Assets::new();
    for root in &args[1..] {
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
    let interp = InterpState::new(compiled.interp_slots, 4, 48);

    // roots: every router field (the fill's evaluation set)
    let mut roots: Vec<u32> = compiled.router.values().copied().collect();

    // walk the DAG, find Cache2d/FlatCache nodes
    let mut seen = std::collections::HashSet::new();
    let mut stack: Vec<u32> = roots;
    let mut caches: Vec<(u32, &'static str, u32)> = Vec::new(); // (idx, kind, child)
    while let Some(i) = stack.pop() {
        if !seen.insert(i) {
            continue;
        }
        match &compiled.arena[i as usize] {
            crussty_chunkgen::density::WKind::FlatCache { child } => {
                caches.push((i, "flat_cache", *child));
                stack.push(*child);
            }
            crussty_chunkgen::density::WKind::Cache2d { child } => {
                caches.push((i, "cache_2d", *child));
                stack.push(*child);
            }
            _ => {
                for c in compiled.arena[i as usize].children() {
                    stack.push(c);
                }
            }
        }
    }
    eprintln!("reachable cache nodes: {}", caches.len());

    // probe each child at two y values (in-chunk x,z)
    let mut ydep = 0;
    for (idx, kind, child) in &caches {
        let p1 = Pt { x: 3, y: 10, z: 7 };
        let p2 = Pt { x: 3, y: 200, z: 7 };
        let v1 = env.eval(&compiled, *child, p1, Mode::Direct, &interp, None);
        let v2 = env.eval(&compiled, *child, p2, Mode::Direct, &interp, None);
        if v1.to_bits() != v2.to_bits() {
            ydep += 1;
            // how far does this node reach upward? find which roots contain it
            let mut used_by = Vec::new();
            for (f, &ri) in &compiled.router {
                // quick reachability check
                if reaches(&compiled, ri, *idx) {
                    used_by.push(f.as_str());
                }
            }
            println!(
                "YDEP {kind} node={idx} child={child} v(y10)={v1:.6} v(y200)={v2:.6} used_by={used_by:?}"
            );
        }
    }
    println!("total cache nodes: {} y-dependent: {ydep}", caches.len());
}

fn reaches(compiled: &crussty_chunkgen::density::Compiled, root: u32, target: u32) -> bool {
    let mut seen = std::collections::HashSet::new();
    let mut stack = vec![root];
    while let Some(i) = stack.pop() {
        if i == target {
            return true;
        }
        if !seen.insert(i) {
            continue;
        }
        match &compiled.arena[i as usize] {
            crussty_chunkgen::density::WKind::FlatCache { child }
            | crussty_chunkgen::density::WKind::Cache2d { child }
            | crussty_chunkgen::density::WKind::CacheOnce { child }
            | crussty_chunkgen::density::WKind::CacheAllInCell { child } => stack.push(*child),
            _ => {
                for c in compiled.arena[i as usize].children() {
                    stack.push(c);
                }
            }
        }
    }
    false
}
