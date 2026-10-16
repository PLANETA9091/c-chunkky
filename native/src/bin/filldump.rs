//! Dump native fill blocks as "x y z blockname" (oracle dump format).
//!
//! Usage: filldump <extracted_root>... -- cx cz [seed] [outfile]

use crussty_chunkgen::density::Env;
use crussty_chunkgen::nochunk::{ChunkGen, OUT_LEN};
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

fn base(name: &str) -> &str {
    name.split('[').next().unwrap_or(name)
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
    let outfile = pos.get(3).cloned().unwrap_or_else(|| "filldump.txt".into());

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
    let mut gen = ChunkGen::new(env);
    let mut out = [0u16; OUT_LEN];
    gen.fill(cx, cz, &mut out);

    // u16 block ids -> names via the noise-stage REG table
    let mut s = String::with_capacity(1 << 22);
    let h = 384i32;
    let min_y = -64i32;
    for y in 0..h {
        for z in 0..16usize {
            for x in 0..16usize {
                // OUT index layout: (y-minY)*256 + z*16 + x
                let idx = (y as usize) * 256 + z * 16 + x;
                let b = out[idx];
                if b == 0 {
                    continue; // air
                }
                let wy = min_y + y as i32;
                let wx = cx * 16 + x as i32;
                let wz = cz * 16 + z as i32;
                let name = REG
                    .iter()
                    .find(|(id, _)| *id == b)
                    .map(|(_, n)| *n)
                    .unwrap_or("unknown");
                s.push_str(&format!("{wx} {wy} {wz} {name}\n"));
            }
        }
    }
    std::fs::write(&outfile, s).expect("write");
    eprintln!("wrote {outfile}");
}
