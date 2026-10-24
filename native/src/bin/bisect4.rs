//! Walk slot-0's tree with both eval and eval4; print the shallowest
//! diverging nodes.
use crussty_chunkgen::density::{Env, InterpState, Mode, Pt, WKind};
use std::collections::BTreeSet;

fn children_of(n: &WKind) -> Vec<u32> {
    match n {
        WKind::Const(_) => vec![],
        WKind::Noise { .. } | WKind::OldBlendedNoise { .. } => vec![],
        WKind::WeirdScaledSampler { .. } => vec![],
        WKind::EndIslands => vec![],
        WKind::ShiftedNoise { sx, sy, sz, .. } => vec![*sx, *sy, *sz],
        WKind::ShiftA { .. } | WKind::ShiftB { .. } => vec![],
        WKind::Clamp { input, .. } | WKind::Mapped { input, .. } | WKind::MulOrAdd { input, .. } => {
            vec![*input]
        }
        WKind::Cache2d { child }
        | WKind::CacheOnce { child }
        | WKind::FlatCache { child }
        | WKind::CacheAllInCell { child } => vec![*child],
        WKind::Interpolated { child, .. } => vec![*child],
        WKind::YClampedGradient { .. } => vec![],
        WKind::Spline { coordinate, .. } => vec![*coordinate],
        WKind::Ap2 { a, b, .. } => vec![*a, *b],
        WKind::RangeChoice { input, .. } => vec![*input],
        WKind::Gradient { .. } => vec![],
        WKind::Lerp { alpha, first, second } => vec![*alpha, *first, *second],
        WKind::IntervalSelect { input, functions, .. } => {
            let mut v = vec![*input];
            v.extend_from_slice(functions);
            v
        }
        WKind::FindTopSurface { density, upper_bound, .. } => vec![*density, *upper_bound],
        WKind::Pow { base, exponent } => vec![*base, *exponent],
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let px: i32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(52);
    let py: i32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(64);
    let pz: i32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(-32);
    let seed: i64 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(0);
    let env: &'static Env = Box::leak(Box::new(Env::new(seed)));
    let gen = crussty_chunkgen::nochunk::ChunkGen::new(env);
    let compiled = gen.compiled_pub();
    let children = gen.interp_children_pub().to_vec();
    let interp = InterpState::new(compiled.interp_slots, 4, 48);
    let pt = Pt { x: px, y: py, z: pz };
    let pts = [pt, pt, pt, pt];
    let child = children[0];

    // BFS from the root; report the first (shallowest) diverging node.
    let memo4 = crussty_chunkgen::density::V4Memo::new(compiled.arena.len());
    let mut frontier = vec![(child, 0u32)];
    let mut seen = BTreeSet::new();
    let mut found = 0;
    while let Some((idx, depth)) = frontier.pop() {
        if !seen.insert(idx) {
            continue;
        }
        let scalar = env.eval(compiled, idx, pt, Mode::Slice, &interp, None);
        let batch = env.eval4(compiled, idx, &pts, Mode::Slice, &interp, &memo4);
        if let WKind::Spline { coordinate, points } = &compiled.arena[idx as usize] {
            let f_scalar = env.eval(compiled, *coordinate, pt, Mode::Slice, &interp, None) as f32;
            let f4 = env.eval4(compiled, *coordinate, &pts, Mode::Slice, &interp, &memo4);
            println!(
                "spline idx={idx} depth={depth}: f_scalar={f_scalar:e} f_batch={:e} bits_eq={} npoints={}",
                f4[0] as f32, f_scalar.to_bits() == (f4[0] as f32).to_bits(), points.len()
            );
        }
        if scalar.to_bits() != batch[0].to_bits() {
            println!(
                "DIVERGE depth={depth} idx={idx} kind={:?} scalar={:e} batch={:e}",
                std::mem::discriminant(&compiled.arena[idx as usize]),
                scalar,
                batch[0]
            );
            found += 1;
            if found >= 60 {
                break;
            }
            // descend
            for c in children_of(&compiled.arena[idx as usize]) {
                frontier.push((c, depth + 1));
            }
        } else {
            for c in children_of(&compiled.arena[idx as usize]) {
                frontier.push((c, depth + 1));
            }
        }
    }
    if found == 0 {
        println!("no divergence found in tree walk?!");
    }
}
