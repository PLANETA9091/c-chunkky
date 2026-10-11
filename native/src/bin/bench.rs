// bench: fill sanity + timing + density oracle eval mode.
//
// Usage:
//   bench fill [cx cz]           — generate one chunk, print stats + layers
//   bench perf [n]               — time n chunk fills
//   bench eval <file.tsv>        — evaluate final_density at (x,y,z) points:
//                                  lines "x y z" -> prints "x y z <f64 bits hex>"
//   bench router <name> <file.tsv> — evaluate a named router field
//   bench root <name> <file.tsv> — evaluate a named density_function root

use crussty_chunkgen::density::{Env, InterpState, Mode, Pt, WKind};
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
            // usage: bench <cmd> <field> <points.tsv> [seed]
            let field: String = args.get(2).cloned().unwrap_or_else(|| "final_density".to_string());
            let file = args.get(3).cloned().unwrap_or_default();
            let seed: i64 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(0);
            cmd_eval(&field, &file, seed);
        }
        "blendcalc" => {
            let file = args.get(2).cloned().unwrap_or_default();
            let seed: i64 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(0);
            cmd_blendcalc(&file, seed);
        }
        "blended" => {
            // usage: bench blended <seed> — dump terrain BlendedNoise state
            let seed: i64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
            cmd_blended(seed);
        }
        "walk" => {
            // usage: bench walk <misses.tsv> <seed> — java-compatible tree walk
            let file = args.get(2).cloned().unwrap_or_default();
            let seed: i64 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(0);
            cmd_walk(&file, seed);
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

fn cmd_eval(field: &str, file: &str, seed: i64) {
    eprintln!("DBG field={field} file={file} seed={seed}");
    let env = Env::new(seed);
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

fn cmd_walk(file: &str, seed: i64) {
    let env = Env::new(seed);
    let compiled = env.compile();
    let root = *compiled
        .router
        .get("final_density")
        .expect("final_density");
    let interp = InterpState::new(compiled.interp_slots, 4, 48);
    let data = std::fs::read_to_string(file).expect("misses file");
    let stdout = std::io::stdout();
    use std::io::Write;
    let mut out = stdout.lock();
    for line in data.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut it = line.split_whitespace();
        let x: i32 = it.next().unwrap().parse().unwrap();
        let y: i32 = it.next().unwrap().parse().unwrap();
        let z: i32 = it.next().unwrap().parse().unwrap();
        let pt = Pt { x, y, z };
        let v = env.eval(&compiled, root, pt, Mode::Direct, &interp);
        writeln!(out, "POINT {x} {y} {z} {:016x}", v.to_bits()).unwrap();
        walk_node(&env, &compiled, root, pt, "fd", &mut out, &interp);
    }
}

fn walk_node(
    env: &Env,
    c: &crussty_chunkgen::density::Compiled,
    idx: u32,
    pt: Pt,
    path: &str,
    out: &mut dyn std::io::Write,
    interp: &InterpState,
) {
    let v = env.eval(c, idx, pt, Mode::Direct, interp);
    let bits = v.to_bits();
    match &c.arena[idx as usize] {
        WKind::Cache2d { child } => {
            writeln!(out, "{path} [wrapper Cache2D] {bits:016x}").unwrap();
            walk_node(env, c, *child, pt, &format!("{path}.w"), out, interp);
        }
        WKind::CacheOnce { child } => {
            writeln!(out, "{path} [wrapper CacheOnce] {bits:016x}").unwrap();
            walk_node(env, c, *child, pt, &format!("{path}.w"), out, interp);
        }
        WKind::FlatCache { child } => {
            writeln!(out, "{path} [wrapper FlatCache] {bits:016x}").unwrap();
            walk_node(env, c, *child, pt, &format!("{path}.w"), out, interp);
        }
        WKind::CacheAllInCell { child } => {
            writeln!(out, "{path} [wrapper CacheAllInCell] {bits:016x}").unwrap();
            walk_node(env, c, *child, pt, &format!("{path}.w"), out, interp);
        }
        WKind::Interpolated { child, .. } => {
            writeln!(out, "{path} [wrapper NoiseInterpolator] {bits:016x}").unwrap();
            walk_node(env, c, *child, pt, &format!("{path}.w"), out, interp);
        }
        WKind::MulOrAdd { is_add, input, argument } => {
            let kind = if *is_add { "MulOrAdd" } else { "MulOrAdd" };
            writeln!(out, "{path} {kind} {bits:016x}").unwrap();
            // argument1() = Constant(argument) — java walks it as .a
            let cbits = argument.to_bits();
            writeln!(out, "{path}.a Constant {cbits:016x}").unwrap();
            walk_node(env, c, *input, pt, &format!("{path}.b"), out, interp);
        }
        WKind::Ap2 { a, b, .. } => {
            writeln!(out, "{path} Ap2 {bits:016x}").unwrap();
            walk_node(env, c, *a, pt, &format!("{path}.a"), out, interp);
            walk_node(env, c, *b, pt, &format!("{path}.b"), out, interp);
        }
        WKind::Clamp { .. } | WKind::Mapped { .. } | WKind::Noise { .. } => {
            writeln!(out, "{path} {} {bits:016x}", c.arena[idx as usize].java_kind()).unwrap();
            if let Some(child) = c.arena[idx as usize].child_of() {
                let suffix = match c.arena[idx as usize] {
                    WKind::Mapped { .. } => ".i",
                    _ => ".a",
                };
                walk_node(env, c, child, pt, &format!("{path}{suffix}"), out, interp);
            }
        }
        WKind::ShiftedNoise { sx, sy, sz, .. } => {
            writeln!(out, "{path} ShiftedNoise {bits:016x}").unwrap();
            walk_node(env, c, *sx, pt, &format!("{path}.sx"), out, interp);
            walk_node(env, c, *sy, pt, &format!("{path}.sy"), out, interp);
            walk_node(env, c, *sz, pt, &format!("{path}.sz"), out, interp);
        }
        WKind::RangeChoice { input, when_in_range, when_out_of_range, .. } => {
            writeln!(out, "{path} RangeChoice {bits:016x}").unwrap();
            walk_node(env, c, *input, pt, &format!("{path}.i"), out, interp);
            walk_node(env, c, *when_in_range, pt, &format!("{path}.in"), out, interp);
            walk_node(env, c, *when_out_of_range, pt, &format!("{path}.out"), out, interp);
        }
        WKind::Spline { .. } => {
            writeln!(out, "{path} Spline {bits:016x}").unwrap();
        }
        WKind::WeirdScaledSampler { input, .. } => {
            writeln!(out, "{path} WeirdScaledSampler {bits:016x}").unwrap();
            walk_node(env, c, *input, pt, &format!("{path}.a"), out, interp);
        }
        WKind::ShiftA { .. } | WKind::ShiftB { .. } | WKind::OldBlendedNoise { .. } => {
            writeln!(out, "{path} {} {bits:016x}", c.arena[idx as usize].java_kind()).unwrap();
        }
        WKind::Const(v) => {
            writeln!(out, "{path} Constant {:016x}", v.to_bits()).unwrap();
        }
        WKind::EndIslands => {
            writeln!(out, "{path} EndIslandDensityFunction {bits:016x}").unwrap();
        }
        WKind::YClampedGradient { .. } => {
            writeln!(out, "{path} YClampedGradient {bits:016x}").unwrap();
        }
    }
}

fn cmd_blended(seed: i64) {
    use crussty_chunkgen::noise::BlendedNoise;
    use crussty_chunkgen::rng::{XoroshiroRandomSource, XoroshiroPositionalRandomFactory};
    let mut root = XoroshiroRandomSource::new(seed);
    let root_factory: XoroshiroPositionalRandomFactory = root.fork_positional();
    let mut terrain_src = root_factory.from_hash_of("minecraft:terrain");
    let blended = BlendedNoise::create(&mut terrain_src, 0.25, 0.125, 80.0, 160.0, 8.0);
    for (which, pn) in [
        ("minLimitNoise", &blended.min_noise),
        ("maxLimitNoise", &blended.max_noise),
        ("mainNoise", &blended.main_noise),
    ] {
        for (i, oct) in pn.noise_levels.iter().enumerate() {
            match oct {
                None => println!("B {which} {i} null"),
                Some(o) => {
                    let mut hex = String::with_capacity(512);
                    for b in o.p.iter() {
                        hex.push_str(&format!("{b:02x}"));
                    }
                    println!(
                        "B {which} {i} xo={:016x} yo={:016x} zo={:016x} p={hex}",
                        o.xo.to_bits(),
                        o.yo.to_bits(),
                        o.zo.to_bits()
                    );
                }
            }
        }
    }
}

fn cmd_blendcalc(file: &str, seed: i64) {
    use crussty_chunkgen::noise::{BlendedNoise, PerlinNoise};
    use crussty_chunkgen::rng::{XoroshiroRandomSource, XoroshiroPositionalRandomFactory};
    let mut root = XoroshiroRandomSource::new(seed);
    let root_factory: XoroshiroPositionalRandomFactory = root.fork_positional();
    let mut terrain_src = root_factory.from_hash_of("minecraft:terrain");
    let b = BlendedNoise::create(&mut terrain_src, 0.25, 0.125, 80.0, 160.0, 8.0);
    let data = std::fs::read_to_string(file).expect("misses file");
    let stdout = std::io::stdout();
    use std::io::Write;
    let mut out = stdout.lock();
    for line in data.lines() {
        if line.trim().is_empty() { continue; }
        let mut it = line.split_whitespace();
        let bx: i32 = it.next().unwrap().parse().unwrap();
        let by: i32 = it.next().unwrap().parse().unwrap();
        let bz: i32 = it.next().unwrap().parse().unwrap();
        writeln!(out, "PT {bx} {by} {bz}").unwrap();
        trace_blend(&b, bx, by, bz, &mut out);
    }

    fn wrap(value: f64) -> f64 {
        let l = (value / 3.3554432E7 + 0.5).floor() as i64; // Mth.lfloor
        value - l as f64 * 3.3554432E7
    }

    fn trace_blend(b: &BlendedNoise, block_x: i32, block_y: i32, block_z: i32, out: &mut dyn Write) {
        let d = block_x as f64 * b.xz_multiplier;
        let d1 = block_y as f64 * b.y_multiplier;
        let d2 = block_z as f64 * b.xz_multiplier;
        let d3 = d / b.xz_factor;
        let d4 = d1 / b.y_factor;
        let d5 = d2 / b.xz_factor;
        let d6 = b.y_multiplier * b.smear_scale_multiplier;
        let d7 = d6 / b.y_factor;
        writeln!(out, "d {:016x} d1 {:016x} d2 {:016x} d3 {:016x} d4 {:016x} d5 {:016x} d6 {:016x} d7 {:016x}",
            d.to_bits(), d1.to_bits(), d2.to_bits(), d3.to_bits(), d4.to_bits(), d5.to_bits(), d6.to_bits(), d7.to_bits()).unwrap();
        let mut d8 = 0.0; let mut d9 = 0.0; let mut d10 = 0.0; let mut d11 = 1.0;
        for i in 0..8 {
            if let Some(oct) = b.main_noise.get_octave_noise(i) {
                let v = oct.noise_5(wrap(d3 * d11), wrap(d4 * d11), wrap(d5 * d11), d7 * d11, d4 * d11);
                d10 += v / d11;
                writeln!(out, "main {i} {:016x}", v.to_bits()).unwrap();
            } else {
                writeln!(out, "main {i} null").unwrap();
            }
            d11 /= 2.0;
        }
        let d12 = (d10 / 10.0 + 1.0) / 2.0;
        let flag1 = d12 >= 1.0;
        let flag2 = d12 <= 0.0;
        writeln!(out, "d10 {:016x} d12 {:016x} flag1 {} flag2 {}", d10.to_bits(), d12.to_bits(), flag1 as u8, flag2 as u8).unwrap();
        d11 = 1.0;
        for i1 in 0..16 {
            let d13 = wrap(d * d11);
            let d14 = wrap(d1 * d11);
            let d15 = wrap(d2 * d11);
            let d16 = d6 * d11;
            if !flag1 {
                if let Some(oct) = b.min_noise.get_octave_noise(i1) {
                    let v = oct.noise_5(d13, d14, d15, d16, d1 * d11);
                    d8 += v / d11;
                    writeln!(out, "min {i1} {:016x}", v.to_bits()).unwrap();
                } else {
                    writeln!(out, "min {i1} null").unwrap();
                }
            }
            if !flag2 {
                if let Some(oct) = b.max_noise.get_octave_noise(i1) {
                    let v = oct.noise_5(d13, d14, d15, d16, d1 * d11);
                    d9 += v / d11;
                    writeln!(out, "max {i1} {:016x}", v.to_bits()).unwrap();
                } else {
                    writeln!(out, "max {i1} null").unwrap();
                }
            }
            d11 /= 2.0;
        }
        let t = d12;
        let (a, bb) = (d8 / 512.0, d9 / 512.0);
        let v = if t < 0.0 { a } else if t > 1.0 { bb } else { a + t * (bb - a) };
        writeln!(out, "d8 {:016x} d9 {:016x} result {:016x}", d8.to_bits(), d9.to_bits(), (v / 128.0).to_bits()).unwrap();
    }
}
