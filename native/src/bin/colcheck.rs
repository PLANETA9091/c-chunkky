use crussty_chunkgen::density::{Env, WKind};
fn main() {
    let seed: i64 = 3257840388504953787;
    let env: &'static Env = Box::leak(Box::new(Env::new(seed)));
    let gen = crussty_chunkgen::nochunk::ChunkGen::new(env);
    let c = gen.compiled_pub();
    let child0 = c.interp_children[0];
    let mut stack = vec![child0];
    let mut seen = std::collections::HashSet::new();
    let mut slots = 0;
    while let Some(i) = stack.pop() {
        if !seen.insert(i) { continue; }
        if c.col_idx[i as usize] != u32::MAX { slots += 1; }
        match &c.arena[i as usize] {
            WKind::Cache2d { child, .. } | WKind::FlatCache { child, .. } => {
                println!("Cache2d/FlatCache node {} col_slot={:?}", i, c.col_idx[i as usize]);
                stack.push(*child);
            }
            WKind::Clamp { input, .. } | WKind::Mapped { input, .. } => stack.push(*input),
            WKind::Ap2 { a, b, .. } => { stack.push(*a); stack.push(*b); }
            WKind::MulOrAdd { input, .. } => stack.push(*input),
            WKind::WeirdScaledSampler { input, .. } => stack.push(*input),
            WKind::RangeChoice { input, when_in_range, when_out_of_range, .. } => {
                stack.push(*input); stack.push(*when_in_range); stack.push(*when_out_of_range);
            }
            WKind::ShiftedNoise { sx, sy, sz, .. } => { stack.push(*sx); stack.push(*sy); stack.push(*sz); }
            WKind::Spline { coordinate, points } => {
                stack.push(*coordinate);
                for (_, sv, _) in points { if let crussty_chunkgen::density::SplineValueW::Node(n) = sv { stack.push(*n); } }
            }
            _ => {}
        }
    }
    println!("reachable col slots from child0: {slots}");
}
