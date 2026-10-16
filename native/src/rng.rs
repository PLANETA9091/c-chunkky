// Vanilla RNG: LegacyRandomSource (java.util.Random LCG) + XoroshiroRandomSource.

// ---------------- LegacyRandomSource (java.util.Random 48-bit LCG) ----------------
#[derive(Clone)]
pub struct LegacyRandom {
    seed: u64, // 48 bits used
}

impl LegacyRandom {
    pub fn new(seed: i64) -> Self {
        let mut r = LegacyRandom { seed: 0 };
        r.set_seed(seed);
        r
    }

    pub fn set_seed(&mut self, seed: i64) {
        self.seed = ((seed as u64) ^ 0x5DEECE66Du64) & ((1u64 << 48) - 1);
    }

    #[inline]
    fn next(&mut self, bits: u32) -> i32 {
        self.seed = self
            .seed
            .wrapping_mul(0x5DEECE66Du64)
            .wrapping_add(0xBu64)
            & ((1u64 << 48) - 1);
        ((self.seed >> (48 - bits)) as i32) as i32
    }

    pub fn next_i32(&mut self) -> i32 {
        self.next(32)
    }

    pub fn next_int_bound(&mut self, bound: i32) -> i32 {
        if bound <= 0 {
            panic!("Bad bound");
        }
        // java.util.Random.nextInt
        if (bound & -bound) == bound {
            return ((bound as i64 * self.next(31) as i64) >> 31) as i32;
        }
        let mut bits: i32;
        let mut val: i32;
        loop {
            bits = self.next(31);
            val = bits % bound;
            if (bits as i64).wrapping_sub(val as i64).wrapping_add((bound - 1) as i64)
                < (1i64 << 31)
            {
                break;
            }
        }
        val
    }

    pub fn next_long(&mut self) -> i64 {
        let hi = self.next(32) as i64;
        let lo = self.next(32) as i64;
        (hi << 32).wrapping_add(lo)
    }

    pub fn next_bool(&mut self) -> bool {
        self.next(1) != 0
    }

    pub fn next_f32(&mut self) -> f32 {
        self.next(24) as f32 * (1.0 / ((1 << 24) as f32))
    }

    pub fn next_f64(&mut self) -> f64 {
        let hi = self.next(26) as u64;
        let lo = self.next(27) as u64;
        ((hi << 27) | lo) as f64 * (1.0 / ((1i64 << 53) as f64))
    }

    pub fn fork_positional(&mut self) -> LegacyPositionalRandomFactory {
        LegacyPositionalRandomFactory::new(self.next_long())
    }
}

#[derive(Clone)]
pub struct LegacyPositionalRandomFactory {
    seed: i64,
}

impl LegacyPositionalRandomFactory {
    pub fn new(seed: i64) -> Self {
        LegacyPositionalRandomFactory { seed }
    }

    pub fn at(&self, x: i32, _y: i32, z: i32) -> LegacyRandom {
        let mut r = LegacyRandom::new(0);
        r.set_seed(self.seed);
        // LegacyRandomSource$LegacyPositionalRandomFactory.at:
        // setSeed(seed); consumeCount(y? ) ...
        // Actual vanilla: long l = x * ... this is used for bedrock gradient legacy path only.
        r.set_seed((x as i64) * 25214903917i64 ^ (z as i64) * 116129781i64 ^ self.seed);
        r
    }
}

// ---------------- Xoroshiro ----------------
#[derive(Clone)]
pub struct Xoroshiro128PlusPlus {
    lo: i64,
    hi: i64,
}

impl Xoroshiro128PlusPlus {
    pub fn from_seed(seed: i64) -> Self {
        let (lo, hi) = upgrade_seed_to_128bit(seed);
        Xoroshiro128PlusPlus { lo, hi }
    }

    pub fn from_128(lo: i64, hi: i64) -> Self {
        Xoroshiro128PlusPlus { lo, hi }
    }

    #[inline]
    pub fn next_long(&mut self) -> i64 {
        let lo = self.lo;
        let hi = self.hi;
        let result = (lo.wrapping_add(hi).rotate_left(17)).wrapping_add(lo);
        let new_hi = hi ^ lo;
        self.lo = lo.rotate_left(49) ^ new_hi ^ (new_hi << 21);
        self.hi = new_hi.rotate_left(28);
        result
    }
}

#[inline]
pub fn mix_stafford13(mut v: i64) -> i64 {
    v = (v ^ ((v as u64 >> 30) as i64)).wrapping_mul(-4658895280553007687i64);
    v = (v ^ ((v as u64 >> 27) as i64)).wrapping_mul(-7723592293110705685i64);
    v ^ ((v as u64 >> 31) as i64)
}

