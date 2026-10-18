//! Debug: place patch_ref from the real probe pack with a permissive mock level.
use crussty_chunkgen::features_deco::{DecoLevel, DecoLevelExt, HeightmapTy};
use crussty_chunkgen::features_placement::BlockPos;
use crussty_chunkgen::features_place_ffi::{run_feature, DecoState};
use crussty_chunkgen::worldgen::Assets;

struct Mock;
impl DecoLevelExt for Mock {
    fn state_is(&self, _: BlockPos, _: &[String]) -> bool { false }
    fn fluid_is(&self, _: BlockPos, _: &[String]) -> bool { false }
    fn tag_is(&self, _: BlockPos, _: &str) -> bool { false }
}
impl DecoLevel for Mock {
    fn min_y(&self) -> i32 { -64 }
    fn max_y(&self) -> i32 { 320 }
    fn height(&self, _: HeightmapTy, _: i32, _: i32) -> i32 { 64 }
    fn state(&self, _: BlockPos) -> u32 { 0 }
    fn is_air(&self, _: BlockPos) -> bool { true }
    fn fluid(&self, _: BlockPos) -> u32 { 0 }
    fn biome_has_feature(&self, _: BlockPos, _: u32) -> bool { true }
    fn can_survive(&self, _: &str, _: BlockPos) -> bool { true }
    fn is_replaceable(&self, _: BlockPos) -> bool { false }
    fn is_solid(&self, _: BlockPos) -> bool { false }
    fn has_sturdy_face(&self, _: BlockPos, _: u8) -> bool { false }
    fn is_outside_build_height(&self, _: i32) -> bool { false }
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
    let root = std::path::Path::new("/home/z/my-project/c-faster/probe/datapacks/f1/crussty-f1");
    let mut assets = Assets::new();
    collect(root, &mut assets);
    let d = crussty_chunkgen::features_master::build_deco_ctx(&assets).expect("ctx");
    let state: &DecoState = &d.state;
    println!("states: {}", state.states.len());
    {
        let f = &state.reg.placed["crussty_f1:flowers"];
        println!("flowers mods = {:?}", f.placement);
        println!("flowers cfg = {:?}", state.reg.configured["crussty_f1:flowers"]);
        // decode the raw file text through the same path
        let text = std::fs::read_to_string("/home/z/my-project/c-faster/probe/datapacks/f1/crussty-f1/data/crussty_f1/worldgen/placed_feature/flowers.json").unwrap();
        let j = crussty_chunkgen::json::parse(&text).unwrap();
        let arr = j.get("placement").unwrap().as_arr().unwrap();
        for (i, e) in arr.iter().enumerate() {
            let m = crussty_chunkgen::features_json::decode_placement_modifier(e);
            println!("raw mod {i} = {:?}", m);
        }
    }
    // patch_ref parity vs PatchRef.java: chunk (100,100), step=10, gid=2
    {
        let mut out = vec![0i32; 5 * 20000];
        let n = run_feature(state, &Mock, 3257840388504953787, 1600, 1600, "crussty_f1:patch_ref", 2, 10, &mut out);
        let mut xs = Vec::new();
        for c in out[..n * 5].chunks(5) {
            xs.push((c[0], c[1], c[2], c[4]));
        }
        println!("patch_ref native ops (n={n}): {:?}", xs);
    }
    for name in ["crussty_f1:patch_ref", "crussty_f1:rotated", "crussty_f1:ore_gold", "crussty_f1:disk_a", "crussty_f1:patch_simple"] {
        let mut out = vec![0i32; 5 * 20000];
        let n = run_feature(state, &Mock, 3257840388504953787, 1600, 1600, name, 17, 10, &mut out);
        let ops = &out[..n * 5];
        let writes = ops.chunks(5).filter(|c| c[4] & 32 == 0).count();
        let marks = ops.chunks(5).filter(|c| c[4] & 32 != 0).count();
        let mut minx = i32::MAX; let mut maxx = i32::MIN; let mut miny = i32::MAX; let mut maxy = i32::MIN;
        for c in ops.chunks(5) {
            minx = minx.min(c[0]); maxx = maxx.max(c[0]);
            miny = miny.min(c[1]); maxy = maxy.max(c[1]);
        }
        println!("{name}: ops={n} writes={writes} marks={marks} x=[{minx}..{maxx}] y=[{miny}..{maxy}]");
    }
}
