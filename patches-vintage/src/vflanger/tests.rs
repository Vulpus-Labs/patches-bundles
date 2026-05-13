use super::*;
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

#[test]
fn descriptor_shape() {
    let h = ModuleHarness::build::<VFlanger>(&[]);
    let d = h.descriptor();
    assert_eq!(d.module_name, "VFlanger");
    assert_eq!(d.inputs.len(), 5);
    assert_eq!(d.outputs.len(), 1);
}

#[test]
fn silent_input_bounded_output() {
    // With zero input, the flanger's delay line should never see a nonzero
    // sample, so the output must stay at (or extremely close to) 0. The old
    // < 0.5 bound would have tolerated a factor-of-10⁵ regression. The feedback
    // path and all-pass filter can produce sub-LSB residuals when denormals
    // flush, so use a conservative 1e-6 ceiling rather than exact zero.
    let mut h = ModuleHarness::build_full::<VFlanger>(params![], ENV, shape());
    for _ in 0..((SR * 0.2) as usize) {
        h.set_mono("in", 0.0);
        h.tick();
        let y = h.read_mono("out");
        assert!(
            y.is_finite() && y.abs() < 1.0e-6,
            "silent input produced audible output: {y}"
        );
    }
}

#[test]
fn sine_does_not_explode_with_heavy_resonance() {
    let mut h = ModuleHarness::build_full::<VFlanger>(
        params![
            "rate_hz" => 0.5_f32,
            "depth" => 0.8_f32,
            "manual_ms" => 2.0_f32,
            "feedback" => 0.9_f32,
        ],
        ENV,
        shape(),
    );
    for i in 0..((SR * 0.5) as usize) {
        let t = i as f32 / SR;
        let x = 0.3 * (std::f32::consts::TAU * 440.0 * t).sin();
        h.set_mono("in", x);
        h.tick();
        let y = h.read_mono("out");
        assert!(y.is_finite() && y.abs() < 5.0, "diverged at i={i}: {y}");
    }
}

#[test]
fn lf_bypass_preserves_low_frequencies() {
    // At 60 Hz a BF-2B-style flanger with lf_bypass=on should pass most
    // of the signal energy through the dry LF path. Without bypass the
    // comb-filter notches can take chunks out of the fundamental.
    fn rms_at(lf_bypass: bool) -> f32 {
        let mut h = ModuleHarness::build_full::<VFlanger>(
            params![
                "rate_hz" => 0.05_f32,
                "depth" => 0.0_f32,
                "manual_ms" => 2.0_f32,
                "feedback" => 0.0_f32,
                "lf_bypass" => lf_bypass,
            ],
            ENV,
            shape(),
        );
        let settle = (SR * 0.1) as usize;
        let n = (SR * 0.2) as usize;
        let mut sum = 0.0_f64;
        let mut cnt = 0usize;
        for i in 0..(settle + n) {
            let t = i as f32 / SR;
            h.set_mono("in", (std::f32::consts::TAU * 60.0 * t).sin());
            h.tick();
            if i >= settle {
                let y = h.read_mono("out") as f64;
                sum += y * y;
                cnt += 1;
            }
        }
        (sum / cnt as f64).sqrt() as f32
    }
    let on = rms_at(true);
    let off = rms_at(false);
    assert!(on > off * 1.2, "lf_bypass on should preserve more 60 Hz energy: on={on}, off={off}");
}
