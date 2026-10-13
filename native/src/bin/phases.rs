//! Per-phase fill timings (slice / cellfill / material) using CRUS_PROFILE=1.
use crussty_chunkgen::density::Env;
use crussty_chunkgen::nochunk::{ChunkGen, OUT_LEN};

fn main() {
    std::env::set_var("CRUS_PROFILE", "1");
    // Diagnostic counters (Instant::now in the noise hot path) are opt-in:
    // CRUS_DIAG=1. Without it the measured window is clean.
    let diag = std::env::var("CRUS_DIAG").ok().as_deref() == Some("1");
    crussty_chunkgen::density::PROF_ON
        .store(diag, std::sync::atomic::Ordering::Relaxed);
    let seed: i64 = 3257840388504953787;
    let env: &'static Env = Box::leak(Box::new(Env::new(seed)));
    let mut gen = ChunkGen::new(env);
    let mut out = [0u16; OUT_LEN];
    for cz in 0..2 {
        for cx in 0..2 {
            gen.fill(cx, cz, &mut out);
        }
    }
    // zero diagnostic counters before the measured window
    crussty_chunkgen::density::PROF_NOISE_NS.store(0, std::sync::atomic::Ordering::Relaxed);
    crussty_chunkgen::density::PROF_BLEND_NS.store(0, std::sync::atomic::Ordering::Relaxed);
    crussty_chunkgen::density::PROF_NOISE_CALLS.store(0, std::sync::atomic::Ordering::Relaxed);
    crussty_chunkgen::density::PROF_BLEND_CALLS.store(0, std::sync::atomic::Ordering::Relaxed);
    gen.prof.reset_phases();
    let n = 16i32;
    let t0 = std::time::Instant::now();
    for i in 0..n {
        gen.fill((i % 4) - 2, (i / 4) - 2, &mut out);
    }
    let wall = t0.elapsed().as_nanos() as f64 / n as f64 / 1e6;
    let p = gen.prof;
    let f = |v: u64| v as f64 / n as f64 / 1e6;
    println!(
        "wall {:.2} ms | slice {:.2} | cellfill {:.2} | material {:.2} | other {:.2}",
        wall,
        f(p.fill_slice_ns),
        f(p.cellfill_ns),
        f(p.material_ns),
        wall - f(p.fill_slice_ns) - f(p.cellfill_ns) - f(p.material_ns),
    );
    println!(
        "blocks: subst+ {} subst- {} ore_reached {} aquifer_air {}",
        p.blocks_subst_pos, p.blocks_subst_neg, p.blocks_ore_reached, p.blocks_aquifer_air,
    );
    println!(
        "diag: normalnoise-batch {:.2} ms ({} calls) | blended-batch {:.2} ms ({} calls) per chunk",
        f(crussty_chunkgen::density::PROF_NOISE_NS.load(
            std::sync::atomic::Ordering::Relaxed
        )),
        crussty_chunkgen::density::PROF_NOISE_CALLS.load(std::sync::atomic::Ordering::Relaxed) / 16,
        f(crussty_chunkgen::density::PROF_BLEND_NS.load(
            std::sync::atomic::Ordering::Relaxed
        )),
        crussty_chunkgen::density::PROF_BLEND_CALLS.load(std::sync::atomic::Ordering::Relaxed) / 16,
    );
    print_slot_anatomy(&gen);
}

/// Dump per-slot child node + column-cache/y-invariance verdict.
fn print_slot_anatomy(gen: &ChunkGen<'static>) {
    let c = &gen.compiled;
    for (slot, &child) in c.interp_children.iter().enumerate() {
        let col = c.col_idx[child as usize];
        println!(
            "slot {slot}: child {} col_cache={}",
            child,
            if col == u32::MAX { "NO".into() } else { format!("yes({col})") }
        );
    }
}
