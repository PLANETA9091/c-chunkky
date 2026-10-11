// Quick probe: what does the compiled cellfill program look like for the
// vanilla overworld (length, op mix)?
use crussty_chunkgen::density::{compile_cellfill, Env};

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
}
