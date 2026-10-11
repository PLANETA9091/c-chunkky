fn main() {
    let seed: i64 = 3257840388504953787;
    let env: &'static crussty_chunkgen::density::Env = Box::leak(Box::new(crussty_chunkgen::density::Env::new(seed)));
    let mut gen = crussty_chunkgen::nochunk::ChunkGen::new(env);
    let mut out = [0u16; crussty_chunkgen::nochunk::OUT_LEN];
    // warmup
    for cz in 0..2 { for cx in 0..2 { gen.fill(cx, cz, &mut out); } }
    // timed
    let t0 = std::time::Instant::now();
    let n = 24;
    for i in 0..n {
        gen.fill((i % 4) as i32 - 2, (i / 4) as i32 - 2, &mut out);
    }
    let dt = t0.elapsed().as_nanos() as f64 / n as f64 / 1e6;
    println!("mean fill: {:.2} ms", dt);
}
