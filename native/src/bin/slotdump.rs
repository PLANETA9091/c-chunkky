// slotdump: print the subtree of each Interpolated child slot with node
// kinds, noise y_scale/xz_scale and column-cache verdicts — diagnostics for
// the perf sprint (which NormalNoise calls are y-invariant and why they are
// not column-cached).
use crussty_chunkgen::density::{Env, WKind};

fn main() {
    let seed: i64 = 3257840388504953787;
    let env = Env::new(seed);
    let compiled = env.compile();
    let mut shown = std::collections::HashSet::new();
    for (slot, &child) in compiled.interp_children.iter().enumerate() {
        println!("=== slot {slot} (child {})", child);
        dump(&compiled, child, 1, &mut shown);
    }
    // summary: how many arena nodes carry a col slot
    let mut cols = 0;
    for (i, ci) in compiled.col_idx.iter().enumerate() {
        if *ci != u32::MAX {
            cols += 1;
            let _ = i;
        }
    }
    println!("total col-cache nodes: {cols} of {}", compiled.col_idx.len());
}

fn indent(d: usize) -> String {
    "  ".repeat(d)
}

fn dump(c: &crussty_chunkgen::density::Compiled, idx: u32, d: usize, shown: &mut std::collections::HashSet<u32>) {
    if d > 4 || !shown.insert(idx) {
        println!("{}#{} (seen)", indent(d), idx);
        return;
    }
    let col = c.col_idx[idx as usize];
    let coltag = if col == u32::MAX { "" } else { " [COL]" };
    match &c.arena[idx as usize] {
        WKind::Const(v) => println!("{}{idx} Const {v}{coltag}", indent(d)),
        WKind::Noise { noise_id, xz_scale, y_scale } => {
            let nid = c.noise_resolved[*noise_id as usize];
            let name = c.noise_ids.get(*noise_id as usize).cloned().unwrap_or_default();
            println!(
                "{}{idx} Noise {}(inst {nid}) xz={xz_scale} y={y_scale}{coltag}",
                indent(d),
                name
            );
        }
        WKind::ShiftedNoise { noise_id, xz_scale, y_scale, sx, sy, sz } => {
            println!("{}{idx} ShiftedNoise xz={xz_scale} y={y_scale}{coltag}", indent(d));
            dump(c, *sx, d + 1, shown);
            dump(c, *sy, d + 1, shown);
            dump(c, *sz, d + 1, shown);
        }
        WKind::ShiftA { noise_id } => {
            println!("{}{idx} ShiftA{coltag}", indent(d));
            let _ = noise_id;
        }
        WKind::ShiftB { noise_id } => {
            println!("{}{idx} ShiftB{coltag}", indent(d));
            let _ = noise_id;
        }
        WKind::OldBlendedNoise { instance } => {
            println!("{}{idx} OldBlended inst={instance}{coltag}", indent(d));
        }
        WKind::Clamp { input, min, max } => {
            println!("{}{idx} Clamp [{min},{max}]{coltag}", indent(d));
            dump(c, *input, d + 1, shown);
        }
        WKind::Mapped { kind, input } => {
            let kn = match kind {
                crussty_chunkgen::density::MapKind::Abs => "Abs",
                crussty_chunkgen::density::MapKind::Square => "Square",
                crussty_chunkgen::density::MapKind::Cube => "Cube",
                crussty_chunkgen::density::MapKind::HalfNegative => "HalfNeg",
                crussty_chunkgen::density::MapKind::QuarterNegative => "QuarterNeg",
                crussty_chunkgen::density::MapKind::Squeeze => "Squeeze",
                crussty_chunkgen::density::MapKind::Negate => "Negate",
                crussty_chunkgen::density::MapKind::Reciprocal => "Reciprocal",
                crussty_chunkgen::density::MapKind::Log => "Log",
                crussty_chunkgen::density::MapKind::Sign => "Sign",
                crussty_chunkgen::density::MapKind::Round => "Round",
            };
            println!("{}{idx} Mapped {kn}{coltag}", indent(d));
            dump(c, *input, d + 1, shown);
        }
        WKind::Ap2 { kind, a, b } => {
            let kn = match kind {
                crussty_chunkgen::density::Ap2Kind::Add => "Add",
                crussty_chunkgen::density::Ap2Kind::Mul => "Mul",
                crussty_chunkgen::density::Ap2Kind::Min => "Min",
                crussty_chunkgen::density::Ap2Kind::Max => "Max",
                crussty_chunkgen::density::Ap2Kind::Div => "Div",
                crussty_chunkgen::density::Ap2Kind::Sub => "Sub",
            };
            println!("{}{idx} Ap2 {kn}{coltag}", indent(d));
            dump(c, *a, d + 1, shown);
            dump(c, *b, d + 1, shown);
        }
        WKind::RangeChoice { input, min_inclusive, max_exclusive, when_in_range, when_out_of_range } => {
            println!(
                "{}{idx} RangeChoice [{min_inclusive},{max_exclusive}){coltag}",
                indent(d)
            );
            dump(c, *input, d + 1, shown);
            dump(c, *when_in_range, d + 1, shown);
            dump(c, *when_out_of_range, d + 1, shown);
        }
        _ => println!("{}{idx} (other kind){coltag}", indent(d)),
    }
}
