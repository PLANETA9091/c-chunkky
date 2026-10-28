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

/// IntProvider subset (ConstantInt, UniformInt, WeightedListInt, ClampedInt).
/// ConstantInt consumes NO RNG. Weighted/clamped entries are leaked into
/// &'static — the feature registry is built once per process anyway.
#[derive(Clone, Copy, Debug)]
pub enum IntProvider {
    Constant(i32),
    Uniform { lo: i32, hi: i32 }, // inclusive bounds
    /// BiasedToBottomInt: min + nextInt(nextInt(range+1) + 1) (nested draw)
    BiasedToBottom { lo: i32, hi: i32 },
    /// WeightedListInt: nextInt(totalWeight) + linear walk -> inner provider
    /// sample (the JSON "data" is itself a provider, not a constant).
    WeightedList { entries: &'static [(&'static IntProvider, u32)] },
    /// ClampedInt: ALWAYS draws through the inner provider, then clamps
    Clamped { inner: &'static IntProvider, lo: i32, hi: i32 },
    /// ClampedNormalInt: (int) clamp(mean + deviation * nextGaussian, lo, hi)
    /// — f32 mean/deviation, f64 gaussian; Mth.clamp on f32.
    ClampedNormal { mean: f32, deviation: f32, lo: i32, hi: i32 },
    /// 26.3 TrapezoidInt (min/max/plateau fields): symmetric shortcut
    /// (plateau==0 && max==-min) draws 2; else 3 draws.
    Trapezoid { lo: i32, hi: i32, plateau: i32 },
    /// 26.3 VeryBiasedToBottomInt: min + nextInt(nextInt(nextInt(range+1)+1)+1)
    VeryBiasedToBottom { lo: i32, hi: i32 },
}

impl IntProvider {
    /// vanilla UniformInt.sample = Mth.randomBetweenInclusive(min, max):
    /// min + nextInt(max-min+1) — ALWAYS draws (even min==max -> nextInt(1)
    /// consumes next(31) and returns 0).
    #[inline]
    pub fn sample(&self, rng: &mut WorldgenRandom) -> i32 {
        match *self {
            IntProvider::Constant(v) => v,
            IntProvider::Uniform { lo, hi } => rng.next_int_bound((hi - lo + 1).max(1)) + lo,
            IntProvider::BiasedToBottom { lo, hi } => {
                let range = (hi - lo + 1).max(1);
                let inner = rng.next_int_bound(range) + 1;
                lo + rng.next_int_bound(inner)
            }
            IntProvider::WeightedList { entries } => {
                let total: u32 = entries.iter().map(|(_, w)| *w).sum();
                let mut roll = rng.next_int_bound(total.max(1) as i32) as u32;
                for (v, w) in entries {
                    if roll < *w {
                        return v.sample(rng);
                    }
                    roll -= *w;
                }
                entries.last().map(|(v, _)| v.sample(rng)).unwrap_or(0)
            }
            IntProvider::Clamped { inner, lo, hi } => inner.sample(rng).clamp(lo, hi),
            IntProvider::ClampedNormal { mean, deviation, lo, hi } => {
                let g = rng.next_gaussian();
                let v = (mean as f64 + deviation as f64 * g) as f32;
                v.clamp(lo as f32, hi as f32) as i32
            }
            IntProvider::Trapezoid { lo, hi, plateau } => {
                // 26.3 TrapezoidInt.sample: symmetric shortcut (2 draws),
                // plateau==range (1 draw), else 3 draws (end then start)
                if plateau == 0 && hi == -lo {
                    rng.next_int_bound((hi + 1).max(1)) - rng.next_int_bound((hi + 1).max(1))
                } else {
                    let range = hi - lo;
                    if plateau >= range {
                        rng.next_int_bound((range + 1).max(1)) + lo
                    } else {
                        let ps = (range - plateau) / 2;
                        let pe = range - ps;
                        lo + rng.next_int_bound(pe + 1) + rng.next_int_bound(ps + 1)
                    }
                }
            }
            IntProvider::VeryBiasedToBottom { lo, hi } => {
                let range = (hi - lo + 1).max(1);
                lo + rng.next_int_bound(rng.next_int_bound(rng.next_int_bound(range) + 1) + 1)
            }
        }
    }
}

