//! Semantic reference model of the vanilla NoiseChunk batch fill:
//! - corner values of every Interpolated slot under final_density via POINT
//!   evals (Mode::Slice == Direct here: the fd tree has no nested interp);
//! - per-block fd = full-tree eval in Mode::Block after driving the
//!   InterpState chain in the exact vanilla order (cellX asc, cellZ asc,
//!   cellY desc, inCellY desc, updateForY -> updateForX -> updateForZ);
//! - Mth.lerp f64 arithmetic identical to vanilla.
//!
//! Output: "x y z <fd_bits_i64> <fd_val>" for every block of the chunk.
//!
//! Usage: batchref <extracted_root>... -- cx cz [seed]

use crussty_chunkgen::density::{Env, InterpState, Mode, Pt};
use crussty_chunkgen::worldgen::Assets;
use std::path::Path;

fn collect(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                collect(root, &p, out);
            } else if p.extension().map(|x| x == "json").unwrap_or(false) {
                if let Ok(rel) = p.strip_prefix(root) {
                    let rel = rel.to_string_lossy().replace('\\', "/");
                    let Some(stripped) = rel.strip_prefix("data/") else { continue };
                    if let Ok(bytes) = std::fs::read(&p) {
                        out.push((stripped.to_string(), bytes));
                    }
                }
            }
        }
    }
}

