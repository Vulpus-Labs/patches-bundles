//! OLA with Hann applied once (COLA property, no normalisation needed).

use patches_fft_harness::slot_deck::SlotDeckConfig;
use patches_fft_harness::WindowBuffer;

use super::support::run_ola;

#[test]
fn ola_hann_overlap_2() {
    // Hann window w = sin², F=2 (50% overlap).
    // COLA identity: sin²(θ) + sin²(θ+π/2) = sin²(θ) + cos²(θ) = 1.
    // Overlap-add sum = 1 in steady state → no normalisation required.
    let cfg = SlotDeckConfig::new(32, 2, 16).unwrap();
    let latency = cfg.total_latency();
    let window = WindowBuffer::new(cfg.window_size, |n| (std::f32::consts::PI * n).sin().powi(2));
    let outputs = run_ola(cfg, &window);

    let check_from = latency + 32;
    for (i, &s) in outputs[check_from..].iter().enumerate() {
        assert!(
            (s - 1.0).abs() < 1e-5,
            "OLA Hann F=2 failed at sample {}: expected 1.0, got {}",
            check_from + i,
            s
        );
    }
}

#[test]
fn ola_hann_overlap_4() {
    // Hann window w = sin², F=4 (75% overlap).
    // Overlap-add sum = 2 in steady state (Hann COLA sum for 75% overlap).
    // Without normalisation the raw output is 2.0, not 1.0 — demonstrating
    // why WOLA is needed when an additional synthesis window is also applied.
    let cfg = SlotDeckConfig::new(32, 4, 16).unwrap();
    let latency = cfg.total_latency();
    let window = WindowBuffer::new(cfg.window_size, |n| (std::f32::consts::PI * n).sin().powi(2));
    let outputs = run_ola(cfg, &window);

    let check_from = latency + 32;
    for (i, &s) in outputs[check_from..].iter().enumerate() {
        assert!(
            (s - 2.0).abs() < 1e-5,
            "OLA Hann F=4 failed at sample {}: expected 2.0, got {}",
            check_from + i,
            s
        );
    }
}