/// VerticalAnchor (26.3): resolved against the WorldGenerationContext at
/// sample time. SeaLevel = generator.getSeaLevel() + offset (overworld 63).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Anchor {
    Abs(i32),
    AboveBottom(i32),
    BelowTop(i32),
    SeaLevel(i32),
}

impl Anchor {
    #[inline]
    pub fn resolve(&self, min_y: i32, gen_depth: i32, sea_level: i32) -> i32 {
        match *self {
            Anchor::Abs(v) => v,
            Anchor::AboveBottom(v) => min_y + v,
            Anchor::BelowTop(v) => min_y + gen_depth - 1 - v,
            Anchor::SeaLevel(v) => sea_level + v,
        }
    }
}

/// HeightProvider (26.3 heightproviders/*). Uniform/Trapezoid/biased bounds
/// keep their Anchor type — 26.3 vanilla data uses above_bottom/below_top
/// bounds inside uniform heights (89 occurrences: ores, patches); treating
/// them as absolute was a latent legacy bug.
/// Trapezoid plateau: -1 = plain triangle (vanilla TrapezoidHeight.of(min,max)).
#[derive(Clone, Copy, Debug)]
pub enum HeightProvider {
    Abs(i32),
    AboveBottom(i32),
    BelowTop(i32),
    SeaLevel(i32),
    Uniform { lo: Anchor, hi: Anchor },
    Trapezoid { lo: Anchor, hi: Anchor, plateau: i32 },
    /// 26.3 BiasedToBottomHeight: guard max-min-inner+1 <= 0 -> min (no draw);
    /// limit = nextInt(max-min-inner+1); nextInt(limit+inner) + min.
    BiasedToBottom { lo: Anchor, hi: Anchor, inner: i32 },
    /// 26.3 VeryBiasedToBottomHeight: same guard, three nested
    /// randomBetweenInclusive draws.
    VeryBiasedToBottom { lo: Anchor, hi: Anchor, inner: i32 },
    /// 26.3 WeightedListHeight: nextInt(totalWeight), then the vanilla
    /// selection walk (selection -= weight; < 0 -> entry) -> delegate.
    WeightedList { entries: &'static [(&'static HeightProvider, u32)] },
}

