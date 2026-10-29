//! Debug: build the DecoCtx from the actual F1 probe pack dir and print the
//! plan / biome features / registry state. Run:
//!   cargo run --release --bin f1dbg

use crussty_chunkgen::worldgen::Assets;

fn collect(dir: &std::path::Path, out: &mut Assets) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect(&p, out);
        } else if p.extension().map(|x| x == "json").unwrap_or(false) {
            if let Ok(b) = std::fs::read(&p) {
                let rel = p.to_string_lossy().to_string();
                // .../<root>/data/<ns>/worldgen/... -> <ns>/worldgen/...
                let rel = rel.rsplit_once("/data/").map(|(_, r)| r.to_string()).unwrap_or(rel);
                out.insert(rel, b);
            }
        }
    }
}

fn run_rotated_case() {
    // reproduce the probe's rotated feature (step 9, gid 2) at chunk (-1,-1)
    // with a permissive mock level; print ops.
    use crussty_chunkgen::features_deco::{DecoLevel, DecoLevelExt, HeightmapTy};
    use crussty_chunkgen::features_placement::BlockPos;
    use crussty_chunkgen::features_master::{build_plan, chunk_plan, run_feature_master};
    use crussty_chunkgen::features_json::decode_registry;
    use std::collections::HashMap;

    struct Mock;
    impl DecoLevelExt for Mock {
        fn state_is(&self, _: BlockPos, _: &[String]) -> bool { false }
        fn fluid_is(&self, _: BlockPos, _: &[String]) -> bool { false }
        fn tag_is(&self, _: BlockPos, _: &str) -> bool { false }
    }
    impl DecoLevel for Mock {
        fn min_y(&self) -> i32 { -64 }
        fn max_y(&self) -> i32 { 320 }
        fn height(&self, _: HeightmapTy, _: i32, _: i32) -> i32 { 320 }
        fn state(&self, _: BlockPos) -> u32 { 0 }
        fn is_air(&self, _: BlockPos) -> bool { false }
        fn fluid(&self, _: BlockPos) -> u32 { 0 }
        fn biome_has_feature(&self, _: BlockPos, _: u32) -> bool { true }
        fn can_survive(&self, _: &str, _: BlockPos) -> bool { true }
        fn is_replaceable(&self, _: BlockPos) -> bool { false }
        fn is_solid(&self, _: BlockPos) -> bool { false }
        fn has_sturdy_face(&self, _: BlockPos, _: u8) -> bool { false }
        fn is_outside_build_height(&self, _: i32) -> bool { false }
    }

    let root = std::path::Path::new("/home/z/my-project/c-faster/probe/datapacks/f1/crussty-f1");
    let mut assets = Assets::new();
    collect(root, &mut assets);
    // feature jsons are needed too: collect already takes them? (packRel-like
    // filter is only in the JNI path; collect() takes every json) — good.
    let mut configured = Vec::new();
    let mut placed = Vec::new();
    for p in assets.paths().cloned().collect::<Vec<_>>() {
        if let Some((ns, rest)) = p.split_once("/worldgen/") {
            let text = String::from_utf8(assets.get(&p).unwrap().to_vec()).unwrap();
            if let Some(f) = rest.strip_prefix("configured_feature/") {
                configured.push((format!("{ns}:{}", f.trim_end_matches(".json")), text));
            } else if let Some(f) = rest.strip_prefix("placed_feature/") {
                placed.push((format!("{ns}:{}", f.trim_end_matches(".json")), text));
            }
        }
    }
    let reg = decode_registry(configured.into_iter(), placed.into_iter());
    let bf: HashMap<String, Vec<Vec<String>>> = [
        ("crussty_f1:test_a".to_string(), vec![
            vec![], vec![], vec![], vec![], vec![], vec![],
            vec!["crussty_f1:ore_replace".to_string()],
            vec![], vec![],
            ["crussty_f1:flowers","crussty_f1:weighted","crussty_f1:rotated","crussty_f1:rand_sel","crussty_f1:srs","crussty_f1:rbs","crussty_f1:offset_water","crussty_f1:pred_gate","crussty_f1:rarity_gate"].iter().map(|s| s.to_string()).collect(),
            vec![]]),
    ].into_iter().collect();
    let ordered: Vec<(String, Vec<Vec<String>>)> = bf.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    let plan = build_plan(&ordered).unwrap();
    let uid_names: Vec<String> = plan.steps.iter().flatten().cloned().collect();
    let state = crussty_chunkgen::features_place_ffi::DecoState::seal(reg, &uid_names);
    let level = Mock;
    let mut out = vec![0i32; 500];
    let n = run_feature_master(
        &state,
        &level,
        crussty_chunkgen::features_master::decoration_seed(3257840388504953787, -16, -16),
        None,
        BlockPos::new(-16, -64, -16),
        "crussty_f1:rotated",
        9, 2,
        &mut out,
    );
    println!("rotated ops (n={n}):");
    for i in 0..n {
        let (rx, y, rz, st, fl) = (out[i*5], out[i*5+1], out[i*5+2], out[i*5+3], out[i*5+4]);
        println!("  P rel=({rx},{rz}) y={y} state={st} axis={} flags={fl:#b}", (fl >> 2) & 3);
    }
}

