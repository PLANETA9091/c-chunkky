//! Features placement chain (stage F1): vanilla PlacementModifier semantics, bit-exact.
//!
//! Contract: reference/features_contract.md §3; goldens: reference/java/ChainRef.java.
//!
//! CRITICAL — lazy pull semantics: PlacedFeature.placeWithContext builds
//! `stream = stream.flatMap(p -> M.getPositions(ctx, random, p))` per modifier. Java streams
//! pull LAZILY and DEPTH-FIRST: one upstream position is pushed through ALL remaining
//! modifiers (recursively) before the next upstream position is touched. The recursive
//! `expand()` below replicates exactly that; a breadth-first loop WOULD MISMATCH (pinned by
//! golden tests with branching Count).

use crate::features_rng::WorldgenRandom;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockPos {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl BlockPos {
    pub fn new(x: i32, y: i32, z: i32) -> Self {
        BlockPos { x, y, z }
    }
}

/// IntProvider subset (ConstantInt, UniformInt). ConstantInt consumes NO RNG.
#[derive(Clone, Copy, Debug)]
pub enum IntProvider {
    Constant(i32),
    Uniform { lo: i32, hi: i32 }, // inclusive bounds
}

impl IntProvider {
    #[inline]
    pub fn sample(&self, rng: &mut WorldgenRandom) -> i32 {
        match *self {
            IntProvider::Constant(v) => v,
            IntProvider::Uniform { lo, hi } => {
                if hi <= lo {
                    lo
                } else {
                    rng.next_int_bound(hi - lo + 1) + lo
                }
            }
        }
    }
}

/// HeightProvider subset with ABSOLUTE VerticalAnchors only (dptt-dominant case);
/// Y-anchored/clamped variants resolve via the level context in a later increment.
#[derive(Clone, Copy, Debug)]
pub enum HeightProvider {
    UniformAbs { lo: i32, hi: i32 },
}

impl HeightProvider {
    /// vanilla UniformHeight.sample: resolveY both anchors; empty range -> min (warned);
    /// else Mth.randomBetweenInclusive(random, min, max) = nextInt(span) + min.
    #[inline]
    pub fn sample(&self, rng: &mut WorldgenRandom) -> i32 {
        match *self {
            HeightProvider::UniformAbs { lo, hi } => {
                if lo > hi {
                    lo
                } else {
                    rng.next_int_bound(hi - lo + 1) + lo
                }
            }
        }
    }
}

/// Level-free modifiers (RNG-order-critical core). Level-dependent ones
/// (Heightmap, BiomeFilter, BlockPredicateFilter, SurfaceWaterDepth, EnvironmentScan)
/// join when the DecoLevel bridge lands — they consume no RNG except via their own predicates.
#[derive(Clone, Copy, Debug)]
pub enum Modifier {
    /// x += nextInt(16), z += nextInt(16)  (X strictly before Z)
    InSquare,
    /// n = provider.sample(rng); n copies of the input position (depth-first)
    Count(IntProvider),
    /// pass iff nextFloat() < 1/chance
    RarityFilter { chance: i32 },
    /// x += xz.sample, y += y.sample, z += xz.sample  (xz sampled TWICE: X then Z)
    RandomOffset { xz: IntProvider, y: IntProvider },
    /// pos.atY(UniformHeight(abs lo..abs hi).sample(rng))
    HeightRangeUniformAbs { lo: i32, hi: i32 },
}

/// Depth-first expansion of the modifier chain — equivalent to vanilla's lazy stream pull.
pub fn expand_chain(
    mods: &[Modifier],
    rng: &mut WorldgenRandom,
    pos: BlockPos,
    out: &mut Vec<BlockPos>,
) {
    expand_rec(mods, 0, rng, pos, out);
}

fn expand_rec(
    mods: &[Modifier],
    idx: usize,
    rng: &mut WorldgenRandom,
    pos: BlockPos,
    out: &mut Vec<BlockPos>,
) {
    if idx == mods.len() {
        out.push(pos);
        return;
    }
    match mods[idx] {
        Modifier::InSquare => {
            let x = rng.next_int_bound(16) + pos.x;
            let z = rng.next_int_bound(16) + pos.z;
            expand_rec(mods, idx + 1, rng, BlockPos { x, y: pos.y, z }, out);
        }
        Modifier::Count(p) => {
            let n = p.sample(rng).max(0);
            for _ in 0..n {
                expand_rec(mods, idx + 1, rng, pos, out);
            }
        }
        Modifier::RarityFilter { chance } => {
            if rng.next_f32() < 1.0 / (chance as f32) {
                expand_rec(mods, idx + 1, rng, pos, out);
            }
        }
        Modifier::RandomOffset { xz, y } => {
            let x = pos.x + xz.sample(rng);
            let ny = pos.y + y.sample(rng);
            let z = pos.z + xz.sample(rng);
            expand_rec(mods, idx + 1, rng, BlockPos::new(x, ny, z), out);
        }
        Modifier::HeightRangeUniformAbs { lo, hi } => {
            let h = HeightProvider::UniformAbs { lo, hi }.sample(rng);
            expand_rec(mods, idx + 1, rng, BlockPos { y: h, ..pos }, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Golden parity vs reference/java/ChainRef.java (lazy depth-first semantics).
    /// Chain: InSquare -> Count(U[1..4]) -> RarityFilter(2) -> RandomOffset(xz U[-3..3], y U[0..1])
    ///        -> HeightRange(UniformHeight abs[-32..64])
    /// Feature seed protocol: setDecorationSeed(seed, minBlockX, minBlockZ); setFeatureSeed(deco, 3, 2).
    #[test]
    fn chain_depth_first_goldens() {
        use Modifier::*;
        let mods = [
            InSquare,
            Count(IntProvider::Uniform { lo: 1, hi: 4 }),
            RarityFilter { chance: 2 },
            RandomOffset {
                xz: IntProvider::Uniform { lo: -3, hi: 3 },
                y: IntProvider::Uniform { lo: 0, hi: 1 },
            },
            HeightRangeUniformAbs { lo: -32, hi: 64 },
        ];
        struct Case {
            seed: i64,
            mx: i32,
            mz: i32,
            origins: [(i32, i32); 2], // (x, z) at y=60/70 per Java ref
            want: [Vec<(i32, i32, i32)>; 2],
        }
        let cases = [
            Case {
                seed: 8675309,
                mx: 16,
                mz: -32,
                origins: [(0, 0), (-104, 208)],
                want: [
                    vec![(4, 0, 9), (10, -26, 11)],
                    vec![(-100, 0, 217), (-94, -26, 219)],
                ],
            },
            Case {
                seed: 1234567890,
                mx: -208,
                mz: 304,
                origins: [(0, 0), (-104, 208)],
                want: [
                    vec![(-2, 9, 1), (1, 4, 3)],
                    vec![(-106, 9, 209), (-103, 4, 211)],
                ],
            },
        ];
        for c in cases {
            for (oi, (ox, oz)) in c.origins.into_iter().enumerate() {
                let oy = if oi == 0 { 60 } else { 70 };
                let mut rng = WorldgenRandom::new();
                let deco = rng.set_decoration_seed(c.seed, c.mx, c.mz);
                rng.set_feature_seed(deco, 3, 2);
                let mut out = Vec::new();
                expand_chain(&mods, &mut rng, BlockPos::new(ox, oy, oz), &mut out);
                let got: Vec<(i32, i32, i32)> = out.iter().map(|p| (p.x, p.y, p.z)).collect();
                assert_eq!(got, c.want[oi], "chain mismatch seed={} origin=({},{})", c.seed, ox, oz);
            }
        }
    }

    /// ConstantInt consumes no RNG; empty count terminates the branch silently.
    #[test]
    fn provider_semantics() {
        let mut rng = WorldgenRandom::new();
        rng.set_decoration_seed(5, 1, 1);
        let before = rng.count();
        assert_eq!(IntProvider::Constant(-7).sample(&mut rng), -7);
        assert_eq!(rng.count(), before, "ConstantInt must not burn RNG");
        // UniformHeight empty range: returns min without consuming RNG (vanilla warn path)
        assert_eq!(HeightProvider::UniformAbs { lo: 10, hi: 4 }.sample(&mut rng), 10);
        assert_eq!(rng.count(), before);
        // zero/negative count yields no positions
        let mods = [Modifier::Count(IntProvider::Uniform { lo: -2, hi: -1 })];
        let mut out = Vec::new();
        expand_chain(&mods, &mut rng, BlockPos::new(0, 0, 0), &mut out);
        assert!(out.is_empty());
    }

    /// InSquare bound is power-of-two -> exercises nextInt(16) shift path; offsets stay in-chunk band.
    #[test]
    fn in_square_band() {
        let mut rng = WorldgenRandom::new();
        rng.set_decoration_seed(31337, 8, -8);
        for _ in 0..2000 {
            let mut out = Vec::new();
            expand_chain(&[Modifier::InSquare], &mut rng, BlockPos::new(48, 64, -32), &mut out);
            let p = out[0];
            assert!((48..64).contains(&p.x) && (-32..-16).contains(&p.z), "out of band {:?}", p);
            assert_eq!(p.y, 64);
        }
    }
}
