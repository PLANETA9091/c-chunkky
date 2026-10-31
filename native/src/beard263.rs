//! Vanilla 26.3 `Beardifier` port — FLOAT-accurate (26.3 rewrote the 1.21.8
//! f64 Beardifier to f32 accumulation).
//!
//! Semantics frozen from reference/dec263/.../Beardifier.java (228 lines):
//!   - sampleValueUnchecked: f32 `+=` over pieces (order preserved) then
//!     junctions, all math f32 (`getBuryContribution` branches at 36.0F,
//!     `getBeardContribution` computes distanceSqr in f32, feeds f64
//!     fastInvSqrt, result cast f32, kernel lookup f32).
//!   - BEARD_KERNEL[zi*576 + xi*24 + yi] — same layout/indexing as the 1.21.8
//!     port in beard.rs (reuse kernel()).
//!   - affectedBox: union of piece boxes + junction point boxes, inflated
//!     by 24; point path returns 0 outside, volume path fills 0 outside
//!     (Beardifier.sampleVolume fills the whole buffer).
//!   - Registered as a SamplerContext user field (Beardifier.CONTEXT_KEY);
//!     final_density = add(min(postProcessed, noodle), ContextBoundSampler).

use crate::beard::{Junction, Piece, ADJ_BEARD_BOX, ADJ_BEARD_THIN, ADJ_BURY, ADJ_ENCAPSULATE, ADJ_NONE};
use crate::df263::DensityVolume;
use std::sync::OnceLock;

pub struct Beardifier263 {
    pub pieces: Vec<Piece>,
    pub junctions: Vec<Junction>,
    /// (min_x, min_y, min_z, max_x, max_y, max_z) — None = EMPTY (no pieces).
    pub affected: Option<(i32, i32, i32, i32, i32, i32)>,
}

impl Beardifier263 {
    pub fn empty() -> Self {
        Beardifier263 {
            pieces: Vec::new(),
            junctions: Vec::new(),
            affected: None,
        }
    }

