fn main() {
    let env: &'static crussty_chunkgen::density::Env = Box::leak(Box::new(crussty_chunkgen::density::Env::new(3257840388504953787)));
    let mut gen = crussty_chunkgen::nochunk::ChunkGen::new(env);
    let mut out = [0u16; crussty_chunkgen::nochunk::OUT_LEN];
    for i in 0..8 { gen.fill(i - 2, 1, &mut out); }
    let hits = crussty_chunkgen::nochunk::SLOT0_CONST_HITS.load(std::sync::atomic::Ordering::Relaxed);
    println!("slot0 const-fill corners per chunk: {}", hits / 8);
}
