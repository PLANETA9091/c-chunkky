use crussty_chunkgen::df263::NoiseFactory263;
use crussty_chunkgen::n263::{NormalNoiseParameters, Normalization};
fn main() {
    let mut f = NoiseFactory263::new(3257840388504953787);
    let params = NormalNoiseParameters {
        base_amplitude: 1.1817507833955654,
        base_octave: -5,
        octave_count: 3,
        normalize: Normalization::Enabled,
        amplitude_modifiers: vec![1.0, 0.0, 1.0],
    };
    let n = f.normal("sulfur_cave_gradient", params);
    for (x, y, z) in [(16544,-8,16540),(16544,-8,16542),(16544,-8,16543),(16544,-7,16540),(16544,-6,16540),(16545,-8,16540)] {
        println!("sulf({x},{y},{z}) = {}", n.get(x as f64, y as f64, z as f64));
    }
}
