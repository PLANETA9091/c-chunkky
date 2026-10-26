// surface263walk — local reproduction: ctx_new -> bench biome grid -> fill
// (surface tail runs inside) -> report the traced position's decisions.
use crussty_chunkgen::ffi;

fn main() {
    let seed: i64 = 3257840388504953787;
    let sea: i32 = 63;
    let id = ffi::ctx_new(seed, -64, 384, sea);
    assert!(id != 0, "ctx");
    if ffi::surface_states_of(id).is_none() {
        eprintln!("native surface NOT built");
        std::process::exit(1);
    }
    // identity material ids: state i -> 22+i
    let states = ffi::surface_states_of(id).unwrap();
    let ids: Vec<u16> = (0..states.len() as u16).map(|i| 22 + i).collect();
    ffi::surface_assign_ids(id, &ids).expect("ids");
    // biome names -> temps (legacy random-source noises; values irrelevant
    // except the Temperature cond — fill with the asset base temperature)
    let names = {
        // ctx_biome_names_of covers only the legacy slot; re-derive via the
        // probe: the v263 names live in v263.biomes — expose through JNI? use
        // the ChunkGenNative path: ctxBiomeNames handles v263; the rust helper
        // below is legacy-only, so read the names from the surface conds'
        // biome table instead: approximate with the params263 list.
        let src = crussty_chunkgen::biomes::BiomeSourceData::from_params263(include_str!("../params263_overworld.json")).unwrap();
        src.biome_names
    };
    let (temps, frozen): (Vec<f32>, Vec<bool>) = names.iter().map(|_| (0.5f32, false)).unzip();
    ffi::surface_assign_temps(id, &temps, &frozen).expect("temps");
    // chunk (1034, 1033)
    let (cx, cz) = (1034, 1033);
    // BIOMES first (stores the grid globally)
    let sections = 384 / 16;
    let mut grid = vec![0i32; sections * 64];
    let n = {
        // bench helper: use the same path the JNI takes
        ffi::bench_fill_biomes263(id, cx, cz, &mut grid)
    };
    assert!(n > 0, "biome fill");
    let mut out = [0u16; crussty_chunkgen::nochunk::OUT_LEN];
    let mut marks: Vec<i64> = Vec::new();
    ffi::fill(id, cx, cz, &mut out);
    // find (0,-8,12) local: world (16544,-8,16540) => lx=0, lz=12
    let idx = (( -8 - (-64)) * 256 + 12 * 16 + 0) as usize;
    println!("block at (0,-8,12) = {}", out[idx]);
}
