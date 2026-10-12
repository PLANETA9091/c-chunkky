//! Compare fill_slice (interp.slice1) vs eval_plane_into buffers per plane.
use crussty_chunkgen::density::Env;
use crussty_chunkgen::nochunk::ChunkGen;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let cx: i32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(3);
    let cz: i32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(-2);
    let seed: i64 = args
        .get(3)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0); // bench fill uses Env::new(0)
    let env: &'static Env = Box::leak(Box::new(Env::new(seed)));
    let mut gen = ChunkGen::new(env);
    let cw = gen.cell_width_pub() as i32;
    let ch = gen.cell_height_pub() as i32;
    let ccx = gen.cell_count_xz_pub() as usize;
    let ccy = gen.cell_count_y_pub() as usize;
    let cnmy = gen.cell_noise_min_y_pub();
    let first_cell_x = (cx * 16).div_euclid(cw);
    let first_cell_z = (cz * 16).div_euclid(cw);

    for k in 1..=ccx as i32 {
        // fill_slice path (writes slice1)
        gen.fill_slice(first_cell_x + k, first_cell_z, false);
        // eval_plane_into path
        let children = gen.interp_children_pub().to_vec();
        let mut buf = vec![0f64; children.len() * (ccx + 1) * (ccy + 1)];
        ChunkGen::eval_plane_into(
            env,
            gen.compiled_pub(),
            &children,
            (first_cell_x + k) * cw,
            first_cell_z,
            ccx,
            ccy,
            cnmy,
            cw,
            ch,
            &mut buf,
        );
        // compare against interp.slice1
        let sl = gen.slice1_pub();
        let mut diffs = 0;
        let mut first_diff = (0usize, 0usize, 0usize, 0.0, 0.0);
        for slot in 0..children.len() {
            for i in 0..=ccx {
                for y in 0..=ccy {
                    let idx = slot * (ccx + 1) * (ccy + 1) + i * (ccy + 1) + y;
                    let a = sl[idx];
                    let b = buf[idx];
                    if a.to_bits() != b.to_bits() {
                        if diffs == 0 {
                            first_diff = (slot, i, y, a, b);
                        }
                        diffs += 1;
                    }
                }
            }
        }
        println!(
            "plane k={k}: diffs={diffs} first={:?}",
            first_diff
        );
    }
}
