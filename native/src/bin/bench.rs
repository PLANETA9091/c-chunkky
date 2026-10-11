// bench: fill sanity + timing + density oracle eval mode.
//
// Usage:
//   bench fill [cx cz]           — generate one chunk, print stats + layers
//   bench perf [n]               — time n chunk fills
//   bench eval <file.tsv>        — evaluate final_density at (x,y,z) points:
//                                  lines "x y z" -> prints "x y z <f64 bits hex>"
//   bench router <name> <file.tsv> — evaluate a named router field
//   bench root <name> <file.tsv> — evaluate a named density_function root

use crussty_chunkgen::density::{Env, InterpState, Mode, Pt};
use crussty_chunkgen::nochunk::{ChunkGen, OUT_LEN};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let cmd = args.get(1).map(|s| s.as_str()).unwrap_or("fill");
    match cmd {
        "fill" => {
            let cx = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
            let cz = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(0);
            cmd_fill(cx, cz);
        }
        "perf" => {
            let n = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(64);
            cmd_perf(n);
        }
        "eval" | "router" | "root" => {
            let field: String = if cmd == "eval" {
                "final_density".to_string()
            } else {
                args.get(2).cloned().unwrap_or_default()
            };
            let file = if cmd == "eval" {
                args.get(2).cloned().unwrap_or_default()
            } else {
                args.get(3).cloned().unwrap_or_default()
            };
            cmd_eval(&field, &file);
        }
        other => {
            eprintln!("unknown command: {other}");
            std::process::exit(2);
        }
    }
}

fn cmd_fill(cx: i32, cz: i32) {
    let env = Env::new(0);
    let mut gen = ChunkGen::new(&env);
    let mut out = [0u16; OUT_LEN];
    let t0 = std::time::Instant::now();
    gen.fill(cx, cz, &mut out);
    let dt = t0.elapsed();
    println!("fill cx={cx} cz={cz} took {:?}", dt);

    // per-block-id histogram
    let mut hist: std::collections::BTreeMap<u16, usize> = Default::default();
    for &b in out.iter() {
        *hist.entry(b).or_default() += 1;
    }
    for (id, n) in hist {
        println!("  id {:3} -> {}", id, n);
    }

    // rough layer snapshot every 32 blocks (counts per layer)
    for y in (-64..320).step_by(32) {
        let base = ((y + 64) * 256) as usize;
        let mut counts: std::collections::BTreeMap<u16, usize> = Default::default();
        for &b in &out[base..base + 256] {
            *counts.entry(b).or_default() += 1;
        }
        let summary: Vec<String> =
            counts.iter().map(|(k, v)| format!("{}:{}", k, v)).collect();
        println!("  y={y:4}: {}", summary.join(" "));
    }
}

fn cmd_perf(n: usize) {
    let env = Env::new(0);
    let mut gen = ChunkGen::new(&env);
    let mut out = [0u16; OUT_LEN];
    // warmup
    for i in 0..8 {
        gen.fill(i as i32, 0, &mut out);
    }
    let mut times = Vec::with_capacity(n);
    let t_all = std::time::Instant::now();
    for i in 0..n {
        let t0 = std::time::Instant::now();
        gen.fill(i as i32, 1, &mut out);
        times.push(t0.elapsed().as_secs_f64() * 1000.0);
    }
    let total = t_all.elapsed().as_secs_f64() * 1000.0;
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mean = times.iter().sum::<f64>() / times.len() as f64;
    let p50 = times[times.len() / 2];
    let p99 = times[(times.len() as f64 * 0.99) as usize % times.len()];
    println!(
        "n={} mean={:.3}ms p50={:.3}ms p99={:.3}ms max={:.3}ms total={:.1}ms",
        n,
        mean,
        p50,
        p99,
        times[times.len() - 1],
        total
    );
}

fn cmd_eval(field: &str, file: &str) {
    let env = Env::new(0);
    let compiled = env.compile();
    let root = *compiled
        .router
        .get(field)
        .unwrap_or_else(|| panic!("router field '{field}' missing"));
    let interp = InterpState::new(compiled.interp_slots, 4, 48);
    let data = std::fs::read_to_string(file).expect("points file");
    let stdout = std::io::stdout();
    use std::io::Write;
    let mut out = stdout.lock();
    for line in data.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut it = line.split_whitespace();
        let x: i32 = it.next().unwrap().parse().unwrap();
        let y: i32 = it.next().unwrap().parse().unwrap();
        let z: i32 = it.next().unwrap().parse().unwrap();
        let v = env.eval(&compiled, root, Pt { x, y, z }, Mode::Direct, &interp);
        writeln!(out, "{} {} {} {:016x}", x, y, z, v.to_bits()).unwrap();
    }
}
