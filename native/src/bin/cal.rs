//! Calibrate ns/cycle on this VM: a dependency chain of AVX-512 f64 muls
//! (4-cycle latency each).
use std::arch::x86_64::*;
use std::time::Instant;

#[target_feature(enable = "avx512f")]
unsafe fn mul_chain(a: __m512d, n: u64) -> __m512d {
    let mut x = a;
    let mut i = 0u64;
    while i < n {
        x = _mm512_mul_pd(x, x);
        i += 1;
    }
    x
}

fn main() {
    let a = unsafe { _mm512_set1_pd(1.0000001) };
    let mut sink = 0u64;
    for _ in 0..3 {
        let r = unsafe { mul_chain(a, 1000) };
        let mut tmp = [0f64; 8];
        unsafe { _mm512_storeu_pd(tmp.as_mut_ptr(), r) };
        sink ^= tmp[0].to_bits();
    }
    let t0 = Instant::now();
    const N: u64 = 200_000;
    let r = unsafe { mul_chain(a, N) };
    let mut tmp = [0f64; 8];
    unsafe { _mm512_storeu_pd(tmp.as_mut_ptr(), r) };
    sink ^= tmp[0].to_bits();
    let el = t0.elapsed().as_nanos() as f64;
    println!(
        "sink={sink} total {:.1}ms -> {:.4}ns/cycle (mul 4-cycle latency)",
        el / 1e6,
        el / (N as f64 * 4.0)
    );
    let t1 = Instant::now();
    let mut x = 1u64;
    for _ in 0..N {
        x = x.wrapping_add(0x9E3779B97F4A7C15);
    }
    let el1 = t1.elapsed().as_nanos() as f64;
    println!("scalar add chain: {:.3}ns/iter sink={}", el1 / N as f64, x);
}
