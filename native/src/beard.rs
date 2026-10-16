//! Vanilla `Beardifier` port — structure terrain-adaptation density added to
//! final_density inside the material rule's CacheAllInCell wrapper.
//!
//! Vanilla chain (NoiseBasedChunkGenerator.doFill):
//!   densityFunction = cacheAllInCell(add(finalDensity, BeardifierMarker))
//!   substance = densityFunction.compute(block) = lerp3(finalDensity) + beard(x,y,z)
//! The plugin collects the piece/junction lists via the real vanilla
//! `Beardifier.forStructuresInChunk` collection semantics and ships them to
//! the native core per chunk (`setBeard`), preserving the vanilla iteration
//! order so the f64 summation is bit-identical.

use std::sync::OnceLock;

pub const ADJ_NONE: i32 = 0;
pub const ADJ_BURY: i32 = 1;
pub const ADJ_BEARD_THIN: i32 = 2;
pub const ADJ_BEARD_BOX: i32 = 3;
pub const ADJ_ENCAPSULATE: i32 = 4;

const BEARD_KERNEL_RADIUS: i32 = 12;
const BEARD_KERNEL_SIZE: usize = 24;

#[derive(Clone, Copy, Debug)]
pub struct Piece {
    pub min_x: i32,
    pub min_y: i32,
    pub min_z: i32,
    pub max_x: i32,
    pub max_y: i32,
    pub max_z: i32,
    /// TerrainAdjustment ordinal (vanilla enum order: NONE, BURY, BEARD_THIN,
    /// BEARD_BOX, ENCAPSULATE).
    pub adjustment: i32,
    pub ground_delta: i32,
}

#[derive(Clone, Copy, Debug)]
pub struct Junction {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

#[derive(Default, Clone, Debug)]
pub struct Beard {
    pub pieces: Vec<Piece>,
    pub junctions: Vec<Junction>,
}

impl Beard {
    /// Decode from the plugin serialization:
    /// `[nPieces, (minX,minY,minZ,maxX,maxY,maxZ,adj,delta)*, nJunctions, (x,y,z)*]`
    pub fn decode(data: &[i32]) -> Beard {
        let mut out = Beard::default();
        if data.len() < 4 {
            return out;
        }
        let n_pieces = data[0] as usize;
        let mut p = 1;
        for _ in 0..n_pieces {
            if p + 8 > data.len() {
                return out;
            }
            out.pieces.push(Piece {
                min_x: data[p],
                min_y: data[p + 1],
                min_z: data[p + 2],
                max_x: data[p + 3],
                max_y: data[p + 4],
                max_z: data[p + 5],
                adjustment: data[p + 6],
                ground_delta: data[p + 7],
            });
            p += 8;
        }
        if p >= data.len() {
            return out;
        }
        let n_j = data[p] as usize;
        p += 1;
        for _ in 0..n_j {
            if p + 3 > data.len() {
                return out;
            }
            out.junctions.push(Junction {
                x: data[p],
                y: data[p + 1],
                z: data[p + 2],
            });
            p += 3;
        }
        out
    }

    pub fn is_empty(&self) -> bool {
        self.pieces.is_empty() && self.junctions.is_empty()
    }

