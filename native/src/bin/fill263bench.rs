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
    let mut t_fill_cold = Vec::new();
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

    for round in 0..rounds {
        for &(cx, cz) in &chunks {
            // full fill
            let t0 = std::time::Instant::now();
            g.fill_chunk(cx, cz, &[0, 0], &mut out, &mut marks);
            let dt = t0.elapsed().as_secs_f64() * 1e3;
            t_fill.push(dt);
            // round 0 = first fill of each chunk (cold CPU caches, the state
            // the SERVER actually pays: every chunk is generated exactly once)
            if round == 0 {
                t_fill_cold.push(dt);
            }

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

    // ---- SIMD pointwise parity: scalar-forced walk vs auto-width walk must
    // be bit-identical over full chunk volumes (the vectorized Const/Unary/
    // Clamp/Lerp/Binary shell passes vs their scalar references).
    {
        let mut simd_bad = 0usize;
        for &(cx, cz) in &chunks {
            let volume =
                DensityVolume::block_step((16, g.height, 16), (cx * 16, g.min_y, cz * 16));
            let mut sctx = SamplerCtx263::cached();
            let mut sbuf = sctx.scratch.acquire(volume.size());
            crussty_chunkgen::vecf::set_enabled(false);
            g.final_density.sample_volume(&mut sctx, &mut sbuf, &volume);
            crussty_chunkgen::vecf::set_enabled(true);
            let mut vctx = SamplerCtx263::cached();
            let mut vbuf = vctx.scratch.acquire(volume.size());
            g.final_density.sample_volume(&mut vctx, &mut vbuf, &volume);
            let mut diffs = 0usize;
            for (i, (a, b)) in sbuf.iter().zip(vbuf.iter()).enumerate() {
                if a.to_bits() != b.to_bits() {
                    if diffs < 3 {
                        // index -> (x, y, z) with y fastest:
                        // i = y + (x + z*sx)*sy
                        let sy = volume.size_y as usize;
                        let sx = volume.size_x as usize;
                        let y = i % sy;
                        let xz = i / sy;
                        let x = xz % sx;
                        let z = xz / sx;
                        println!(
                            "  SIMD DIFF {} chunk({},{}) idx={} (x={},y={},z={}) scalar={:08x} ({}) vector={:08x} ({})",
                            diffs, cx, cz, i, x, y, z,
                            a.to_bits(), a, b.to_bits(), b
                        );
                    }
                    diffs += 1;
                }
            }
            if diffs > 0 {
                println!("  SIMD DIFF total {} (chunk {},{})", diffs, cx, cz);
                simd_bad += 1;
            }
            sctx.scratch.release(sbuf);
            vctx.scratch.release(vbuf);
        }
        println!(
            "shell SIMD parity: fails {}/{}",
            simd_bad,
            chunks.len()
        );
    }

    // ---- SERVER PARITY: the full JNI fillBeard wall — fill_chunk + parked
    // _WG heightmap + 3x3 biome hood + native surface (stage C) — the path
    // the in-server honest window actually pays. The fill_chunk sections
    // above measure ONLY the terrain stage; the honest window showed the
    // server paying ~66-90ms per fill vs the 3ms COLD bench median, and the
    // difference lives exactly in this tail. Grids are pre-stored for every
    // chunk (the server steady state: fillBiomes hands its grid to the
    // cache before the matching fill); COLD = round 0 of the loop.
    {
        let Some((mut gs, biomes, surface)) =
            crussty_chunkgen::ffi::build_v263_stack(seed, -64, 384, 63)
        else {
            println!("server_fill  SKIPPED (v263 stack build failed)");
            return;
        };
        for &(cx, cz) in &chunks {
            let n = (gs.height as usize) / 16 * 64;
            let mut grid = vec![0i32; n];
            if let Some(b) = biomes.as_ref() {
                crussty_chunkgen::biome263::fill_biomes263(
                    b,
                    gs.min_y,
                    gs.height,
                    cx,
                    cz,
                    &mut grid,
                );
                crussty_chunkgen::ffi::store_biome_grid263(cx, cz, &grid);
            }
        }
        let mut t_srv = Vec::new();
        let mut t_srv_cold = Vec::new();
        for round in 0..rounds {
            for &(cx, cz) in &chunks {
                let t0 = std::time::Instant::now();
                crussty_chunkgen::ffi::v263_fill_tail(
                    &mut gs,
                    biomes.as_ref(),
                    surface.as_ref(),
                    cx,
                    cz,
                    &[0, 0],
                    &mut out,
                    &mut marks,
                );
                let dt = t0.elapsed().as_secs_f64() * 1e3;
                t_srv.push(dt);
                if round == 0 {
                    t_srv_cold.push(dt);
                }
            }
        }
        println!(
            "server_fill median {:.2}ms  min {:.2}  max {:.2}  (fill+park+hood+surface = JNI wall)",
            median(&mut t_srv),
            t_srv.iter().cloned().fold(f64::MAX, f64::min),
            t_srv.iter().cloned().fold(f64::MIN, f64::max),
        );
        if !t_srv_cold.is_empty() {
            println!(
                "server_fill COLD   median {:.2}ms  (round 0, {} chunks)",
                median(&mut t_srv_cold),
                t_srv_cold.len(),
            );
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
    // cold median: what a never-before-generated chunk costs wall-clock
    // (round 0 above) — the server-visible number, not the cache-warmed loop.
    if !t_fill_cold.is_empty() {
        println!(
            "fill_chunk  COLD   median {:.2}ms  (round 0, {} chunks — honest single-touch)",
            median(&mut t_fill_cold),
            t_fill_cold.len(),
        );
    }
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
