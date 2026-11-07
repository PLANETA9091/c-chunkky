//! cont-125 probe: census the compiled 26.3 final_density tree —
//! which DfSam263 kinds exist, how many shell-ops, how many are
//! constant-foldable subtrees (bit-safe precompute) and how many
//! Gradient nodes are y-only (per-row hoist candidates).
use crussty_chunkgen::df263::{DfSam263, SamRef};
use crussty_chunkgen::fill263::Gen263;
use std::collections::BTreeMap;
use std::sync::Arc;

fn kind_of(n: &DfSam263) -> &'static str {
    match n {
        DfSam263::Constant(_) => "Constant",
        DfSam263::Noise { .. } => "Noise",
        DfSam263::ShiftedXz { .. } => "ShiftedXz",
        DfSam263::Binary { .. } => "Binary",
        DfSam263::ConstAdd { .. } => "ConstAdd",
        DfSam263::ConstSub { .. } => "ConstSub",
        DfSam263::ConstMul { .. } => "ConstMul",
        DfSam263::ConstDiv { .. } => "ConstDiv",
        DfSam263::ConstMin { .. } => "ConstMin",
        DfSam263::ConstMax { .. } => "ConstMax",
        DfSam263::Unary { .. } => "Unary",
        DfSam263::Clamp { .. } => "Clamp",
        DfSam263::Lerp { .. } => "Lerp",
        DfSam263::LerpConstFirst { .. } => "LerpConstFirst",
        DfSam263::LerpConstSecond { .. } => "LerpConstSecond",
        DfSam263::PowConstBase { .. } => "PowConstBase",
        DfSam263::PowConstExp { .. } => "PowConstExp",
        DfSam263::Pow { .. } => "Pow",
        DfSam263::RoundInt { .. } => "RoundInt",
        DfSam263::Round { .. } => "Round",
        DfSam263::RangeChoice { .. } => "RangeChoice",
        DfSam263::RangeChoiceConst { .. } => "RangeChoiceConst",
        DfSam263::IntervalSelect { .. } => "IntervalSelect",
        DfSam263::Cache { .. } => "Cache",
        DfSam263::SliceX { .. } => "SliceX",
        DfSam263::SliceY { .. } => "SliceY",
        DfSam263::SliceZ { .. } => "SliceZ",
        DfSam263::SliceXz { .. } => "SliceXz",
        DfSam263::Gradient { .. } => "Gradient",
        DfSam263::Interpolated { .. } => "Interpolated",
        DfSam263::FindTopSurface { .. } => "FindTopSurface",
        DfSam263::BlendDensity { .. } => "BlendDensity",
        DfSam263::ShiftB { .. } => "ShiftB",
        DfSam263::Blended { .. } => "Blended",
        DfSam263::Beard => "Beard",
        DfSam263::Spline { .. } => "Spline",
    }
}

/// Children in DFS order (matches the fuse compiler's walking order).
fn children(n: &DfSam263) -> Vec<SamRef> {
    use crussty_chunkgen::df263::{BinOp, UnOp};
    let _ = std::marker::PhantomData::<BinOp>;
    let _ = std::marker::PhantomData::<UnOp>;
    let mut v: Vec<SamRef> = Vec::new();
    match n {
        DfSam263::Constant(_)
        | DfSam263::Beard
        | DfSam263::ShiftB { .. } => {}
        DfSam263::Noise { .. }
        | DfSam263::Blended { .. } => {}
        DfSam263::ShiftedXz { shift_x, shift_z, .. } => {
            v.push(shift_x.clone());
            v.push(shift_z.clone());
        }
        DfSam263::Binary { l, r, .. } => {
            v.push(l.clone());
            v.push(r.clone());
        }
        DfSam263::ConstAdd { a, .. }
        | DfSam263::ConstSub { a, .. }
        | DfSam263::ConstMul { a, .. }
        | DfSam263::ConstDiv { a, .. }
        | DfSam263::ConstMin { a, .. }
        | DfSam263::ConstMax { a, .. }
        | DfSam263::Unary { a, .. }
        | DfSam263::Clamp { a, .. }
        | DfSam263::RoundInt { a, .. }
        | DfSam263::Cache { a, .. }
        | DfSam263::SliceX { a, .. }
        | DfSam263::SliceY { a, .. }
        | DfSam263::SliceZ { a, .. }
        | DfSam263::SliceXz { a, .. }
        | DfSam263::BlendDensity { a } => v.push(a.clone()),
        DfSam263::Round { a, mul, .. } => {
            v.push(a.clone());
            v.push(mul.clone());
        }
        DfSam263::Lerp { alpha, first, second } => {
            v.push(alpha.clone());
            v.push(first.clone());
            v.push(second.clone());
        }
        DfSam263::LerpConstFirst { alpha, second, .. } => {
            v.push(alpha.clone());
            v.push(second.clone());
        }
        DfSam263::LerpConstSecond { alpha, first, .. } => {
            v.push(alpha.clone());
            v.push(first.clone());
        }
        DfSam263::PowConstBase { exp, .. } => v.push(exp.clone()),
        DfSam263::PowConstExp { a, .. } => v.push(a.clone()),
        DfSam263::Pow { a, b } => {
            v.push(a.clone());
            v.push(b.clone());
        }
        DfSam263::RangeChoice { a, in_range, out_of_range, .. } => {
            v.push(a.clone());
            v.push(in_range.clone());
            v.push(out_of_range.clone());
        }
        DfSam263::RangeChoiceConst { a, .. } => v.push(a.clone()),
        DfSam263::IntervalSelect { a, thresholds: _, branches } => {
            v.push(a.clone());
            for b in branches {
                v.push(b.clone());
            }
        }
        DfSam263::Gradient { .. } | DfSam263::FindTopSurface { .. } => {}
        DfSam263::Interpolated { a, .. } => v.push(a.clone()),
        DfSam263::Spline { coord, values, .. } => {
            v.push(coord.clone());
            for val in values {
                collect_spline_coords(val, &mut v);
            }
        }
    }
    v
}

