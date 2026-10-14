//! Dump a chunk's block array to a raw file for diffing (diagnostic).
use crussty_chunkgen::density::Env;
use crussty_chunkgen::nochunk::{ChunkGen, OUT_LEN};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let cx: i32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    let cz: i32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
    let path = args.get(3).cloned().unwrap_or_else(|| "out.bin".to_string());
    let seed: i64 = args
        .get(4)
        .and_then(|s| s.parse().ok())
        .unwrap_or(3257840388504953787);
    let env: &'static Env = Box::leak(Box::new(Env::new(seed)));
    let mut gen = ChunkGen::new(env);
    let mut out = [0u16; OUT_LEN];
    gen.fill(cx, cz, &mut out);
    std::fs::write(&path, safe_u16_le(&out)).expect("write");
    println!("wrote {} ({} bytes) for chunk ({cx},{cz})", path, OUT_LEN * 2);
}

fn safe_u16_le(out: &[u16; OUT_LEN]) -> Vec<u8> {
    let mut v = Vec::with_capacity(OUT_LEN * 2);
    for &w in out.iter() {
        v.extend_from_slice(&w.to_le_bytes());
    }
    v
}
