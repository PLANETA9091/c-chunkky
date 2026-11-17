// carvechk: diagnostics for CarveState::from_assets over the dptt pack set.
// Usage: carvechk <extracted_packs_root> [embedded-only]
// The root should contain "data/<ns>/..." json trees (pack roots with their
// ACTIVE overlay dirs merged as extra roots by the caller, like the plugin).
use std::path::Path;
use crussty_chunkgen::worldgen::Assets;

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

fn probe(assets: &Assets, tag: &str) {
    let names = crussty_chunkgen::biomes::BiomeSourceData::from_assets(assets)
        .map(|s| s.biome_names.clone());
    eprintln!("[{tag}] biome source names: {}", names.as_ref().map(|v| v.len()).unwrap_or(0));
    match crussty_chunkgen::carve::CarveState::from_assets(assets, names.as_deref()) {
        Some(_) => eprintln!("[{tag}] CarveState: OK"),
        None => eprintln!("[{tag}] CarveState: NONE (engine disabled)"),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    // 1. embedded-only
    probe(&Assets::new(), "embedded");

    // 2. with pack dirs (args[1..] = extracted pack roots, root data/ merged)
    let mut assets = Assets::new();
    for r in args.iter().skip(1) {
        let mut files: Vec<(String, Vec<u8>)> = Vec::new();
        collect(Path::new(r), Path::new(r), &mut files);
        files.sort();
        let mut nc = 0; let mut nb = 0; let mut nt = 0;
        for (p, b) in files {
            if p.contains("/worldgen/configured_carver/") { nc += 1; }
            if p.contains("/worldgen/biome/") { nb += 1; }
            if p.contains("/tags/block/") { nt += 1; }
            assets.insert(p, b);
        }
        eprintln!("[pack {r}] jsons collected (carver={nc} biome={nb} tags={nt})");
    }
    if args.len() > 1 {
        probe(&assets, "packs");
    }
}
