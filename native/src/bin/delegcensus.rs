//! Delegate census: which placed features hit the F3 hybrid delegate on a
//! pack track, and WHICH node blocks them. Mirrors the runtime gate exactly
//! (features_place::placed_fully_supported = configured-graph walk, placement
//! modifiers NOT included — same as decoRunFeature's early -2 gate).
//!
//! Usage: delegcensus <root1> [root2 ...]   (pack order; LATER roots override)
//! Each root = extracted datapack dir with data/<ns>/worldgen/... (+ optional
//! overlay dirs listed after the root, merged on top: delegcensus tect tect/overlay.datapack terr).
//!
//! Output:
//!   [deleg] per-feature lines for unsupported placed features:
//!     UNSUP <placed> BLOCK=<reason>
//!   then a summary grouped by blocker reason.
//!   Plus a PARITY-RISK section: features that RUN NATIVE but carry an
//!   Unsupported placement modifier (silently no-op'd at run time).

use crussty_chunkgen::features_json::{decode_registry, FeatureCfg, FeatureRegistry};
use crussty_chunkgen::features_deco::BlockPredicate;
use crussty_chunkgen::features_json::StateSrc;
use crussty_chunkgen::features_place::placed_fully_supported;
use crussty_chunkgen::features_vanilla_data::{VANILLA_CONFIGURED, VANILLA_PLACED};
use std::collections::HashMap;
use std::path::Path;

/// state-provider support walk (mirrors features_place::state_src_supported)
fn state_blocker(src: &StateSrc) -> Option<String> {
    match src {
        StateSrc::Simple(_) | StateSrc::Weighted(_) => None,
        StateSrc::RuleBased { fallback, rules } => {
            if let Some(b) = state_blocker(fallback) {
                return Some(b);
            }
            for (p, s) in rules {
                if matches!(p, BlockPredicate::Unsupported(_)) {
                    return Some("rb-pred".into());
                }
                if let Some(b) = state_blocker(s) {
                    return Some(b);
                }
            }
            None
        }
        StateSrc::RandomizedInt { source, .. } => state_blocker(source),
        StateSrc::Unsupported(e) => Some(format!("state:{e}")),
    }
}

fn collect(root: &Path, dir: &Path, out: &mut HashMap<String, String>) {
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                collect(root, &p, out);
            } else if p.extension().map(|x| x == "json").unwrap_or(false) {
                let Some(rel) = p.strip_prefix(root).ok().map(|r| r.to_string_lossy().replace('\\', "/")) else { continue };
                let Some(stripped) = rel.strip_prefix("data/") else { continue };
                let keep = stripped.contains("/worldgen/configured_feature/")
                    || stripped.contains("/worldgen/placed_feature/");
                if !keep {
                    continue;
                }
                let name = stripped
                    .trim_start_matches("worldgen/") // not used, path form below
                    .to_string();
                let _ = name;
                // rel: <ns>/worldgen/<kind>/<file...>  (file may nest)
                let Some((ns, rest)) = stripped.split_once('/') else { continue };
                let Some(rest) = rest.strip_prefix("worldgen/") else { continue };
                let Some((kind, file)) = rest.split_once('/') else { continue };
                if kind != "configured_feature" && kind != "placed_feature" {
                    continue;
                }
                let Some(file) = file.strip_suffix(".json") else { continue };
                let key = format!("{ns}/worldgen/{kind}/{file}");
                if let Ok(bytes) = std::fs::read(&p) {
                    out.insert(key, String::from_utf8_lossy(&bytes).into_owned());
                }
            }
        }
    }
}