#[inline]
fn lerp(delta: f64, a: f64, b: f64) -> f64 {
    a + delta * (b - a) // Mth.lerp
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut roots = Vec::new();
    let mut pos = Vec::new();
    let mut seen = false;
    for a in &args[1..] {
        if a == "--" {
            seen = true;
            continue;
        }
        if !seen {
            roots.push(a.clone());
        } else {
            pos.push(a.clone());
        }
    }
    let cx: i32 = pos.get(0).and_then(|s| s.parse().ok()).unwrap_or(0);
    let cz: i32 = pos.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    let seed: i64 = pos.get(2).and_then(|s| s.parse().ok()).unwrap_or(3257840388504953787);

    let mut assets = Assets::new();
    for root in &roots {
        let root = Path::new(root);
        let mut files = Vec::new();
        collect(root, root, &mut files);
        files.sort();
        for (path, bytes) in files {
            let keep = path.contains("/worldgen/noise_settings/")
                || path.contains("/worldgen/density_function/")
                || path.contains("/worldgen/noise/")
                || path.ends_with("/dimension/overworld.json");
            if keep {
                assets.insert(path, bytes);
            }
        }
    }
    let env: &'static Env = Box::leak(Box::new(Env::new_assets(seed, &assets)));
    let compiled = env.compile();
    let fd_idx = *compiled
        .router
        .get("final_density")
        .expect("final_density in router");

    // 1) collect Interpolated slots under the fd tree
    let mut slots: Vec<u16> = Vec::new();
    let mut stack = vec![fd_idx];
    let mut seen_nodes = std::collections::HashSet::new();
    while let Some(i) = stack.pop() {
        if !seen_nodes.insert(i) {
            continue;
        }
        match &compiled.arena[i as usize] {
            crussty_chunkgen::density::WKind::Interpolated { child, slot } => {
                slots.push(*slot);
                stack.push(*child);
            }
            _ => {
                for c in compiled.arena[i as usize].children() {
                    stack.push(c);
                }
            }
        }
    }
    eprintln!("fd tree interp slots: {slots:?}");

    // 2) corner planes per slot: planes[i][z][y], i in 0..=4 (x corners)
    let cw = 4i32;
    let ch = 8i32;
    let n_xz = 4usize;
    let n_y = 48usize;
    let first_cell_x = cx * 16 / cw;
    let first_cell_z = cz * 16 / cw;
    let cell_noise_min_y = -64 / ch; // floorDiv(minY, cellHeight)
    let mut planes: Vec<Vec<Vec<f64>>> =
        vec![vec![vec![0.0; (n_xz + 1) * (n_y + 1)]; n_xz + 1]; slots.len()];

    for (si, &slot) in slots.iter().enumerate() {
        // find the child index for this slot
        let mut child_idx = None;
        let mut st2 = vec![fd_idx];
        let mut seen2 = std::collections::HashSet::new();
        while let Some(i) = st2.pop() {
            if !seen2.insert(i) {
                continue;
            }
            if let crussty_chunkgen::density::WKind::Interpolated { child, slot: s } =
                &compiled.arena[i as usize]
            {
                if *s == slot as u16 {
                    child_idx = Some(*child);
                }
                st2.push(*child);
            } else {
                for c in compiled.arena[i as usize].children() {
                    st2.push(c);
                }
            }
        }
        let child = child_idx.expect("slot child");
        for i in 0..=n_xz {
            let cxx = (first_cell_x + i as i32) * cw;
            for j in 0..=n_xz {
                let czz = (first_cell_z + j as i32) * cw;
                for y in 0..=n_y {
                    let cy = (y as i32 + cell_noise_min_y) * ch;
                    let pt = Pt { x: cxx, y: cy, z: czz };
                    // Mode::Slice: no nested interp under fd -> == Direct
                    planes[si][i][j * (n_y + 1) + y] =
                        env.eval(&compiled, child, pt, Mode::Slice, &InterpState::new(1, 4, 48), None);
                }
            }
        }
    }

    // 3) per-block fd in vanilla loop order
    let mut interp = InterpState::new(compiled.interp_slots, 4, 48);
    let mut out = String::with_capacity(1 << 22);
    for i2 in 0..n_xz {
        // cellX asc
        for i3 in 0..n_xz {
            // cellZ asc
            for i5 in (0..=n_y - 1).rev() {
                // cellY desc (cell index)
                for i6 in (0..8i32).rev() {
                    // inCellY desc
                    let yf = i6 as f64 / 8.0;
                    let y_global = (i5 as i32 + cell_noise_min_y) * ch + i6;
                    for i9 in 0..4 {
                        // inCellX asc
                        let xf = i9 as f64 / 4.0;
                        let x = cx * 16 + (i2 as i32) * cw + i9;
                        for i12 in 0..4 {
                            // inCellZ asc
                            let zf = i12 as f64 / 4.0;
                            let z = cz * 16 + (i3 as i32) * cw + i12;

                            // drive the chain for every fd slot (vanilla order)
                            for (si, _) in slots.iter().enumerate() {
                                let p0 = &planes[si][i2];       // slice0 = x-plane i2
                                let p1 = &planes[si][i2 + 1];   // slice1 = x-plane i2+1
                                // corners at (z=i3, y=i5): n000..n111
                                let c = |p: &Vec<f64>, zi: usize, yi: usize| p[zi * (n_y + 1) + yi];
                                let n000 = c(p0, i3, i5);
                                let n100 = c(p1, i3, i5);
                                let n010 = c(p0, i3, i5 + 1);
                                let n110 = c(p1, i3, i5 + 1);
                                let n001 = c(p0, i3 + 1, i5);
                                let n101 = c(p1, i3 + 1, i5);
                                let n011 = c(p0, i3 + 1, i5 + 1);
                                let n111 = c(p1, i3 + 1, i5 + 1);
                                // vanilla updateForY/X/Z chain
                                let vxz00 = lerp(yf, n000, n010);
                                let vxz10 = lerp(yf, n100, n110);
                                let vxz01 = lerp(yf, n001, n011);
                                let vxz11 = lerp(yf, n101, n111);
                                let vz0 = lerp(xf, vxz00, vxz10);
                                let vz1 = lerp(xf, vxz01, vxz11);
                                let v = lerp(zf, vz0, vz1);
                                interp.set_chain_value(slots[si] as usize, v);
                            }
                            let pt = Pt { x, y: y_global, z };
                            let fd = env.eval(&compiled, fd_idx, pt, Mode::Block, &interp, None);
                            out.push_str(&format!(
                                "{x} {y_global} {z} {} {fd:.17e}\n",
                                fd.to_bits() as i64
                            ));
                        }
                    }
                }
            }
        }
    }
    print!("{out}");
}
