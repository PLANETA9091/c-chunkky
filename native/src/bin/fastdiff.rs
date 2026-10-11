// Fill one chunk with and without the aquifer fast path; print first diffs.
fn main() {
    let seed: i64 = 3257840388504953787;
    let cx: i32 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    let cz: i32 = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(0);

    std::env::set_var("CRUS_NO_FASTPATH", "1");
    let env: &'static crussty_chunkgen::density::Env =
        Box::leak(Box::new(crussty_chunkgen::density::Env::new(seed)));
    let mut gen = crussty_chunkgen::nochunk::ChunkGen::new(env);
    let mut a = [0u16; crussty_chunkgen::nochunk::OUT_LEN];
    gen.fill(cx, cz, &mut a);

    std::env::remove_var("CRUS_NO_FASTPATH");
    let env2: &'static crussty_chunkgen::density::Env =
        Box::leak(Box::new(crussty_chunkgen::density::Env::new(seed)));
    let mut gen2 = crussty_chunkgen::nochunk::ChunkGen::new(env2);
    let mut b = [0u16; crussty_chunkgen::nochunk::OUT_LEN];
    gen2.fill(cx, cz, &mut b);

    let mut diffs = 0;
    for i in 0..crussty_chunkgen::nochunk::OUT_LEN {
        if a[i] != b[i] {
            diffs += 1;
            if diffs <= 8 {
                let y = (i / 256) as i32 - 64;
                let rem = i % 256;
                let z = (rem / 16) as i32;
                let x = (rem % 16) as i32;
                println!(
                    "diff idx={i} world=({},{},{}) ref_scalar={} fastpath={}",
                    cx * 16 + x, y, cz * 16 + z, a[i], b[i]
                );
            }
        }
    }
    println!("total diffs: {diffs}");
}
