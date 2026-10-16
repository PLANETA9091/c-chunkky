// Plane-phase scaling: 5 planes single-thread vs 2 workers.
use crussty_chunkgen::density::{Env, Mode};
use crussty_chunkgen::nochunk::ChunkGen;

fn main() {
    let seed: i64 = 3257840388504953787;
    let env: &'static Env = Box::leak(Box::new(Env::new(seed)));
    let gen = ChunkGen::new(env);
    let compiled = gen.compiled_pub();
    let children: Vec<u32> = gen.interp_children_pub().to_vec();
    let plane_len = children.len() * 5 * 49;
    let lanes = 4;

    let eval_plane = |plane_x: i32, out: &mut [f64]| {
        crussty_chunkgen::nochunk::ChunkGen::eval_plane_into_pub(
            env, compiled, &children, plane_x, -8, 4, 48, -8, 4, 8, lanes, out,
        );
    };

    // warm
    let mut sink = 0f64;
    for k in 0..5i32 {
        let mut buf = vec![0f64; plane_len];
        eval_plane(16 + k * 4, &mut buf);
        sink += buf[0];
    }

    // serial: 5 planes
    let reps = 10;
    let t0 = std::time::Instant::now();
    for _ in 0..reps {
        for k in 0..5i32 {
            let mut buf = vec![0f64; plane_len];
            eval_plane(64 + k * 4, &mut buf);
            sink += buf[0];
        }
    }
    let serial = t0.elapsed().as_nanos() as f64 / reps as f64 / 1e6;

    // parallel: 2 threads, planes split 3/2
    let t0 = std::time::Instant::now();
    for _ in 0..reps {
        std::thread::scope(|s| {
            let h = s.spawn(|| {
                let mut acc = 0f64;
                for k in [1i32, 3] {
                    let mut buf = vec![0f64; plane_len];
                    eval_plane(128 + k * 4, &mut buf);
                    acc += buf[0];
                }
                acc
            });
            for k in [2i32, 4] {
                let mut buf = vec![0f64; plane_len];
                eval_plane(128 + k * 4, &mut buf);
                sink += buf[0];
            }
            sink += h.join().unwrap();
        });
    }
    let par = t0.elapsed().as_nanos() as f64 / reps as f64 / 1e6;
    println!("5 planes: serial {:.3} ms | 2 workers {:.3} ms | scaling {:.2}x (sink={:e})", serial, par, serial / par, sink);
}
