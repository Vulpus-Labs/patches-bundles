//! Spectral pitch shifter via WOLA.

use patches_dsp::RealPackedFft;

use super::support::{dominant_bin, rms, run_pitch_shift};

#[test]
fn pitch_shift_octave_up_doubles_frequency() {
    let window_size = 1024;
    let source_bin: usize = 8;
    let n_input = window_size * 32;
    let input: Vec<f32> = (0..n_input)
        .map(|i| {
            (2.0 * std::f32::consts::PI * source_bin as f32 * i as f32 / window_size as f32).sin()
        })
        .collect();

    let output = run_pitch_shift(&input, window_size, 4, 12.0);

    let skip = window_size * 6;
    let chunk = &output[skip..skip + window_size];
    let fft = RealPackedFft::new(window_size);
    let bin = dominant_bin(chunk, &fft);

    assert!(
        (bin as i32 - (source_bin * 2) as i32).unsigned_abs() <= 1,
        "expected dominant bin near {}, got {bin}",
        source_bin * 2
    );
}

#[test]
fn pitch_shift_octave_down_halves_frequency() {
    let window_size = 1024;
    let source_bin: usize = 16;
    let n_input = window_size * 32;
    let input: Vec<f32> = (0..n_input)
        .map(|i| {
            (2.0 * std::f32::consts::PI * source_bin as f32 * i as f32 / window_size as f32).sin()
        })
        .collect();

    let output = run_pitch_shift(&input, window_size, 4, -12.0);

    let skip = window_size * 6;
    let chunk = &output[skip..skip + window_size];
    let fft = RealPackedFft::new(window_size);
    let bin = dominant_bin(chunk, &fft);

    assert!(
        (bin as i32 - (source_bin / 2) as i32).unsigned_abs() <= 1,
        "expected dominant bin near {}, got {bin}",
        source_bin / 2
    );
}

#[test]
fn pitch_shift_identity_preserves_signal() {
    let window_size = 256;
    let source_bin = 5;
    let n_input = window_size * 16;
    let input: Vec<f32> = (0..n_input)
        .map(|i| {
            (2.0 * std::f32::consts::PI * source_bin as f32 * i as f32 / window_size as f32).sin()
        })
        .collect();

    let output = run_pitch_shift(&input, window_size, 4, 0.0);

    let skip = window_size * 4;
    let len = window_size * 8;
    let steady_out = &output[skip..skip + len];
    let steady_in = &input[skip..skip + len];

    let error: Vec<f32> = steady_out
        .iter()
        .zip(steady_in.iter())
        .map(|(&a, &b)| a - b)
        .collect();
    let error_rms = rms(&error);
    let signal_rms = rms(steady_in);

    assert!(
        error_rms < signal_rms * 0.15,
        "identity shift should preserve signal: signal_rms={signal_rms}, error_rms={error_rms}"
    );
}

#[test]
fn pitch_shift_fifth_up() {
    let window_size = 1024;
    let source_bin = 10;
    let expected_bin = (source_bin as f32 * 2.0f32.powf(7.0 / 12.0)).round() as usize;
    let n_input = window_size * 32;
    let input: Vec<f32> = (0..n_input)
        .map(|i| {
            (2.0 * std::f32::consts::PI * source_bin as f32 * i as f32 / window_size as f32).sin()
        })
        .collect();

    let output = run_pitch_shift(&input, window_size, 4, 7.0);

    let skip = window_size * 6;
    let chunk = &output[skip..skip + window_size];
    let fft = RealPackedFft::new(window_size);
    let bin = dominant_bin(chunk, &fft);

    assert!(
        (bin as i32 - expected_bin as i32).unsigned_abs() <= 1,
        "expected dominant bin near {expected_bin}, got {bin}"
    );
}