impl HeightProvider {
    /// vanilla HeightProvider.sample: resolve anchors, then the per-type draw.
    /// gen_depth = max_y - min_y + 1; sea_level = generator.getSeaLevel().
    pub fn sample(&self, rng: &mut WorldgenRandom, min_y: i32, gen_depth: i32, sea_level: i32) -> i32 {
        match *self {
            HeightProvider::Abs(v) => v,
            HeightProvider::AboveBottom(v) => min_y + v,
            HeightProvider::BelowTop(v) => min_y + gen_depth - 1 - v,
            HeightProvider::SeaLevel(v) => sea_level + v,
            HeightProvider::Uniform { lo, hi } => {
                let a = lo.resolve(min_y, gen_depth, sea_level);
                let b = hi.resolve(min_y, gen_depth, sea_level);
                // vanilla UniformHeight: min > max -> return min (NO draw);
                // equal bounds still draw (nextInt(1))
                if a > b {
                    a
                } else {
                    rng.next_int_bound(b - a + 1) + a
                }
            }
            HeightProvider::Trapezoid { lo, hi, plateau } => {
                let a = lo.resolve(min_y, gen_depth, sea_level);
                let b = hi.resolve(min_y, gen_depth, sea_level);
                if a > b {
                    a
                } else {
                    let span = b - a;
                    if plateau >= span {
                        rng.next_int_bound(span + 1) + a
                    } else {
                        let i3 = (span - plateau) / 2;
                        let i4 = span - i3;
                        a + rng.next_int_bound(i4 + 1) + rng.next_int_bound(i3 + 1)
                    }
                }
            }
            HeightProvider::BiasedToBottom { lo, hi, inner } => {
                let a = lo.resolve(min_y, gen_depth, sea_level);
                let b = hi.resolve(min_y, gen_depth, sea_level);
                if b - a - inner + 1 <= 0 {
                    a
                } else {
                    let limit = rng.next_int_bound(b - a - inner + 1);
                    rng.next_int_bound(limit + inner) + a
                }
            }
            HeightProvider::VeryBiasedToBottom { lo, hi, inner } => {
                let a = lo.resolve(min_y, gen_depth, sea_level);
                let b = hi.resolve(min_y, gen_depth, sea_level);
                if b - a - inner + 1 <= 0 {
                    a
                } else {
                    // Mth.nextInt(r, lo, hi) = lo + nextInt(hi - lo + 1)
                    let upper = (a + inner) + rng.next_int_bound(b - a - inner + 1);
                    let biased = a + rng.next_int_bound(upper - 1 - a + 1);
                    a + rng.next_int_bound(biased - 1 + inner - a + 1)
                }
            }
            HeightProvider::WeightedList { entries } => {
                let total: u32 = entries.iter().map(|(_, w)| w).sum();
                let mut sel = rng.next_int_bound(total as i32) as u32;
                for (hp, w) in entries {
                    if sel < *w {
                        return hp.sample(rng, min_y, gen_depth, sea_level);
                    }
                    sel -= *w;
                }
                // unreachable for non-empty entries (total == sum of weights)
                entries.last().map(|(hp, _)| *hp).unwrap_or(&HeightProvider::Abs(0))
                    .sample(rng, min_y, gen_depth, sea_level)
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
    /// pos.atY(height.sample(rng, ctx)) — Uniform/Trapezoid/biased over resolved anchors
    HeightRange(HeightProvider),
    /// 26.3 OffsetPlacement: three separate providers, draw order x -> y -> z
    /// (legacy "random_offset" xz_spread/y_spread stays on RandomOffset).
    Offset3 { x: IntProvider, y: IntProvider, z: IntProvider },
    /// 26.3 RandomChancePlacement (PlacementFilter): nextFloat() < chance
    RandomChance(f32),
    /// 26.3 FixedPlacement: static position list; only positions inside the
    /// origin's chunk (SectionPos.blockToSectionCoord) pass. No RNG.
    Fixed(&'static [(i32, i32, i32)]),
    /// 26.3 CuboidPlacement: draws y_size, then xz_size TWICE (width, length);
    /// x-outer/y/z-inner emission with edge/interior filter.
    Cuboid { xz: IntProvider, y: IntProvider, include_edges: bool, include_interior: bool },
}

/// Depth-first expansion of the modifier chain — equivalent to vanilla's lazy stream pull
/// AND to the 26.3 FeaturePlacer iterative stack (reversed push = same order).
/// min_y/gen_depth/sea_level feed Anchor resolution (pass the real dimension values).
pub fn expand_chain(
    mods: &[Modifier],
    rng: &mut WorldgenRandom,
    pos: BlockPos,
    out: &mut Vec<BlockPos>,
) {
    expand_rec(mods, 0, rng, pos, out, 0, 384, 63);
}

pub fn expand_chain_ctx(
    mods: &[Modifier],
    rng: &mut WorldgenRandom,
    pos: BlockPos,
    out: &mut Vec<BlockPos>,
    min_y: i32,
    gen_depth: i32,
    sea_level: i32,
) {
    expand_rec(mods, 0, rng, pos, out, min_y, gen_depth, sea_level);
}

fn expand_rec(
    mods: &[Modifier],
    idx: usize,
    rng: &mut WorldgenRandom,
    pos: BlockPos,
    out: &mut Vec<BlockPos>,
    min_y: i32,
    gen_depth: i32,
    sea_level: i32,
) {
    if idx == mods.len() {
        out.push(pos);
        return;
    }
    match mods[idx] {
            Modifier::InSquare => {
            let x = rng.next_int_bound(16) + pos.x;
            let z = rng.next_int_bound(16) + pos.z;
            expand_rec(mods, idx + 1, rng, BlockPos { x, y: pos.y, z }, out, min_y, gen_depth, sea_level);
        }
        Modifier::Count(p) => {
            let n = p.sample(rng).max(0);
            for _ in 0..n {
                expand_rec(mods, idx + 1, rng, pos, out, min_y, gen_depth, sea_level);
            }
        }
        Modifier::RarityFilter { chance } => {
            if rng.next_f32() < 1.0 / (chance as f32) {
                expand_rec(mods, idx + 1, rng, pos, out, min_y, gen_depth, sea_level);
            }
        }
        Modifier::RandomOffset { xz, y } => {
            let x = pos.x + xz.sample(rng);
            let ny = pos.y + y.sample(rng);
            let z = pos.z + xz.sample(rng);
            expand_rec(mods, idx + 1, rng, BlockPos::new(x, ny, z), out, min_y, gen_depth, sea_level);
        }
        Modifier::HeightRange(hp) => {
            let h = hp.sample(rng, min_y, gen_depth, sea_level);
            expand_rec(mods, idx + 1, rng, BlockPos { y: h, ..pos }, out, min_y, gen_depth, sea_level);
        }
        Modifier::Offset3 { x, y, z } => {
            let nx = pos.x + x.sample(rng);
            let ny = pos.y + y.sample(rng);
            let nz = pos.z + z.sample(rng);
            expand_rec(mods, idx + 1, rng, BlockPos::new(nx, ny, nz), out, min_y, gen_depth, sea_level);
        }
        Modifier::RandomChance(chance) => {
            if rng.next_f32() < chance {
                expand_rec(mods, idx + 1, rng, pos, out, min_y, gen_depth, sea_level);
            }
        }
        Modifier::Fixed(positions) => {
            let cx = pos.x >> 4;
            let cz = pos.z >> 4;
            for &(px, py, pz) in positions {
                if (px >> 4) == cx && (pz >> 4) == cz {
                    expand_rec(mods, idx + 1, rng, BlockPos::new(px, py, pz), out, min_y, gen_depth, sea_level);
                }
            }
        }
        Modifier::Cuboid { xz, y, include_edges, include_interior } => {
            let height = y.sample(rng);
            let width = xz.sample(rng);
            let length = xz.sample(rng);
            for x in 0..=width {
                for yy in 0..=height {
                    for z in 0..=length {
                        if (include_edges || x != 0 && x != width || yy != 0 && yy != height)
                            && (include_edges || z != 0 && z != length || yy != 0 && yy != height)
                            && (include_edges || x != 0 && x != width || z != 0 && z != length)
                            && (include_interior || x == 0 || x == width || yy == 0 || yy == height || z == 0 || z == length)
                        {
                            expand_rec(mods, idx + 1, rng, BlockPos::new(pos.x + x, pos.y + yy, pos.z + z), out, min_y, gen_depth, sea_level);
                        }
                    }
                }
            }
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
            HeightRange(HeightProvider::Uniform { lo: Anchor::Abs(-32), hi: Anchor::Abs(64) }),
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
        // UniformHeight empty range (26.3): min > max guard returns min with
        // NO draw (decompile TrapezoidHeight/UniformHeight sample())
        assert_eq!(HeightProvider::Uniform { lo: Anchor::Abs(10), hi: Anchor::Abs(4) }.sample(&mut rng, 0, 384, 63), 10);
        assert_eq!(rng.count(), before, "26.3 empty range guard must not burn RNG");
        // above_bottom/below_top bounds resolve against the ctx, not absolute
        let v = HeightProvider::Uniform { lo: Anchor::AboveBottom(0), hi: Anchor::AboveBottom(16) }
            .sample(&mut rng, -64, 384, 63)
            - (-64);
        assert!((0..=16).contains(&v), "above_bottom bound must resolve against min_y, got offset {v}");
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
