use crussty_chunkgen::density::{Env, YUpperConst};
fn main() {
    let seed: i64 = 3257840388504953787;
    let env: &'static Env = Box::leak(Box::new(Env::new(seed)));
    let gen = crussty_chunkgen::nochunk::ChunkGen::new(env);
    let c = &gen.compiled;
    for (slot, &child) in c.interp_children.iter().enumerate() {
        let yc = c.interp_slot_y_const.get(slot).copied().flatten();
        let up = c.node_y_upper_const.get(child as usize).copied().flatten();
        println!(
            "slot {slot}: child {child} y_const={:?} upper={:?}",
            yc,
            up.map(|(y0, k)| match k {
                YUpperConst::Val(v) => format!("Val({v}) @y>={y0}"),
                YUpperConst::ZeroU => format!("ZeroU @y>={y0}"),
            })
        );
    }
    // kinds along slot0 subtree
    let mut stack = vec![c.interp_children[0]];
    let mut seen = std::collections::HashSet::new();
    while let Some(n) = stack.pop() {
        if !seen.insert(n) { continue; }
        let kind = match &c.arena[n as usize] {
            crussty_chunkgen::density::WKind::Const(v) => format!("Const({v})"),
            crussty_chunkgen::density::WKind::YClampedGradient { from_y, to_y, from_value, to_value } => format!("YGrad({from_y}->{to_y}, {from_value}->{to_value})"),
            crussty_chunkgen::density::WKind::Ap2 { kind, a, b } => { stack.push(*a); stack.push(*b); format!("Ap2 <- {a},{b}") }
            crussty_chunkgen::density::WKind::RangeChoice { input, min_inclusive, max_exclusive, .. } => { stack.push(*input); format!("RC[{min_inclusive},{max_exclusive}) in={input}") }
            other => format!("other@{n}"),
        };
        println!("  node {n}: {kind}");
    }
}
