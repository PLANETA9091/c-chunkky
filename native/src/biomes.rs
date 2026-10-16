// Native biome resolution for world-datapack multi_noise biome sources.
//
// Bit-exact port of vanilla 1.21.8 Climate.RTree / ParameterList /
// MultiNoiseBiomeSource semantics (Mojang mappings), including:
// - quantizeCoord: (long)(f32_value * 10000.0f32) — f32 multiply, truncation
//   toward zero (Java (long) cast semantics; NaN -> 0);
// - the RTree build: greedy per-dimension bucketization minimizing the sum
//   of subtree span costs, stable sorts over i64 midpoints (wrapping adds,
//   truncating division), CHILDREN_PER_NODE = 6;
// - the search: strict-< descent with the ThreadLocal warm-start leaf
//   (vanilla lastResult) — the query ORDER is part of the semantics, so
//   fill_biomes walks quarts in the exact vanilla fillBiomesFromNoise order
//   (sections ascending, then x, y, z within a section) and keeps the
//   warm-start in a per-thread cell;
// - climate sampling: the RAW router density functions (temperature,
//   vegetation, continents, erosion, depth, ridges) evaluated at the quart's
//   block coordinates in Direct mode (vanilla Sampler uses
//   RandomState.router() unwrapped functions + SinglePointContext; the
//   f64 -> f32 cast happens BEFORE quantizeCoord).

use crate::density::{InterpState, Mode, Pt};
use crate::worldgen::Assets;
use std::cell::Cell;

pub const N_PARAMS: usize = 7;

#[inline]
fn quantize(v: f64) -> i64 {
    let f = v as f32;
    let q = f * 10000.0f32;
    if q.is_nan() {
        0
    } else {
        q as i64
    }
}

/// [min, max] climate parameter range in quantized long space.
#[derive(Clone, Copy, Debug)]
pub struct Param {
    pub min: i64,
    pub max: i64,
}

impl Param {
    #[inline]
    fn distance(&self, v: i64) -> i64 {
        let up = v.wrapping_sub(self.max);
        let down = self.min.wrapping_sub(v);
        if up > 0 {
            up
        } else {
            down.max(0)
        }
    }

    #[inline]
    fn span(&self, other: Option<Param>) -> Param {
        match other {
            None => *self,
            Some(o) => Param {
                min: self.min.min(o.min),
                max: self.max.max(o.max),
            },
        }
    }
}

#[derive(Clone)]
pub struct BiomeEntry {
    /// Stable index into the ctx-level biome name table (Java resolves the
    /// names to registry ids once at boot).
    pub biome_idx: u32,
    pub params: [Param; N_PARAMS], // T, H, C, E, D, W, offset-range
}

// ---------------------------------------------------------------------------
// RTree
// ---------------------------------------------------------------------------

const CHILDREN_PER_NODE: usize = 6;

#[derive(Clone)]
enum Node {
    Leaf(usize),
    SubTree {
        space: Box<[Param; N_PARAMS]>,
        children: Vec<Node>,
    },
}

#[inline]
fn mid_of(p: &Param, absolute: bool) -> i64 {
    // Java: (parameter.min() + parameter.max()) / 2L — wrapping add, Java
    // long division truncates toward zero like Rust i64 /, Math.abs wraps.
    let m = p.min.wrapping_add(p.max) / 2;
    if absolute {
        m.wrapping_abs()
    } else {
        m
    }
}

fn node_space<'a>(node: &'a Node, entries: &'a [BiomeEntry]) -> &'a [Param; N_PARAMS] {
    match node {
        Node::Leaf(i) => &entries[*i].params,
        Node::SubTree { space, .. } => space,
    }
}

#[inline]
fn dist_sum(space: &[Param; N_PARAMS], values: &[i64; N_PARAMS]) -> i64 {
    let mut l: i64 = 0;
    for i in 0..N_PARAMS {
        let d = space[i].distance(values[i]);
        l = l.wrapping_add(d.wrapping_mul(d));
    }
    l
}

/// Java List.sort over a thenComparing chain of per-dim midpoint keys.
fn sort_nodes(children: &mut [Node], entries: &[BiomeEntry], dim0: usize, absolute: bool) {
    children.sort_by(|a, b| {
        for i in 0..N_PARAMS {
            let d = (dim0 + i) % N_PARAMS;
            let ka = mid_of(&node_space(a, entries)[d], absolute);
            let kb = mid_of(&node_space(b, entries)[d], absolute);
            if ka != kb {
                return ka.cmp(&kb);
            }
        }
        std::cmp::Ordering::Equal
    });
}

