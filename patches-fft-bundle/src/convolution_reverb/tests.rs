use super::*;
use patches_sdk::test_support::{ModuleHarness, params};
use std::thread::sleep;
use std::time::{Duration, Instant};

const SR: f32 = 44_100.0;

fn env() -> AudioEnvironment {
    AudioEnvironment { sample_rate: SR, poly_voices: 16, periodic_update_interval: 32, hosted: false }
}

/// `Module::prepare` installs the processor synchronously, so the kit is
/// present before any tick.
#[test]
fn initial_build_installs_processor_synchronously() {
    let h = ModuleHarness::build_with_env::<ConvolutionReverb>(
        params!["mix" => 1.0_f32],
        env(),
    );
    let cr = h.as_any().downcast_ref::<ConvolutionReverb>().unwrap();
    assert!(
        cr.core.kits[0].is_some(),
        "ConvolutionReverb::prepare must install the processor kit synchronously"
    );
}

/// Run a long impulse + silence through each IR variant and verify the
/// output is bounded, not NaN, and eventually contains signal energy.
///
/// Because the processor runs on a background thread, slot completions
/// depend on OS scheduling; we drive many samples and budget wall time
/// for the thread to catch up before checking the output buffer.
fn drive_impulse_and_measure_peak(_variant: super::params::IrVariant) -> f32 {
    // IR variant is now structural — selecting it requires building the
    // harness with a structural override, which the harness doesn't yet
    // expose. For now, exercise only the descriptor default (`room`).
    let mut h = ModuleHarness::build_with_env::<ConvolutionReverb>(
        params!["mix" => 1.0_f32],
        env(),
    );
    h.disconnect_input("mix");

    // Small wall-clock grace for the processor thread to start.
    sleep(Duration::from_millis(20));

    h.set_mono("in", 1.0);
    h.tick();
    h.set_mono("in", 0.0);

    let n = 16_384;
    let mut peak = 0.0_f32;
    let batch = 512;
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut produced = 0;
    while produced < n && Instant::now() < deadline {
        for _ in 0..batch {
            h.tick();
            let v = h.read_mono("out").abs();
            peak = peak.max(v);
            produced += 1;
        }
        sleep(Duration::from_millis(2));
    }
    assert!(peak.is_finite(), "non-finite output");
    assert!(peak < 10.0, "peak {peak} exceeds bounded-response limit");
    peak
}

/// The default IR variant must produce audible output within the budget —
/// confirms the end-to-end pipeline (prepare, thread spawn, convolution,
/// overlap-add) produces signal at all.
#[test]
fn default_ir_variant_produces_signal() {
    let peak = drive_impulse_and_measure_peak(super::params::IrVariant::Room);
    assert!(peak > 0.0, "default IR variant produced silent output within the budget");
}
