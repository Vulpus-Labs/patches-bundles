use super::*;
use patches_sdk::test_support::{ModuleHarness, params};
use std::thread::sleep;
use std::time::{Duration, Instant};

const SR: f32 = 44_100.0;

fn env() -> AudioEnvironment {
    AudioEnvironment { sample_rate: SR, poly_voices: 16, periodic_update_interval: 32, hosted: false }
}

/// The initial build synchronously installs the processor (see
/// [`core::ConvReverbCore::update_parameters`]), so overlap_buffers[0] is
/// already `Some` before any tick.
#[test]
fn initial_build_installs_processor_synchronously() {
    let h = ModuleHarness::build_with_env::<ConvolutionReverb>(
        params!["ir" => super::params::IrVariant::Room, "mix" => 1.0_f32],
        env(),
    );
    let cr = h.as_any().downcast_ref::<ConvolutionReverb>().unwrap();
    assert!(
        cr.core.overlap_buffers[0].is_some(),
        "ConvolutionReverb::build must install the overlap buffer synchronously"
    );
    assert!(
        cr.core.threads[0].is_some(),
        "ConvolutionReverb::build must spawn the processor thread"
    );
}

/// Run a long impulse + silence through each IR variant and verify the
/// output is bounded, not NaN, and eventually contains signal energy.
///
/// Because the processor runs on a background thread, slot completions
/// depend on OS scheduling; we drive many samples and budget wall time
/// for the thread to catch up before checking the output buffer.
fn drive_impulse_and_measure_peak(variant: super::params::IrVariant) -> f32 {
    let mut h = ModuleHarness::build_with_env::<ConvolutionReverb>(
        params!["ir" => variant, "mix" => 1.0_f32],
        env(),
    );
    h.disconnect_input("mix");

    // Small wall-clock grace for the processor thread to start.
    sleep(Duration::from_millis(20));

    h.set_mono("in", 1.0);
    h.tick();
    h.set_mono("in", 0.0);

    // Tick enough samples for the processor thread to push completed
    // slots back; yield periodically so the thread gets CPU time.
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
    assert!(peak < 10.0, "{variant:?}: peak {peak} exceeds bounded-response limit");
    peak
}

/// At least one of the synthetic IR variants must produce audible output
/// within the budget — confirms the end-to-end pipeline (build, thread
/// spawn, convolution, overlap-add) produces signal at all.
#[test]
fn at_least_one_ir_variant_produces_signal() {
    use super::params::IrVariant;
    let peaks: Vec<(IrVariant, f32)> = [IrVariant::Room, IrVariant::Hall, IrVariant::Plate].iter()
        .map(|&v| (v, drive_impulse_and_measure_peak(v)))
        .collect();
    let max_peak = peaks.iter().map(|(_, p)| *p).fold(0.0_f32, f32::max);
    assert!(
        max_peak > 0.0,
        "all IR variants produced silent output within the budget: {peaks:?}"
    );
}
