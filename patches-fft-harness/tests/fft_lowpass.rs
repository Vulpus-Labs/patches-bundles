//! FFT-based crude half-Nyquist lowpass filter — integration test for SlotDeck + RealPackedFft.
//!
//! Processing chain per frame:
//!   1. Analysis Hann window applied to the input frame.
//!   2. Forward FFT.
//!   3. Zero every bin above half-Nyquist (k > N/4), including the Nyquist bin.
//!   4. Inverse FFT.
//!   5. Synthesis Hann window applied.
//!   6. WOLA reconstruction via OverlapBuffer normalisation.
//!
//! Why Hann applied twice needs WOLA normalisation:
//!   Single Hann (sin²) is COLA at 50% overlap — OLA sums to 1.  Applying it a
//!   second time (synthesis window) gives sin⁴, which is not COLA.  `normalised_wola`
//!   precomputes `1 / Σ_k w²[r - k·hop]` for each hop phase, compensating exactly.
//!
//! Why the stopband result is exactly zero (not just small):
//!   A Hann-windowed sinusoid at bin k has non-zero DFT values only at bins
//!   k-1, k, k+1 (the Hann window's 3-point spectrum).  Choosing a test tone at
//!   bin 96 with N=256 places all three non-zero bins (95, 96, 97) above the
//!   cutoff at bin 64, so zeroing clears all spectral energy.

use std::f32::consts::PI;
use patches_dsp::fft::RealPackedFft;
use patches_fft_harness::slot_deck::{OverlapBuffer, SlotDeckConfig};
use patches_fft_harness::WindowBuffer;

const WINDOW_SIZE: usize = 256;
const OVERLAP_FACTOR: usize = 4;
const PROCESSING_BUDGET: usize = 64; // hop_size = 256/4 = 64

fn hann(n: f32) -> f32 {
    (PI * n).sin().powi(2)
}

/// Apply the half-Nyquist lowpass filter to a continuous sample stream, inline
/// (processing thread simulated synchronously on the same thread).
///
/// Processing latency: `config.total_latency()` samples.
/// Returned slice has the same length as `input`; the first `total_latency`
/// values are zero while the pipeline fills.
fn run_fft_lowpass(input: &[f32]) -> Vec<f32> {
    let cfg = SlotDeckConfig::new(WINDOW_SIZE, OVERLAP_FACTOR, PROCESSING_BUDGET).unwrap();
    let fft = RealPackedFft::new(WINDOW_SIZE);
    let analysis_window = WindowBuffer::new(WINDOW_SIZE, hann);
    let synthesis_window = analysis_window.normalised_wola(cfg.hop_size());
    let (mut overlap_buf, mut handle) = OverlapBuffer::new_unthreaded(cfg);

    // Bins above half-Nyquist to zero.
    // Packed layout: buf[0] = DC, buf[1] = Nyquist, buf[2k]/buf[2k+1] = bin k.
    let cutoff_bin = WINDOW_SIZE / 4; // half-Nyquist = N/4

    let mut output = Vec::with_capacity(input.len());
    for &sample in input {
        overlap_buf.write(sample);

        // Inline processor: pop → window → FFT → zero → IFFT → window → push.
        while let Some(mut slot) = handle.pop() {
            // Step 1: analysis window.
            let mut frame = vec![0.0f32; WINDOW_SIZE];
            analysis_window.apply_into(&slot.data, &mut frame);

            // Step 2: forward FFT (in-place).
            fft.forward(&mut frame);

            // Step 3: zero all bins above half-Nyquist.
            frame[1] = 0.0; // Nyquist (buf[1])
            for k in (cutoff_bin + 1)..(WINDOW_SIZE / 2) {
                frame[2 * k]     = 0.0;
                frame[2 * k + 1] = 0.0;
            }

            // Step 4: inverse FFT.
            fft.inverse(&mut frame);

            // Step 5: pre-normalised synthesis window — WOLA correction baked in.
            slot.data.fill(0.0);
            synthesis_window.apply_into(&frame, &mut slot.data);

            let _ = handle.push(slot);
        }

        output.push(overlap_buf.read());
    }

    output
}