fn run_gold_case() {
    // reproduce the probe's gold ore (step 6, gid 2) at chunk (0,0) with a
    // stone-filled mock level (no air anywhere); print RAW candidate set.
    // CRUS_DBG_X / CRUS_DBG_Z override the chunk block-origin (deco seed inputs).
    let dbg_x: i32 = std::env::var("CRUS_DBG_X").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    let dbg_z: i32 = std::env::var("CRUS_DBG_Z").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    eprintln!("[gold] chunk origin ({dbg_x},y,{dbg_z})");
    use crussty_chunkgen::features_deco::{DecoLevel, DecoLevelExt, HeightmapTy};
    use crussty_chunkgen::features_placement::BlockPos;
    use crussty_chunkgen::features_master::{build_plan, decoration_seed, run_feature_master};
    use crussty_chunkgen::features_json::decode_registry;
    use std::collections::HashMap;

    struct Stone;
    impl DecoLevelExt for Stone {
        fn state_is(&self, _: BlockPos, ids: &[String]) -> bool {
            ids.iter().any(|s| s == "minecraft:stone" || s.contains("ore_replaceables") == false && s.ends_with(":stone"))
        }
        fn fluid_is(&self, _: BlockPos, _: &[String]) -> bool { false }
        fn tag_is(&self, _: BlockPos, _t: &str) -> bool { true } // both tags match stone
    }
    impl DecoLevel for Stone {
        fn min_y(&self) -> i32 { -64 }
        fn max_y(&self) -> i32 { 320 }
        fn height(&self, _: HeightmapTy, _: i32, _: i32) -> i32 { 200 }
        fn state(&self, _: BlockPos) -> u32 { 1 }
        fn is_air(&self, _: BlockPos) -> bool { false }
        fn fluid(&self, _: BlockPos) -> u32 { 0 }
        fn biome_has_feature(&self, _: BlockPos, _: u32) -> bool { true }
        fn can_survive(&self, _: &str, _: BlockPos) -> bool { true }
        fn is_replaceable(&self, _: BlockPos) -> bool { false }
        fn is_solid(&self, _: BlockPos) -> bool { true }
        fn has_sturdy_face(&self, _: BlockPos, _: u8) -> bool { true }
        fn is_outside_build_height(&self, y: i32) -> bool { y < -64 || y >= 320 }
    }

    let root = std::path::Path::new("/home/z/my-project/c-faster/probe/datapacks/f1/crussty-f1");
    let mut assets = Assets::new();
    collect(root, &mut assets);
    let mut configured = Vec::new();
    let mut placed = Vec::new();
    for p in assets.paths().cloned().collect::<Vec<_>>() {
        if let Some((ns, rest)) = p.split_once("/worldgen/") {
            let text = String::from_utf8(assets.get(&p).unwrap().to_vec()).unwrap();
            if let Some(f) = rest.strip_prefix("configured_feature/") {
                configured.push((format!("{ns}:{}", f.trim_end_matches(".json")), text));
            } else if let Some(f) = rest.strip_prefix("placed_feature/") {
                placed.push((format!("{ns}:{}", f.trim_end_matches(".json")), text));
            }
        }
    }
    let reg = decode_registry(configured.into_iter(), placed.into_iter());
    let steps: Vec<Vec<String>> = {
        let mut v = vec![Vec::new(); 11];
        v[6] = ["crussty_f1:ore_replace","crussty_f1:ore_iron","crussty_f1:ore_gold","crussty_f1:ore_scattered"].iter().map(|s| s.to_string()).collect();
        v
    };
    let bf: HashMap<String, Vec<Vec<String>>> =
        [("crussty_f1:test_a".to_string(), steps.clone())].into_iter().collect();
    let ordered: Vec<(String, Vec<Vec<String>>)> = bf.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    let plan = build_plan(&ordered).unwrap();
    let uid_names: Vec<String> = plan.steps.iter().flatten().cloned().collect();
    let state = crussty_chunkgen::features_place_ffi::DecoState::seal(reg, &uid_names);
    let level = Stone;
    let mut out = vec![0i32; 50000];
    let n = run_feature_master(
        &state, &level,
        decoration_seed(3257840388504953787, dbg_x, dbg_z),
        None,
        BlockPos::new(dbg_x, -64, dbg_z),
        "crussty_f1:ore_gold",
        6, 2,
        &mut out,
    );
    println!("gold ops (n={n}):");
    for i in 0..n {
        println!("  W {} {} {}", out[i*5], out[i*5+1], out[i*5+2]);
    }
}