    /// Decode the plugin serialization (same contract as beard.rs Beard::decode)
    /// and compute the inflated affected box exactly like
    /// Beardifier.forStructuresInChunk + includeBoundingBox.
    pub fn decode(data: &[i32]) -> Self {
        let base = crate::beard::Beard::decode(data);
        let mut affected: Option<(i32, i32, i32, i32, i32, i32)> = None;
        for p in &base.pieces {
            include_box(
                &mut affected,
                (p.min_x, p.min_y, p.min_z, p.max_x, p.max_y, p.max_z),
            );
        }
        for j in &base.junctions {
            include_box(&mut affected, (j.x, j.y, j.z, j.x, j.y, j.z));
        }
        let affected = affected.map(|b| inflate24(b));
        Beardifier263 {
            pieces: base.pieces,
            junctions: base.junctions,
            affected,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.affected.is_none()
    }

    /// Beardifier.sampleValue — 0 outside the affected box.
    #[inline]
    pub fn sample_value(&self, x: i32, y: i32, z: i32) -> f32 {
        match self.affected {
            None => 0.0,
            Some(b) => {
                if x >= b.0 && x <= b.3 && y >= b.1 && y <= b.4 && z >= b.2 && z <= b.5 {
                    self.sample_value_unchecked(x, y, z)
                } else {
                    0.0
                }
            }
        }
    }

    #[inline]
    fn sample_value_unchecked(&self, x: i32, y: i32, z: i32) -> f32 {
        let mut noise_value = 0.0f32;
        for piece in &self.pieces {
            let dx = 0.max((piece.min_x - x).max(x - piece.max_x));
            let dz = 0.max((piece.min_z - z).max(z - piece.max_z));
            let ground_y = piece.min_y + piece.ground_delta;
            let dy_to_ground = y - ground_y;
            match piece.adjustment {
                ADJ_NONE => {}
                ADJ_BURY => {
                    noise_value += get_bury_contribution(
                        dx as f32,
                        dy_to_ground as f32 / 2.0f32,
                        dz as f32,
                    );
                }
                ADJ_BEARD_THIN => {
                    noise_value +=
                        get_beard_contribution(dx, dy_to_ground, dz, dy_to_ground) * 0.8f32;
                }
                ADJ_BEARD_BOX => {
                    let dy = 0.max((ground_y - y).max(y - piece.max_y));
                    noise_value += get_beard_contribution(dx, dy, dz, dy_to_ground) * 0.8f32;
                }
                ADJ_ENCAPSULATE => {
                    let dy = 0.max((piece.min_y - y).max(y - piece.max_y));
                    noise_value += get_bury_contribution(
                        dx as f32 / 2.0f32,
                        dy as f32 / 2.0f32,
                        dz as f32 / 2.0f32,
                    ) * 0.8f32;
                }
                _ => {}
            }
        }
        for j in &self.junctions {
            let dx = x - j.x;
            let dy = y - j.y;
            let dz = z - j.z;
            noise_value += get_beard_contribution(dx, dy, dz, dy) * 0.4f32;
        }
        noise_value
    }

    /// Beardifier.sampleVolume — fill(0) then the affected-box ∩ volume range.
    ///
    /// Column-pruned: a piece whose clamped x/z distance to the column
    /// exceeds its contribution support (kernel xi/zi bounds -> early return
    /// 0 for EVERY y; bury distance_sq >= 36 -> 0) contributes exactly 0.0
    /// to the whole column, so it is dropped from the per-column loop —
    /// bit-exact by construction (the reference path adds 0.0 for it).
    /// Same for the per-piece y-support prune. cont-84d: the unpruned loop
    /// measured ~4.6ms/fill in structure-rich windows (nobeard discriminator,
    /// CI 681905dc: terrain 8.95 vs 4.34 same region).
    pub fn fill_volume(&self, vol: &DensityVolume, out: &mut [f32]) {
        out.fill(0.0);
        let Some(b) = self.affected else { return };
        // volume.intersects(box) — block ranges
        if vol.max_block_x() < b.0 || vol.min_x > b.3 {
            return;
        }
        if vol.max_block_y() < b.1 || vol.min_y > b.4 {
            return;
        }
        if vol.max_block_z() < b.2 || vol.min_z > b.5 {
            return;
        }
        let min_x = i32::max(0, floor_div(b.0 - vol.min_x, vol.step_x));
        let min_y = i32::max(0, floor_div(b.1 - vol.min_y, vol.step_y));
        let min_z = i32::max(0, floor_div(b.2 - vol.min_z, vol.step_z));
        let max_x = i32::min(vol.size_x - 1, floor_div(b.3 - vol.min_x, vol.step_x));
        let max_y = i32::min(vol.size_y - 1, floor_div(b.4 - vol.min_y, vol.step_y));
        let max_z = i32::min(vol.size_z - 1, floor_div(b.5 - vol.min_z, vol.step_z));
        let vol_y_lo = vol.block_y(min_y);
        let vol_y_hi = vol.block_y(max_y);
        // active entries: index into pieces | junctions (pieces.len() offset),
        // with the column-fixed dx/dz hoisted out of the y loop. Junction
        // support is ASYMMETRIC: kernel index = d+12 in [0..24) ->
        // d in [-12..11] (negative side reaches index 0, exp(-9) ~= 1.2e-4
        // — NOT droppable).
        let mut act: Vec<u32> = Vec::with_capacity(self.pieces.len() + self.junctions.len());
        let mut dxs: Vec<i32> = Vec::with_capacity(self.pieces.len() + self.junctions.len());
        let mut dzs: Vec<i32> = Vec::with_capacity(self.pieces.len() + self.junctions.len());
        for z in min_z..=max_z {
            let block_z = vol.block_z(z);
            for x in min_x..=max_x {
                let block_x = vol.block_x(x);
                // per-column active set: pieces + junctions whose x/z support
                // covers this column. Skipped entries add 0.0 for every y of
                // the column in the reference path (bounds above).
                act.clear();
                dxs.clear();
                dzs.clear();
                for (pi, piece) in self.pieces.iter().enumerate() {
                    let dx = 0.max((piece.min_x - block_x).max(block_x - piece.max_x));
                    let supp_x = if piece.adjustment == ADJ_BURY { 5 } else { 11 };
                    if dx > supp_x {
                        continue;
                    }
                    let dz = 0.max((piece.min_z - block_z).max(block_z - piece.max_z));
                    if dz > supp_x {
                        continue;
                    }
                    let ground_y = piece.min_y + piece.ground_delta;
                    let y_lo = match piece.adjustment {
                        ADJ_BEARD_BOX => ground_y - 12,
                        ADJ_ENCAPSULATE => piece.min_y - 12,
                        _ => ground_y - 12,
                    };
                    let y_hi = match piece.adjustment {
                        ADJ_BEARD_BOX => piece.max_y + 12,
                        ADJ_ENCAPSULATE => piece.max_y + 12,
                        _ => ground_y + 12,
                    };
                    if y_lo > vol_y_hi || y_hi < vol_y_lo {
                        continue;
                    }
                    act.push(pi as u32);
                    dxs.push(dx);
                    dzs.push(dz);
                }
                for (ji, j) in self.junctions.iter().enumerate() {
                    let dx = block_x - j.x;
                    if !(-12..=11).contains(&dx) {
                        continue;
                    }
                    let dz = block_z - j.z;
                    if !(-12..=11).contains(&dz) {
                        continue;
                    }
                    if j.y + 12 < vol_y_lo || j.y - 12 > vol_y_hi {
                        continue;
                    }
                    act.push((self.pieces.len() + ji) as u32);
                    dxs.push(dx);
                    dzs.push(dz);
                }
                if act.is_empty() {
                    continue;
                }
                for y in min_y..=max_y {
                    let block_y = vol.block_y(y);
                    let index = vol.index_unchecked(x, y, z);
                    let mut noise_value = 0.0f32;
                    for (k, &a) in act.iter().enumerate() {
                        let dz = dzs[k];
                        if (a as usize) < self.pieces.len() {
                            let piece = &self.pieces[a as usize];
                            let dx = dxs[k];
                            let ground_y = piece.min_y + piece.ground_delta;
                            let dy_to_ground = block_y - ground_y;
                            noise_value += match piece.adjustment {
                                ADJ_BURY => get_bury_contribution(
                                    dx as f32,
                                    dy_to_ground as f32 / 2.0f32,
                                    dz as f32,
                                ),
                                ADJ_BEARD_THIN => get_beard_contribution(
                                    dx,
                                    dy_to_ground,
                                    dz,
                                    dy_to_ground,
                                ) * 0.8f32,
                                ADJ_BEARD_BOX => {
                                    let dy = 0.max((ground_y - block_y).max(block_y - piece.max_y));
                                    get_beard_contribution(dx, dy, dz, dy_to_ground) * 0.8f32
                                }
                                ADJ_ENCAPSULATE => {
                                    let dy = 0.max((piece.min_y - block_y).max(block_y - piece.max_y));
                                    get_bury_contribution(
                                        dx as f32 / 2.0f32,
                                        dy as f32 / 2.0f32,
                                        dz as f32 / 2.0f32,
                                    ) * 0.8f32
                                }
                                _ => 0.0,
                            };
                        } else {
                            let j = &self.junctions[a as usize - self.pieces.len()];
                            let dx = dxs[k];
                            let dy = block_y - j.y;
                            noise_value += get_beard_contribution(dx, dy, dz, dy) * 0.4f32;
                        }
                    }
                    out[index] = noise_value;
                }
            }
        }
    }
}

#[inline]
fn floor_div(a: i32, b: i32) -> i32 {
    // Java Math.floorDiv exact integer semantics.
    let q = a / b;
    if (a % b != 0) && ((a < 0) != (b < 0)) {
        q - 1
    } else {
        q
    }
}

fn include_box(cur: &mut Option<(i32, i32, i32, i32, i32, i32)>, b: (i32, i32, i32, i32, i32, i32)) {
    match *cur {
        None => *cur = Some(b),
        Some(c) => {
            *cur = Some((
                c.0.min(b.0),
                c.1.min(b.1),
                c.2.min(b.2),
                c.3.max(b.3),
                c.4.max(b.4),
                c.5.max(b.5),
            ));
        }
    }
}

fn inflate24(b: (i32, i32, i32, i32, i32, i32)) -> (i32, i32, i32, i32, i32, i32) {
    (
        b.0 - 24,
        b.1 - 24,
        b.2 - 24,
        b.3 + 24,
        b.4 + 24,
        b.5 + 24,
    )
}

/// Vanilla 26.3 getBuryContribution — f32 branch form
/// (`distanceSq >= 36.0F ? 0.0F : 1.0F - Mth.sqrt(distanceSq) / 6.0F`).
#[inline]
fn get_bury_contribution(dx: f32, dy: f32, dz: f32) -> f32 {
    let distance_sq = dx * dx + dy * dy + dz * dz;
    if distance_sq >= 36.0f32 {
        0.0
    } else {
        1.0f32 - mth_sqrt(distance_sq) / 6.0f32
    }
}

/// Mth.sqrt(float) = (float)Math.sqrt((double)x).
#[inline]
fn mth_sqrt(x: f32) -> f32 {
    (x as f64).sqrt() as f32
}

/// Vanilla 26.3 getBeardContribution — f32 distance/multiplier, f64
/// fastInvSqrt, f32 kernel multiply.
#[inline]
fn get_beard_contribution(dx: i32, dy: i32, dz: i32, y_to_ground: i32) -> f32 {
    let xi = dx + 12;
    let yi = dy + 12;
    let zi = dz + 12;
    if !(0..24).contains(&xi) || !(0..24).contains(&yi) || !(0..24).contains(&zi) {
        return 0.0;
    }
    let dy_with_offset = y_to_ground as f32 + 0.5f32;
    // Mth.lengthSquared((float)dx, dyWithOffset, (float)dz) — f32 adds, left-assoc.
    let distance_sqr = (dx as f32 * dx as f32 + dy_with_offset * dy_with_offset) + dz as f32 * dz as f32;
    // (float)Mth.fastInvSqrt((double)(distanceSqr / 2.0F))
    let fis = fast_inv_sqrt((distance_sqr / 2.0f32) as f64) as f32;
    let value = -dy_with_offset * fis / 2.0f32;
    value * kernel()[(zi as usize * 24 + xi as usize) * 24 + yi as usize]
}

/// Mth.fastInvSqrt (bit-exact magic-constant Newton step).
#[inline]
fn fast_inv_sqrt(mut number: f64) -> f64 {
    let xhalf = 0.5 * number;
    let mut l = number.to_bits() as i64;
    l = 6910469410427058090i64.wrapping_sub(l >> 1);
    number = f64::from_bits(l as u64);
    number * (1.5 - xhalf * number * number)
}

/// Shared BEARD_KERNEL with beard.rs (identical layout: [z*576 + x*24 + y]).
fn kernel() -> &'static [f32; 13824] {
    static KERNEL: OnceLock<[f32; 13824]> = OnceLock::new();
    KERNEL.get_or_init(|| {
        let mut a = [0f32; 13824];
        for z in 0..24i32 {
            for x in 0..24i32 {
                for y in 0..24i32 {
                    let dx = (x - 12) as f64;
                    let dy = (y - 12) as f64 + 0.5;
                    let dz = (z - 12) as f64;
                    let d = dx * dx + dy * dy + dz * dz;
                    a[(z * 576 + x * 24 + y) as usize] = (-(d / 16.0)).exp() as f32;
                }
            }
        }
        a
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_is_zero() {
        let b = Beardifier263::empty();
        assert_eq!(b.sample_value(1, 2, 3), 0.0);
        let mut v = vec![1.0f32; 16];
        b.fill_volume(
            &DensityVolume::block_step((2, 2, 2), (0, 0, 0)),
            &mut v,
        );
        assert!(v.iter().all(|&x| x == 0.0));
    }

    #[test]
    fn beard_thin_piece_f32_semantics() {
        // One BEARD_THIN piece at origin. 26.3 sign semantics: NEGATIVE above
        // the ground (carves), POSITIVE below (solidifies) —
        // -dyWithOffset * fastInvSqrt(distanceSqr/2) / 2 * kernel.
        let b = Beardifier263::decode(&[
            1, -8, -8, -8, 8, 8, 8, 2, 0, 0, // 1 piece, BEARD_THIN, delta 0
            0,
        ]);
        assert!(!b.is_empty());
        let above = b.sample_value(0, -7, 0); // dyToGround = +1
        assert!(above < 0.0, "above ground must carve, got {above}");
        let below = b.sample_value(0, -16, 0); // dyToGround = -8
        assert!(below > 0.0, "below ground must solidify, got {below}");
        let v_out = b.sample_value(100, 100, 100);
        assert_eq!(v_out, 0.0, "outside affected box must be exactly 0");
    }

    #[test]
    fn fill_volume_column_pruned_bit_exact() {
        // Seeded random pieces (ALL adjustment types incl. NONE) + junctions;
        // the pruned fill_volume must be bit-identical to the unpruned
        // reference (sample_value_unchecked) on every cell.
        let mut seed: u64 = 0xBe4dF11_263;
        let mut rnd = move |m: u64| {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((seed >> 33) as u64) % m
        };
        for case in 0..64u64 {
            let npieces = 1 + rnd(12) as usize;
            let njunc = rnd(8) as usize;
            let mut pieces = Vec::new();
            for _ in 0..npieces {
                let cx = rnd(64) as i32 - 32;
                let cz = rnd(64) as i32 - 32;
                let sx = 1 + rnd(24) as i32;
                let sy = 1 + rnd(40) as i32;
                let sz = 1 + rnd(24) as i32;
                let adj = match rnd(6) {
                    0 => ADJ_NONE,
                    1 => ADJ_BURY,
                    2 => ADJ_BEARD_THIN,
                    3 => ADJ_BEARD_BOX,
                    4 => ADJ_ENCAPSULATE,
                    _ => rnd(5) as i32, // includes out-of-ordinal values
                };
                pieces.push(Piece {
                    min_x: cx - sx / 2,
                    min_y: -40 + rnd(30) as i32,
                    min_z: cz - sz / 2,
                    max_x: cx + sx - sx / 2,
                    max_y: 0,
                    max_z: cz + sz - sz / 2,
                    adjustment: adj,
                    ground_delta: -(rnd(2) as i32),
                });
                let p = pieces.last_mut().unwrap();
                p.max_y = p.min_y + sy;
            }
            let mut junctions = Vec::new();
            for _ in 0..njunc {
                junctions.push(Junction {
                    x: rnd(96) as i32 - 48,
                    y: -40 + rnd(40) as i32,
                    z: rnd(96) as i32 - 48,
                });
            }
            let b = Beardifier263 {
                pieces: pieces.clone(),
                junctions: junctions.clone(),
                affected: {
                    let mut aff: Option<(i32, i32, i32, i32, i32, i32)> = None;
                    for p in &pieces {
                        include_box(&mut aff, (p.min_x, p.min_y, p.min_z, p.max_x, p.max_y, p.max_z));
                    }
                    for j in &junctions {
                        include_box(&mut aff, (j.x, j.y, j.z, j.x, j.y, j.z));
                    }
                    aff.map(inflate24)
                },
            };
            // volume near the piece cloud, overlapping the affected box
            let vol = DensityVolume::block_step(
                (16, 64, 16),
                (-8 + (rnd(16) as i32 - 8), -40, -8 + (rnd(16) as i32 - 8)),
            );
            let n = vol.size();
            let mut a = vec![0f32; n];
            let mut c = vec![0f32; n];
            b.fill_volume(&vol, &mut a);
            // reference: the OLD unpruned loop
            if let Some((bx0, by0, bz0, bx1, by1, bz1)) = b.affected {
                if vol.max_block_x() >= bx0 && vol.min_x <= bx1
                    && vol.max_block_y() >= by0 && vol.min_y <= by1
                    && vol.max_block_z() >= bz0 && vol.min_z <= bz1
                {
                    let min_x = i32::max(0, floor_div(bx0 - vol.min_x, vol.step_x));
                    let min_y = i32::max(0, floor_div(by0 - vol.min_y, vol.step_y));
                    let min_z = i32::max(0, floor_div(bz0 - vol.min_z, vol.step_z));
                    let max_x = i32::min(vol.size_x - 1, floor_div(bx1 - vol.min_x, vol.step_x));
                    let max_y = i32::min(vol.size_y - 1, floor_div(by1 - vol.min_y, vol.step_y));
                    let max_z = i32::min(vol.size_z - 1, floor_div(bz1 - vol.min_z, vol.step_z));
                    for z in min_z..=max_z {
                        for x in min_x..=max_x {
                            for y in min_y..=max_y {
                                let idx = vol.index_unchecked(x, y, z);
                                c[idx] = b.sample_value_unchecked(
                                    vol.block_x(x), vol.block_y(y), vol.block_z(z),
                                );
                            }
                        }
                    }
                }
            }
            for i in 0..n {
                assert_eq!(
                    a[i].to_bits(), c[i].to_bits(),
                    "case {case} cell {i} mismatch: {} vs {}",
                    a[i], c[i]
                );
            }
        }
    }
}
