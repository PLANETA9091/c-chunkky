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
pub mod nochunk;
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