fn json_type(text: &str) -> String {
    // crude but fine for diagnostics: first "type": "..." at depth 1
    let pat = "\"type\": \"";
    if let Some(i) = text.find(pat) {
        let rest = &text[i + pat.len()..];
        if let Some(j) = rest.find('"') {
            return rest[..j].to_string();
        }
    }
    "?".into()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: delegcensus <root1> [root2 ...] (pack order, later overrides)");
        std::process::exit(2);
    }
    // merged pack assets (later roots override earlier)
    let mut merged: HashMap<String, String> = HashMap::new();
    for root in &args {
        let rp = Path::new(root);
        collect(rp, rp, &mut merged);
        eprintln!("[delegcensus] root {}: {} feature jsons", root, merged.len());
    }
    // split into configured/placed with <ns>:<name> keys (decode order matters:
    // pack entries first, then vanilla fallback fills gaps — master protocol)
    let mut configured: Vec<(String, String)> = Vec::new();
    let mut placed: Vec<(String, String)> = Vec::new();
    for (k, v) in &merged {
        // k: <ns>/worldgen/configured_feature/<file>
        if let Some(idx) = k.find("/worldgen/configured_feature/") {
            let ns = &k[..idx];
            let f = &k[idx + "/worldgen/configured_feature/".len()..];
            configured.push((format!("{ns}:{f}"), v.clone()));
        } else if let Some(idx) = k.find("/worldgen/placed_feature/") {
            let ns = &k[..idx];
            let f = &k[idx + "/worldgen/placed_feature/".len()..];
            placed.push((format!("{ns}:{f}"), v.clone()));
        }
    }
    configured.sort();
    placed.sort();
    let pack_cfg: std::collections::HashSet<String> = configured.iter().map(|(n, _)| n.clone()).collect();
    let pack_pl: std::collections::HashSet<String> = placed.iter().map(|(n, _)| n.clone()).collect();
    for (n, t) in VANILLA_CONFIGURED {
        if !pack_cfg.contains(*n) {
            configured.push(((*n).to_string(), (*t).to_string()));
        }
    }
    for (n, t) in VANILLA_PLACED {
        if !pack_pl.contains(*n) {
            placed.push(((*n).to_string(), (*t).to_string()));
        }
    }
    let raw_cfg: HashMap<String, String> = configured.iter().cloned().collect();
    let raw_pl: HashMap<String, String> = placed.iter().cloned().collect();
    let configured_total_debug = (configured.len(), placed.len());
    // DEBUG: stepwise decode of one known-problematic inline placed
    for probe in ["terralith:cave/stone/granite_pillar_large"] {
        if let Some((_, text)) = placed.iter().find(|(n, _)| n == probe) {
            match crussty_chunkgen::json::parse(text) {
                Err(e) => println!("[dbg] {probe}: parse err {e:?}"),
                Ok(j) => {
                    let fo = j.get("feature");
                    println!(
                        "[dbg] {probe}: feature-is-str={} has-feature={}",
                        fo.and_then(|v| v.as_str()).is_some(),
                        fo.is_some()
                    );
                    if let Some(arr) = j.get("placement").and_then(|v| v.as_arr()) {
                        for (i, e) in arr.iter().enumerate() {
                            let m = crussty_chunkgen::features_json::decode_placement_modifier(e);
                            println!("[dbg]   mod[{i}] {} -> {}", e.get("type").and_then(|v| v.as_str()).unwrap_or("?"), match &m {
                                crussty_chunkgen::features_deco::Mod::Unsupported(r) => format!("UNSUPPORTED({r})"),
                                _ => "ok".into(),
                            });
                        }
                    }
                }
            }
        } else {
            println!("[dbg] {probe}: NOT IN INPUT");
        }
    }
    let reg = decode_registry(configured.into_iter(), placed.into_iter());

    // inline-feature placed refs: decode_registry DROPS them (unsupported) —
    // the runtime gate delegates them via placed-missing. Count explicitly.
    let mut inline_placed: Vec<String> = Vec::new();
    for (name, text) in &raw_pl {
        if let Ok(j) = crussty_chunkgen::json::parse(text) {
            if j.get("feature").and_then(|v| v.as_str()).is_none() {
                if j.get("feature").is_some() {
                    inline_placed.push(name.clone());
                }
            }
        }
    }

    // first blocker walk (mirrors cfg_fully_supported, returns the reason)
    fn cfg_blocker(reg: &FeatureRegistry, cfg_name: &str, depth: u32, raw: &HashMap<String, String>) -> Option<String> {
        if depth > 64 {
            return Some("depth>64".into());
        }
        let Some(cfg) = reg.configured.get(cfg_name) else {
            return Some(format!("cfg-missing:{cfg_name}"));
        };
        match cfg {
            FeatureCfg::NoOp
            | FeatureCfg::SimpleBlock { .. }
            | FeatureCfg::ReplaceBlock { .. }
            | FeatureCfg::Ore { .. }
            | FeatureCfg::ScatteredOre { .. }
            | FeatureCfg::Disk { .. }
            | FeatureCfg::Tree { .. }
            | FeatureCfg::VegetationPatch { .. }
            | FeatureCfg::NetherrackReplaceBlobs { .. }
            | FeatureCfg::Seagrass { .. }
            | FeatureCfg::Spring { .. }
            | FeatureCfg::BlockPile { .. }
            | FeatureCfg::Kelp
            | FeatureCfg::UnderwaterMagma { .. }
            | FeatureCfg::MultifaceGrowth { .. }
            | FeatureCfg::Vines => None,
            FeatureCfg::BlockColumn { layers, allowed, .. } => {
                if matches!(allowed, BlockPredicate::Unsupported(_)) {
                    return Some("bc-allowed".into());
                }
                for (_, sp) in layers {
                    if let Some(b) = state_blocker(sp) {
                        return Some(b);
                    }
                }
                None
            }
            FeatureCfg::RootSystem { tree_feature, allowed_tree_position, root_state, hanging_root_state, .. } => {
                if matches!(allowed_tree_position, BlockPredicate::Unsupported(_)) {
                    return Some("rs-atp".into());
                }
                if let Some(b) = state_blocker(root_state) {
                    return Some(b);
                }
                if let Some(b) = state_blocker(hanging_root_state) {
                    return Some(b);
                }
                blocker(reg, tree_feature, depth + 1, raw)
            }
            // native gate keeps this as a delegate (RNG-stream BlockEntity seeds)
            FeatureCfg::MonsterRoom => Some("monster_room (BE seeds)".into()),
            FeatureCfg::RandomSelector { entries, default } => {
                for (_, n) in entries {
                    if let Some(b) = blocker(reg, n, depth + 1, raw) {
                        return Some(b);
                    }
                }
                blocker(reg, default, depth + 1, raw)
            }
            FeatureCfg::SimpleRandomSelector { entries } => {
                for n in entries {
                    if let Some(b) = blocker(reg, n, depth + 1, raw) {
                        return Some(b);
                    }
                }
                None
            }
            FeatureCfg::RandomBoolSelector { if_true, if_false, .. } => blocker(reg, if_true, depth + 1, raw)
                .or_else(|| blocker(reg, if_false, depth + 1, raw)),
            FeatureCfg::RandomPatch { inner, .. } => blocker(reg, inner, depth + 1, raw),
            FeatureCfg::Unsupported(_) => {
                // prefer the real type name from the raw json
                let t = raw.get(cfg_name).map(|s| json_type(s)).unwrap_or_else(|| "?".into());
                Some(if t == "?" { "cf-type(?)".into() } else { t })
            }
        }
    }
    fn blocker(reg: &FeatureRegistry, name: &str, depth: u32, raw: &HashMap<String, String>) -> Option<String> {
        if depth > 64 {
            return Some("depth>64".into());
        }
        let Some(pd) = reg.placed.get(name) else {
            return Some(format!("placed-missing:{name}"));
        };
        cfg_blocker(reg, &pd.feature, depth + 1, raw)
    }

    let mut unsup: Vec<(String, String)> = Vec::new();
    let mut n_ok = 0usize;
    let mut names: Vec<&String> = reg.placed.keys().collect();
    names.sort();
    for name in &names {
        if placed_fully_supported(&reg, name, 0) {
            n_ok += 1;
        } else {
            let b = blocker(&reg, name, 0, &raw_cfg).unwrap_or("?".into());
            unsup.push(((*name).clone(), b));
        }
    }
    println!("[deleg] placed total={} (raw files: cfg={} pl={}) native-ok={} delegate={}",
        reg.placed.len(),
        configured_total_debug.0, configured_total_debug.1,
        n_ok, unsup.len());
    println!("[deleg] inline-feature placed (always delegate): {}", inline_placed.len());
    println!("[deleg] configured total={} (decode-unsupported={})", reg.configured.len(), {
        reg.configured.values().filter(|c| matches!(c, FeatureCfg::Unsupported(_))).count()
    });
    // group by blocker
    let mut byb: HashMap<String, Vec<String>> = HashMap::new();
    for (n, b) in &unsup {
        byb.entry(b.clone()).or_default().push(n.clone());
    }
    let mut byb: Vec<(String, Vec<String>)> = byb.into_iter().collect();
    byb.sort_by_key(|(_, v)| std::cmp::Reverse(v.len()));
    for (b, names) in &byb {
        println!("[deleg] BLOCK {} x{}", b, names.len());
        for n in names.iter().take(12) {
            println!("          {}", n);
        }
        if names.len() > 12 {
            println!("          ... +{}", names.len() - 12);
        }
    }
}
