use crussty_chunkgen::density::Env;
fn main() {
    let env = Env::new(3257840388504953787);
    let compiled = env.compile();
    println!("compiled.interp_slots = {}", compiled.interp_slots);
    println!("col_count = {}", compiled.col_count);
    let gen = crussty_chunkgen::nochunk::ChunkGen::new(&env);
    println!("interp_children.len() = {}", gen.interp_children_len());
    println!("slice0 len = {}", gen.slice0_len());
    println!("plane_cache len = {}", gen.plane_cache_len());
}
