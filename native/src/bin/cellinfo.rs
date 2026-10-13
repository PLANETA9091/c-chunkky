use crussty_chunkgen::density::{compile_cellfill, flatten_cellfill, Env, CellOp};
fn main() {
    let env = Env::new(3257840388504953787);
    let compiled = env.compile();
    let root = compiled.router["final_density"];
    let prog = compile_cellfill(&compiled.arena, root).expect("cellfill program");
    println!("CellOp count = {}", prog.len());
    let mut kinds = std::collections::BTreeMap::new();
    for op in &prog {
        let k = match op {
            CellOp::PushConst(_) => "PushConst",
            CellOp::Clamp { .. } => "Clamp",
            CellOp::Abs => "Abs",
            CellOp::Lerp3 { .. } => "Lerp3",
            _ => "other",
        };
        *kinds.entry(k).or_insert(0usize) += 1;
    }
    println!("{:?}", kinds);
    if let Some(flat) = flatten_cellfill(&prog) {
        println!("FlatProg ops = {}", flat.ops.len());
    }
    // count lerp3 slots
    let mut slots = std::collections::BTreeSet::new();
    for op in &prog {
        if let CellOp::Lerp3 { slot } = op { slots.insert(*slot); }
    }
    println!("lerp3 slots = {}", slots.len());
}
