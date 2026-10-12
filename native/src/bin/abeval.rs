use crussty_chunkgen::density::*;
use crussty_chunkgen::nochunk::ChunkGen;
use std::time::Instant;

fn main() {
    let seed: i64 = 3257840388504953787;
    let env: &'static Env = Box::leak(Box::new(Env::new(seed)));
    let gen = ChunkGen::new(env);
    let compiled = &gen.compiled;
    let child0 = compiled.interp_children[0];
    let interp = InterpState::new(compiled.interp_slots, 4, 48);
    // real corner grid of one x-plane: 5 z * 49 y
    let mut pts: Vec<Pt> = Vec::new();
    for i in 0..5i32 {
        for y in 0..49i32 {
            pts.push(Pt { x: 48, y: (y - 8) * 8, z: -32 + i * 4 });
        }
    }
    let total = pts.len();
    let mut sink = 0u64;
    // warm
    for _ in 0..50 {
        let mut memo4 = V4Memo::new(compiled.arena.len());
        for c in pts.chunks(4) {
            let p4 = [c[0], c[c.len().min(4) - 1], c[0], c[0]];
            let p4 = [c[0], c[(1).min(c.len()-1)], c[(2).min(c.len()-1)], c[(3).min(c.len()-1)]];
            let v = env.eval4(compiled, child0, &p4, Mode::Slice, &interp, &memo4);
            sink ^= v[0].to_bits();
        }
        let mut memo8 = V8Memo::new(compiled.arena.len());
        for c in pts.chunks(8) {
            let mut p8 = [c[0]; 8];
            for (k, pt) in c.iter().enumerate() { p8[k] = *pt; }
            let v = env.eval8(compiled, child0, &p8, Mode::Slice, &interp, &memo8);
            sink ^= v[0].to_bits();
        }
    }
    let n = 60;
    let t0 = Instant::now();
    for _ in 0..n {
        let mut memo4 = V4Memo::new(compiled.arena.len());
        for c in pts.chunks(4) {
            let p4 = [c[0], c[(1).min(c.len()-1)], c[(2).min(c.len()-1)], c[(3).min(c.len()-1)]];
            let v = env.eval4(compiled, child0, &p4, Mode::Slice, &interp, &memo4);
            sink ^= v[0].to_bits() ^ v[1].to_bits() ^ v[2].to_bits() ^ v[3].to_bits();
        }
    }
    let d4 = t0.elapsed();
    let t1 = Instant::now();
    for _ in 0..n {
        let mut memo8 = V8Memo::new(compiled.arena.len());
        for c in pts.chunks(8) {
            let mut p8 = [c[0]; 8];
            for (k, pt) in c.iter().enumerate() { p8[k] = *pt; }
            let v = env.eval8(compiled, child0, &p8, Mode::Slice, &interp, &memo8);
            for x in v { sink ^= x.to_bits(); }
        }
    }
    let d8 = t1.elapsed();
    println!("total corners per plane: {total}");
    println!("eval4 x2 walks: {:?}  => {:.2} us/corner", d4 / n, d4.as_nanos() as f64 / n as f64 / total as f64 / 1000.0);
    println!("eval8 x1 walks: {:?}  => {:.2} us/corner", d8 / n, d8.as_nanos() as f64 / n as f64 / total as f64 / 1000.0);
    println!("sink={sink}");
}