fn rms(samples: &[f32]) -> f32 {
    let sum_sq: f32 = samples.iter().map(|&x| x * x).sum();
    (sum_sq / samples.len() as f32).sqrt()
}

/// Sine at frequency bin `freq_bin` relative to `WINDOW_SIZE`:
///   f = freq_bin × fs / WINDOW_SIZE
///
/// Integer bin keeps the tone at an exact DFT bin, minimising spectral leakage
/// (the Hann-windowed DFT of such a tone is non-zero only at bins k-1, k, k+1).
fn make_sine(n_samples: usize, freq_bin: usize) -> Vec<f32> {
    (0..n_samples)
        .map(|i| (2.0 * PI * freq_bin as f32 * i as f32 / WINDOW_SIZE as f32).sin())
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn fft_lowpass_passes_low_frequency_tone() {
    // Bin 16 (fs/16) is well inside the passband (cutoff = bin 64 = N/4).
    // All Hann-window spectral energy lives at bins 15, 16, 17 — none get zeroed.
    // WOLA normalisation gives exact reconstruction; RMS should match within 1%.
    let freq_bin = 16;
    let cfg = SlotDeckConfig::new(WINDOW_SIZE, OVERLAP_FACTOR, PROCESSING_BUDGET).unwrap();
    let latency = cfg.total_latency();
    let n_samples = latency + WINDOW_SIZE * 4;

    let input  = make_sine(n_samples, freq_bin);
    let output = run_fft_lowpass(&input);

    // Skip latency + one extra window to ensure steady state.
    let settle     = latency + WINDOW_SIZE;
    let input_rms  = rms(&input[settle..]);
    let output_rms = rms(&output[settle..]);

    assert!(
        output_rms > 0.99 * input_rms,
        "Low-frequency tone (bin {freq_bin}) not preserved: \
         input_rms={input_rms:.5}, output_rms={output_rms:.5}"
    );
}

#[test]
fn fft_lowpass_attenuates_high_frequency_tone() {
    // Bin 96 (3×fs/8) is above the cutoff (bin 64 = N/4).
    // Hann window's non-zero DFT bins are 95, 96, 97 — all above the cutoff.
    // After zeroing and IFFT the time-domain result is (within fp precision) zero.
    let freq_bin = 96;
    let cfg = SlotDeckConfig::new(WINDOW_SIZE, OVERLAP_FACTOR, PROCESSING_BUDGET).unwrap();
    let latency = cfg.total_latency();
    let n_samples = latency + WINDOW_SIZE * 4;

    let input  = make_sine(n_samples, freq_bin);
    let output = run_fft_lowpass(&input);

    // After settling, output should be essentially silent.
    let settle     = latency + WINDOW_SIZE;
    let output_rms = rms(&output[settle..]);

    assert!(
        output_rms < 1e-4,
        "High-frequency tone (bin {freq_bin}) not attenuated: output_rms={output_rms:.6}"
    );
}

#[test]
fn fft_lowpass_passes_dc() {
    // Constant 1.0 input is entirely at bin 0 (DC), well inside the passband.
    // WOLA reconstruction with Hann applied twice should recover 1.0 exactly.
    let cfg = SlotDeckConfig::new(WINDOW_SIZE, OVERLAP_FACTOR, PROCESSING_BUDGET).unwrap();
    let latency = cfg.total_latency();
    let n_samples = latency + WINDOW_SIZE * 4;

    let input: Vec<f32> = vec![1.0; n_samples];
    let output = run_fft_lowpass(&input);

    let settle = latency + WINDOW_SIZE;
    for (i, &s) in output[settle..].iter().enumerate() {
        assert!(
            (s - 1.0).abs() < 1e-4,
            "DC pass failed at sample {}: expected 1.0, got {s:.6}",
            settle + i
        );
    }
}
