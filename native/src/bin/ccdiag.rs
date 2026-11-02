//! cont-89 diag — count interpolated_volume invocations (and cache-engaged
//! ones) per fill shape: who feeds the corner cache besides the two main
//! 4x8 interpolated nodes. Prints per-call (cell grid dims, engaged, miss
//! fraction) aggregates by grid size.

use crussty_chunkgen::df263::{DensityVolume, SamplerCtx263, PROF_CC_BOX, PROF_CC_HIT, PROF_CC_MISS};
use crussty_chunkgen::fill263::Gen263;

fn main() {
    let g = Gen263::new(3257840388504953787, -64, 384, 63);
    let stamp = crussty_chunkgen::df263::corner_live_stamp();
    println!("corner_live_stamp = {stamp}");

    // one fill's worth of density sampling, cache engaged (stamped ctx),
    // sequential (no strips): the honest per-fill upper bound.
    let volume = DensityVolume::block_step((16, 384, 16), (16 * 11, -64, 16 * 11));
    let h0 = PROF_CC_HIT.load(std::sync::atomic::Ordering::Relaxed);
    let m0 = PROF_CC_MISS.load(std::sync::atomic::Ordering::Relaxed);
    let b0 = PROF_CC_BOX.load(std::sync::atomic::Ordering::Relaxed);
    let mut buf = vec![0f32; volume.size()];
    let mut ctx = SamplerCtx263::cached();
    ctx.cc_gen = stamp;
    g.final_density.sample_volume(&mut ctx, &mut buf, &volume);
    let h1 = PROF_CC_HIT.load(std::sync::atomic::Ordering::Relaxed);
    let m1 = PROF_CC_MISS.load(std::sync::atomic::Ordering::Relaxed);
    let b1 = PROF_CC_BOX.load(std::sync::atomic::Ordering::Relaxed);
    println!("fill#1 (cold, sequential): probes={} (hit {} miss {}) box-samples={}",
        (h1 - h0) + (m1 - m0), h1 - h0, m1 - m0, b1 - b0);

    // second fill = the neighbour chunk (what a window actually does)
    let volume2 = DensityVolume::block_step((16, 384, 16), (16 * 12, -64, 16 * 11));
    let h0 = h1; let m0 = m1; let b0 = b1;
    let mut buf2 = vec![0f32; volume2.size()];
    let mut ctx2 = SamplerCtx263::cached();
    ctx2.cc_gen = stamp;
    g.final_density.sample_volume(&mut ctx2, &mut buf2, &volume2);
    let h1 = PROF_CC_HIT.load(std::sync::atomic::Ordering::Relaxed);
    let m1 = PROF_CC_MISS.load(std::sync::atomic::Ordering::Relaxed);
    let b1 = PROF_CC_BOX.load(std::sync::atomic::Ordering::Relaxed);
    println!("fill#2 (neighbour): probes={} (hit {} miss {}) box-samples={}",
        (h1 - h0) + (m1 - m0), h1 - h0, m1 - m0, b1 - b0);

    // strip shapes: 4 strips of fill#2 (the parallel regime's work)
    let mut total = (0u64, 0u64, 0u64);
    for s in 0..4i32 {
        let sz0 = s * 4;
        let strip = DensityVolume {
            size_x: 16, size_y: 384, size_z: 4,
            min_x: volume2.min_x, min_y: -64, min_z: volume2.min_z + sz0,
            step_x: 1, step_y: 1, step_z: 1,
        };
        let h0 = PROF_CC_HIT.load(std::sync::atomic::Ordering::Relaxed);
        let m0 = PROF_CC_MISS.load(std::sync::atomic::Ordering::Relaxed);
        let b0 = PROF_CC_BOX.load(std::sync::atomic::Ordering::Relaxed);
        let mut sb = vec![0f32; strip.size()];
        let mut sctx = SamplerCtx263::cached();
        sctx.cc_gen = stamp;
        g.final_density.sample_volume(&mut sctx, &mut sb, &strip);
        let h1 = PROF_CC_HIT.load(std::sync::atomic::Ordering::Relaxed);
        let m1 = PROF_CC_MISS.load(std::sync::atomic::Ordering::Relaxed);
        let b1 = PROF_CC_BOX.load(std::sync::atomic::Ordering::Relaxed);
        println!("strip{s}: probes={} (hit {} miss {}) box={}",
            (h1 - h0) + (m1 - m0), h1 - h0, m1 - m0, b1 - b0);
        total.0 += (h1 - h0) + (m1 - m0);
        total.1 += h1 - h0;
        total.2 += b1 - b0;
    }
    println!("4 strips total: probes={} hit={} box={}", total.0, total.1, total.2);
}
