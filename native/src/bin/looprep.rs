//! cont-125: replicate perlin263_fill_col's y-loop verbatim and time it —
//! isolates whether the real kernel's ~21 ns/layer-pt is intrinsic to the
//! loop body or comes from the call structure around it.
use crussty_chunkgen::df263::DensityVolume;
use crussty_chunkgen::n263::{gradient_dot_f32, lerp3_f32, smoothstep, wrap, mth_floor};
use std::time::Instant;

/// verbatim y-loop from perlin263_fill_col (plain perlin path), one layer
#[allow(clippy::too_many_arguments)]
fn layer_loop(
    p: &[u16; 256],
    freq: f64,
    off_x: f64,
    off_y: f64,
    off_z: f64,
    amp: f32,
    out: &mut [f32],
    vol: &DensityVolume,
) {
    let mut index = 0usize;
    for z in 0..vol.size_z {
        let wz = wrap(vol.block_z(z) as f64 * freq) + off_z;
        let fz = mth_floor(wz);
        let rz = (wz - fz as f64) as f32;
        let rz1 = rz - 1.0f32;
        let sz = smoothstep(rz);
        for x in 0..vol.size_x {
            let wx = wrap(vol.block_x(x) as f64 * freq) + off_x;
            let fx = mth_floor(wx);
            let rx = (wx - fx as f64) as f32;
            let rx1 = rx - 1.0f32;
            let sx = smoothstep(rx);
            let x0 = p[(fx & 0xFF) as usize] as usize;
            let x1 = x0 >> 8;
            for y in 0..vol.size_y {
                let wy = wrap(vol.block_y(y) as f64 * freq) + off_y;
                let fy = mth_floor(wy);
                let rel_y_f64 = wy - fy as f64;
                let ry = rel_y_f64 as f32;
                let sy = smoothstep(ry);
                let ry1 = ry - 1.0f32;
                let xy00 = p[(x0.wrapping_add(fy as usize)) & 0xFF] as usize;
                let xy01 = xy00 >> 8;
                let z000 = p[(xy00.wrapping_add(fz as usize)) & 0xFF] as usize;
                let z001 = p[(xy01.wrapping_add(fz as usize)) & 0xFF] as usize;
                let d000 = gradient_dot_f32(z000 & 15, rx, ry, rz);
                let d001 = gradient_dot_f32(z000 >> 8 & 15, rx, ry, rz1);
                let d010 = gradient_dot_f32(z001 & 15, rx, ry1, rz);
                let d011 = gradient_dot_f32(z001 >> 8 & 15, rx, ry1, rz1);
                let xy10 = p[(x1.wrapping_add(fy as usize)) & 0xFF] as usize;
                let xy11 = xy10 >> 8;
                let z100 = p[(xy10.wrapping_add(fz as usize)) & 0xFF] as usize;
                let z101 = p[(xy11.wrapping_add(fz as usize)) & 0xFF] as usize;
                let d100 = gradient_dot_f32(z100 & 15, rx1, ry, rz);
                let d101 = gradient_dot_f32(z100 >> 8 & 15, rx1, ry, rz1);
                let d110 = gradient_dot_f32(z101 & 15, rx1, ry1, rz);
                let d111 = gradient_dot_f32(z101 >> 8 & 15, rx1, ry1, rz1);
                out[index] += amp * lerp3_f32(sx, sy, sz, d000, d100, d010, d110, d001, d101, d011, d111);
                index += 1;
            }
        }
    }
}


