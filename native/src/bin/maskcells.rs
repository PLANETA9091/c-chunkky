// cont-151 diagnostic: mask-cell probe — prints the set CarvingMask263 bits
// of one chunk within a Y range as "x y z" lines (or the total count).
// Compare against the vanilla-carved water cells (chunkdump A/B) to separate
// mask-geometry divergence from aquifer-substance divergence.
use crussty_chunkgen::carver263::generate_carver_mask;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let seed: i64 = args[1].parse().unwrap();
    let cx: i32 = args[2].parse().unwrap();
    let cz: i32 = args[3].parse().unwrap();
    let y_lo: i32 = args[4].parse().unwrap();
    let y_hi: i32 = args[5].parse().unwrap();
    let mask = generate_carver_mask(seed, cx, cz, -64, 384);
    mask.visit(|x, z, by, ty| {
        let lo = by.max(y_lo);
        let hi = ty.min(y_hi);
        for y in lo..=hi {
            println!("{} {} {}", x, y, z);
        }
        true
    });
}
