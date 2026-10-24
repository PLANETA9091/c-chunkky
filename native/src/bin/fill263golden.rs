//! fill263 golden — native Gen263 fill for the oracle chunks, dumped in the
//! same line format as oracle263/FillOracle263.java ("fill cx cz idx id" +
//! "mark cx cz packedPos"). Compared with reference/oracle263_fill.txt.

use crussty_chunkgen::fill263::Gen263;
use crussty_chunkgen::nochunk::OUT_LEN;

fn main() {
    let seed = 3257840388504953787i64;
    let g = Gen263::new(seed, -64, 384, 63);
    let chunks = [(0, 0), (1, -2), (3, 1), (-2, 3)];
    let mut out = [0u16; OUT_LEN];
    let mut marks: Vec<i64> = Vec::new();
    let mut s = String::with_capacity(1 << 26);
    let t0 = std::time::Instant::now();
    for (cx, cz) in chunks {
        g.fill_chunk(cx, cz, &[0, 0], &mut out, &mut marks);
        // out[] uses the JNI layout ((blockY-minY)*256 + z*16 + x); the oracle
        // emits the DENSITY VOLUME index (y_rel + (x + z*16)*384) — translate.
        for z in 0..16i32 {
            for x in 0..16i32 {
                for y in (0..384i32).rev() {
                    let id = out[(y * 256 + z * 16 + x) as usize];
                    if id != 0 {
                        let vol_idx = y + (x + z * 16) * 384;
                        s.push_str(&format!("fill {cx} {cz} {vol_idx} {id}\n"));
                    }
                }
            }
        }
        for m in &marks {
            s.push_str(&format!("mark {cx} {cz} {m}\n"));
        }
        marks.clear();
    }
    let dt = t0.elapsed();
    std::fs::write("/home/z/my-project/c-faster/reference/oracle263_fill_native.txt", &s).unwrap();
    eprintln!("fill263golden done: {} bytes, 4 chunks, {:?}", s.len(), dt);
}
