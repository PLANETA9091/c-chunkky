// Isolated bench for the 26.3 native biome stage: build the state (preset
// parse + tree build) and time fill_biomes263 per chunk. Run:
//   cargo run --release --bin biome263bench
use crussty_chunkgen::biomes::BiomeSourceData;
use crussty_chunkgen::biome263::{BiomeState263, fill_biomes263};
use crussty_chunkgen::fill263::Gen263;
use std::time::Instant;

fn main() {
    let t0 = Instant::now();
    let src = BiomeSourceData::from_params263(include_str!("../params263_overworld.json"))
        .expect("preset parse");
    println!(
        "parse+tree: {:?} ({} entries, {} names)",
        t0.elapsed(),
        src.entries.len(),
        src.biome_names.len()
    );
    let g = Gen263::new(3257840388504953787, -64, 384, 63);
    let st = BiomeState263 {
        src,
        climate: g.climate.clone(),
        y_invariant: [true, true, true, true, false, true],
    };
    let mut out = vec![0i32; 24 * 64];
    // warm
    for cz in 1000..1004 {
        fill_biomes263(&st, -64, 384, 1000, cz, &mut out);
    }
    let t1 = Instant::now();
    const N: i32 = 16;
    for i in 0..N {
        fill_biomes263(&st, -64, 384, 2000 + i, 2000, &mut out);
    }
    let per = t1.elapsed() / N as u32;
    println!("fill_biomes263 median-ish per chunk: {:?}", per);
    // breakdown: re-run one chunk with per-field timing approximation
    let t2 = Instant::now();
    for i in 0..4 {
        fill_biomes263(&st, -64, 384, 3000 + i, 3000, &mut out);
    }
    println!("4 more chunks: {:?}", t2.elapsed());
    // distinct biomes sanity
    let mut seen = std::collections::HashSet::new();
    for v in &out {
        seen.insert(*v);
    }
    println!("distinct in last chunk: {}", seen.len());
}
