// Dev tool: load worldgen from a datapack-extraction directory (later packs
// override earlier ones) and print registry stats. Usage:
//   dpload <extracted_root>...   (each root contains data/<ns>/worldgen/...)
//   dpload --gold                (embedded vanilla sanity)

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
                    // repo-relative path: data/<ns>/worldgen/... -> <ns>/worldgen/...
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
    let mut assets = Assets::new();
    if args.len() > 1 && args[1] != "--gold" {
        for root in &args[1..] {
            let root = Path::new(root);
            let mut files = Vec::new();
            collect(root, root, &mut files);
            files.sort();
            let mut n = 0;
            for (path, bytes) in files {
                let keep = path.contains("/worldgen/noise_settings/")
                    || path.contains("/worldgen/density_function/")
                    || path.contains("/worldgen/noise/")
                    || path.ends_with("/dimension/overworld.json");
                if keep {
                    assets.insert(path, bytes);
                    n += 1;
                }
            }
            eprintln!("pack root {}: {} worldgen jsons", root.display(), n);
        }
    }
    let t0 = std::time::Instant::now();
    let wg = if args.len() > 1 && args[1] != "--gold" {
        crussty_chunkgen::worldgen::WorldGen::load_with(&assets)
    } else {
        crussty_chunkgen::worldgen::WorldGen::load_default("dpload")
    };
    let dt = t0.elapsed();
    println!("noise_settings resolved : {}", if args.len() > 1 && args[1] != "--gold" { assets.resolve_noise_settings() } else { "minecraft:overworld (embedded)".into() });
    println!("sea_level              : {}", wg.sea_level);
    println!("min_y/height           : {}/{}", wg.min_y, wg.height);
    println!("aquifers/ore_veins     : {}/{}", wg.aquifers_enabled, wg.ore_veins_enabled);
    println!("legacy_random_source   : {}", wg.legacy_random_source);
    println!("noise params           : {}", wg.noise_params.len());
    println!("df roots (files)       : {}", wg.root_indices.len());
    println!("arena nodes            : {}", wg.df_arena.len());
    let total: usize = wg.type_counts.values().sum();
    println!("constructed nodes      : {total} in {dt:?}");
    let mut rows: Vec<_> = wg.type_counts.iter().collect();
    rows.sort_by_key(|(_, c)| std::cmp::Reverse(**c));
    for (t, c) in rows {
        println!("  {c:6}  {t}");
    }
    // compile + a probe fill to prove the interpreter wires up end to end
    let t1 = std::time::Instant::now();
    let env = crussty_chunkgen::density::Env::new_assets(3257840388504953787, &assets);
    let compiled = env.compile();
    println!("compile                : ok in {:?} (arena {})", t1.elapsed(), compiled.arena.len());
    let t2 = std::time::Instant::now();
    let mut gen = crussty_chunkgen::nochunk::ChunkGen::new(&env);
    let mut out = [0u16; crussty_chunkgen::nochunk::OUT_LEN];
    gen.fill(0, 0, &mut out);
    println!("fill(0,0) first        : {:?}", t2.elapsed());
    for i in 1..4 {
        let t = std::time::Instant::now();
        gen.fill(i, 0, &mut out);
        println!("fill({i},0) steady        : {:?}", t.elapsed());
    }
    let n_stone = out.iter().filter(|&&b| b == 1).count();
    let n_water = out.iter().filter(|&&b| b == 3).count();
    let n_air = out.iter().filter(|&&b| b == 0).count();
    println!("fill(3,0) content      : air={n_air} stone={n_stone} water={n_water}");
}
