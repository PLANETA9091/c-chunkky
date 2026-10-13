fn main() {
    let seed: i64 = 3257840388504953787;
    let env = crussty_chunkgen::density::Env::new(seed);
    let gen = crussty_chunkgen::nochunk::ChunkGen::new(&env);
    println!("arena.len = {}", gen.compiled.arena.len());
}
