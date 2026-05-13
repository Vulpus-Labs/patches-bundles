//! Test utilities for drum modules: `assert_within!` parity with
//! patches-dsp / patches-core, plus spectral helpers built on
//! `patches_dsp::RealPackedFft` for asserting drum output shape.

use patches_dsp::RealPackedFft;

/// Assert that `actual` is within an absolute `delta` of `expected`.
///
/// Mirror of the `assert_within!` macro in `patches-dsp` /
/// `patches-core`, kept local so this crate stays dep-light.
macro_rules! assert_within {
    ($expected:expr, $actual:expr, $delta:expr) => {{
        let expected: f32 = $expected;
        let actual: f32 = $actual;
        let delta: f32 = $delta;
        assert!(
            (expected - actual).abs() < delta,
            "assert_within failed: expected {}, actual {}, delta {}",
            expected,
            actual,
            delta
        );
    }};
    ($expected:expr, $actual:expr, $delta:expr, $($arg:tt)+) => {{
        let expected: f32 = $expected;
        let actual: f32 = $actual;
        let delta: f32 = $delta;
        assert!(
            (expected - actual).abs() < delta,
            $($arg)+
        );
    }};
}

pub(crate) use assert_within;

fn bin_magnitude(packed: &[f32], bin: usize) -> f32 {
    let n = packed.len();
    if bin == 0 {
        packed[0].abs()
    } else if bin == n / 2 {
        packed[1].abs()
    } else {
        packed[2 * bin].hypot(packed[2 * bin + 1])
    }
}

/// Zero-pad or truncate `signal` to `fft_size`, run forward FFT, and return
/// linear bin magnitudes of length `fft_size / 2 + 1`.
pub fn magnitude_spectrum(signal: &[f32], fft_size: usize) -> Vec<f32> {
    assert!(fft_size.is_power_of_two(), "fft_size must be a power of two");
    let fft = RealPackedFft::new(fft_size);
    let mut buf = vec![0.0f32; fft_size];
    let len = signal.len().min(fft_size);
    buf[..len].copy_from_slice(&signal[..len]);
    fft.forward(&mut buf);
    (0..=fft_size / 2).map(|k| bin_magnitude(&buf, k)).collect()
}

/// Convert a frequency (Hz) to a bin index at `sample_rate` with `fft_size`.
pub fn freq_to_bin(freq_hz: f32, sample_rate: f32, fft_size: usize) -> usize {
    (freq_hz * fft_size as f32 / sample_rate).round() as usize
}

/// Sum squared magnitudes across `[lo_hz, hi_hz)`.
pub fn band_energy(
    spectrum: &[f32],
    sample_rate: f32,
    fft_size: usize,
    lo_hz: f32,
    hi_hz: f32,
) -> f32 {
    let lo = freq_to_bin(lo_hz, sample_rate, fft_size).min(spectrum.len() - 1);
    let hi = freq_to_bin(hi_hz, sample_rate, fft_size).min(spectrum.len() - 1);
    spectrum[lo..hi].iter().map(|m| m * m).sum()
}

/// Bin index (excluding DC and Nyquist) with the greatest magnitude.
pub fn dominant_bin(spectrum: &[f32]) -> usize {
    let mut best = 1;
    let mut best_mag = 0.0f32;
    for (i, &m) in spectrum.iter().enumerate().take(spectrum.len() - 1).skip(1) {
        if m > best_mag {
            best_mag = m;
            best = i;
        }
    }
    best
}

/// Windowed RMS envelope: non-overlapping blocks of `window` samples.
pub fn windowed_rms(signal: &[f32], window: usize) -> Vec<f32> {
    assert!(window > 0);
    signal
        .chunks(window)
        .map(|c| {
            let sum_sq: f32 = c.iter().map(|&x| x * x).sum();
            (sum_sq / c.len() as f32).sqrt()
        })
        .collect()
}
