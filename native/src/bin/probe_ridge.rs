use crussty_chunkgen::density::{Env, WKind};
fn describe(compiled: &crussty_chunkgen::density::Compiled, idx: u32, depth: usize) {
    let pad = "  ".repeat(depth);
    match &compiled.arena[idx as usize] {
        WKind::Const(v) => println!("{pad}Const({v})"),
        WKind::Noise { noise_id, xz_scale, y_scale } => println!("{pad}Noise id={noise_id} xz={xz_scale} y={y_scale}"),
        WKind::Interpolated { child, slot } => { println!("{pad}Interpolated slot={slot}"); describe(compiled, *child, depth + 1); }
        WKind::Mapped { kind, input } => { println!("{pad}Mapped kind={}", *kind as i32); describe(compiled, *input, depth + 1); }
        WKind::Clamp { input, min, max } => { println!("{pad}Clamp [{min}, {max}]"); describe(compiled, *input, depth + 1); }
        WKind::MulOrAdd { is_add, input, argument } => { println!("{pad}MulOrAdd add={is_add} arg={argument}"); describe(compiled, *input, depth + 1); }
        WKind::Cache2d { child } => { println!("{pad}Cache2d"); describe(compiled, *child, depth + 1); }
        WKind::CacheOnce { child } => { println!("{pad}CacheOnce"); describe(compiled, *child, depth + 1); }
        WKind::FlatCache { child } => { println!("{pad}FlatCache"); describe(compiled, *child, depth + 1); }
        WKind::CacheAllInCell { child } => { println!("{pad}CacheAllInCell"); describe(compiled, *child, depth + 1); }
        WKind::YClampedGradient { from_y, to_y, from_value, to_value } => println!("{pad}YGrad [{from_y},{to_y}] -> [{from_value},{to_value}]"),
        WKind::Ap2 { kind, a, b } => { println!("{pad}Ap2 kind={}", *kind as i32); describe(compiled, *a, depth + 1); describe(compiled, *b, depth + 1); }
        WKind::RangeChoice { input, min_inclusive, max_exclusive, when_in_range, when_out_of_range } => {
            println!("{pad}RangeChoice [{min_inclusive},{max_exclusive})");
            describe(compiled, *input, depth + 1);
            describe(compiled, *when_in_range, depth + 1);
            describe(compiled, *when_out_of_range, depth + 1);
        }
        _ => println!("{pad}OTHER"),
    }
}
fn main() {
    let seed: i64 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(3257840388504953787);
    let env = Env::new(seed);
    let compiled = env.compile();
    for name in ["vein_toggle", "vein_ridged", "vein_gap"] {
        println!("== {name}");
        describe(&compiled, compiled.router[name], 1);
    }
}
