// crussty_chunkgen — vanilla-exact 1.21.8 noise-stage world generator.
// Modules:
//   assets_manifest (build.rs) — embedded vanilla worldgen JSONs
//   json        — compact JSON parser
//   rng/md5/noise — vanilla RNG + noise primitives (pre-existing, parity-checked)
//   worldgen    — asset registry + density function graph construction
//   density     — density function interpreter with vanilla cache semantics
//   nochunk     — NoiseChunk-equivalent chunk fill (cells, aquifer, deepslate)

pub mod json;
pub mod rng;
pub mod md5;
pub mod noise;
pub mod worldgen;
pub mod density;
pub mod biomes;
pub mod surface;
pub mod surface_fill;
pub mod beard;
pub mod nochunk;
pub mod features_rng;
pub mod features_math;
pub mod features_placement;
pub mod features_tree;
pub mod features_vanilla_data;
pub mod features_deco;
pub mod features_json;
pub mod features_place;
pub mod features_place_ffi;
pub mod features_master;
pub mod features_jni;
pub mod features_snap;
pub mod features_survive;
pub mod ffi;

include!(concat!(env!("OUT_DIR"), "/assets_manifest.rs"));

/// Look up an embedded asset by path relative to assets/, e.g.
/// "minecraft/worldgen/noise_settings/overworld.json".
pub fn asset(path: &str) -> Option<&'static [u8]> {
    ASSET_PATHS
        .binary_search(&path)
        .ok()
        .map(|i| ASSET_BYTES[i])
}

pub fn abi_version() -> u32 {
    2
}
