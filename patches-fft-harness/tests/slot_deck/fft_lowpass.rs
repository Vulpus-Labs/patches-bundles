//! FFT-based brick-wall low-pass filter via WOLA.

use super::support::{rms, run_fft_lowpass};

#[test]
fn fft_lowpass_passes_low_frequency() {
    let window_size = 256;
    let n_input = window_size * 16;
    let freq_bin = 3.0;
    let input: Vec<f32> = (0..n_input)
        .map(|i| (2.0 * std::f32::consts::PI * freq_bin * i as f32 / window_size as f32).sin())
        .collect();

    let output = run_fft_lowpass(&input, window_size, 4);

    let skip = window_size * 3;
    let steady = &output[skip..skip + window_size * 8];
    let input_rms = rms(&input[skip..skip + window_size * 8]);
    let output_rms = rms(steady);

    assert!(
        output_rms > input_rms * 0.9,
        "low-freq signal should pass: input_rms={input_rms}, output_rms={output_rms}"
    );
}

#[test]
fn fft_lowpass_attenuates_high_frequency() {
    let window_size = 256;
    let n_input = window_size * 16;
    let freq_bin = 96.0;
    let input: Vec<f32> = (0..n_input)
        .map(|i| (2.0 * std::f32::consts::PI * freq_bin * i as f32 / window_size as f32).sin())
        .collect();

    let output = run_fft_lowpass(&input, window_size, 4);

    let skip = window_size * 3;
    let steady = &output[skip..skip + window_size * 8];
    let input_rms = rms(&input[skip..skip + window_size * 8]);
    let output_rms = rms(steady);

    assert!(
        output_rms < input_rms * 0.1,
        "high-freq signal should be attenuated: input_rms={input_rms}, output_rms={output_rms}"
    );
}

#[test]
fn fft_lowpass_mixed_signal_preserves_low_removes_high() {
    let window_size = 256;
    let n_input = window_size * 16;
    let lo_bin = 4.0;
    let hi_bin = 100.0;
    let input: Vec<f32> = (0..n_input)
        .map(|i| {
            let t = i as f32 / window_size as f32;
            let lo = (2.0 * std::f32::consts::PI * lo_bin * t).sin();
            let hi = (2.0 * std::f32::consts::PI * hi_bin * t).sin();
            lo + hi
        })
        .collect();

    let output = run_fft_lowpass(&input, window_size, 4);

    let skip = window_size * 3;
    let len = window_size * 8;
    let steady = &output[skip..skip + len];

    let expected_lo: Vec<f32> = (skip..skip + len)
        .map(|i| (2.0 * std::f32::consts::PI * lo_bin * i as f32 / window_size as f32).sin())
        .collect();

    let error_rms = rms(
        &steady
            .iter()
            .zip(expected_lo.iter())
            .map(|(&a, &b)| a - b)
            .collect::<Vec<_>>(),
    );
    let signal_rms = rms(&expected_lo);

    assert!(
        error_rms < signal_rms * 0.2,
        "filtered output should match low component: signal_rms={signal_rms}, error_rms={error_rms}"
    );
}