    /// Vanilla Beardifier.compute — summation order preserved (pieces, then
    /// junctions; sequential `+=` exactly like the iterator loops).
    pub fn contribution(&self, x: i32, y: i32, z: i32) -> f64 {
        let mut d = 0.0f64;
        for piece in &self.pieces {
            let max = 0.max((piece.min_x - x).max(x - piece.max_x));
            let max1 = 0.max((piece.min_z - z).max(z - piece.max_z));
            let i3 = piece.min_y + piece.ground_delta;
            let i4 = y - i3;
            let i5: i32 = match piece.adjustment {
                ADJ_NONE => 0,
                ADJ_BURY | ADJ_BEARD_THIN => i4,
                ADJ_BEARD_BOX => 0.max((i3 - y).max(y - piece.max_y)),
                ADJ_ENCAPSULATE => 0.max((piece.min_y - y).max(y - piece.max_y)),
                _ => 0,
            };
            d += match piece.adjustment {
                ADJ_NONE => 0.0,
                ADJ_BURY => get_bury_contribution(max as f64, i5 as f64 / 2.0, max1 as f64),
                ADJ_BEARD_THIN | ADJ_BEARD_BOX => get_beard_contribution(max, i5, max1, i4) * 0.8,
                ADJ_ENCAPSULATE => {
                    get_bury_contribution(max as f64 / 2.0, i5 as f64 / 2.0, max1 as f64 / 2.0) * 0.8
                }
                _ => 0.0,
            };
        }
        for j in &self.junctions {
            let dx = x - j.x;
            let dy = y - j.y;
            let dz = z - j.z;
            d += get_beard_contribution(dx, dy, dz, dy) * 0.4;
        }
        d
    }
}

/// Vanilla Mth.fastInvSqrt (f64 magic-constant Newton step) — bit-exact.
#[inline]
fn fast_inv_sqrt(mut number: f64) -> f64 {
    let d = 0.5 * number;
    let mut l = number.to_bits() as i64;
    l = 6910469410427058090i64.wrapping_sub(l >> 1);
    number = f64::from_bits(l as u64);
    number * (1.5 - d * number * number)
}

#[inline]
fn length_squared3(x: f64, y: f64, z: f64) -> f64 {
    x * x + y * y + z * z
}

/// Vanilla Beardifier.getBeardContribution — kernel lookup with the height
/// falloff multiplier; kernel is float-rounded exactly like vanilla's
/// float[] BEARD_KERNEL.
#[inline]
fn get_beard_contribution(x: i32, y: i32, z: i32, height: i32) -> f64 {
    let i = x + BEARD_KERNEL_RADIUS;
    let i1 = y + BEARD_KERNEL_RADIUS;
    let i2 = z + BEARD_KERNEL_RADIUS;
    if (0..BEARD_KERNEL_SIZE as i32).contains(&i)
        && (0..BEARD_KERNEL_SIZE as i32).contains(&i1)
        && (0..BEARD_KERNEL_SIZE as i32).contains(&i2)
    {
        let d = height as f64 + 0.5;
        let d1 = length_squared3(x as f64, d, z as f64);
        let d2 = -d * fast_inv_sqrt(d1 / 2.0) / 2.0;
        d2 * kernel()[i2 as usize * 576 + i as usize * 24 + i1 as usize] as f64
    } else {
        0.0
    }
}

/// Vanilla Beardifier.getBuryContribution.
#[inline]
fn get_bury_contribution(x: f64, y: f64, z: f64) -> f64 {
    let len = length_squared3(x, y, z).sqrt();
    clamped_map(len, 0.0, 6.0, 1.0, 0.0)
}

/// Vanilla Mth.clampedMap: clampedLerp(outMin, outMax, inverseLerp(in, inMin, inMax)).
#[inline]
fn clamped_map(input: f64, in_min: f64, in_max: f64, out_min: f64, out_max: f64) -> f64 {
    let delta = (input - in_min) / (in_max - in_min);
    if delta < 0.0 {
        out_min
    } else if delta > 1.0 {
        out_max
    } else {
        // Mth.lerp(delta, start, end) = start + delta * (end - start)
        out_min + delta * (out_max - out_min)
    }
}

/// BEARD_KERNEL[i*576 + i1*24 + i2] = computeBeardContribution(i1-12, i2-12, i-12)
/// where computeBeardContribution(x,y,z) = exp(-(x^2 + (y+0.5)^2 + z^2)/16),
/// float-rounded exactly like vanilla's float[] table.
static KERNEL: OnceLock<[f32; 13824]> = OnceLock::new();

fn kernel() -> &'static [f32; 13824] {
    KERNEL.get_or_init(|| {
        let mut array = [0f32; 13824];
        for i in 0..24i32 {
            for i1 in 0..24i32 {
                for i2 in 0..24i32 {
                    let x = (i1 - 12) as f64;
                    let y = (i2 - 12) as f64 + 0.5;
                    let z = (i - 12) as f64;
                    let d = length_squared3(x, y, z);
                    array[(i * 576 + i1 * 24 + i2) as usize] = (-d / 16.0).exp() as f32;
                }
            }
        }
        array
    })
}
