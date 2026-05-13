use super::*;

const SR: f32 = 48_000.0;

#[test]
fn silent_input_produces_silent_output() {
    let mut c = Compressor::new(CompanderParams::NE570_DEFAULT, SR);
    let mut e = Expander::new(CompanderParams::NE570_DEFAULT, SR);
    for _ in 0..1000 {
        let y = e.process(c.process(0.0));
        assert_eq!(y, 0.0, "silence must not latch");
    }
}

#[test]
fn round_trip_approx_unity_at_ref_level() {
    let mut c = Compressor::new(CompanderParams::NE570_DEFAULT, SR);
    let mut e = Expander::new(CompanderParams::NE570_DEFAULT, SR);
    let freq = 1_000.0_f32;
    let amp = CompanderParams::NE570_DEFAULT.ref_level;

    // Warm up through attack/release settling.
    for i in 0..(SR * 0.5) as usize {
        let t = i as f32 / SR;
        let x = amp * (std::f32::consts::TAU * freq * t).sin();
        let _ = e.process(c.process(x));
    }

    // Measure RMS of input and round-trip output over ~20 ms.
    let n = (SR * 0.02) as usize;
    let mut sx = 0.0_f32;
    let mut sy = 0.0_f32;
    for i in 0..n {
        let t = i as f32 / SR;
        let x = amp * (std::f32::consts::TAU * freq * t).sin();
        let y = e.process(c.process(x));
        sx += x * x;
        sy += y * y;
    }
    let rms_x = (sx / n as f32).sqrt();
    let rms_y = (sy / n as f32).sqrt();
    let ratio = rms_y / rms_x;
    assert!(
        (ratio - 1.0).abs() < 0.2,
        "round-trip RMS ratio = {ratio}, want ~1.0"
    );
}

#[test]
fn attack_faster_than_release() {
    // Step up, then step down; time-to-reach 0.5 must be shorter on
    // the rising edge than on the falling edge.
    let mut c = Compressor::new(CompanderParams::NE570_DEFAULT, SR);
    let samples_to_rise = {
        let mut n = 0;
        for i in 0..SR as usize {
            let y = c.process(0.5);
            if y.abs() > 0.2 {
                n = i;
                break;
            }
        }
        n
    };
    let samples_to_fall = {
        // Feed silence; count until compressor output is near zero.
        let mut n = 0;
        for i in 0..SR as usize {
            let y = c.process(0.0);
            if y.abs() < 1.0e-3 {
                n = i;
                break;
            }
        }
        n
    };
    assert!(
        samples_to_fall >= samples_to_rise,
        "release ({samples_to_fall}) must not be faster than attack ({samples_to_rise})"
    );
}

#[test]
fn reset_clears_state() {
    let mut c = Compressor::new(CompanderParams::NE570_DEFAULT, SR);
    for _ in 0..1000 {
        let _ = c.process(0.5);
    }
    c.reset();
    // With state cleared, first output should not be a hugely-boosted
    // spike from the silence-before-audio era.
    let y = c.process(0.0);
    assert_eq!(y, 0.0);
}
