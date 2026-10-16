use crussty_chunkgen::density::Env;
use crussty_chunkgen::nochunk::{ChunkGen, OUT_LEN};
fn main() {
    let seed: i64 = 3257840388504953787;
    let env: &'static Env = Box::leak(Box::new(Env::new(seed)));
    let mut gen = ChunkGen::new(env);
    let mut out = [0u16; OUT_LEN];
    for i in 0..8 {
        gen.fill(i, 1, &mut out);
        println!("fill {} plane_hits={}", i, gen.prof.plane_hits);
    }
}
