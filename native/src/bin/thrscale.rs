use std::time::Instant;
use std::hint::black_box;

fn work(n: usize) -> f64 {
    let mut acc = 0.0f64;
    let mut x = 1.0f64;
    for i in 0..n {
        x = x * 1.0000001 + (i as f64) * 1e-9;
        acc += x.sqrt();
    }
    acc
}

fn main() {
    let n = 30_000_000;
    let t0 = Instant::now();
    let a = work(n);
    black_box(a);
    let t1 = t0.elapsed();
    println!("1t: {:?} ({:.0} MOPT/s)", t1, n as f64 / t1.as_secs_f64() / 1e6);

    for nthr in [2usize, 3] {
        let t0 = Instant::now();
        std::thread::scope(|s| {
            let mut hs = Vec::new();
            for _ in 0..nthr {
                hs.push(s.spawn(move || work(n)));
            }
            let mut sink = 0.0;
            for h in hs { sink += h.join().unwrap(); }
            black_box(sink);
        });
        let dt = t0.elapsed();
        println!("{}t: {:?} ({:.0} MOPT/s, speedup = {:.2})", nthr, dt,
            nthr as f64 * n as f64 / dt.as_secs_f64() / 1e6,
            (nthr as f64 * n as f64 / dt.as_secs_f64()) / (n as f64 / t1.as_secs_f64()));
    }
}
