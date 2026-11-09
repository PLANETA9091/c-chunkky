//! dp-slot anatomy: per Interpolated-slot corner-eval timing + tree inventory
//! (noise nodes, col-cache slots, y-invariance) on the datapack path.
//! Usage: dpanatomy <dp_root...>

use crussty_chunkgen::density::{Env, Mode, Pt};
use crussty_chunkgen::worldgen::Assets;
use std::path::Path;
use std::time::Instant;

fn collect(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                collect(root, &p, out);
            } else if p.extension().map(|x| x == "json").unwrap_or(false) {
                if let Ok(rel) = p.strip_prefix(root) {
                    let rel = rel.to_string_lossy().replace('\\', "/");
                    let Some(stripped) = rel.strip_prefix("data/") else { continue };
                    if let Ok(bytes) = std::fs::read(&p) {
                        out.push((stripped.to_string(), bytes));
                    }
                }
            }
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let seed: i64 = std::env::var("CRUS_BENCH_SEED")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3257840388504953787);
    let mut assets = Assets::new();
    for root in &args[1..] {
        let root = Path::new(root);
        let mut files = Vec::new();
        collect(root, root, &mut files);
        files.sort();
        for (path, bytes) in files {
            let keep = path.contains("/worldgen/noise_settings/")
                || path.contains("/worldgen/density_function/")
                || path.contains("/worldgen/noise/")
                || path.ends_with("/dimension/overworld.json");
            if keep {
                assets.insert(path, bytes);
            }
        }
    }
    let env: &'static Env = Box::leak(Box::new(Env::new_assets(seed, &assets)));
    let compiled = std::sync::Arc::new(env.compile());
    let c = &compiled;

    // arena inventory
    let mut counts = std::collections::BTreeMap::new();
    for n in &c.arena {
        let k = match n {
            crussty_chunkgen::density::WKind::Const(_) => "Const",
            crussty_chunkgen::density::WKind::Noise { .. } => "Noise",
            crussty_chunkgen::density::WKind::ShiftedNoise { .. } => "ShiftedNoise",
            crussty_chunkgen::density::WKind::ShiftA { .. } => "ShiftA",
            crussty_chunkgen::density::WKind::ShiftB { .. } => "ShiftB",
            crussty_chunkgen::density::WKind::OldBlendedNoise { .. } => "OldBlended",
            crussty_chunkgen::density::WKind::Clamp { .. } => "Clamp",
            crussty_chunkgen::density::WKind::Mapped { .. } => "Mapped",
            crussty_chunkgen::density::WKind::Ap2 { .. } => "Ap2",
            crussty_chunkgen::density::WKind::RangeChoice { .. } => "RangeChoice",
            crussty_chunkgen::density::WKind::YClampedGradient { .. } => "YClampedGradient",
            crussty_chunkgen::density::WKind::Spline { .. } => "Spline",
            crussty_chunkgen::density::WKind::WeirdScaledSampler { .. } => "WeirdScaled",
            crussty_chunkgen::density::WKind::EndIslands => "EndIslands",
            crussty_chunkgen::density::WKind::Cache2d { .. } => "Cache2d",
            crussty_chunkgen::density::WKind::FlatCache { .. } => "FlatCache",
            crussty_chunkgen::density::WKind::CacheOnce { .. } => "CacheOnce",
            crussty_chunkgen::density::WKind::CacheAllInCell { .. } => "CacheAllInCell",
            crussty_chunkgen::density::WKind::Interpolated { .. } => "Interpolated",
            crussty_chunkgen::density::WKind::MulOrAdd { .. } => "MulOrAdd",
            _ => "Other263",
        };
        *counts.entry(k).or_insert(0usize) += 1;
    }
    let cols = c.col_idx.iter().filter(|&&v| v != u32::MAX).count();
    println!("arena {} nodes | col-cache slots {} | {:?}",
        c.arena.len(), cols,
        counts.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(" "));

    // slot-0 subtree inventory: y-variant vs col-cached nodes, noise fanout
    {
        use crussty_chunkgen::density::WKind;
        let mut seen = vec![false; c.arena.len()];
        let mut stack = vec![c.interp_children[0]];
        let (mut n_yv, mut n_col, mut n_noise_col, mut n_noise_yv, mut n_spline_yv) =
            (0usize, 0usize, 0usize, 0usize, 0usize);
        let mut noise_names: Vec<String> = Vec::new();
        while let Some(i) = stack.pop() {
            if seen[i as usize] {
                continue;
            }
            seen[i as usize] = true;
            let col = c.col_idx[i as usize] != u32::MAX;
            if col {
                n_col += 1;
            } else {
                n_yv += 1;
            }
            match &c.arena[i as usize] {
                WKind::Noise { noise_id, .. } => {
                    if col {
                        n_noise_col += 1;
                    } else {
                        n_noise_yv += 1;
                        noise_names.push(
                            c.noise_ids.get(*noise_id as usize).cloned().unwrap_or_default(),
                        );
                    }
                }
                WKind::Spline { .. } => {
                    if !col {
                        n_spline_yv += 1;
                    }
                }
                WKind::ShiftedNoise { sx, sy, sz, .. } => {
                    stack.extend([*sx, *sy, *sz]);
                }
                WKind::Clamp { input, .. }
                | WKind::Mapped { input, .. }
                | WKind::MulOrAdd { input, .. }
                | WKind::Cache2d { child: input }
                | WKind::CacheOnce { child: input }
                | WKind::FlatCache { child: input }
                | WKind::CacheAllInCell { child: input } => stack.push(*input),
                WKind::Ap2 { a, b, .. } => {
                    stack.push(*a);
                    stack.push(*b);
                }
                WKind::RangeChoice { input, when_in_range, when_out_of_range, .. } => {
                    stack.push(*input);
                    stack.push(*when_in_range);
                    stack.push(*when_out_of_range);
                }
                _ => {}
            }
        }
        noise_names.sort();
        noise_names.dedup();
        // octave inventory of the slot-0 subtree's noise nodes
        let mut oct_inv: Vec<(String, usize, usize)> = Vec::new();
        {
            let mut visit_oct = |idx: u32, st: &mut std::collections::HashSet<u32>, out: &mut Vec<(String, usize, usize)>| {
                // iterative walk
                let mut stack = vec![idx];
                while let Some(i) = stack.pop() {
                    if !st.insert(i) { continue; }
                    match &c.arena[i as usize] {
                        WKind::Noise { noise_id, .. } | WKind::ShiftedNoise { noise_id, .. } => {
                            let nidx = c.noise_resolved[*noise_id as usize] as usize;
                            let n = &env.noise.list[nidx];
                            let oct = n.first.noise_levels.iter().filter(|x| x.is_some()).count()
                                + n.second.noise_levels.iter().filter(|x| x.is_some()).count();
                            out.push((c.noise_ids.get(*noise_id as usize).cloned().unwrap_or_default(), oct, nidx));
                        }
                        _ => {}
                        _ => {}
                    }
                    for ch in c.arena[i as usize].children() {
                        stack.push(ch);
                    }
                }
            };
            let mut seen = std::collections::HashSet::new();
            visit_oct(c.interp_children[0], &mut seen, &mut oct_inv);
            oct_inv.sort();
            oct_inv.dedup();
        }
        println!("slot0 noise octave inventory: {:?}", oct_inv);
        println!(
            "slot0 subtree: y-variant nodes {n_yv} (col-cached {n_col}) | noise: col {n_noise_col}, yv {n_noise_yv} {:?} | splines yv {n_spline_yv}",
            noise_names
        );

        // which y-variant noises bypass the col path (col_ok=false)?
        let mut bypass: Vec<String> = Vec::new();
        for (i, n) in c.arena.iter().enumerate() {
            if let WKind::Noise { noise_id, y_scale, .. } = n {
                if c.col_idx[i] == u32::MAX {
                    let inst = &env.noise.list[c.noise_resolved[*noise_id as usize] as usize];
                    if inst.first.amplitudes.len() > crussty_chunkgen::noise::COL_MAX_OCTAVES {
                        bypass.push(format!(
                            "{} (oct={} yscale={y_scale})",
                            c.noise_ids.get(*noise_id as usize).cloned().unwrap_or_default(),
                            inst.first.amplitudes.len()
                        ));
                    }
                }
            }
        }
        println!("col_bypass noises: {:?}", bypass);
    }


    // per-slot plane eval timing: 2 z-corners x 385 y (one x-plane, dp grid)
    let interp = crussty_chunkgen::density::InterpState::new(c.interp_slots, 1, (env.wg.height / env.wg.cell_height) as usize);
    let mut memo = crussty_chunkgen::density::MemoCtx::new(c.arena.len())
        .with_cols(c.col_count as usize);
    let cell_h = env.wg.cell_height;
    let n_y = (env.wg.height / cell_h) as i32;
    for (slot, &child) in c.interp_children.iter().enumerate() {
        let mut sink = 0f64;
        // warm
        for zc in 0..2i32 {
            for yi in 0..4 {
                memo.next_serial();
                let pt = Pt { x: 0, y: env.wg.min_y + yi * cell_h, z: zc * 16 };
                sink += env.eval(c, child, pt, Mode::Direct, &interp, Some(&mut memo));
            }
        }
        let t0 = Instant::now();
        for zc in 0..2i32 {
            for yi in 0..=n_y {
                memo.next_serial();
                let pt = Pt { x: 0, y: env.wg.min_y + yi * cell_h, z: zc * 16 };
                sink += env.eval(c, child, pt, Mode::Direct, &interp, Some(&mut memo));
            }
        }
        let el = t0.elapsed();
        println!(
            "slot {slot} child {child}: plane eval {:?} (770 pts) sink={:e}",
            el,
            sink * 0.0
        );
    }
}
