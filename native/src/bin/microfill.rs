// Micro-benchmark: exact cellfill inner loop in isolation vs with ChunkGen fill.
use crussty_chunkgen::density::{Env, InterpState, Pt};
use crussty_chunkgen::nochunk::{ChunkGen, OUT_LEN};

#[inline]
fn lerp(t: f64, a: f64, b: f64) -> f64 {
    a + t * (b - a)
}

fn main() {
    let env = Env::new(0);
    let mut gen = ChunkGen::new(&env);
    let mut out = [0u16; OUT_LEN];

    // warm
    for i in 0..8 {
        gen.fill(i, 0, &mut out);
    }

    // standalone replication of the cellfill math
    let interp = InterpState::new(8, 4, 48);
    let mut cell_cache = vec![0.0f64; 128];
    let mut sink = 0f64;
    let cells = 768usize;
    let iters = 16usize;
    let t0 = std::time::Instant::now();
    for _ in 0..iters {
        for _c in 0..cells {
            let mut iy = 7i32;
            while iy >= 0 {
                let dy = iy as f64 * 0.125;
                let mut ix = 0i32;
                while ix < 4 {
                    let dx = ix as f64 * 0.25;
                    let mut idx = 0usize;
                    // pretend rows from corners: use constants
                    let (mut row1, mut row2) = (0f64, 0f64);
                    for k in 0..8 {
                        row1 += interp_standin(k, dx, dy);
                        row2 += interp_standin(k + 8, dx, dy);
                    }
                    let mut dz = 0.0f64;
                    while dz < 1.0 {
                        cell_cache[idx] = lerp(dz, row1, row2);
                        idx += 1;
                        dz += 0.25;
                    }
                    sink += cell_cache[0];
                    ix += 1;
                }
                iy -= 1;
            }
        }
    }
    let dt = t0.elapsed();
    println!(
        "standalone cellfill math: {} cells x {} iters = {:?} ({:.3}ms/cell-set) sink={:e}",
        cells,
        iters,
        dt,
        dt.as_secs_f64() * 1000.0 / iters as f64,
        sink
    );
}

#[inline]
fn interp_standin(_k: i32, dx: f64, dy: f64) -> f64 {
    dx * 0.1 + dy * 0.2
}

// keep Pt used
#[allow(dead_code)]
fn _use_pt(_p: Pt) {}
