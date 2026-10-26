// surfnoise — compare the 26.3 surface noise instance (asset params).
use crussty_chunkgen::df263::NoiseFactory263;
use crussty_chunkgen::n263::{NormalNoiseParameters, Normalization};
fn main() {
    let seed: i64 = 3257840388504953787;
    let mut f = NoiseFactory263::new(seed);
    let params = NormalNoiseParameters {
        base_amplitude: 0.9381732587751008,
        base_octave: -6,
        octave_count: 3,
        normalize: Normalization::Enabled,
        amplitude_modifiers: vec![],
    };
    let n = f.normal("surface", params);
    let mut root = crussty_chunkgen::rng::XoroshiroRandomSource::new(seed);
    let _ = root.fork_positional();
    println!("surface(16416,0,16448) = {}", n.get(16416.0, 0.0, 16448.0));
    println!("surface(16416,0,16449) = {}", n.get(16416.0, 0.0, 16449.0));
}
