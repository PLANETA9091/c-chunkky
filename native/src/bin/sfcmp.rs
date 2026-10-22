//! Native surface-stage parity check: fills a chunk with the native engine
//! (noise + surface takeover) and compares the block map against an oracle
//! SURFACE-stage dump (OracleNoiseStagePlugin "SURFACEBLOCKS" section).
//!
//! Usage: sfcmp <dp_root...> -- <oracle_surface_dump> <cx> <cz> [seed]

use crussty_chunkgen::ffi::{fill, surface_assign_ids, surface_assign_temps, surface_states_of};
use crussty_chunkgen::worldgen::Assets;
use crussty_chunkgen::{nochunk, surface};
use std::collections::HashMap;
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
    let dump_file = pos.get(0).cloned().expect("oracle dump file");
    let cx: i32 = pos.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    let cz: i32 = pos.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
    let seed: i64 = pos
        .get(3)
        .and_then(|s| s.parse().ok())
        .unwrap_or(3257840388504953787);

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
                || path.contains("/worldgen/multi_noise_biome_source_parameter_list/")
                || path.ends_with("/dimension/overworld.json");
            if keep {
                assets.insert(path, bytes);
            }
        }
    }

    // snapshot biome temps BEFORE assets is consumed by ctx_new_assets
    let names_pre: Vec<String> = {
        // native biome names come from the dimension json — the same order
        // ctxBiomeNames uses; derive from the pack biome source by running
        // the same collection on assets (cheap: parse only).
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
    println!("biomes: {:?}", crussty_chunkgen::ffi::ctx_biome_names_of(id).map(|v| v.len()));
    let states = match surface_states_of(id) {
        Some(s) => s,
        None => {
            eprintln!("no native surface engine in ctx (biomes={:?})",
                crussty_chunkgen::ffi::ctx_biome_names_of(id).map(|v| v.len()));
            std::process::exit(3);
        }
    };
    let ids: Vec<u16> = (0..states.len() as u16).map(|i| 1000 + i).collect();
    surface_assign_ids(id, &ids).expect("assign ids");
    // biome temps (precomputed from the pack/vanilla biome JSONs)
    let temps: Vec<f32> = temps_pre.iter().map(|t| t.0).collect();
    let frozen: Vec<bool> = temps_pre.iter().map(|t| t.1).collect();
    surface_assign_temps(id, &temps, &frozen).expect("assign temps");

    let mut buf = vec![0u16; nochunk::OUT_LEN];
    let arr: &mut [u16; nochunk::OUT_LEN] = buf.as_mut_slice().try_into().unwrap();
    fill(id, cx, cz, arr);

    // native block map
    let min_y = -64i32; // dptt overworld
    let height = 384usize;
    let mut native: HashMap<(i32, i32, i32), String> = HashMap::new();
    for y in 0..height as i32 {
        for z in 0..16i32 {
            for x in 0..16i32 {
                let idx = ((y * 256) + z * 16 + x) as usize;
                let v = arr[idx];
                if v == 0 {
                    continue;
                }
                let s = if (v as usize) < MAT_NAMES.len() {
                    MAT_NAMES[v as usize].to_string()
                } else {
                    states[(v - 1000) as usize].clone()
                };
                native.insert((cx * 16 + x, min_y + y, cz * 16 + z), s);
            }
        }
    }

    // oracle map
    let text = std::fs::read_to_string(&dump_file).expect("oracle dump readable");
    let mut oracle: HashMap<(i32, i32, i32), String> = HashMap::new();
    let mut in_section = false;
    for line in text.lines() {
        if line == "SURFACEBLOCKS" {
            in_section = true;
            continue;
        }
        if in_section {
            if !line.contains(' ') || line.starts_with("STATUS") {
                break;
            }
            let parts: Vec<&str> = line.split(' ').collect();
            if parts.len() < 4 {
                continue;
            }
            let x: i32 = parts[0].parse().unwrap_or(0);
            let yy: i32 = parts[1].parse().unwrap_or(0);
            let z: i32 = parts[2].parse().unwrap_or(0);
            oracle.insert((x, yy, z), parts[3..].join(" "));
        }
    }

    let mut wrong = 0;
    let mut missing = 0;
    let mut extra = 0;
    let mut shown = 0;
    let mut pair_hist: std::collections::HashMap<(String, String), usize> = Default::default();
    let mut y_hist: std::collections::BTreeMap<i32, usize> = Default::default();
    for (k, os) in &oracle {
        match native.get(k) {
            None => {
                missing += 1;
                if shown < 15 {
                    println!("MISSING {k:?} oracle={os}");
                    shown += 1;
                }
            }
            Some(ns) if ns != os => {
                wrong += 1;
                *pair_hist.entry((os.clone(), ns.clone())).or_default() += 1;
                *y_hist.entry(k.1).or_default() += 1;
                if shown < 15 {
                    println!("WRONG   {k:?} oracle={os} native={ns}");
                    shown += 1;
                }
            }
            _ => {}
        }
    }
    let mut pairs: Vec<(_, _)> = pair_hist.into_iter().collect();
    pairs.sort_by(|a, b| b.1.cmp(&a.1));
    for ((os, ns), n) in pairs.iter().take(20) {
        println!("PAIR x{n:5}  {os} -> {ns}");
    }
    let ys: Vec<String> = y_hist
        .iter()
        .map(|(y, n)| format!("{y}:{n}"))
        .collect();
    println!("YDIST {}", ys.join(" "));
    shown = 0;
    for (k, ns) in &native {
        if !oracle.contains_key(k) {
            extra += 1;
            if shown < 15 {
                println!("EXTRA   {k:?} native={ns}");
                shown += 1;
            }
        }
    }
    let total = oracle.len();
    println!(
        "RESULT chunk {cx}_{cz}: oracle={total} native={} wrong={wrong} missing={missing} extra={extra}",
        native.len()
    );
    if wrong + missing + extra == 0 {
        println!("PARITY OK (0 divergent of {total})");
    } else {
        std::process::exit(1);
    }
}
