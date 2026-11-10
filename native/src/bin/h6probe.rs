//! H6 probe: dp-path arena census incl. the spline-value blind spot.
//! Prints every node reachable from slot0 (interp_children[0]) — spline
//! point VALUES included (children() skips them, which is exactly the
//! compile-time blind spot this task is after) — with noise y-scale and
//! column-cache verdicts. Also locates the OldBlended node's ancestors.
//!
//! Usage: h6probe <dp_root...>
use crussty_chunkgen::density::{Env, SplineValueW, WKind};
use crussty_chunkgen::worldgen::Assets;
use std::path::Path;

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

    // full-reach walk including spline point values
    let mut seen = vec![false; c.arena.len()];
    let mut parents: Vec<Option<u32>> = vec![None; c.arena.len()];
    let mut stack = vec![c.interp_children[0]];
    let mut nodes = Vec::new();
    while let Some(i) = stack.pop() {
        if seen[i as usize] {
            continue;
        }
        seen[i as usize] = true;
        nodes.push(i);
        let mut push = |child: u32, parent: u32, parents: &mut Vec<Option<u32>>, stack: &mut Vec<u32>| {
            if parents[child as usize].is_none() && child != c.interp_children[0] {
                parents[child as usize] = Some(parent);
            }
            stack.push(child);
        };
        match &c.arena[i as usize] {
            WKind::Noise { noise_id, xz_scale, y_scale } => {
                let _ = (noise_id, xz_scale, y_scale);
            }
            WKind::ShiftedNoise { sx, sy, sz, .. } => {
                push(*sx, i, &mut parents, &mut stack);
                push(*sy, i, &mut parents, &mut stack);
                push(*sz, i, &mut parents, &mut stack);
            }
            WKind::Spline { coordinate, points } => {
                push(*coordinate, i, &mut parents, &mut stack);
                for (loc, v, der) in points {
                    if let SplineValueW::Node(n) = v {
                        push(*n, i, &mut parents, &mut stack);
                    }
                    let _ = (loc, der);
                }
            }
            WKind::RangeChoice { input, when_in_range, when_out_of_range, .. } => {
                push(*input, i, &mut parents, &mut stack);
                push(*when_in_range, i, &mut parents, &mut stack);
                push(*when_out_of_range, i, &mut parents, &mut stack);
            }
            WKind::Lerp { alpha, first, second } => {
                push(*alpha, i, &mut parents, &mut stack);
                push(*first, i, &mut parents, &mut stack);
                push(*second, i, &mut parents, &mut stack);
            }
            WKind::IntervalSelect { input, functions, .. } => {
                push(*input, i, &mut parents, &mut stack);
                for f in functions {
                    push(*f, i, &mut parents, &mut stack);
                }
            }
            WKind::Clamp { input, .. }
            | WKind::Mapped { input, .. }
            | WKind::MulOrAdd { input, .. }
            | WKind::Cache2d { child: input }
            | WKind::CacheOnce { child: input }
            | WKind::FlatCache { child: input }
            | WKind::CacheAllInCell { child: input } => {
                push(*input, i, &mut parents, &mut stack)
            }
            WKind::Ap2 { a, b, .. } => {
                push(*a, i, &mut parents, &mut stack);
                push(*b, i, &mut parents, &mut stack);
            }
            WKind::WeirdScaledSampler { input, .. } => {
                push(*input, i, &mut parents, &mut stack)
            }
            _ => {}
        }
    }

    // classify
    let mut yv_noisy: Vec<String> = Vec::new();
    let mut col_missing_2d: Vec<String> = Vec::new();
    let mut col_covered: Vec<String> = Vec::new();
    for &i in &nodes {
        let ii = i as usize;
        let col = c.col_idx[ii] != u32::MAX;
        match &c.arena[ii] {
            WKind::Noise { noise_id, y_scale, .. }
            | WKind::ShiftedNoise { noise_id, y_scale, .. } => {
                let name = c
                    .noise_ids
                    .get(*noise_id as usize)
                    .cloned()
                    .unwrap_or_default();
                let tag = format!("{name} (node {i} yscale={y_scale})");
                if col {
                    col_covered.push(tag);
                } else if *y_scale == 0.0 {
                    col_missing_2d.push(format!("{tag} [2D UNCACHED]"));
                } else {
                    yv_noisy.push(tag);
                }
            }
            WKind::ShiftA { noise_id } | WKind::ShiftB { noise_id } => {
                let name = c
                    .noise_ids
                    .get(*noise_id as usize)
                    .cloned()
                    .unwrap_or_default();
                let k = if matches!(&c.arena[ii], WKind::ShiftA { .. }) {
                    "ShiftA"
                } else {
                    "ShiftB"
                };
                if col {
                    col_covered.push(format!("{k}:{name} (node {i})"));
                } else {
                    col_missing_2d.push(format!("{k}:{name} (node {i}) [2D UNCACHED]"));
                }
            }
            WKind::OldBlendedNoise { .. } => {
                let mut chain = format!("OldBlended node {i} parents:");
                let mut p = parents[ii];
                while let Some(pp) = p {
                    chain.push_str(&format!(" {}", pp));
                    p = parents[pp as usize];
                }
                col_missing_2d.push(chain);            }
            _ => {}
        }
    }
    println!("reach incl spline-values: {} nodes", nodes.len());
    println!("col-covered noise nodes ({}):", col_covered.len());
    for s in &col_covered {
        println!("  OK   {s}");
    }
    println!("2D UNCACHED y-invariant noise ({}):", col_missing_2d.len());
    for s in &col_missing_2d {
        println!("  MISS {s}");
    }
    println!("3D (y-variant) noise nodes: {}", yv_noisy.len());

    // spline census: spline nodes whose point VALUES contain noise
    let mut splines_with_noise = 0usize;
    for &i in &nodes {
        if let WKind::Spline { points, .. } = &c.arena[i as usize] {
            let mut has_noise = false;
            for (_, v, _) in points {
                if let SplineValueW::Node(n) = v {
                    if seen[*n as usize] {
                        has_noise = true;
                    }
                }
            }
            if has_noise {
                splines_with_noise += 1;
            }
        }
    }
    println!("spline nodes with noise in point values: {splines_with_noise}");
}
