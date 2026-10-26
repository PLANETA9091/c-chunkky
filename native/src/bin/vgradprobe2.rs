// vgradprobe2 — intermediate state dump, mirrors VgradProbe2.java.
use crussty_chunkgen::rng::{mth_get_seed, XoroshiroRandomSource};
fn main() {
    let seed: i64 = 3257840388504953787;
    let mut rs = XoroshiroRandomSource::new(seed);
    let root = rs.fork_positional();
    println!("rootFactory lo/hi = {:?}", root.seeds());
    let mut src = root.from_hash_of("minecraft:bedrock_floor");
    let fac = src.fork_positional();
    println!("bedrockFactory lo/hi = {:?}", fac.seeds());
    println!("mthGetSeed(16384,-63,16499) = {}", mth_get_seed(16384, -63, 16499));
    let mut r = fac.at(16384, -63, 16499);
    println!("nextFloat={}", r.next_f32());
}
