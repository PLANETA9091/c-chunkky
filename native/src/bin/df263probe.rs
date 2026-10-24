//! Probe spaghetti_2d internals vs Java oracle probes.
use std::rc::Rc;
use crussty_chunkgen::df263::{BinOp, DfSam263, SamRef, SamplerCtx263};
use crussty_chunkgen::df263_router::RouterBuilder263;

fn sam(v: DfSam263) -> SamRef { Rc::new(v) }
fn constant(v: f32) -> SamRef { sam(DfSam263::Constant(v)) }

fn main() {
    let mut builder = RouterBuilder263::new(3257840388504953787);
    let router = builder.build_overworld();
    // rebuild the sub-tree with the SAME builder pieces is not exposed; instead
    // probe via the router's final tree: sample spaghetti_2d is exposed; for
    // internals rebuild here using the factory.
    let f = &mut builder.factory;
    let spa2d_mod = f.normal("spaghetti_2d_modulator", crussty_chunkgen::n263::NormalNoiseParameters{
        base_amplitude: 0.955388882960065, base_octave: -11, octave_count: 1,
        normalize: crussty_chunkgen::n263::Normalization::Enabled, amplitude_modifiers: vec![]});
    let spa2d = f.normal("spaghetti_2d", crussty_chunkgen::n263::NormalNoiseParameters{
        base_amplitude: 0.955388882960065, base_octave: -7, octave_count: 1,
        normalize: crussty_chunkgen::n263::Normalization::Enabled, amplitude_modifiers: vec![]});
    let spa2d_thick = f.normal("spaghetti_2d_thickness", crussty_chunkgen::n263::NormalNoiseParameters{
        base_amplitude: 0.955388882960065, base_octave: -11, octave_count: 1,
        normalize: crussty_chunkgen::n263::Normalization::Enabled, amplitude_modifiers: vec![]});
    let spa2d_elev = f.normal("spaghetti_2d_elevation", crussty_chunkgen::n263::NormalNoiseParameters{
        base_amplitude: 0.955388882960065, base_octave: -8, octave_count: 1,
        normalize: crussty_chunkgen::n263::Normalization::Enabled, amplitude_modifiers: vec![]});
    // NOTE: builder.build_overworld already consumed these names with the SAME
    // factory+cache -> the cached Rc instances are reused (same forked noise).
    let _ = &router;

    let noise = |n: Rc<crussty_chunkgen::df263::N263>, xz: f64, ys: f64| sam(DfSam263::Noise { noise: n, xz_scale: xz, y_scale: ys });
    let add = |l: SamRef, r: SamRef| sam(DfSam263::Binary { op: BinOp::Add, l, r, r_bound: 0.0 });
    // elevation = remap(noise(elev,1,0), -1,1,-8,8): factor 8, offset 0 -> mul(8)
    let elev = sam(DfSam263::ConstMul { a: noise(spa2d_elev, 1.0, 0.0), c: 8.0 });
    // thick = remap(noise(thick,2,1), -1,1,-0.6,-1.3): factor -0.35, offset -0.95
    let thick = sam(DfSam263::ConstAdd {
        a: sam(DfSam263::ConstMul { a: noise(spa2d_thick, 2.0, 1.0), c: -0.35 }),
        c: -0.95,
    });
    let grad = sam(DfSam263::Gradient { axis: crussty_chunkgen::df263::Axis::Y, tiling: crussty_chunkgen::df263::Tiling::ClampToEdge,
        from_coord: -64, aux: 320, from_value: 8.0, factor: (-48.0)/384.0 });
    let sloped = sam(DfSam263::Unary { op: crussty_chunkgen::df263::UnOp::Abs, a: add(sam(DfSam263::Cache { id: 900, a: elev.clone() }), grad.clone()) });
    let layer_ridged = sam(DfSam263::Unary { op: crussty_chunkgen::df263::UnOp::Cube, a: add(sloped.clone(), thick.clone()) });
    // wrap2d
    let mut branches = Vec::new();
    for r in [0.5f32, 0.75, 1.0, 2.0, 3.0] {
        branches.push(sam(DfSam263::ConstMul { a: noise(spa2d.clone(), 1.0/(r as f64), 1.0/(r as f64)), c: r }));
    }
    let wrap2d_inner = sam(DfSam263::IntervalSelect {
        a: noise(spa2d_mod, 2.0, 1.0), thresholds: vec![-0.75, -0.5, 0.5, 0.75], branches });
    let wrap2d = sam(DfSam263::Unary { op: crussty_chunkgen::df263::UnOp::Abs, a: wrap2d_inner });
    let cave_noise = add(wrap2d.clone(), sam(DfSam263::ConstMul { a: thick.clone(), c: 0.083 }));

    let mut ctx = SamplerCtx263::uncached();
    let mut lcg = 88172645463325252i64;
    let names = ["elev", "thick", "wrap2d", "sloped", "layer_ridged", "cave_noise"];
    let fns = [elev, thick, wrap2d, sloped, layer_ridged, cave_noise];
    for (fi, fnc) in fns.iter().enumerate() {
        for i in 0..24 {
            lcg ^= lcg << 13; lcg ^= ((lcg as u64) >> 7) as i64; lcg ^= lcg << 17;
            let x = ((lcg as u64 >> 33) % 96) as i32 - 16;
            lcg ^= lcg << 13; lcg ^= ((lcg as u64) >> 7) as i64; lcg ^= lcg << 17;
            let z = ((lcg as u64 >> 33) % 96) as i32 - 16;
            let ys = [-60i32, -16, 32, 80, 128, 200, 256, 318];
            let y = ys[i % 8];
            let v = fnc.sample_value(&mut ctx, x, y, z);
            println!("pt probe_{} {} {} {} {:x}", names[fi], x, y, z, v.to_bits());
        }
    }
}