fn main() {
    run_gold_case();
    let root = std::path::Path::new("/home/z/my-project/c-faster/probe/datapacks/f1/crussty-f1");
    let mut assets = Assets::new();
    collect(root, &mut assets);
    println!("assets: {} files", assets.paths().count());
    for p in assets.paths() {
        println!("  {p}");
    }
    match crussty_chunkgen::features_master::build_deco_ctx(&assets) {
        Some(d) => {
            println!("\nplan steps: {}", d.plan.steps.len());
            for (i, s) in d.plan.steps.iter().enumerate() {
                println!("  step {i}: {:?}", s);
            }
            println!("uid_names: {:?}", d.uid_names);
            for (b, steps) in &d.biome_features {
                println!("biome {b}: {} steps, refs: {:?}",
                    steps.len(),
                    steps.iter().flatten().collect::<Vec<_>>());
            }
            println!("state table ({}): {:?}", d.state.states.len(), d.state.states);
            println!("uid_index: {:?}", d.state.uid_index);
            println!("unsupported_cfg={} unsupported_placed={}",
                d.state.reg.unsupported_configured, d.state.reg.unsupported_placed);
            let mut names: Vec<&String> = d.state.reg.configured.keys().collect();
            names.sort();
            for n in names {
                let c = &d.state.reg.configured[n];
                let k = match c {
                    crussty_chunkgen::features_json::FeatureCfg::NoOp => "NoOp".to_string(),
                    crussty_chunkgen::features_json::FeatureCfg::SimpleBlock { .. } => "SimpleBlock".to_string(),
                    crussty_chunkgen::features_json::FeatureCfg::ReplaceBlock { targets } => format!("ReplaceBlock({})", targets.len()),
                    crussty_chunkgen::features_json::FeatureCfg::Ore { targets, size, .. } => format!("Ore(size={size} t={})", targets.len()),
                    crussty_chunkgen::features_json::FeatureCfg::ScatteredOre { targets, size, .. } => format!("ScatteredOre(size={size} t={})", targets.len()),
                    crussty_chunkgen::features_json::FeatureCfg::RandomSelector { entries, .. } => format!("RandomSelector({})", entries.len()),
                    crussty_chunkgen::features_json::FeatureCfg::SimpleRandomSelector { entries } => format!("SRS({})", entries.len()),
                    crussty_chunkgen::features_json::FeatureCfg::RandomBoolSelector { .. } => "RBS".to_string(),
                    crussty_chunkgen::features_json::FeatureCfg::Disk { .. } => "Disk".to_string(),
                    crussty_chunkgen::features_json::FeatureCfg::Tree(_) => "Tree".to_string(),
                    crussty_chunkgen::features_json::FeatureCfg::VegetationPatch(_) => "VegPatch".to_string(),
                    crussty_chunkgen::features_json::FeatureCfg::RandomPatch { tries, inner, .. } => format!("RandomPatch(tries={tries} -> {inner})"),
                    crussty_chunkgen::features_json::FeatureCfg::Disk { target: _, radius: _, .. } => "Disk".to_string(),
                    crussty_chunkgen::features_json::FeatureCfg::NetherrackReplaceBlobs { .. } => "ReplaceBlobs".to_string(),
                    crussty_chunkgen::features_json::FeatureCfg::Seagrass { .. } => "Seagrass".to_string(),
                    crussty_chunkgen::features_json::FeatureCfg::Spring { .. } => "Spring".to_string(),
                    crussty_chunkgen::features_json::FeatureCfg::BlockPile { .. } => "BlockPile".to_string(),
                    crussty_chunkgen::features_json::FeatureCfg::Kelp => "Kelp".to_string(),
                    crussty_chunkgen::features_json::FeatureCfg::MonsterRoom => "MonsterRoom".to_string(),
                    crussty_chunkgen::features_json::FeatureCfg::UnderwaterMagma { .. } => "UnderwaterMagma".to_string(),
                    crussty_chunkgen::features_json::FeatureCfg::Vines => "Vines".to_string(),
                    crussty_chunkgen::features_json::FeatureCfg::MultifaceGrowth { .. } => "MultifaceGrowth".to_string(),
                    crussty_chunkgen::features_json::FeatureCfg::BlockColumn { layers, .. } => format!("BlockColumn({})", layers.len()),
                    crussty_chunkgen::features_json::FeatureCfg::RootSystem { .. } => "RootSystem".to_string(),
                    crussty_chunkgen::features_json::FeatureCfg::Unsupported(e) => format!("Unsupported({e})"),
                };
                println!("  cfg {n}: {k}");
            }
            let mut pn: Vec<&String> = d.state.reg.placed.keys().collect();
            pn.sort();
            for n in pn {
                let pd = &d.state.reg.placed[n];
                let bad = pd.placement.iter().any(|m| matches!(m, crussty_chunkgen::features_deco::Mod::Unsupported(_)));
                println!("  placed {n}: feature={} mods={} bad={}", pd.feature, pd.placement.len(), bad);
            }
            let runs = crussty_chunkgen::features_master::chunk_plan(
                &d.plan,
                &d.biome_features,
                &["crussty_f1:test_a".to_string()],
            );
            println!("chunk_plan(test_a): {} runs {:?}", runs.len(), runs);
        }
        None => println!("build_deco_ctx returned NONE"),
    }
}