/// 2x y-unrolled interleaved variant: two independent points' chains
/// interleaved to overlap load latency. Bit-exact by construction: each
/// point performs the identical op sequence on identical inputs.
#[allow(clippy::too_many_arguments)]
fn layer_loop_x2(
    p: &[u16; 256],
    freq: f64,
    off_x: f64,
    off_y: f64,
    off_z: f64,
    amp: f32,
    out: &mut [f32],
    vol: &DensityVolume,
) {
    let mut index = 0usize;
    for z in 0..vol.size_z {
        let wz = wrap(vol.block_z(z) as f64 * freq) + off_z;
        let fz = mth_floor(wz);
        let rz = (wz - fz as f64) as f32;
        let rz1 = rz - 1.0f32;
        let sz = smoothstep(rz);
        for x in 0..vol.size_x {
            let wx = wrap(vol.block_x(x) as f64 * freq) + off_x;
            let fx = mth_floor(wx);
            let rx = (wx - fx as f64) as f32;
            let rx1 = rx - 1.0f32;
            let sx = smoothstep(rx);
            let x0 = p[(fx & 0xFF) as usize] as usize;
            let x1 = x0 >> 8;
            let mut y = 0usize;
            let ny = vol.size_y as usize;
            while y + 2 <= ny {
                // point A (y) and point B (y+1) — independent chains
                let wy_a = wrap(vol.block_y(y as i32) as f64 * freq) + off_y;
                let fy_a = mth_floor(wy_a);
                let ry_a = (wy_a - fy_a as f64) as f32;
                let sy_a = smoothstep(ry_a);
                let ry1_a = ry_a - 1.0f32;
                let xy00_a = p[(x0.wrapping_add(fy_a as usize)) & 0xFF] as usize;
                let xy01_a = xy00_a >> 8;
                let z000_a = p[(xy00_a.wrapping_add(fz as usize)) & 0xFF] as usize;
                let z001_a = p[(xy01_a.wrapping_add(fz as usize)) & 0xFF] as usize;
                let xy10_a = p[(x1.wrapping_add(fy_a as usize)) & 0xFF] as usize;
                let xy11_a = xy10_a >> 8;
                let z100_a = p[(xy10_a.wrapping_add(fz as usize)) & 0xFF] as usize;
                let z101_a = p[(xy11_a.wrapping_add(fz as usize)) & 0xFF] as usize;
                let d000_a = gradient_dot_f32(z000_a & 15, rx, ry_a, rz);
                let d001_a = gradient_dot_f32(z000_a >> 8 & 15, rx, ry_a, rz1);
                let d010_a = gradient_dot_f32(z001_a & 15, rx, ry1_a, rz);
                let d011_a = gradient_dot_f32(z001_a >> 8 & 15, rx, ry1_a, rz1);
                let d100_a = gradient_dot_f32(z100_a & 15, rx1, ry_a, rz);
                let d101_a = gradient_dot_f32(z100_a >> 8 & 15, rx1, ry_a, rz1);
                let d110_a = gradient_dot_f32(z101_a & 15, rx1, ry1_a, rz);
                let d111_a = gradient_dot_f32(z101_a >> 8 & 15, rx1, ry1_a, rz1);
                let r_a = amp * lerp3_f32(sx, sy_a, sz, d000_a, d100_a, d010_a, d110_a, d001_a, d101_a, d011_a, d111_a);

                let wy_b = wrap(vol.block_y(y as i32 + 1) as f64 * freq) + off_y;
                let fy_b = mth_floor(wy_b);
                let ry_b = (wy_b - fy_b as f64) as f32;
                let sy_b = smoothstep(ry_b);
                let ry1_b = ry_b - 1.0f32;
                let xy00_b = p[(x0.wrapping_add(fy_b as usize)) & 0xFF] as usize;
                let xy01_b = xy00_b >> 8;
                let z000_b = p[(xy00_b.wrapping_add(fz as usize)) & 0xFF] as usize;
                let z001_b = p[(xy01_b.wrapping_add(fz as usize)) & 0xFF] as usize;
                let xy10_b = p[(x1.wrapping_add(fy_b as usize)) & 0xFF] as usize;
                let xy11_b = xy10_b >> 8;
                let z100_b = p[(xy10_b.wrapping_add(fz as usize)) & 0xFF] as usize;
                let z101_b = p[(xy11_b.wrapping_add(fz as usize)) & 0xFF] as usize;
                let d000_b = gradient_dot_f32(z000_b & 15, rx, ry_b, rz);
                let d001_b = gradient_dot_f32(z000_b >> 8 & 15, rx, ry_b, rz1);
                let d010_b = gradient_dot_f32(z001_b & 15, rx, ry1_b, rz);
                let d011_b = gradient_dot_f32(z001_b >> 8 & 15, rx, ry1_b, rz1);
                let d100_b = gradient_dot_f32(z100_b & 15, rx1, ry_b, rz);
                let d101_b = gradient_dot_f32(z100_b >> 8 & 15, rx1, ry_b, rz1);
                let d110_b = gradient_dot_f32(z101_b & 15, rx1, ry1_b, rz);
                let d111_b = gradient_dot_f32(z101_b >> 8 & 15, rx1, ry1_b, rz1);
                let r_b = amp * lerp3_f32(sx, sy_b, sz, d000_b, d100_b, d010_b, d110_b, d001_b, d101_b, d011_b, d111_b);

                out[index] += r_a;
                out[index + 1] += r_b;
                index += 2;
                y += 2;
            }
            if y < ny {
                let wy = wrap(vol.block_y(y as i32) as f64 * freq) + off_y;
                let fy = mth_floor(wy);
                let ry = (wy - fy as f64) as f32;
                let sy = smoothstep(ry);
                let ry1 = ry - 1.0f32;
                let xy00 = p[(x0.wrapping_add(fy as usize)) & 0xFF] as usize;
                let xy01 = xy00 >> 8;
                let z000 = p[(xy00.wrapping_add(fz as usize)) & 0xFF] as usize;
                let z001 = p[(xy01.wrapping_add(fz as usize)) & 0xFF] as usize;
                let d000 = gradient_dot_f32(z000 & 15, rx, ry, rz);
                let d001 = gradient_dot_f32(z000 >> 8 & 15, rx, ry, rz1);
                let d010 = gradient_dot_f32(z001 & 15, rx, ry1, rz);
                let d011 = gradient_dot_f32(z001 >> 8 & 15, rx, ry1, rz1);
                let xy10 = p[(x1.wrapping_add(fy as usize)) & 0xFF] as usize;
                let xy11 = xy10 >> 8;
                let z100 = p[(xy10.wrapping_add(fz as usize)) & 0xFF] as usize;
                let z101 = p[(xy11.wrapping_add(fz as usize)) & 0xFF] as usize;
                let d100 = gradient_dot_f32(z100 & 15, rx1, ry, rz);
                let d101 = gradient_dot_f32(z100 >> 8 & 15, rx1, ry, rz1);
                let d110 = gradient_dot_f32(z101 & 15, rx1, ry1, rz);
                let d111 = gradient_dot_f32(z101 >> 8 & 15, rx1, ry1, rz1);
                out[index] += amp * lerp3_f32(sx, sy, sz, d000, d100, d010, d110, d001, d101, d011, d111);
                index += 1;
            }
        }
    }
}

