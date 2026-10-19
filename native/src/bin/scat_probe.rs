//! Bit-exact draw-by-draw comparison of the scattered_ore pipeline against the
//! Java oracle (ScatRef4.java). Reproduces the FULL placed-feature draw stream
//! for chunk (99,98), gid 0, step 6, seed 3257840388504953787, placement
//! [count=4, in_square, height_range uniform abs -16..24, biome] and prints
//! every draw value plus the spos rounds, matching the Java output format.

use crussty_chunkgen::features_rng::WorldgenRandom;

fn main() {
    let seed: i64 = std::env::var("SCAT_SEED")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3257840388504953787);
    let bx: i32 = std::env::var("SCAT_BX").ok().and_then(|v| v.parse().ok()).unwrap_or(1584);
    let bz: i32 = std::env::var("SCAT_BZ").ok().and_then(|v| v.parse().ok()).unwrap_or(1568);
    let step: i32 = std::env::var("SCAT_STEP").ok().and_then(|v| v.parse().ok()).unwrap_or(6);
    let y_lo: i32 = std::env::var("SCAT_YLO").ok().and_then(|v| v.parse().ok()).unwrap_or(-16);

    for gid in 0..6 {
        let mut rng = WorldgenRandom::new();
        let deco = rng.set_decoration_seed(seed, bx, bz);
        rng.set_feature_seed(deco, gid, step);
        let mut out = String::new();
        for p in 0..4 {
            let px = bx + rng.next_int_bound(16);
            let pz = bz + rng.next_int_bound(16);
            let py = rng.next_int_bound(41) + y_lo;
            let n = rng.next_int_bound(8);
            out.push_str(&format!(" c{p}=({px},{py},{pz},{n})["));
            let mut world: std::collections::HashMap<(i32, i32, i32), &str> = Default::default();
            for i in 0..n {
                let mag = i.min(7);
                let names = ["x", "y", "z"];
                let mut ax = [0i32; 3];
                for (a_idx, a) in ax.iter_mut().enumerate() {
                    let f1 = rng.next_f32();
                    let f2 = rng.next_f32();
                    let v = (f1 - f2) * mag as f32;
                    *a = round_java(v);
                    if std::env::var("SCAT_DRAWS").is_ok() {
                        println!(
                            "D gid={gid} c={p} spos={i} axis={} f1={:08x} f2={:08x} v={v:.9} -> {}",
                            names[a_idx],
                            f1.to_bits(),
                            f2.to_bits(),
                            *a
                        );
                    }
                }
                let sx = px + ax[0]; let sy = py + ax[1]; let sz = pz + ax[2];
                out.push_str(&format!("{sx}/{sy}/{sz} "));
                // rule_test model: target0 random_block(stone, 0.7) -> copper_ore;
                // target1 tag deepslate_ore_replaceables (deepslate, y<0) -> deepslate_copper_ore
                let pos = (sx, sy, sz);
                let st = world.get(&pos).copied().unwrap_or(if sy < 0 { "deepslate" } else { "stone" });
                let mut placed = None;
                if st == "stone" {
                    let rf = rng.next_f32();
                    if std::env::var("SCAT_DRAWS").is_ok() {
                        println!("R gid={gid} c={p} spos={i} rule=stone f={:08x} hit={}", rf.to_bits(), rf < 0.7);
                    }
                    if rf < 0.7 { placed = Some("copper_ore"); }
                } else if st == "deepslate" {
                    placed = Some("deepslate_copper_ore");
                }
                if let Some(w) = placed {
                    world.insert(pos, w);
                }
            }
            out.push(']');
        }
        println!("G {gid} |{out}");
    }
}

/// Java Math.round(float): (int)Math.floor(v + 0.5f) — ties toward +infinity.
fn round_java(v: f32) -> i32 {
    (v + 0.5).floor() as i32
}
