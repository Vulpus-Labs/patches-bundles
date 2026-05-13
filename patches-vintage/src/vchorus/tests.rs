use super::*;
use crate::vchorus::core::{Mode, Variant};
use patches_sdk::test_support::{params, ModuleHarness};
use patches_sdk::{AudioEnvironment, ModuleShape};

const SR: f32 = 48_000.0;
const ENV: AudioEnvironment = AudioEnvironment {
    sample_rate: SR,
    poly_voices: 16,
    periodic_update_interval: 32,
    hosted: false,
};

fn shape() -> ModuleShape {
    ModuleShape { channels: 1 }
}

fn run_sine(h: &mut ModuleHarness, n: usize) -> (Vec<f32>, Vec<f32>) {
    let mut l = Vec::with_capacity(n);
    let mut r = Vec::with_capacity(n);
    for i in 0..n {
        let t = i as f32 / SR;
        let x = (std::f32::consts::TAU * 440.0 * t).sin();
        h.set_stereo("in", x, x);
        h.tick();
        let (lo, ro) = h.read_stereo("out");
        l.push(lo);
        r.push(ro);
    }
    (l, r)
}

fn xcorr_lr(l: &[f32], r: &[f32]) -> f32 {
    let n = l.len() as f32;
    let ml = l.iter().sum::<f32>() / n;
    let mr = r.iter().sum::<f32>() / n;
    let mut num = 0.0_f32;
    let mut dl = 0.0_f32;
    let mut dr = 0.0_f32;
    for i in 0..l.len() {
        let a = l[i] - ml;
        let b = r[i] - mr;
        num += a * b;
        dl += a * a;
        dr += b * b;
    }
    let d = (dl * dr).sqrt();
    if d < 1.0e-9 { 0.0 } else { num / d }
}

#[test]
fn descriptor_shape() {
    let h = ModuleHarness::build::<VChorus>(&[]);
    let d = h.descriptor();
    assert_eq!(d.module_name, "VChorus");
    assert_eq!(d.inputs.len(), 3);
    assert_eq!(d.outputs.len(), 1);
}

#[test]
fn off_on_bright_bypasses_signal() {
    let mut h = ModuleHarness::build_full::<VChorus>(
        params![
            "variant" => Variant::Bright,
            "mode" => Mode::Off,
            "hiss" => 0.0_f32,
        ],
        ENV,
        shape(),
    );
    h.set_stereo("in", 0.42, -0.17);
    h.tick();
    let (lo, ro) = h.read_stereo("out");
    assert!((lo - 0.42).abs() < 1.0e-5);
    assert!((ro + 0.17).abs() < 1.0e-5);
}

#[test]
fn hiss_silent_at_zero_and_bounded_at_one() {
    let mut h = ModuleHarness::build_full::<VChorus>(
        params!["mode" => Mode::One, "hiss" => 0.0_f32],
        ENV,
        shape(),
    );
    h.set_stereo("in", 0.0, 0.0);
    for _ in 0..((SR * 0.2) as usize) {
        h.tick();
    }
    assert!(h.read_stereo("out").0.abs() < 1.0e-4);

    let mut h2 = ModuleHarness::build_full::<VChorus>(
        params!["mode" => Mode::One, "hiss" => 1.0_f32],
        ENV,
        shape(),
    );
    let mut peak = 0.0_f32;
    for _ in 0..((SR * 0.1) as usize) {
        h2.set_stereo("in", 0.0, 0.0);
        h2.tick();
        peak = peak.max(h2.read_stereo("out").0.abs());
    }
    assert!(
        peak > 0.0 && peak < 0.02,
        "hiss peak {peak} out of expected range [0, 0.02]"
    );
}

#[test]
fn mode_both_on_bright_more_modulated_than_mode_one() {
    let mut h1 = ModuleHarness::build_full::<VChorus>(
        params![
            "variant" => Variant::Bright,
            "mode" => Mode::One,
            "hiss" => 0.0_f32,
        ],
        ENV,
        shape(),
    );
    let n = (SR * 0.8) as usize;
    let (l1, r1) = run_sine(&mut h1, n);
    let c1 = xcorr_lr(&l1, &r1);

    let mut h2 = ModuleHarness::build_full::<VChorus>(
        params![
            "variant" => Variant::Bright,
            "mode" => Mode::Both,
            "hiss" => 0.0_f32,
        ],
        ENV,
        shape(),
    );
    let (l2, r2) = run_sine(&mut h2, n);
    let c2 = xcorr_lr(&l2, &r2);

    assert!(
        c2 < c1,
        "mode both should yield lower L/R correlation than mode one: both={c2}, one={c1}"
    );
}

#[test]
fn dark_variant_does_not_bypass_when_off() {
    let mut h = ModuleHarness::build_full::<VChorus>(
        params![
            "variant" => Variant::Dark,
            "mode" => Mode::Off,
            "hiss" => 0.0_f32,
        ],
        ENV,
        shape(),
    );
    let mut all_equal = true;
    for i in 0..((SR * 0.1) as usize) {
        let t = i as f32 / SR;
        let x = 0.5 * (std::f32::consts::TAU * 440.0 * t).sin();
        h.set_stereo("in", x, x);
        h.tick();
        let out = h.read_stereo("out").0;
        if (out - x).abs() > 1.0e-4 {
            all_equal = false;
            break;
        }
    }
    assert!(!all_equal, "dark + off must still pass through the BBD");
}
