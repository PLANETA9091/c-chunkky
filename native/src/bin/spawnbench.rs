use std::time::Instant;
fn main() {
    // spawn cost microbench: scope+spawn+join of N threads, 100 reps
    for nthr in [1usize, 2, 3] {
        let mut walls = Vec::new();
        for _ in 0..100 {
            let t0 = Instant::now();
            std::thread::scope(|s| {
                let mut hs = Vec::new();
                for _ in 0..nthr {
                    hs.push(s.spawn(|| {
                        // tiny work: touch 16KB
                        let mut v = vec![0f64; 2048];
                        v[0] = 1.0;
                        v
                    }));
                }
                for h in hs { h.join().unwrap(); }
            });
            walls.push(t0.elapsed().as_nanos() as f64 / 1000.0);
        }
        walls.sort_by(|a,b| a.partial_cmp(b).unwrap());
        println!("nthr={} spawn+join: p50={:.1}us p90={:.1}us mean={:.1}us", nthr, walls[50], walls[90], walls.iter().sum::<f64>()/100.0);
    }
}