pub fn upgrade_seed_to_128bit(seed: i64) -> (i64, i64) {
    let s1 = seed ^ 7640891576956012809i64;
    let s2 = s1.wrapping_add(-7046029254386353131i64);
    (mix_stafford13(s1), mix_stafford13(s2))
}

/// Mth.getSeed(x, y, z)
#[inline]
pub fn mth_get_seed(x: i32, y: i32, z: i32) -> i64 {
    let mut l = ((x as i64).wrapping_mul(3129871))
        ^ ((z as i64).wrapping_mul(116129781))
        ^ (y as i64);
    l = l
        .wrapping_mul(l)
        .wrapping_mul(42317861)
        .wrapping_add(l.wrapping_mul(11));
    l >> 16
}

#[derive(Clone)]
pub struct XoroshiroRandomSource {
    rng: Xoroshiro128PlusPlus,
    // gaussian state unused for worldgen (worldgen never calls nextGaussian)
}

impl XoroshiroRandomSource {
    /// vanilla XoroshiroRandomSource.nextBoolean = (nextLong() & 1) != 0
    pub fn next_bool(&mut self) -> bool {
        (self.next_long() & 1) != 0
    }
    pub fn new(seed: i64) -> Self {
        XoroshiroRandomSource {
            rng: Xoroshiro128PlusPlus::from_seed(seed),
        }
    }

    pub fn from_128(lo: i64, hi: i64) -> Self {
        XoroshiroRandomSource {
            rng: Xoroshiro128PlusPlus::from_128(lo, hi),
        }
    }

    #[inline]
    pub fn next_long(&mut self) -> i64 {
        self.rng.next_long()
    }

    #[inline]
    pub fn next_i32(&mut self) -> i32 {
        self.rng.next_long() as i32
    }

    /// Vanilla: (float)(nextBits(24)) * 5.9604645E-8f; nextBits = nextLong() >>> 40 (LOGICAL shift)
    #[inline]
    pub fn next_f32(&mut self) -> f32 {
        let bits = ((self.rng.next_long() as u64) >> 40) as i32;
        let f = (bits as f32) * 5.960464477539063e-8f32;
        f
    }

    /// Vanilla quirk: nextBits(53) [nextLong() >>> 11, LOGICAL] -> l2f (round to f32!) -> fmul by 2^-53 (f32) -> f2d
    #[inline]
    pub fn next_f64(&mut self) -> f64 {
        let bits = ((self.rng.next_long() as u64) >> 11) as i64;
        let f = (bits as f32) * 1.1102230246251565e-16f32;
        f as f64
    }

    /// Lemire with unsigned rejection (matches XoroshiroRandomSource.nextInt(bound))
    pub fn next_int_bound(&mut self, bound: i32) -> i32 {
        if bound <= 0 {
            panic!("Bound must be positive");
        }
        let mut m = (self.next_i32() as u32 as u64).wrapping_mul(bound as u64);
        let mut l = m & 0xFFFFFFFFu64;
        if l < bound as u64 {
            let threshold = ((-(bound as i64)) as u32).wrapping_rem(bound as u32) as u64;
            while l < threshold {
                m = (self.next_i32() as u32 as u64).wrapping_mul(bound as u64);
                l = m & 0xFFFFFFFFu64;
            }
        }
        (m >> 32) as i32
    }

    pub fn fork_positional(&mut self) -> XoroshiroPositionalRandomFactory {
        let lo = self.rng.next_long();
        let hi = self.rng.next_long();
        XoroshiroPositionalRandomFactory { seed_lo: lo, seed_hi: hi }
    }
}

#[derive(Clone)]
pub struct XoroshiroPositionalRandomFactory {
    seed_lo: i64,
    seed_hi: i64,
}

impl XoroshiroPositionalRandomFactory {
    pub fn at(&self, x: i32, y: i32, z: i32) -> XoroshiroRandomSource {
        let seed = mth_get_seed(x, y, z);
        XoroshiroRandomSource::from_128(seed ^ self.seed_lo, self.seed_hi)
    }

    /// fromHashOf with full ResourceLocation string ("minecraft:foo" or "bedrock_floor" etc.).
    pub fn from_hash_of(&self, name: &str) -> XoroshiroRandomSource {
        let (lo, hi) = crate::md5::seed_from_hash_of(name);
        XoroshiroRandomSource::from_128(lo ^ self.seed_lo, hi ^ self.seed_hi)
    }
}

// ---------------- PositionalRandomFactory entry (java RandomSource.at(x,y,z)) ----------------
/// RandomSupport.seedFromHashOf returns 128 bits; RandomSource.fromHashOf for the ROOT factory
/// creates XoroshiroRandomSource(md5(name) XOR factory seeds).
pub fn root_xoroshiro(seed: i64) -> XoroshiroRandomSource {
    XoroshiroRandomSource::new(seed)
}
