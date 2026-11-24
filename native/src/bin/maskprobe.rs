// cont-151 diagnostic: mask-volume probe for the carvers A/B divergence.
// Prints the number of CarvingMask263 set bits for chunks in a region —
// a pure function of (seed, cx, cz) per the port; compare with the per-chunk
// dAIR matrix from the ab263 carvers pair to localize geometry vs apply.
use crussty_chunkgen::carver263::generate_carver_mask;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let seed: i64 = args.get(1).map(|s| s.parse().unwrap()).unwrap_or(3257840388504953787);
    let cx0: i32 = args.get(2).map(|s| s.parse().unwrap()).unwrap_or(1024);
    let cz0: i32 = args.get(3).map(|s| s.parse().unwrap()).unwrap_or(1024);
    let n: i32 = args.get(4).map(|s| s.parse().unwrap()).unwrap_or(12);
    println!("seed={} origin=({}, {}) size={}", seed, cx0, cz0, n);
    for cz in cz0..cz0 + n {
        for cx in cx0..cx0 + n {
            let mask = generate_carver_mask(seed, cx, cz, -64, 384);
            let mut bits = 0usize;
            mask.visit(|_x, _z, _by, _ty| {
                bits += (_ty - _by + 1) as usize;
                true
            });
            print!("{} ", bits);
        }
        println!();
    }
}
