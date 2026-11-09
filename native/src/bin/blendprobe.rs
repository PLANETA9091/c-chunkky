//! Blend anatomy probe: octave counts of the Tectonic blend instances.
use crussty_chunkgen::density::Env;

fn collect(root: &std::path::Path, out: &mut Vec<(String, Vec<u8>)>) {
    if let Ok(rd) = std::fs::read_dir(root) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                collect(&p, out);
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
    let seed: i64 = std::env::var("CRUS_BENCH_SEED")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3257840388504953787);
    let mut assets = crussty_chunkgen::worldgen::Assets::new();
    for root in &args[1..] {
        let root = std::path::Path::new(root);
        let mut files = Vec::new();
        collect(root, &mut files);
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
    let gen = crussty_chunkgen::nochunk::ChunkGen::new(env);
    let blended = &gen.compiled.blended;
    println!("blend instances: {}", blended.len());
    for (i, b) in blended.iter().enumerate() {
        let octs = |n: &crussty_chunkgen::noise::PerlinNoise| -> usize {
            n.noise_levels.iter().filter(|l| l.is_some()).count()
        };
        println!(
            "  [{i}] min.live={}/{} max.live={}/{} main.live={}/{} xz_mult={:.6} y_mult={:.6} xz_f={:.6} y_f={:.6} smear={:.6}",
            octs(&b.min_noise), b.min_noise.noise_levels.len(),
            octs(&b.max_noise), b.max_noise.noise_levels.len(),
            octs(&b.main_noise), b.main_noise.noise_levels.len(),
            b.xz_multiplier, b.y_multiplier, b.xz_factor, b.y_factor, b.smear_scale_multiplier,
        );
    }
}
