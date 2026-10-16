//! Deep-dive the ShiftedNoise node divergence: inputs, per-octave values.
use crussty_chunkgen::density::{Env, InterpState, Mode, Pt, WKind};

fn main() {
    let seed: i64 = 0;
    let env: &'static Env = Box::leak(Box::new(Env::new(seed)));
    let gen = crussty_chunkgen::nochunk::ChunkGen::new(env);
    let compiled = gen.compiled_pub();
    let interp = InterpState::new(compiled.interp_slots, 4, 48);
    let pt = Pt { x: 52, y: 64, z: -32 };
    let pts = [pt, pt, pt, pt];

    let WKind::ShiftedNoise { noise_id, xz_scale, y_scale, sx, sy, sz } = &compiled.arena[8] else {
        println!("node 8 is not ShiftedNoise"); return;
    };
    let sxv = env.eval(compiled, *sx, pt, Mode::Slice, &interp, None);
    let syv = env.eval(compiled, *sy, pt, Mode::Slice, &interp, None);
    let szv = env.eval(compiled, *sz, pt, Mode::Slice, &interp, None);
    let memo4 = crussty_chunkgen::density::V4Memo::new(compiled.arena.len());
    let sxa = env.eval4(compiled, *sx, &pts, Mode::Slice, &interp, &memo4);
    let sya = env.eval4(compiled, *sy, &pts, Mode::Slice, &interp, &memo4);
    let sza = env.eval4(compiled, *sz, &pts, Mode::Slice, &interp, &memo4);
    println!("sx: scalar={sxv:e} batch={:e} eq={}", sxa[0], sxv.to_bits() == sxa[0].to_bits());
    println!("sy: scalar={syv:e} batch={:e} eq={}", sya[0], syv.to_bits() == sya[0].to_bits());
    println!("sz: scalar={szv:e} batch={:e} eq={}", sza[0], szv.to_bits() == sza[0].to_bits());

    let x = pt.x as f64 * xz_scale + sxv;
    let y = pt.y as f64 * y_scale + syv;
    let z = pt.z as f64 * xz_scale + szv;
    let xb = pt.x as f64 * xz_scale + sxa[0];
    let yb = pt.y as f64 * y_scale + sya[0];
    let zb = pt.z as f64 * xz_scale + sza[0];
    println!("x: {x:e} vs {xb:e} eq={}", x.to_bits() == xb.to_bits());
    println!("y: {y:e} vs {yb:e} eq={}", y.to_bits() == yb.to_bits());
    println!("z: {z:e} vs {zb:e} eq={}", z.to_bits() == zb.to_bits());

    let n = &env.noise.list[compiled.noise_resolved[*noise_id as usize] as usize];
    let vs = n.get_value(x, y, z);
    let vb = n.get_value_batch(&[xb, xb, xb, xb], &[yb, yb, yb, yb], &[zb, zb, zb, zb]);
    println!("noise value: scalar={vs:e} batch={:e} eq={}", vb[0], vs.to_bits() == vb[0].to_bits());

    // per-octave drill (PerlinNoise internals via public fields)
    for i in 0..n.first.noise_levels.len() {
        if let Some(nl) = &n.first.noise_levels[i] {
            let f = n.first.lowest_freq_input_factor;
            let wf = f * (1u32 << i) as f64;
            let a = nl.noise(wf * x, wf * y, wf * z);
            let b = nl.noise(wf * xb, wf * yb, wf * zb);
            if a.to_bits() != b.to_bits() {
                println!("first octave {i}: scalar={a:e} batch-input={b:e}");
            }
        }
    }
}
