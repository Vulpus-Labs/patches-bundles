//! WOLA with Hann applied twice (analysis + synthesis window, normalisation
//! required).

use patches_fft_harness::slot_deck::SlotDeckConfig;
use patches_fft_harness::WindowBuffer;

use super::support::run_wola;

#[test]
fn wola_hann_overlap_2() {
    // Hann applied twice → effective window w² = sin⁴, which is not COLA.
    // normalised_wola computes norm[p] = 1 / Σ_k w[p+k·hop]², compensating exactly.
    let cfg = SlotDeckConfig::new(32, 2, 16).unwrap();
    let latency = cfg.total_latency();
    let window = WindowBuffer::new(cfg.window_size, |n| (std::f32::consts::PI * n).sin().powi(2));
    let outputs = run_wola(cfg, &window);

    let check_from = latency + 32;
    for (i, &s) in outputs[check_from..].iter().enumerate() {
        assert!(
            (s - 1.0).abs() < 1e-5,
            "WOLA Hann F=2 failed at sample {}: expected 1.0, got {}",
            check_from + i,
            s
        );
    }
}

#[test]
fn wola_hann_overlap_4() {
    // Same as above with 75% overlap. normalised_wola handles the different
    // hop-phase sums automatically.
    let cfg = SlotDeckConfig::new(32, 4, 16).unwrap();
    let latency = cfg.total_latency();
    let window = WindowBuffer::new(cfg.window_size, |n| (std::f32::consts::PI * n).sin().powi(2));
    let outputs = run_wola(cfg, &window);

    let check_from = latency + 32;
    for (i, &s) in outputs[check_from..].iter().enumerate() {
        assert!(
            (s - 1.0).abs() < 1e-5,
            "WOLA Hann F=4 failed at sample {}: expected 1.0, got {}",
            check_from + i,
            s
        );
    }
}
