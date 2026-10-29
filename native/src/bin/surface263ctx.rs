// surface263ctx — reproduce the PLUGIN ctx path exactly: ffi::ctx_new
// (v263 engine + surface263) and dump the surface handshake results.
fn main() {
    let seed: i64 = 3257840388504953787;
    let sea: i32 = 63;
    let id = crussty_chunkgen::ffi::ctx_new(seed, -64, 384, sea);
    println!("ctx id = {id}");
    let states = crussty_chunkgen::ffi::surface_states_of(id);
    match states {
        Some(s) => println!("surface states = {}", s.len()),
        None => println!("surface states = NONE (native surface NOT built)"),
    }
    let names = crussty_chunkgen::ffi::ctx_biome_names_of(id);
    println!("biome names = {}", names.map(|n| n.len()).unwrap_or(0));
}
