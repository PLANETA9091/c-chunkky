// vgradprobe — mirror of /tmp/vgrad/VgradProbe.java (26.3 factory chain).
use crussty_chunkgen::rng::{XoroshiroRandomSource};
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let seed: i64 = args[1].parse().unwrap();
    let name = args[2].clone();
    let x: i32 = args[3].parse().unwrap();
    let y: i32 = args[4].parse().unwrap();
    let z: i32 = args[5].parse().unwrap();
    let mut rs = XoroshiroRandomSource::new(seed);
    let root = rs.fork_positional();
    let mut src = root.from_hash_of(&name);
    let fac = src.fork_positional();
    let mut r = fac.at(x, y, z);
    println!("nextFloat={}", r.next_f32());
    let mut r2 = fac.at(x, y, z);
    println!("again={}", r2.next_f32());
}
