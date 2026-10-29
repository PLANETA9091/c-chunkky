//! Ore parity repro: run the real pack ore_gold through the native place
//! engine with an OreRef-identical mock level (everything stone, no air,
//! OCEAN_FLOOR_WG height 200, world -64..320) and print W x y z per write
//! in emission order. Diff against `java OreRef seed ox oz gid step`.
use crussty_chunkgen::features_deco::{DecoLevel, DecoLevelExt, HeightmapTy};
use crussty_chunkgen::features_placement::BlockPos;
use crussty_chunkgen::features_place_ffi::{run_feature, DecoState};
use crussty_chunkgen::worldgen::Assets;

struct Mock;
impl DecoLevelExt for Mock {
    fn state_is(&self, _: BlockPos, _: &[String]) -> bool { false }
    fn fluid_is(&self, _: BlockPos, _: &[String]) -> bool { false }
    fn tag_is(&self, _: BlockPos, _: &str) -> bool { true } // stone world: tag_match hits
}
impl DecoLevel for Mock {
    fn min_y(&self) -> i32 { -64 }
    fn max_y(&self) -> i32 { 320 }
    fn height(&self, _: HeightmapTy, _: i32, _: i32) -> i32 { 200 }
    fn state(&self, _: BlockPos) -> u32 { 0 }
    fn is_air(&self, _: BlockPos) -> bool { false } // no air: isAdjacentToAir false
    fn fluid(&self, _: BlockPos) -> u32 { 0 }
    fn biome_has_feature(&self, _: BlockPos, _: u32) -> bool { true }
    fn can_survive(&self, _: &str, _: BlockPos) -> bool { true }
    fn is_replaceable(&self, _: BlockPos) -> bool { false }
    fn is_solid(&self, _: BlockPos) -> bool { true }
    fn has_sturdy_face(&self, _: BlockPos, _: u8) -> bool { true }
    fn is_outside_build_height(&self, y: i32) -> bool { y < -64 || y >= 320 }
}

fn collect(dir: &std::path::Path, out: &mut Assets) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect(&p, out);
        } else if p.extension().map(|x| x == "json").unwrap_or(false) {
            if let Ok(b) = std::fs::read(&p) {
                let rel = p.to_string_lossy().to_string();
                let rel = rel.rsplit_once("/data/").map(|(_, r)| r.to_string()).unwrap_or(rel);
                out.insert(rel, b);
            }
        }
    }
}

fn main() {
    let mut a = std::env::args().skip(1);
    let seed: i64 = a.next().unwrap().parse().unwrap();
    let ox: i32 = a.next().unwrap().parse().unwrap();
    let oz: i32 = a.next().unwrap().parse().unwrap();
    let gid: i32 = a.next().unwrap().parse().unwrap();
    let step: i32 = a.next().unwrap().parse().unwrap();
    let name = a.next().unwrap_or_else(|| "crussty_f1:ore_gold".into());

    let root = std::path::Path::new("/home/z/my-project/c-faster/probe/datapacks/f1/crussty-f1");
    let mut assets = Assets::new();
    collect(root, &mut assets);
    let d = crussty_chunkgen::features_master::build_deco_ctx(&assets).expect("ctx");
    let state: &DecoState = &d.state;

    let mut out = vec![0i32; 5 * 65536];
    let n = run_feature(state, &Mock, seed, ox, oz, &name, gid, step, &mut out);
    for i in 0..n {
        let x = out[i * 5] + ox;
        let y = out[i * 5 + 1];
        let z = out[i * 5 + 2] + oz;
        let fl = out[i * 5 + 4];
        println!("W {} {} {} f{}", x, y, z, fl);
    }
    println!("# wrote={}", n);
}
