//! Shared helpers for SlotDeck integration tests.

use patches_fft_harness::slot_deck::{OverlapBuffer, SlotDeckConfig};
use patches_fft_harness::{SpectralPitchShifter, WindowBuffer};
use patches_dsp::RealPackedFft;

/// Run plain OLA: constant-1 input, processor applies `window` once.
#[allow(dead_code)]
pub fn run_ola(cfg: SlotDeckConfig, window: &WindowBuffer) -> Vec<f32> {
    let n_samples = cfg.total_latency() * 4;
    let (mut buf, mut handle) = OverlapBuffer::new_unthreaded(cfg);
    let mut outputs = Vec::with_capacity(n_samples);

    for _ in 0..n_samples {
        buf.write(1.0_f32);
        while let Some(mut slot) = handle.pop() {
            // Apply window in-place.
            let mut windowed = vec![0.0_f32; slot.data.len()].into_boxed_slice();
            window.apply_into(&slot.data, &mut windowed);
            slot.data.copy_from_slice(&windowed);
            let _ = handle.push(slot);
        }
        outputs.push(buf.read());
    }

    outputs
}

/// Run WOLA: constant-1 input, processor applies `window` twice (analysis
/// before processing, synthesis after — simulating a spectral processor that
/// preserves signal). The effective OLA window is w², which is not COLA, so
/// normalisation is needed.
#[allow(dead_code)]
pub fn run_wola(cfg: SlotDeckConfig, window: &WindowBuffer) -> Vec<f32> {
    let synth_window = window.normalised_wola(cfg.hop_size());
    let n_samples = cfg.total_latency() * 4;
    let (mut buf, mut handle) = OverlapBuffer::new_unthreaded(cfg);
    let mut outputs = Vec::with_capacity(n_samples);

    for _ in 0..n_samples {
        buf.write(1.0_f32);
        while let Some(mut slot) = handle.pop() {
            // Analysis window: applied to input before spectral processing.
            let mut analysis = vec![0.0_f32; slot.data.len()].into_boxed_slice();
            window.apply_into(&slot.data, &mut analysis);
            // (spectral processing would go here — identity for this test)
            // Pre-normalised synthesis window — WOLA correction baked in.
            slot.data.fill(0.0);
            synth_window.apply_into(&analysis, &mut slot.data);
            let _ = handle.push(slot);
        }
        outputs.push(buf.read());
    }

    outputs
}

/// Run a crude FFT brick-wall low-pass through the WOLA pipeline.
#[allow(dead_code)]
pub fn run_fft_lowpass(input: &[f32], window_size: usize, overlap_factor: usize) -> Vec<f32> {
    let cfg = SlotDeckConfig::new(window_size, overlap_factor, window_size).unwrap();
    let analysis_window = WindowBuffer::new(window_size, |n| {
        (std::f32::consts::PI * n).sin().powi(2)
    });
    let synth_window = analysis_window.normalised_wola(cfg.hop_size());
    let fft = RealPackedFft::new(window_size);

    let n_samples = input.len() + cfg.total_latency();
    let (mut buf, mut handle) = OverlapBuffer::new_unthreaded(cfg);
    let mut outputs = Vec::with_capacity(n_samples);

    for i in 0..n_samples {
        let sample = if i < input.len() { input[i] } else { 0.0 };
        buf.write(sample);

        while let Some(mut slot) = handle.pop() {
            // Analysis window
            let mut frame = vec![0.0f32; slot.data.len()].into_boxed_slice();
            analysis_window.apply_into(&slot.data, &mut frame);

            // Forward FFT
            fft.forward(&mut frame);

            // Zero all bins above half-Nyquist (i.e. above bin N/4).
            let quarter_n = window_size / 4;
            frame[1] = 0.0;
            for k in (quarter_n + 1)..(window_size / 2) {
                frame[2 * k] = 0.0;
                frame[2 * k + 1] = 0.0;
            }

            // Inverse FFT
            fft.inverse(&mut frame);

            // Pre-normalised synthesis window — WOLA correction baked in.
            slot.data.fill(0.0);
            synth_window.apply_into(&frame, &mut slot.data);
            let _ = handle.push(slot);
        }

        outputs.push(buf.read());
    }

    outputs
}

#[allow(dead_code)]
pub fn run_pitch_shift(
    input: &[f32],
    window_size: usize,
    overlap_factor: usize,
    semitones: f32,
) -> Vec<f32> {
    let cfg = SlotDeckConfig::new(window_size, overlap_factor, window_size).unwrap();
    let hop_size = cfg.hop_size();
    let analysis_window = WindowBuffer::new(window_size, |n| {
        (std::f32::consts::PI * n).sin().powi(2)
    });
    let synth_window = analysis_window.normalised_wola(hop_size);
    let fft = RealPackedFft::new(window_size);
    let mut shifter = SpectralPitchShifter::new(window_size, hop_size);
    shifter.set_shift_semitones(semitones);

    let n_samples = input.len() + cfg.total_latency();
    let (mut buf, mut handle) = OverlapBuffer::new_unthreaded(cfg);
    let mut outputs = Vec::with_capacity(n_samples);

    for i in 0..n_samples {
        let sample = if i < input.len() { input[i] } else { 0.0 };
        buf.write(sample);

        while let Some(mut slot) = handle.pop() {
            // Analysis window
            let mut frame = vec![0.0f32; slot.data.len()].into_boxed_slice();
            analysis_window.apply_into(&slot.data, &mut frame);

            // Forward FFT → pitch shift → inverse FFT
            fft.forward(&mut frame);
            shifter.transform(&mut frame);
            fft.inverse(&mut frame);

            // Pre-normalised synthesis window — WOLA correction baked in.
            slot.data.fill(0.0);
            synth_window.apply_into(&frame, &mut slot.data);
            let _ = handle.push(slot);
        }

        outputs.push(buf.read());
    }

    outputs
}

#[allow(dead_code)]
pub fn dominant_bin(signal: &[f32], fft: &RealPackedFft) -> usize {
    let n = fft.len();
    let mut buf = vec![0.0f32; n];
    buf[..signal.len().min(n)].copy_from_slice(&signal[..signal.len().min(n)]);
    fft.forward(&mut buf);

    let mut best_k = 0;
    let mut best_mag = 0.0f32;
    for k in 1..(n / 2) {
        let mag = buf[2 * k].hypot(buf[2 * k + 1]);
        if mag > best_mag {
            best_mag = mag;
            best_k = k;
        }
    }
    best_k
}

/// Measure RMS power of a signal.
#[allow(dead_code)]
pub fn rms(signal: &[f32]) -> f32 {
    let sum_sq: f32 = signal.iter().map(|&x| x * x).sum();
    (sum_sq / signal.len() as f32).sqrt()
}
