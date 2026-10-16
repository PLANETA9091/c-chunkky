//! Compare native datapack fill against the Java oracle noise-stage dump
//! (oracle_noisestage/chunk_X_Y.txt, non-air blocks, "x y z blockkey").
//!
//! Usage: dpcmp <extracted_root>... -- oracle_dir cx cz [seed]

use crussty_chunkgen::worldgen::Assets;
use std::collections::{HashMap, HashSet};
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

const REG: &[(u16, &str)] = &[
    (0, "minecraft:air"), (1, "minecraft:stone"), (2, "minecraft:bedrock"),
    (3, "minecraft:water"), (4, "minecraft:grass_block"), (5, "minecraft:dirt"),
    (6, "minecraft:deepslate"), (7, "minecraft:sand"), (8, "minecraft:gravel"),
    (9, "minecraft:granite"), (10, "minecraft:diorite"), (11, "minecraft:andesite"),
    (12, "minecraft:tuff"), (13, "minecraft:sandstone"), (14, "minecraft:clay"),
    (15, "minecraft:powder_snow"), (16, "minecraft:ice"), (17, "minecraft:lava"),
    (18, "minecraft:copper_ore"), (19, "minecraft:raw_copper_block"),
    (20, "minecraft:deepslate_iron_ore"), (21, "minecraft:raw_iron_block"),
];

fn base(name: &str) -> &str {
    name.split('[').next().unwrap_or(name)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut roots = Vec::new();
    let mut oracle_dir = String::new();
    let mut cx = 0i32;
    let mut cz = 0i32;
    let mut seed = 3257840388504953787i64;
    let mut after = false;
    let mut ai = 0;
    while ai < args.len() {
        match args[ai].as_str() {
            "--" => after = true,
            _ if !after => roots.push(args[ai].clone()),
            _ => {
                let v = args[ai].clone();
                match ai {
                    _ if oracle_dir.is_empty() => oracle_dir = v,
                    _ if args[ai - 1].is_empty() => {}
                    _ => {}
                }
                // positional after --: oracle_dir cx cz [seed]
            }
        }
        ai += 1;
    }
    // simpler: re-parse positionally
    let mut pos: Vec<String> = Vec::new();
    let mut seen_dash = false;
    for a in &args[1..] {
        if a == "--" { seen_dash = true; continue; }
        if !seen_dash { roots.push(a.clone()); } else { pos.push(a.clone()); }
    }
    oracle_dir = pos.get(0).cloned().unwrap_or_default();
    cx = pos.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    cz = pos.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
    seed = pos.get(3).and_then(|s| s.parse().ok()).unwrap_or(seed);

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
            if keep { assets.insert(path, bytes); }
        }
    }
    let env: &'static crussty_chunkgen::density::Env = Box::leak(Box::new(
        crussty_chunkgen::density::Env::new_assets(seed, &assets),
    ));
    let mut gen = crussty_chunkgen::nochunk::ChunkGen::new(env);
    let mut out = [0u16; crussty_chunkgen::nochunk::OUT_LEN];
    gen.fill(cx, cz, &mut out);

    let id2name: HashMap<u16, &str> = REG.iter().copied().collect();
    let name_of = |id: u16| -> String {
        id2name.get(&id).map(|s| s.to_string()).unwrap_or(format!("id{id}"))
    };

    // oracle dump: non-air blocks
    let path = format!("{oracle_dir}/chunk_{cx}_{cz}.txt");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    let mut ref_map: HashMap<(i32, i32, i32), String> = HashMap::new();
    for line in text.lines() {
        let mut it = line.split_whitespace();
        let x: i32 = it.next().unwrap().parse().unwrap();
        let y: i32 = it.next().unwrap().parse().unwrap();
        let z: i32 = it.next().unwrap().parse().unwrap();
        let b = it.next().unwrap().to_string();
        ref_map.insert((x, y, z), b);
    }

    let idx = |x: i32, y: i32, z: i32| -> usize {
        let lx = ((x % 16) + 16) % 16;
        let lz = ((z % 16) + 16) % 16;
        (((y + 64) as usize) * 256) + (lz as usize) * 16 + (lx as usize)
    };

    let mut diffs: Vec<((i32, i32, i32), String, String)> = Vec::new();
    let mut transitions: HashMap<(String, String), usize> = HashMap::new();
    let mut per_y: HashMap<i32, usize> = HashMap::new();
    for (&(x, y, z), rb) in &ref_map {
        let ours = name_of(out[idx(x, y, z)]);
        let ours_b = base(&ours);
        let ref_b = base(rb);
        if ours_b != ref_b {
            diffs.push(((x, y, z), ref_b.to_string(), ours_b.to_string()));
            *transitions.entry((ref_b.to_string(), ours_b.to_string())).or_insert(0) += 1;
            *per_y.entry(y).or_insert(0) += 1;
        }
    }
    // ours non-air where ref air
    let mut extra_nonair = 0usize;
    for iy in 0..384usize {
        for i in 0..256usize {
            let id = out[iy * 256 + i];
            if id != 0 {
                let y = iy as i32 - 64;
                let x = cx * 16 + (i % 16) as i32;
                let z = cz * 16 + (i / 16) as i32;
                if !ref_map.contains_key(&(x, y, z)) {
                    extra_nonair += 1;
                    if extra_nonair <= 5 {
                        println!("EXTRA nonair at {x} {y} {z}: {}", name_of(id));
                    }
                }
            }
        }
    }

    let total = ref_map.len();
    println!(
        "chunk ({cx},{cz}): oracle blocks={total} divergent={} extra_nonair={extra_nonair}  ({:.3}% of nonair)",
        diffs.len(),
        100.0 * diffs.len() as f64 / total.max(1) as f64
    );
    let mut rows: Vec<_> = transitions.iter().collect();
    rows.sort_by_key(|(_, c)| std::cmp::Reverse(**c));
    for ((a, b), c) in rows.iter().take(12) {
        println!("  {c:6}  {a} -> {b}");
    }
    let mut ys: Vec<_> = per_y.iter().collect();
    ys.sort_by_key(|(y, _)| **y);
    print!("  per-y: ");
    for (y, c) in ys.iter().take(40) {
        print!("{y}:{c} ");
    }
    println!();
    println!("--- first 15 divergences ---");
    for ((x, y, z), r, o) in diffs.iter().take(15) {
        println!("  {x} {y} {z}  ref={r} ours={o}");
    }
    // aquifer cell clustering of divergences
    let mut cells: HashMap<(i32, i32, i32), usize> = HashMap::new();
    for ((x, y, z), _, _) in &diffs {
        let g = (
            (x - 5).div_euclid(16),
            (y + 1).div_euclid(12),
            (z - 5).div_euclid(16),
        );
        *cells.entry(g).or_insert(0) += 1;
    }
    let mut crows: Vec<_> = cells.iter().collect();
    crows.sort_by_key(|(_, c)| std::cmp::Reverse(**c));
    print!("  top cells: ");
    for ((gx, gy, gz), c) in crows.iter().take(10) {
        print!("({gx},{gy},{gz}):{c} ");
    }
    println!();
    let _ = HashSet::<String>::new();
}
