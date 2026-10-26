//! cont-54 fiddle probe: native fiddle picks vs grid content at the exact
//! biome-branch divergent blocks (interior cells, chunk 1021/1034).

use crussty_chunkgen::biome263::BiomeState263;
use crussty_chunkgen::fill263::Gen263;
use crussty_chunkgen::sha256::obfuscate_seed;
use crussty_chunkgen::surface263::{fiddle_biome_pick, Neighborhood263};

fn main() {
    let seed = 3257840388504953787i64;
    let (min_y, height) = (-64i32, 384i32);
    let src = crussty_chunkgen::biomes::BiomeSourceData::from_params263(include_str!(
        "../params263_overworld.json"
    ))
    .expect("params263");
    let g = Gen263::new(seed, min_y, height, 63);
    let state = std::sync::Arc::new(BiomeState263 {
        src,
        climate: g.climate.clone(),
        y_invariant: [true, true, true, true, false, true],
    });
    let (cx, cz) = (1021i32, 1034i32);
    let n = (height / 16 * 64) as usize;
    let mut grids: [Option<Box<[i32]>>; 9] = std::array::from_fn(|_| None);
    for dz in -1i32..=1 {
        for dx in -1i32..=1 {
            let mut buf = vec![0i32; n].into_boxed_slice();
            crussty_chunkgen::biome263::fill_biomes263(
                &state,
                min_y,
                height,
                cx + dx,
                cz + dz,
                &mut buf,
            );
            grids[((dx + 1) * 3 + (dz + 1)) as usize] = Some(buf);
        }
    }
    let hood = Neighborhood263 { cx, cz, min_y, sections: 24, grids };
    let zoom = obfuscate_seed(seed);
    println!("zoom_seed={zoom}");
    let blocks: [(i32, i32, i32, &str, &str); 7] = [
        (16349, -19, 16554, "CINNABAR", "DEEPSLATE"),
        (16338, 41, 16550, "STONE", "SULFUR"),
        (16338, -19, 16555, "SULFUR", "DEEPSLATE"),
        (16338, -20, 16553, "SULFUR", "DEEPSLATE"),
        (16341, -20, 16549, "SULFUR", "DEEPSLATE"),
        (16347, -19, 16552, "SULFUR", "DEEPSLATE"),
        (16342, -24, 16552, "CINNABAR", "DEEPSLATE"),
    ];
    for (x, y, z, mn, mv) in blocks {
        let (qx, qy, qz, bi) = fiddle_biome_pick(&hood, zoom, x, y, z);
        println!(
            "block=({x},{y},{z}) pick_quart=({qx},{qy},{qz}) bi={bi} native_mat={mn} vanilla_mat={mv}"
        );
    }
    // also print the grid biome around the boundary quarts for one column
    for qy in -7..=-3 {
        let bi = hood.quart(16336 >> 2, qy, 16544 >> 2);
        println!("grid column quart (4084,{qy},4136) bi={bi}");
    }
}