fn main() {
    // a real p_adj table: build one via the factory's offset noise (its first
    // perlin layer), extracted from a compiled Gen263 through the public API
    // is overkill — build a synthetic-but-valid table instead: the probe
    // measures SPEED, not parity (the table content is cost-neutral).
    let mut p = [0u16; 256];
    for i in 0..256 {
        let a = (i as u64).wrapping_mul(0x9E3779B97F4A7C15);
        let b = a.rotate_left(21).wrapping_mul(0xBF58476D1CE4E5B9);
        p[i] = ((b >> 48) as u16) | ((((b >> 32) as u16) & 0x0F) << 8);
    }

    for (sx, sy, sz) in [(16usize, 384usize, 16usize), (5, 49, 5), (17, 98, 17)] {
        let vol = DensityVolume {
            size_x: sx as i32,
            size_y: sy as i32,
            size_z: sz as i32,
            min_x: 0,
            min_y: -64,
            min_z: 0,
            step_x: 1,
            step_y: 1,
            step_z: 1,
        };
        let total = (sx * sy * sz) as u64;
        let mut out = vec![0f32; total as usize];
        let layers = 6u64;
        let pts_l = total * layers;
        let reps = if total < 4000 { 20000 } else { 300 };
        for _ in 0..5 {
            for l in 0..layers {
                layer_loop(&p, 0.125 * (1 << l) as f64, l as f64 * 0.7, l as f64 * 1.3, l as f64 * 0.4, 1.0, &mut out, &vol);
            }
        }
        let t0 = Instant::now();
        for _ in 0..reps {
            for l in 0..layers {
                layer_loop(&p, 0.125 * (1 << l) as f64, l as f64 * 0.7, l as f64 * 1.3, l as f64 * 0.4, 1.0, &mut out, &vol);
            }
        }
        let dt = t0.elapsed().as_secs_f64() / reps as f64;
        // parity check x1 vs x2 first
        let mut out2 = vec![0f32; total as usize];
        for l in 0..layers {
            layer_loop(&p, 0.125 * (1 << l) as f64, l as f64 * 0.7, l as f64 * 1.3, l as f64 * 0.4, 1.0, &mut out2, &vol);
        }
        let mut out3 = vec![0f32; total as usize];
        for l in 0..layers {
            layer_loop_x2(&p, 0.125 * (1 << l) as f64, l as f64 * 0.7, l as f64 * 1.3, l as f64 * 0.4, 1.0, &mut out3, &vol);
        }
        let par = out2.iter().zip(out3.iter()).filter(|(a, b)| a.to_bits() != b.to_bits()).count();
        let t1 = Instant::now();
        for _ in 0..reps {
            for l in 0..layers {
                layer_loop_x2(&p, 0.125 * (1 << l) as f64, l as f64 * 0.7, l as f64 * 1.3, l as f64 * 0.4, 1.0, &mut out, &vol);
            }
        }
        let dt2 = t1.elapsed().as_secs_f64() / reps as f64;
        println!(
            "{sx:3}x{sy:3}x{sz:3}  pts={total:6}  x1={:6.2}  x2={:6.2} ns/layer-pt  (parity diffs={par})",
            dt * 1e9 / pts_l as f64,
            dt2 * 1e9 / pts_l as f64,
        );
    }
}
