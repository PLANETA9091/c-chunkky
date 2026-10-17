//! Native fill (noise+surface) -> .crsf dump for direct comparison with
//! server/plugin/vanilla reference dumps.
//!
//! Usage: crsfout <dp_root...> -- <cx> <cz> [seed] [out.crsf]

use crussty_chunkgen::ffi::{fill, surface_assign_ids, surface_assign_temps, surface_states_of};
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

const MAT_NAMES: [&str; 22] = [
    "minecraft:air",
    "minecraft:stone",
    "minecraft:bedrock",
    "minecraft:water[level=0]",
    "minecraft:grass_block[snowy=false]",
    "minecraft:dirt",
    "minecraft:deepslate[axis=y]",
    "minecraft:sand",
    "minecraft:gravel",
    "minecraft:granite",
    "minecraft:diorite",
    "minecraft:andesite",
    "minecraft:tuff",
    "minecraft:sandstone",
    "minecraft:clay",
    "minecraft:powder_snow",
    "minecraft:ice",
    "minecraft:lava[level=0]",
    "minecraft:copper_ore",
    "minecraft:raw_copper_block",
    "minecraft:deepslate_iron_ore",
    "minecraft:raw_iron_block",
];

fn biome_temp(assets: &Assets, name: &str) -> (f32, bool) {
    let (ns, file) = match name.split_once(':') {
        Some((ns, f)) => (ns, f),
        None => ("minecraft", name),
    };
    let key = format!("data/{ns}/worldgen/biome/{file}.json");
    let bytes = assets
        .get(&key)
        .map(|b| b.to_vec())
        .or_else(|| {
            crussty_chunkgen::asset(&format!("minecraft/worldgen/biome/{file}.json"))
                .map(|b| b.to_vec())
        });
    let Some(bytes) = bytes else {
        return (0.8, false);
    };
    if let Ok(j) = crussty_chunkgen::json::parse(std::str::from_utf8(&bytes).unwrap_or("")) {
        let t = j.get("temperature").and_then(|v| v.as_f64()).unwrap_or(0.8) as f32;
        let frozen = j
            .get("temperature_modifier")
            .and_then(|v| v.as_str())
            .map(|s| s.contains("frozen"))
            .unwrap_or(false);
        return (t, frozen);
    }
    (0.8, false)
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
    let cx: i32 = pos.get(0).and_then(|s| s.parse().ok()).unwrap_or(0);
    let cz: i32 = pos.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    let seed: i64 = pos
        .get(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(3257840388504953787);
    let outfile = pos
        .get(3)
        .cloned()
        .unwrap_or_else(|| format!("/tmp/crsfout_{cx}_{cz}.crsf"));

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

    let names_pre: Vec<String> = {
        let src = crussty_chunkgen::biomes::BiomeSourceData::from_assets(&assets);
        match src {
            Some(src) => src.biome_names,
            None => Vec::new(),
        }
    };
    let temps_pre: Vec<(f32, bool)> = names_pre.iter().map(|n| biome_temp(&assets, n)).collect();

    let id = match crussty_chunkgen::ffi::ctx_new_assets(seed, 63, assets) {
        Ok(id) => id,
        Err(e) => {
            eprintln!("ctx_new_assets failed: {e}");
            std::process::exit(2);
        }
    };
    let states = match surface_states_of(id) {
        Some(s) => s,
        None => {
            eprintln!("no native surface engine in ctx");
            std::process::exit(2);
        }
    };
    // assign identity material ids: state i -> 22+i (palette order below)
    let ids: Vec<u16> = (0..states.len() as u16).map(|i| 22 + i).collect();
    surface_assign_ids(id, &ids).expect("assign ids");
    let (temps, frozen): (Vec<f32>, Vec<bool>) =
        temps_pre.iter().map(|(t, f)| (*t, *f)).unzip();
    surface_assign_temps(id, &temps, &frozen).expect("assign temps");

    let mut buf = vec![0u16; crussty_chunkgen::nochunk::OUT_LEN];
    fill(id, cx, cz, buf.as_mut_slice().try_into().unwrap());

    // palette = MAT_NAMES ++ surface states; buffer ids are palette indices
    let mut palette: Vec<String> = MAT_NAMES.iter().map(|s| s.to_string()).collect();
    palette.extend(states.iter().cloned());
    println!("palette={}", palette.len());

    // write .crsf: magic, version, cx, cz, miny, height, pal, idx
    let miny: i32 = -64;
    let height: i32 = 384;
    let mut out: Vec<u8> = Vec::with_capacity(24 + 4 + 64 + 98304 * 2);
    out.extend_from_slice(&0x46534243u32.to_le_bytes());
    out.extend_from_slice(&1i32.to_le_bytes());
    out.extend_from_slice(&cx.to_le_bytes());
    out.extend_from_slice(&cz.to_le_bytes());
    out.extend_from_slice(&miny.to_le_bytes());
    out.extend_from_slice(&height.to_le_bytes());
    out.extend_from_slice(&(palette.len() as u32).to_le_bytes());
    for p in &palette {
        out.extend_from_slice(&(p.len() as u16).to_le_bytes());
        out.extend_from_slice(p.as_bytes());
    }
    for v in &buf {
        out.extend_from_slice(&v.to_le_bytes());
    }
    std::fs::write(&outfile, &out).expect("write crsf");
    println!("wrote {outfile}");

    // state census summary
    let mut counts = std::collections::HashMap::new();
    for &v in &buf {
        *counts.entry(v).or_insert(0usize) += 1;
    }
    let mut v: Vec<(usize, u16)> = counts.into_iter().map(|(k, x)| (x, k)).collect();
    v.sort_unstable_by(|a, b| b.0.cmp(&a.0));
    for (n, idv) in v.iter().take(10) {
        println!("  {:6} x{}", palette.get(*idv as usize).unwrap_or(&"?".into()), n);
    }
    crussty_chunkgen::ffi::ctx_free(id);
}