fn bucketize(nodes: &[Node]) -> Vec<Vec<Node>> {
    // int i = (int)Math.pow(6.0, Math.floor(Math.log(size - 0.01) / Math.log(6.0)));
    let n = nodes.len();
    let bucket = if n == 0 {
        1
    } else {
        let b = 6.0f64.powf(((n as f64 - 0.01).ln() / 6.0f64.ln()).floor());
        let b = b as usize;
        if b == 0 {
            1
        } else {
            b
        }
    };
    let mut out = Vec::new();
    let mut cur = Vec::new();
    for node in nodes {
        cur.push(node.clone());
        if cur.len() >= bucket {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn subtree_space(children: &[Node], entries: &[BiomeEntry]) -> Box<[Param; N_PARAMS]> {
    let mut space: [Option<Param>; N_PARAMS] = Default::default();
    for node in children {
        let s = node_space(node, entries);
        for d in 0..N_PARAMS {
            space[d] = Some(s[d].span(space[d]));
        }
    }
    Box::new(std::array::from_fn(|d| space[d].unwrap()))
}

fn build(mut children: Vec<Node>, entries: &[BiomeEntry]) -> Node {
    if children.is_empty() {
        panic!("biome RTree: no children");
    }
    if children.len() == 1 {
        return children.pop().unwrap();
    }
    if children.len() <= CHILDREN_PER_NODE {
        // Java: sort by sum over dims of Math.abs(mid) (wrapping)
        children.sort_by_key(|node| {
            let s = node_space(node, entries);
            let mut l: i64 = 0;
            for d in 0..N_PARAMS {
                l = l.wrapping_add(mid_of(&s[d], true));
            }
            l
        });
        let space = subtree_space(&children, entries);
        return Node::SubTree {
            space,
            children,
        };
    }
    let mut best_cost = i64::MAX;
    let mut best_dim = 0usize;
    let mut best_buckets: Vec<Vec<Node>> = Vec::new();
    for dim in 0..N_PARAMS {
        let mut ch = children.clone();
        sort_nodes(&mut ch, entries, dim, false);
        let buckets = bucketize(&ch);
        let mut cost: i64 = 0;
        for b in &buckets {
            let space = subtree_space(b, entries);
            for d in 0..N_PARAMS {
                cost = cost.wrapping_add(space[d].max.wrapping_sub(space[d].min).wrapping_abs());
            }
        }
        if best_cost > cost {
            best_cost = cost;
            best_dim = dim;
            best_buckets = buckets;
        }
    }
    // Java: sort(list, paramSpaceSize, i, true) — the SubTree list by |mid|.
    let mut subs: Vec<(Box<[Param; N_PARAMS]>, Vec<Node>)> = best_buckets
        .into_iter()
        .map(|b| {
            let space = subtree_space(&b, entries);
            (space, b)
        })
        .collect();
    subs.sort_by_key(|(space, _)| mid_of(&space[best_dim], true));
    let children: Vec<Node> = subs.into_iter().map(|(_, b)| build(b, entries)).collect();
    let space = subtree_space(&children, entries);
    Node::SubTree {
        space,
        children,
    }
}

// ---------------------------------------------------------------------------
// Compiled biome source
// ---------------------------------------------------------------------------

pub struct BiomeSourceData {
    /// Deduplicated biome names (stable order = first appearance).
    pub biome_names: Vec<String>,
    pub entries: Vec<BiomeEntry>,
    root: Node,
}

impl BiomeSourceData {
    /// Parse the winning multi_noise biome source from the pack assets
    /// (dimension/overworld.json -> generator.biome_source).
    pub fn from_assets(assets: &Assets) -> Option<BiomeSourceData> {
        let mut dim_path: Option<&String> = None;
        for p in assets.paths() {
            if p.ends_with("/dimension/overworld.json") {
                dim_path = match dim_path {
                    Some(cur) if cur <= p => Some(cur),
                    _ => Some(p),
                };
            }
        }
        let dp = dim_path?;
        let text = std::str::from_utf8(assets.get(dp)?).ok()?;
        let json = crate::json::parse(text).ok()?;
        let bs = json.get("generator")?.get("biome_source")?;
        let ty = bs.get("type").and_then(crate::json::Json::as_str)?;
        if ty != "minecraft:multi_noise" {
            return None;
        }
        if bs.get("preset").is_some() {
            // vanilla preset lists are not data-driven; unsupported for now
            return None;
        }
        let list = bs.get("biomes")?.as_arr()?;
        let mut names: Vec<String> = Vec::new();
        let mut entries: Vec<BiomeEntry> = Vec::new();
        for el in list {
            let name = el.get("biome").and_then(crate::json::Json::as_str)?;
            let biome_idx = match names.iter().position(|n| n == name) {
                Some(i) => i as u32,
                None => {
                    names.push(name.to_string());
                    (names.len() - 1) as u32
                }
            };
            let p = el.get("parameters")?;
            let get = |key: &str| -> Option<(i64, i64)> {
                let v = p.get(key)?;
                if let Some(arr) = v.as_arr() {
                    if arr.len() != 2 {
                        return None;
                    }
                    let a = arr[0].as_f64()?;
                    let b = arr[1].as_f64()?;
                    Some((quantize(a), quantize(b)))
                } else {
                    let a = v.as_f64()?;
                    let q = quantize(a);
                    Some((q, q))
                }
            };
            let (t, h, c, e, d, w, o) = (
                get("temperature")?,
                get("humidity")?,
                get("continentalness")?,
                get("erosion")?,
                get("depth")?,
                get("weirdness")?,
                get("offset")?,
            );
            entries.push(BiomeEntry {
                biome_idx,
                params: [
                    Param { min: t.0, max: t.1 },
                    Param { min: h.0, max: h.1 },
                    Param { min: c.0, max: c.1 },
                    Param { min: e.0, max: e.1 },
                    Param { min: d.0, max: d.1 },
                    Param { min: w.0, max: w.1 },
                    Param { min: o.0, max: o.1 },
                ],
            });
        }
        if entries.is_empty() {
            return None;
        }
        let leaves: Vec<Node> = (0..entries.len()).map(Node::Leaf).collect();
        let root = build(leaves, &entries);
        Some(BiomeSourceData {
            biome_names: names,
            entries,
            root,
        })
    }

    #[inline]
    fn leaf_distance(&self, leaf: usize, values: &[i64; N_PARAMS]) -> i64 {
        dist_sum(&self.entries[leaf].params, values)
    }

    /// Vanilla RTree.search with the warm-start leaf; returns the leaf index.
    fn search(&self, node: &Node, values: &[i64; N_PARAMS], last: Option<usize>) -> usize {
        match node {
            Node::Leaf(i) => *i,
            Node::SubTree { children, .. } => {
                let mut best_dist = match last {
                    Some(li) => self.leaf_distance(li, values),
                    None => i64::MAX,
                };
                let mut best = last;
                for child in children {
                    let d1 = dist_sum(node_space(child, &self.entries), values);
                    if best_dist > d1 {
                        let leaf2 = self.search(child, values, best);
                        let is_same = matches!(child, Node::Leaf(i) if *i == leaf2);
                        let d2 = if is_same {
                            d1
                        } else {
                            self.leaf_distance(leaf2, values)
                        };
                        if best_dist > d2 {
                            best_dist = d2;
                            best = Some(leaf2);
                        }
                    }
                }
                match best {
                    Some(b) => b,
                    None => match children.first() {
                        Some(Node::Leaf(i)) => *i,
                        Some(_) => self.search(&children[0], values, None),
                        None => 0,
                    },
                }
            }
        }
    }

    /// Vanilla-order warm-start search for one quart.
    #[inline]
    fn find_value(&self, values: &[i64; N_PARAMS]) -> usize {
        let tree_id = self as *const BiomeSourceData as usize;
        let (_, last) = SEARCH_LAST.get();
        let last = if SEARCH_LAST.get().0 == tree_id { last } else { None };
        let leaf = self.search(&self.root, values, last);
        SEARCH_LAST.set((tree_id, Some(leaf)));
        leaf
    }
}

thread_local! {
    /// Vanilla RTree.lastResult — per-thread warm-start across chunks, keyed
    /// by the tree instance (vanilla keeps the ThreadLocal per RTree; sharing
    /// across trees could bias tie-breaks with a foreign leaf index).
    static SEARCH_LAST: Cell<(usize, Option<usize>)> = const { Cell::new((0, None)) };
}

/// Diagnostic timers (ns, accumulated; reset by the caller).
pub static SEARCH_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static EVAL_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

// ---------------------------------------------------------------------------
// Climate sampling + per-chunk fill
// ---------------------------------------------------------------------------

pub struct ClimateDFs {
    /// Router arena indices: temperature, vegetation, continents, erosion,
    /// depth, ridges.
    pub idx: [u32; 6],
    /// Per-field compile-time y-invariance (Direct mode). Fields flagged
    /// true may reuse the per-column (x, z) quart value across all y.
    pub y_invariant: [bool; 6],
}

impl ClimateDFs {
    pub fn from_router(
        router: &std::collections::HashMap<String, u32>,
        env: &crate::density::Env,
        compiled: &crate::density::Compiled,
    ) -> Option<ClimateDFs> {
        let g = |k: &str| router.get(k).copied();
        let idx = [
            g("temperature")?,
            g("vegetation")?,
            g("continents")?,
            g("erosion")?,
            g("depth")?,
            g("ridges")?,
        ];
        let y_invariant = std::array::from_fn(|i| env.df_y_invariant(compiled, idx[i]));
        Some(ClimateDFs { idx, y_invariant })
    }
}

/// Fill the chunk's biome quart indexes in the exact vanilla
/// fillBiomesFromNoise order (sections ascending; x, y, z within a section).
/// out.len() must be sections * 64; layout: out[s*64 + x*16 + y*4 + z].
pub fn fill_biomes(
    src: &BiomeSourceData,
    climate: &ClimateDFs,
    env: &crate::density::Env,
    compiled: &crate::density::Compiled,
    cx: i32,
    cz: i32,
    out: &mut [i32],
) {
    let height = env.wg.height as usize;
    let sections = height / 16;
    let quart_x0 = cx * 4;
    let quart_z0 = cz * 4;
    let quart_y0_base = env.wg.min_y / 4;
    let interp = InterpState::new(compiled.interp_slots, 4, height / 8);
    // Climate DFs: per (x, z) quart column the 5 y-independent fields are
    // evaluated once; depth is per quart (y-dependent).
    let mut col_cache: [Option<[i64; 6]>; 16] = Default::default(); // [x*4+z]
    // Scalar CSE + column cache (y-invariant Cache2d/FlatCache subtrees —
    // e.g. the terrain offset forest under depth) across all quart evals.
    let mut memo = crate::density::MemoCtx::new(compiled.arena.len())
        .with_cols(compiled.col_count as usize);
    for sy in 0..sections {
        let quart_y0 = quart_y0_base + (sy as i32) * 4;
        for i1 in 0..4i32 {
            for i2 in 0..4i32 {
                for i3 in 0..4i32 {
                    let qx = quart_x0 + i1;
                    let qz = quart_z0 + i3;
                    let col = (i1 * 4 + i3) as usize;
                    let qy = quart_y0 + i2;
                    let pt = Pt {
                        x: qx * 4,
                        y: qy * 4,
                        z: qz * 4,
                    };
                    // Per-column cached fields; y-variant fields (per the
                    // compile-time check) re-evaluate at every quart.
                    let cached = climate.y_invariant;
                    let t_eval = std::time::Instant::now();
                    memo.next_serial();
                    let getq = |fi: usize| -> i64 {
                        quantize(env.eval(
                            compiled,
                            climate.idx[fi],
                            pt,
                            Mode::Direct,
                            &interp,
                            Some(&memo),
                        ))
                    };
                    let cols = if let Some(c) = col_cache[col] {
                        c
                    } else {
                        let mut c = [0i64; 6];
                        for fi in 0..6usize {
                            if fi == 4 {
                                continue; // depth is per-quart below
                            }
                            c[fi] = getq(fi);
                        }
                        col_cache[col] = Some(c);
                        c
                    };
                    let depth = getq(4);
                    let pick = |fi: usize| -> i64 {
                        if fi == 4 {
                            depth
                        } else if cached[fi] {
                            cols[fi]
                        } else {
                            getq(fi)
                        }
                    };
                    let values = [
                        pick(0), // temperature
                        pick(1), // humidity (vegetation)
                        pick(2), // continentalness (continents)
                        pick(3), // erosion
                        depth,   // depth
                        pick(5), // weirdness (ridges)
                        0,       // offset
                    ];
                    EVAL_NS.fetch_add(t_eval.elapsed().as_nanos() as u64, std::sync::atomic::Ordering::Relaxed);
                    let t_search = std::time::Instant::now();
                    let leaf = src.find_value(&values);
                    SEARCH_NS.fetch_add(t_search.elapsed().as_nanos() as u64, std::sync::atomic::Ordering::Relaxed);
                    let bi = src.entries[leaf].biome_idx as i32;
                    out[sy * 64 + (i1 as usize) * 16 + (i2 as usize) * 4 + i3 as usize] = bi;
                }
            }
        }
    }
}
