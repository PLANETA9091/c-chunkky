//! Features RNG protocol (stage F foundation): WorldgenRandom decoration seeding,
//! bit-exact vs vanilla 1.21.8 (decompile: WorldgenRandom + XoroshiroRandomSource).
//!
//! Contract: reference/features_contract.md §1-2; golden parity: reference/java/WgrRef.java.
//!
//! WorldgenRandom wraps XoroshiroRandomSource (decoration loop uses the xoroshiro path):
//! - next(bits) = (int)(xoro.nextLong() >>> (64-bits))   [overridden]
//! - nextLong() = next(32)<<32 + next(32)                 [BitRandomSource default, 2 xoro calls]
//! - setDecorationSeed: setSeed(levelSeed); a=nextLong()|1; b=nextLong()|1;
//!   seed = minX*a + minZ*b ^ levelSeed (returns seed)
//! - setFeatureSeed(deco, index, step): setSeed(deco + index + 10000*step)
//! - Paper featureSeeds override: setDecorationSeed(configSeed, x, z) — same call shape.

use crate::rng::{upgrade_seed_to_128bit, Xoroshiro128PlusPlus};

pub struct WorldgenRandom {
    rng: Xoroshiro128PlusPlus,
    count: u32,
}

impl WorldgenRandom {
    pub fn new() -> Self {
        // vanilla ctor: WorldgenRandom(new XoroshiroRandomSource(RandomSupport.generateUniqueSeed()))
        // the unique seed is never observable (immediately reseeded by setDecorationSeed)
        WorldgenRandom {
            rng: Xoroshiro128PlusPlus::from_seed(0),
            count: 0,
        }
    }

    /// vanilla XoroshiroRandomSource.setSeed = upgradeSeedTo128bit + fresh engine
    pub fn set_seed(&mut self, seed: i64) {
        let (lo, hi) = upgrade_seed_to_128bit(seed);
        self.rng = Xoroshiro128PlusPlus::from_128(lo, hi);
    }

    /// WorldgenRandom.next(bits) — delegated (int)(xoro.nextLong() >>> (64-bits))
    #[inline]
    pub fn next_bits(&mut self, bits: u32) -> i32 {
        self.count += 1;
        ((self.rng.next_long() as u64) >> (64 - bits)) as i32
    }

    /// BitRandomSource default nextLong() routed through the overridden next():
    /// (next(32) << 32) + next(32) — TWO xoroshiro nextLong calls, high half from the first.
    #[inline]
    pub fn next_long(&mut self) -> i64 {
        let hi_half = self.next_bits(32) as i64;
        let lo_half = self.next_bits(32) as i64;
        (hi_half << 32) + lo_half
    }

    /// RandomSource.nextInt(bound) default via next(31) (BitRandomSource) — NOT Lemire:
    /// WorldgenRandom overrides next(bits), so the wrapped Lemire never runs here.
    pub fn next_int_bound(&mut self, bound: i32) -> i32 {
        debug_assert!(bound > 0);
        if bound & (bound - 1) == 0 {
            return ((bound as i64) * (self.next_bits(31) as i64) >> 31) as i32;
        }
        let mut i = self.next_bits(31);
        let mut r = i % bound;
        while i - r + (bound - 1) < 0 {
            i = self.next_bits(31);
            r = i % bound;
        }
        r
    }

    #[inline]
    pub fn next_bool(&mut self) -> bool {
        self.next_bits(1) != 0
    }

    /// vanilla nextFloat: (float)next(24) * 5.9604645E-8F — 24 bits from next(24)
    #[inline]
    pub fn next_f32(&mut self) -> f32 {
        (self.next_bits(24) as u32 as f32) * 5.960464477539063e-8f32
    }

    /// vanilla nextDouble: ((next(26) << 27) + next(27)) → l2f → * 1.110223E-16F (f32!) → f2d
    #[inline]
    pub fn next_f64(&mut self) -> f64 {
        let a = self.next_bits(26) as i64;
        let b = self.next_bits(27) as i64;
        let l = (a << 27) + b;
        let f = (l as f32) * 1.1102230246251565e-16f32;
        f as f64
    }

    /// XoroshiroRandomSource.nextGaussian is not used by any dptt top feature; guarded accessor.
    pub fn count(&self) -> u32 {
        self.count
    }

    /// returns the per-chunk decoration seed `l` used by all setFeatureSeed calls of this chunk
    pub fn set_decoration_seed(&mut self, level_seed: i64, min_chunk_block_x: i32, min_chunk_block_z: i32) -> i64 {
        self.set_seed(level_seed);
        let a = self.next_long() | 1;
        let b = self.next_long() | 1;
        let s = (min_chunk_block_x as i64)
            .wrapping_mul(a)
            .wrapping_add((min_chunk_block_z as i64).wrapping_mul(b))
            ^ level_seed;
        self.set_seed(s);
        s
    }

    pub fn set_feature_seed(&mut self, decoration_seed: i64, index: i32, decoration_step: i32) {
        let s = decoration_seed
            .wrapping_add(index as i64)
            .wrapping_add(10000i64.wrapping_mul(decoration_step as i64));
        self.set_seed(s);
    }
}

