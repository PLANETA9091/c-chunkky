// surface263check — offline smoke: build the 26.3 surface state without a
// JVM (biomes from the embedded preset) and print the parsed config stats.
use crussty_chunkgen::biomes::BiomeSourceData;

fn main() {
    let seed: i64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(3257840388504953787);
    let sea: i32 = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(63);
    let g = crussty_chunkgen::fill263::Gen263::new(seed, -64, 384, sea);
    let src = BiomeSourceData::from_params263(include_str!("../params263_overworld.json"))
        .expect("preset parse");
    let biomes = crussty_chunkgen::biome263::BiomeState263 {
        src,
        climate: g.climate.clone(),
        y_invariant: [true, true, true, true, false, true],
    };
    let st = match crussty_chunkgen::surface263::build_surface263(seed, -64, 384, 8, sea, Some(&biomes)) {
        Some(s) => s,
        None => {
            eprintln!("surface263: build FAILED");
            std::process::exit(1);
        }
    };
    println!("states={} conds={} rules={}", st.cfg.states.len(), st.cfg.conds.len(), st.cfg.rules.len());
    println!("noises={}", st.noises.len());
    println!("surface_handle={} secondary={} clay={}", st.surface_handle, st.secondary_handle, st.clay_handle);
    println!("dfs={} (copper={} iron={} richness={} gap={})", st.dfs.len(), st.ore_copper, st.ore_iron, st.ore_richness, st.ore_gap);
    println!("eroded_badlands_biome={:?}", st.eroded_badlands_biome);
    println!("frozen_ocean_biome={:?} deep={:?}", st.frozen_ocean_biome, st.deep_frozen_ocean_biome);
    println!("ext_noise_handles={:?}", {
        let mut v: Vec<_> = st.ext_noise_handles.iter().collect();
        v.sort();
        v
    });
    println!("snow_state={:?} packed_ice_state={:?}", st.snow_block_state, st.packed_ice_state);
    // state census: print the first 10 + the fluid ones
    for (i, s) in st.cfg.states.iter().take(12).enumerate() {
        println!("  state[{i}] = {s}");
    }
    for (i, s2) in st.cfg.states.iter().enumerate() {
        println!("  FULLSTATE[{i}] = {s2}");
    }
    let fluids: Vec<&String> = st
        .cfg
        .states
        .iter()
        .filter(|s| s.starts_with("minecraft:water") || s.starts_with("minecraft:lava"))
        .collect();
    println!("fluid states: {fluids:?}");
}
// appended: full state list
