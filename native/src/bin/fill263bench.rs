//! fill263 stage-level bench: aquifer ctor / final_density volume sample /
//! substance loop / full fill_chunk, median over rounds.

use crussty_chunkgen::aquifer263::Aquifer263;
use crussty_chunkgen::df263::{DensityVolume, SamplerCtx263};
use crussty_chunkgen::fill263::Gen263;
use crussty_chunkgen::nochunk::OUT_LEN;

fn median(v: &mut [f64]) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

fn main() {
    let seed = 3257840388504953787i64;
    let g = Gen263::new(seed, -64, 384, 63);
    let chunks: Vec<(i32, i32)> = (0..5).flat_map(|cx| (0..5).map(move |cz| (cx, cz))).collect();
    let rounds = 7;

    let mut t_fill = Vec::new();
    let mut t_aq = Vec::new();
    let mut t_vol = Vec::new();
    let mut t_sub = Vec::new();
    let mut t_fuse = Vec::new();
    let mut fuse_bad = 0usize;
    let mut out = [0u16; OUT_LEN];
    let mut marks = Vec::new();

    // warmup
    for &(cx, cz) in chunks.iter().take(3) {
        g.fill_chunk(cx, cz, &[0, 0], &mut out, &mut marks);
    }

    for _ in 0..rounds {
        for &(cx, cz) in &chunks {
            // full fill
            let t0 = std::time::Instant::now();
            g.fill_chunk(cx, cz, &[0, 0], &mut out, &mut marks);
            t_fill.push(t0.elapsed().as_secs_f64() * 1e3);

            // stage replication (fresh ctx like fill_chunk)
            let volume =
                DensityVolume::block_step((16, g.height, 16), (cx * 16, g.min_y, cz * 16));
            let mut ctx = SamplerCtx263::cached();

            let a0 = std::time::Instant::now();
            let mut aquifer = g.aquifer_for(&mut ctx, &volume);
            t_aq.push(a0.elapsed().as_secs_f64() * 1e3);

            let mut buf = ctx.scratch.acquire(volume.size());
            let v0 = std::time::Instant::now();
            g.final_density.sample_volume(&mut ctx, &mut buf, &volume);
            t_vol.push(v0.elapsed().as_secs_f64() * 1e3);

            let s0 = std::time::Instant::now();
            let mut n = 0usize;
            for z in 0..volume.size_z {
                for x in 0..volume.size_x {
                    for y in (0..volume.size_y).rev() {
                        let density = buf[volume.index_unchecked(x, y, z)];
                        let ty = aquifer.compute_substance(
                            &mut ctx,
                            volume.block_x(x),
                            volume.block_y(y),
                            volume.block_z(z),
                            density as f64,
                        );
                        if ty.is_some() {
                            n += 1;
                        }
                    }
                }
            }
            t_sub.push(s0.elapsed().as_secs_f64() * 1e3);
            let _ = n;

            // fused flat-program timing: honest production ordering — fresh
            // ctx + its own aquifer prefill (the aquifer warms the same
            // cache cells the walk would), then the fused volume sample.
            match &g.final_density_fuse {
                Some(prog) => {
                    let mut fctx = SamplerCtx263::cached();
                    let _faq = g.aquifer_for(&mut fctx, &volume);
                    let mut fbuf = fctx.scratch.acquire(volume.size());
                    let f0 = std::time::Instant::now();
                    prog.run(&mut fctx, &mut fbuf, &volume);
                    t_fuse.push(f0.elapsed().as_secs_f64() * 1e3);
                    // bit-parity check against the walk buffer (caches are
                    // pure memoization — results must not depend on them)
                    if fbuf
                        .iter()
                        .zip(buf.iter())
                        .any(|(a, b)| a.to_bits() != b.to_bits())
                    {
                        fuse_bad += 1;
                    }
                    fctx.scratch.release(fbuf);
                }
                None => t_fuse.push(f64::NAN),
            }
            ctx.scratch.release(buf);
        }
    }

    // ---- prof report (exclusive per-kind + interp internals) ----
    use crussty_chunkgen::df263::{PROF_INTERP_CELL, PROF_INTERP_FILL, PROF_KINDS, PROF_KIND_NAMES, PROF_ON_263};
    PROF_ON_263.store(true, std::sync::atomic::Ordering::Relaxed);
    for &(cx, cz) in chunks.iter().take(6) {
        g.fill_chunk(cx, cz, &[0, 0], &mut out, &mut marks);
    }
    PROF_ON_263.store(false, std::sync::atomic::Ordering::Relaxed);
    let ms = |v: &std::sync::atomic::AtomicU64| v.load(std::sync::atomic::Ordering::Relaxed) as f64 / 1e6 / 6.0;
    println!("== per-kind exclusive ms/chunk (6-chunk avg) ==");
    for (i, name) in PROF_KIND_NAMES.iter().enumerate() {
        let v = ms(&PROF_KINDS[i]);
        if v > 0.001 {
            println!("  {v:7.3}  {name}");
        }
    }
    println!("  {:7.3}  interp: cell-grid sample", ms(&PROF_INTERP_CELL));
    println!("  {:7.3}  interp: trilinear fill", ms(&PROF_INTERP_FILL));

    println!(
        "fill_chunk  median {:.2}ms  min {:.2}  max {:.2}",
        median(&mut t_fill),
        t_fill.iter().cloned().fold(f64::MAX, f64::min),
        t_fill.iter().cloned().fold(f64::MIN, f64::max),
    );
    println!(
        "aquifer ctor median {:.2}ms  (sum-of-parts check)",
        median(&mut t_aq)
    );
    println!(
        "volume sample median {:.2}ms",
        median(&mut t_vol)
    );
    let fuse_nan = t_fuse.iter().any(|v| v.is_nan());
    if fuse_nan {
        println!("volume FUSED    : n/a (compile returned None — stack budget)");
    } else {
        let prog = g.final_density_fuse.as_ref().unwrap();
        println!(
            "volume FUSED    median {:.2}ms  (ops {}, slots {}, bit-parity fails: {fuse_bad}/{})",
            median(&mut t_fuse),
            prog.ops.len(),
            prog.slots.len(),
            t_fuse.len()
        );
    }
    println!(
        "substance loop median {:.2}ms",
        median(&mut t_sub)
    );
}
