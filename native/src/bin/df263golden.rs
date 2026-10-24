//! df263 golden tests: bit-exact vs Paper 26.3 router oracle
//! (oracle263/RouterOracle263.java -> reference/oracle263_df_strided.txt).
//! Full volume dump: reference/oracle263_df.txt (gitignored, 71MB).

use std::collections::HashMap;

use crussty_chunkgen::df263::{DensityVolume, SamRef, SamplerCtx263};
use crussty_chunkgen::df263_router::{OverworldRouter263, RouterBuilder263};

const SEED: i64 = 3257840388504953787;

struct Golden {
    points: Vec<(String, i32, i32, i32, u32)>,
    vols: Vec<((String, i32, i32), Vec<(usize, u32)>)>,
}

fn load_goldens(path: &str) -> Golden {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    let mut points = Vec::new();
    let mut vols: HashMap<(String, i32, i32), Vec<(usize, u32)>> = HashMap::new();
    for line in text.lines() {
        let mut it = line.split_whitespace();
        match it.next() {
            Some("pt") => {
                let name = it.next().unwrap().to_string();
                if name.starts_with("probe_") {
                    continue; // internals probes handled separately
                }
                let x: i32 = it.next().unwrap().parse().unwrap();
                let y: i32 = it.next().unwrap().parse().unwrap();
                let z: i32 = it.next().unwrap().parse().unwrap();
                let bits = u32::from_str_radix(it.next().unwrap(), 16).unwrap();
                points.push((name, x, y, z, bits));
            }
            Some("vol") => {
                let name = it.next().unwrap().to_string();
                let cx: i32 = it.next().unwrap().parse().unwrap();
                let cz: i32 = it.next().unwrap().parse().unwrap();
                let index: usize = it.next().unwrap().parse().unwrap();
                let bits = u32::from_str_radix(it.next().unwrap(), 16).unwrap();
                vols.entry((name, cx, cz)).or_default().push((index, bits));
            }
            _ => panic!("bad golden line: {line}"),
        }
    }
    Golden {
        points,
        vols: vols.into_iter().collect(),
    }
}

fn sampler<'a>(router: &'a OverworldRouter263, name: &str) -> &'a SamRef {
    match name {
        "temperature" => &router.temperature,
        "vegetation" => &router.vegetation,
        "continents" => &router.continents,
        "erosion" => &router.erosion,
        "depth" => &router.depth,
        "ridges" => &router.ridges,
        "offset" => &router.offset,
        "factor" => &router.factor,
        "jaggedness" => &router.jaggedness,
        "base_3d" => &router.base_3d,
        "sloped_cheese" => &router.sloped_cheese,
        "noodle" => &router.noodle,
        "entrances" => &router.entrances,
        "spaghetti_2d" => &router.spaghetti_2d,
        "pillars" => &router.pillars,
        "spaghetti_roughness" => &router.spaghetti_roughness,
        "chunk_surface_level" => &router.chunk_surface_level,
        "final_density" => &router.final_density,
        other => panic!("unknown fn {other}"),
    }
}

fn main() {
    df263_router_bit_exact_vs_oracle263();
}

fn df263_router_bit_exact_vs_oracle263() {
    let full = concat!(env!("CARGO_MANIFEST_DIR"), "/../reference/oracle263_df.txt");
    let path = if std::path::Path::new(full).exists() { full } else { concat!(env!("CARGO_MANIFEST_DIR"), "/../reference/oracle263_df_strided.txt") };
    println!("using goldens: {path}");
    let golden = load_goldens(path);
    let mut builder = RouterBuilder263::new(SEED);
    let router = builder.build_overworld();

    // point path (EMPTY_UNCACHED semantics)
    let mut point_fail = 0;
    for (name, x, y, z, bits) in &golden.points {
        let s = sampler(&router, name);
        let mut ctx = SamplerCtx263::uncached();
        let v = s.sample_value(&mut ctx, *x, *y, *z);
        if v.to_bits() != *bits {
            if point_fail < 8 {
                println!("PT MISMATCH {name} ({x},{y},{z}): got {:08x} want {:08x}", v.to_bits(), bits);
            }
            point_fail += 1;
        }
    }
    if point_fail > 0 {
        let mut by_fn: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        // re-run to count per function
        for (name, x, y, z, bits) in &golden.points {
            let s = sampler(&router, name);
            let mut ctx = SamplerCtx263::uncached();
            let v = s.sample_value(&mut ctx, *x, *y, *z);
            if v.to_bits() != *bits {
                *by_fn.entry(name.clone()).or_default() += 1;
            }
        }
        println!("point diverge by fn: {by_fn:?}");
    }
    assert_eq!(point_fail, 0, "{point_fail}/{} point samples diverged", golden.points.len());

    // volume path (cached ctx, block step 1)
    let mut vol_fail = 0;
    let mut zero_fail = 0usize;
    let mut vol_total = 0;
    for ((name, cx, cz), samples) in &golden.vols {
        let s = sampler(&router, name);
        let vol = DensityVolume::block_step((16, 384, 16), (cx * 16, -64, cz * 16));
        let mut ctx = SamplerCtx263::cached();
        let mut buf = vec![0.0f32; vol.size()];
        s.sample_volume(&mut ctx, &mut buf, &vol);
        for (index, bits) in samples {
            vol_total += 1;
            let got = buf[*index].to_bits();
            let is_signed_zero = got & 0x7FFF_FFFF == 0 && *bits & 0x7FFF_FFFF == 0;
            if got != *bits {
                if is_signed_zero {
                    zero_fail += 1;
                    continue;
                }
                if vol_fail < 8 {
                    let i = *index as i32;
                    let y = i % vol.size_y;
                    let x = (i / vol.size_y) % vol.size_x;
                    let z = i / (vol.size_y * vol.size_x);
                    println!(
                        "VOL MISMATCH {name} c({cx},{cz}) idx {index} blk({x},{y},{z})-abs({},{},{}): got {:08x} want {:08x}",
                        cx * 16 + x,
                        -64 + y,
                        cz * 16 + z,
                        buf[*index].to_bits(),
                        bits
                    );
                }
                vol_fail += 1;
            }
        }
    }
    assert_eq!(vol_fail, 0, "{vol_fail}/{vol_total} volume samples diverged");
    println!(
        "df263 goldens OK: {} points, {} volume samples ({} signed-zero-only, tolerated: -0.0>0 == 0.0>0)",
        golden.points.len(),
        vol_total,
        zero_fail
    );
}
