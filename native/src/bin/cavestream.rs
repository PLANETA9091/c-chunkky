// cont-151: native cave draw-stream probe — mirrors CaveStreamProbe.java
// exactly (same seed math, same draw order) to diff the LCG streams.
use crussty_chunkgen::rng::LegacyRandom;

fn vb2b(rng: &mut LegacyRandom) -> i32 {
    let a = rng.next_int_bound(14 - 0 + 1);
    let b = rng.next_int_bound(a + 1);
    0 + rng.next_int_bound(b + 1)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let ws: i64 = args[1].parse().unwrap();
    let idx: i64 = args[2].parse().unwrap();
    let cx: i32 = args[3].parse().unwrap();
    let cz: i32 = args[4].parse().unwrap();
    let prob: f32 = if idx == 0 { 0.15 } else { 0.07 };
    let min_gen_y: i32 = -64;
    let mut n_starts = 0i32;
    for dx in -8..=8i32 {
        for dz in -8..=8i32 {
            let px = cx + dx;
            let pz = cz + dz;
            let mut rng = LegacyRandom::new(0);
            let s = ws + idx;
            // setLargeFeatureSeed(s, px, pz)
            rng.set_seed(s);
            let x_scale = rng.next_long();
            let z_scale = rng.next_long();
            rng.set_seed((px as i64).wrapping_mul(x_scale) ^ (pz as i64).wrapping_mul(z_scale) ^ s);
            let roll = rng.next_f32();
            if roll > prob {
                continue;
            }
            n_starts += 1;
            let cave_count = vb2b(&mut rng);
            print!("src=({}, {}) roll={:.9} count={}", px, pz, roll, cave_count);
            for cave in 0..cave_count.min(3) {
                let x = px * 16 + rng.next_int_bound(16);
                let y = (min_gen_y + 8) + rng.next_int_bound(180 - (min_gen_y + 8) + 1);
                let z = pz * 16 + rng.next_int_bound(16);
                let hrm = rng.next_f32() * (1.4f32 - 0.7f32) + 0.7f32;
                let vrm = rng.next_f32() * (1.3f32 - 0.8f32) + 0.8f32;
                let fl = rng.next_f32() * (-0.4f32 - (-1.0f32)) + (-1.0f32);
                print!(
                    " | c{}(x={} y={} z={} hrm={:.9} vrm={:.9} fl={:.9}",
                    cave, x, y, z, hrm, vrm, fl
                );
                let room = rng.next_int_bound(4) == 0;
                if room {
                    let room_ys = rng.next_f32() * (0.9f32 - 0.1f32) + 0.1f32;
                    let room_t = 1.0f32 + rng.next_f32() * 6.0f32;
                    let tunnels = 1 + rng.next_int_bound(4);
                    print!(" ROOM ys={:.9} t={:.9} tunnels={}", room_ys, room_t, tunnels);
                } else {
                    print!(" NOROOM");
                }
                print!(")");
            }
            println!();
        }
    }
    println!("starts={}", n_starts);
}
