fn main() {
    println!("avx2={}", std::arch::is_x86_feature_detected!("avx2"));
    println!("avx512f={}", std::arch::is_x86_feature_detected!("avx512f"));
}
