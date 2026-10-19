//! Java Math.sin/cos bit parity for the f32-promoted input domain.
//!
//! HotSpot's x86-64 Math.sin/cos intrinsics (Intel LIBM derived) differ from
//! the system libm (glibc) in the last ulp for a small fraction of inputs.
//! OreFeature feeds `Math.sin(f)`/`Math.cos(f)` (f = nextFloat * (float)PI)
//! into sphere floor() boundaries, so a 1-ulp difference flips candidate
//! sets and shifts the RNG stream of every later vein in the chunk.
//!
//! The input domain is finite: f is one of 2^24 f32 values produced by
//! `next(24) * 2^-24 * (float)PI`. Exhaustive JVM scan found 60,946 of them
//! where Java differs from glibc; for those the table stores Java's low-32
//! result bits (the high 32 bits always agree with libm).

static FIX: std::sync::OnceLock<Vec<(u32, u32, u32)>> = std::sync::OnceLock::new();

fn table() -> &'static [(u32, u32, u32)] {
    FIX.get_or_init(|| {
        let b: &[u8] = include_bytes!("../sincos_fix.bin");
        let n = u32::from_le_bytes(b[0..4].try_into().unwrap()) as usize;
        (0..n)
            .map(|i| {
                let o = 4 + i * 12;
                let k = u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
                let s = u32::from_le_bytes(b[o + 4..o + 8].try_into().unwrap());
                let c = u32::from_le_bytes(b[o + 8..o + 12].try_into().unwrap());
                (k, s, c)
            })
            .collect()
    })
}

/// Recover the exact f32 bit pattern behind an f32-promoted f64, if it is
/// one. A promotion keeps the value: sign + (exp-896) + mantissa<<29, so the
/// bottom 29 bits of the f64 pattern must be zero and the exponent must lie
/// in the normal f32 range.
#[inline]
fn f32_bits_of(d: f64) -> Option<u32> {
    let b = d.to_bits();
    if b & ((1u64 << 29) - 1) != 0 {
        return None;
    }
    let sign = (b >> 63) as u32;
    let exp11 = ((b >> 52) & 0x7FF) as i32;
    // normal f32 exponents 1..=254 (biased32) -> biased64 = exp32 + 896
    if !(897..=1150).contains(&exp11) {
        return None;
    }
    let mant = (b >> 29) as u32 & 0x007F_FFFF;
    Some((sign << 31) | (((exp11 - 896) as u32) << 23) | mant)
}

#[inline]
fn corrected(d: f64, g: f64, java_lo: u32) -> f64 {
    f64::from_bits((g.to_bits() & 0xFFFF_FFFF_0000_0000) | java_lo as u64)
}

/// Java Math.sin bit-exact for f32-promoted inputs; libm otherwise.
pub fn java_sin(d: f64) -> f64 {
    let g = d.sin();
    match f32_bits_of(d).and_then(|k| table().binary_search_by_key(&k, |e| e.0).ok()) {
        Some(i) => corrected(d, g, table()[i].1),
        None => g,
    }
}

/// Java Math.cos bit-exact for f32-promoted inputs; libm otherwise.
pub fn java_cos(d: f64) -> f64 {
    let g = d.cos();
    match f32_bits_of(d).and_then(|k| table().binary_search_by_key(&k, |e| e.0).ok()) {
        Some(i) => corrected(d, g, table()[i].2),
        None => g,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sin_cos_golden_bits() {
        let t = table();
        assert_eq!(t.len(), 60946, "table size");
        // every table entry: java_sin differs from glibc only in the low 32
        let mut checked = 0;
        for (fb, slo, clo) in t.iter().step_by(7) {
            let d = f32::from_bits(*fb) as f64;
            let js = f64::from_bits((d.sin().to_bits() & 0xFFFF_FFFF_0000_0000) | *slo as u64);
            let jc = f64::from_bits((d.cos().to_bits() & 0xFFFF_FFFF_0000_0000) | *clo as u64);
            assert_eq!(java_sin(d).to_bits(), js.to_bits(), "sin {fb:08x}");
            assert_eq!(java_cos(d).to_bits(), jc.to_bits(), "cos {fb:08x}");
            // and the correction must actually CHANGE the glibc value here
            // (the entry exists because java != glibc for sin or cos)
            assert!(
                java_sin(d).to_bits() != d.sin().to_bits() || java_cos(d).to_bits() != d.cos().to_bits(),
                "entry {fb:08x} must correct something"
            );
            checked += 1;
        }
        assert!(checked > 3000, "sampled {checked}");
    }

    #[test]
    fn plain_inputs_passthrough() {
        // non-f32-promoted inputs fall through to libm
        let d = 0.1234567890123456789;
        assert_eq!(java_sin(d).to_bits(), d.sin().to_bits());
        assert_eq!(java_cos(d).to_bits(), d.cos().to_bits());
        // a non-divergent f32-promoted input passes through unchanged
        let f = 0.25f32;
        assert_eq!(java_sin(f as f64).to_bits(), (f as f64).sin().to_bits());
    }
}
