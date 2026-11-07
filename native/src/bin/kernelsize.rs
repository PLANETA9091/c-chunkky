//! cont-125 probe: perlin column-kernel efficiency vs volume size.
//! The fill path averages ~557 pts/kernel-call (220 calls/fill); the
//! cont-124 A/B measured 4.0 ns/pt on the big replication volumes.
//! If small volumes pay a large per-call/column overhead, the real
//! fill-path ns/pt is much higher and call-granularity is the lever.
use crussty_chunkgen::df263::{
    DensityVolume, DfSam263, NoiseFactory263, SamplerCtx263,
};
use crussty_chunkgen::n263::{Normalization, NormalNoiseParameters};
use std::sync::Arc;
use std::time::Instant;

fn main() {
    let mut factory = NoiseFactory263::new(3257840388504953787);
    let params = NormalNoiseParameters {
        base_amplitude: 0.9381732587751005,
        base_octave: -3,
        octave_count: 4,
        normalize: Normalization::Enabled,
        amplitude_modifiers: vec![1.0, 1.0, 1.0, 0.0],
    };
    let noise = factory.normal("offset", params);
    let node: Arc<DfSam263> = Arc::new(DfSam263::Noise {
        noise,
        xz_scale: 1.0,
        y_scale: 1.0,
    });

    // shapes the fill path actually samples
    let shapes: Vec<(usize, usize, usize)> = vec![
        (5, 49, 5),   // outer interp cell grid 4x8x4
        (19, 25, 19), // mid grid
        (17, 98, 17), // inner grid 1x4x1 full
        (16, 96, 16), // block grid
        (25, 1, 25),  // 2D plane (shift/temperature style)
        (4, 8, 4),    // single cell volume (aquifer strip size)
    ];
    println!("shape                pts     ns/pt   ns/col  (warm, prof off)");
    let mut first = true;
    for (sx, sy, sz) in shapes {
        let vol = DensityVolume {
            size_x: sx as i32,
            size_y: sy as i32,
            size_z: sz as i32,
            min_x: 0,
            min_y: -64,
            min_z: 0,
            step_x: 1,
            step_y: 1,
            step_z: 1,
        };
        let mut out = vec![0f32; vol.size()];
        let reps = if vol.size() < 4000 { 20000 } else { 2000 };
        // ONE ctx for the whole shape loop — ctx construction (scratch pools,
        // beardifier) must not pollute the kernel timing.
        let mut ctx = SamplerCtx263::uncached();
        for _ in 0..10 {
            node.sample_volume(&mut ctx, &mut out, &vol);
        }
        // layer count of this noise (sanity for the ns/pt math)
        if first {
            if let DfSam263::Noise { noise, .. } = &*node {
                if let crussty_chunkgen::df263::N263::Normal(n) = &**noise {
                    println!("  (layers={})", n.stack.layers.len());
                }
            }
            first = false;
        }
        crussty_chunkgen::df263::prof263_reset();
        let t0 = Instant::now();
        for _ in 0..reps {
            node.sample_volume(&mut ctx, &mut out, &vol);
        }
        let dt = t0.elapsed().as_secs_f64() / reps as f64;
        crussty_chunkgen::df263::PROF_FILLS_263.store(1, std::sync::atomic::Ordering::Relaxed);
        println!(
            "  prof: {}",
            crussty_chunkgen::df263::prof263_nf_snapshot()
        );
        crussty_chunkgen::df263::prof263_reset();
        let cols = (sx * sz) as f64;
        println!(
            "{sx:3}x{sy:3}x{sz:3}      {:8}   {:7.2}  {:7.2}",
            vol.size(),
            dt * 1e9 / vol.size() as f64,
            dt * 1e9 / cols
        );
    }
}