fn collect_spline_coords(v: &crussty_chunkgen::df263::SplineRef, out: &mut Vec<SamRef>) {
    use crussty_chunkgen::df263::SplineRef;
    match v {
        SplineRef::Constant(_) => {}
        SplineRef::Multi { coord, values, .. } => {
            out.push(coord.clone());
            for c in values {
                collect_spline_coords(c, out);
            }
        }
    }
}

fn main() {
    let seed = 3257840388504953787i64;
    let g = Gen263::new(seed, -64, 384, 63);
    let root = g.final_density.clone();

    let mut counts: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut const_only = 0usize; // foldable subtrees (const-propagates to a Constant)
    let mut total = 0usize;
    let mut grad_y = 0usize;
    let mut grad_axis = 0usize;

    fn is_const(n: &SamRef) -> bool {
        matches!(&**n, DfSam263::Constant(_))
            || (match &**n {
                DfSam263::Binary { l, r, .. } => is_const(l) && is_const(r),
                DfSam263::ConstAdd { a, .. }
                | DfSam263::ConstSub { a, .. }
                | DfSam263::ConstMul { a, .. }
                | DfSam263::ConstDiv { a, .. }
                | DfSam263::ConstMin { a, .. }
                | DfSam263::ConstMax { a, .. }
                | DfSam263::Unary { a, .. }
                | DfSam263::Clamp { a, .. } => is_const(a),
                DfSam263::Lerp { alpha, first, second } => {
                    is_const(alpha) && is_const(first) && is_const(second)
                }
                DfSam263::LerpConstFirst { alpha, second, .. } => {
                    is_const(alpha) && is_const(second)
                }
                DfSam263::LerpConstSecond { alpha, first, .. } => {
                    is_const(alpha) && is_const(first)
                }
                _ => false,
            })
    }

    // iterative DFS with dedup on Arc pointer identity (DAG!)
    let mut seen: std::collections::HashSet<usize> = std::collections::HashSet::new();
    let mut stack: Vec<SamRef> = vec![root.clone()];
    while let Some(n) = stack.pop() {
        let key = Arc::as_ptr(&n) as usize;
        if !seen.insert(key) {
            continue;
        }
        total += 1;
        *counts.entry(kind_of(&n)).or_insert(0) += 1;
        match &*n {
            DfSam263::Gradient { axis, .. } => {
                grad_axis += 1;
                if format!("{axis:?}").contains('Y') {
                    grad_y += 1;
                }
            }
            _ => {}
        }
        if is_const(&n) {
            const_only += 1;
        }
        for c in children(&n) {
            stack.push(c);
        }
    }

    println!("tree nodes (dedup by ptr): {total}");
    for (k, v) in &counts {
        println!("  {k}: {v}");
    }
    println!("const-only foldable nodes: {const_only}");
    println!("Gradient nodes: {grad_axis} (y-axis: {grad_y})");

    // Reference weights: each unique node's DAG-path count from the root =
    // how many walk visits it receives per evaluation (memoized path count).
    // paths(n) = 1 + sum(children paths(c)); diamonds multiply.
    let mut path_memo: std::collections::HashMap<usize, u64> = std::collections::HashMap::new();
    let mut node_kinds: std::collections::HashMap<usize, &'static str> =
        std::collections::HashMap::new();
    fn paths(
        n: &SamRef,
        memo: &mut std::collections::HashMap<usize, u64>,
        kinds: &mut std::collections::HashMap<usize, &'static str>,
    ) -> u64 {
        let key = Arc::as_ptr(n) as usize;
        if let Some(&p) = memo.get(&key) {
            return p;
        }
        kinds.insert(key, kind_of(&n));
        let mut p = 1u64;
        for c in children(n) {
            p = p.saturating_add(paths(&c, memo, kinds));
        }
        memo.insert(key, p);
        p
    }
    let total_paths = paths(&root, &mut path_memo, &mut node_kinds);
    println!("total walk-path length (DAG refs per eval): {total_paths}");

    let mut by_weight: Vec<(u64, &'static str)> = path_memo
        .iter()
        .map(|(k, p)| (*p, node_kinds[k]))
        .collect();
    by_weight.sort_unstable_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(b.1)));
    let mut agg: BTreeMap<&'static str, u64> = BTreeMap::new();
    for (p, k) in &by_weight {
        *agg.entry(k).or_insert(0) += p;
    }
    println!("walk visits per kind (sum of DAG path counts):");
    for (k, v) in &agg {
        println!("  {k}: {v}");
    }
}
