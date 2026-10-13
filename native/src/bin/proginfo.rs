// Quick probe: what does the compiled cellfill program look like for the
// vanilla overworld (length, op mix)?
use crussty_chunkgen::density::{compile_cellfill, flatten_cellfill, Env};

fn main() {
    let seed: i64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(3257840388504953787);
    let env = Env::new(seed);
    let compiled = env.compile();
    let root = compiled.router["final_density"];
    match compile_cellfill(&compiled.arena, root) {
        Some(prog) => {
            println!("cellfill program: {} ops", prog.len());
            let mut counts = std::collections::BTreeMap::new();
            for op in &prog {
                let name = format!("{op:?}").split(['(', ' ', '{']).next().unwrap().to_string();
                *counts.entry(name).or_insert(0usize) += 1;
            }
            for (k, v) in counts {
                println!("  {k}: {v}");
            }
        }
        None => println!("cellfill: None (fallback interpreter)"),
    }
    // flat program stats
    if let Some(prog) = compile_cellfill(&compiled.arena, root) {
        match flatten_cellfill(&prog) {
            Some(flat) => println!(
                "flat: {} ops, result slot {}, OK",
                flat.ops.len(),
                flat.result
            ),
            None => println!("flat: None (stack too deep)"),
        }
    }
    // column-cache stats: how many Cache2d/FlatCache nodes got slots
    let assigned = compiled.col_idx.iter().filter(|&&c| c != u32::MAX).count();
    println!(
        "col-cache: {} slots / {} cache2d+flatcache candidates, arena {} nodes",
        compiled.col_count,
        assigned,
        compiled.arena.len()
    );
}
// (appended below in main — see edit)