impl Default for WorldgenRandom {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Golden parity vs reference/java/WgrRef.java (decompile 1.21.8).
    /// Format: (levelSeed, minX, minZ) -> (decoSeed, [6 sampled values after 3 setFeatureSeed calls])
    #[test]
    fn worldgen_random_decoration_parity() {
        struct Case {
            seed: i64,
            x: i32,
            z: i32,
            deco: i64,
            vals: [i64; 5],
        }
        let cases = vec![
            // DECO 1234567890 0 0 -> 1234567890
            Case { seed: 1234567890, x: 0, z: 0, deco: 1234567890, vals: [9152915212902283947, 1459985699, -3178328509786079719, 24553, -1434937826977143596] },
            // DECO 1234567890 16 -16
            Case { seed: 1234567890, x: 16, z: -16, deco: -8357490867922322894, vals: [-7308689524570357606, 727468739, 8014419690935470692, 21614, 6329666092589212847] },
            // DECO -987654321098 -208 304
            Case { seed: -987654321098, x: -208, z: 304, deco: 7813607792433748758, vals: [-4971746718412609220, 1393167729, 3688588814647093322, 3388, -227344300104407490] },
            // DECO 1 1024 -1024
            Case { seed: 1, x: 1024, z: -1024, deco: -4516840819430021119, vals: [-3486128813658165710, 1314603310, -8548003050838093492, 5573, 7714155229113819998] },
            // DECO 8675309 3360 -1024
            Case { seed: 8675309, x: 3360, z: -1024, deco: 4673652647464623693, vals: [6519285307708369053, 1674771438, 8140329943028582869, 49561, -203997241452768165] },
            // DECO -7046029254386353131 -5 5
            Case { seed: -7046029254386353131, x: -5, z: 5, deco: 8522311605028874821, vals: [839792428837595466, 377266601, 5003134583664527548, 33635, -1529977352618456635] },
        ];
        for c in cases {
            let mut r = WorldgenRandom::new();
            let deco = r.set_decoration_seed(c.seed, c.x, c.z);
            assert_eq!(deco, c.deco, "decoration seed mismatch for {} @ {},{}", c.seed, c.x, c.z);
            // mirror WgrRef.main: 3 feature seeds, mixed nextLong / next(bits) consumption
            r.set_feature_seed(deco, 7, 6);
            assert_eq!(r.next_long(), c.vals[0], "f(7,6).nextLong {} {}", c.seed, c.x);
            assert_eq!(r.next_bits(31), c.vals[1] as i32, "f(7,6).next(31) {} {}", c.seed, c.x);
            r.set_feature_seed(deco, 0, 1);
            assert_eq!(r.next_long(), c.vals[2], "f(0,1).nextLong {} {}", c.seed, c.x);
            assert_eq!(r.next_bits(16), c.vals[3] as i32, "f(0,1).next(16) {} {}", c.seed, c.x);
            r.set_feature_seed(deco, 4291, 10);
            assert_eq!(r.next_long(), c.vals[4], "f(4291,10).nextLong {} {}", c.seed, c.x);
        }
    }

    /// nextInt(bound) default path (next(31) + rejection) — power-of-two and non-power-of-two.
    /// f32/f64/int-bound quirks vs Java goldens (WgrRef FQUIRK line):
    /// seed 8675309 @ (16,-32), then setFeatureSeed(8675309, 3, 2).
    /// NOTE: WorldgenRandom.next(bits) routes through the WRAPPED xoroshiro (not the LCG),
    /// so nextFloat/nextDouble/nextInt(bound) here consume next(bits) — bit-exact parity target.
    #[test]
    fn float_and_bound_goldens() {
        let mut r = WorldgenRandom::new();
        let deco = r.set_decoration_seed(8675309, 16, -32);
        r.set_feature_seed(8675309, 3, 2);
        let _ = deco;
        // f32 raw bits: 1052622002, 1053691766, 1064260483
        for exp in [1052622002u32, 1053691766, 1064260483] {
            assert_eq!(r.next_f32().to_bits(), exp, "f32 bits");
        }
        // f64 raw bits (quantized via f32!): 4604423528840691712, 4600114865430855680, 4603988224778436608
        for exp in [4604423528840691712u64, 4600114865430855680, 4603988224778436608] {
            assert_eq!(r.next_f64().to_bits(), exp, "f64 bits");
        }
        // nextInt pairs (16,10): (4,9), (13,0), (12,2)
        for (b, exp) in [(16, 4), (10, 9), (16, 13), (10, 0), (16, 12), (10, 2)] {
            assert_eq!(r.next_int_bound(b), exp, "nextInt({})", b);
        }
    }

    /// Distribution sanity for the rejection path (non-power-of-two bound).
    #[test]
    fn next_int_bound_semantics() {
        let mut r = WorldgenRandom::new();
        r.set_decoration_seed(42, 3, -7);
        let mut sum = 0i64;
        let mut buckets = [0i64; 6];
        for _ in 0..6000 {
            let v = r.next_int_bound(6);
            assert!((0..6).contains(&v));
            buckets[v as usize] += 1;
            sum += v as i64;
        }
        // mean of uniform [0,6) = 2.5; LCG+rejection is well-distributed — sanity band
        let mean = sum as f64 / 6000.0;
        assert!(mean > 2.2 && mean < 2.8, "mean {}", mean);
        for b in buckets {
            assert!(b > 700, "bucket underflow {}", b);
        }
    }

    /// Determinism smoke: same decoration seed -> identical f32/f64/bool streams.
    #[test]
    fn deterministic_streams() {
        let mut a = WorldgenRandom::new();
        let mut b = WorldgenRandom::new();
        a.set_decoration_seed(99, 5, 5);
        b.set_decoration_seed(99, 5, 5);
        for _ in 0..100 {
            assert_eq!(a.next_f32(), b.next_f32());
            assert_eq!(a.next_f64(), b.next_f64());
            assert_eq!(a.next_bool(), b.next_bool());
        }
    }
}
