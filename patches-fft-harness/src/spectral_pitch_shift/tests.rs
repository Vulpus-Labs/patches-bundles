use super::*;
use crate::test_support::assert_within;

#[test]
fn principal_argument_wraps_correctly() {
    assert_within!(0.0, principal_argument(0.0), 1e-6);
    // PI and -PI are equivalent; principal_argument may return either.
    assert_within!(PI, principal_argument(PI).abs(), 1e-5);
    assert_within!(PI, principal_argument(3.0 * PI).abs(), 1e-5);
    assert_within!(PI, principal_argument(-3.0 * PI).abs(), 1e-5);
    // Small values pass through unchanged.
    assert_within!(1.0, principal_argument(1.0), 1e-6);
    assert_within!(-1.0, principal_argument(-1.0), 1e-6);
}

#[test]
fn lerp_basic() {
    let data = [0.0f32, 1.0, 2.0, 3.0];
    assert_within!(0.0, lerp(&data, 0.0), 1e-6);
    assert_within!(0.5, lerp(&data, 0.5), 1e-6);
    assert_within!(2.5, lerp(&data, 2.5), 1e-6);
    // Clamp at end.
    assert_within!(3.0, lerp(&data, 10.0), 1e-6);
}

#[test]
fn identity_shift_preserves_spectrum() {
    let window_size = 64;
    let hop_size = 16;
    let mut shifter = SpectralPitchShifter::new(window_size, hop_size);
    shifter.set_shift_ratio(1.0);

    // Single peak at bin 4 — no DC energy to avoid edge-case
    // interactions with region boundaries.
    let mut spectrum = vec![0.0f32; window_size];
    spectrum[8] = 0.5; // bin 4 real
    spectrum[9] = 0.3; // bin 4 imag

    let original = spectrum.clone();
    shifter.transform(&mut spectrum);

    // With ratio=1.0, delta=0 for every peak, and the complex rotation
    // is a multiple of 2π on the first frame → output ≈ input.
    for (i, (&a, &b)) in original.iter().zip(spectrum.iter()).enumerate() {
        assert_within!(a, b, 1e-3, "bin {i}: expected {a}, got {b}");
    }
}

#[test]
fn octave_up_shifts_bins() {
    let window_size = 64;
    let hop_size = 16;
    let mut shifter = SpectralPitchShifter::new(window_size, hop_size);
    shifter.set_shift_semitones(12.0); // octave up → ratio = 2.0

    // Put energy at bin 4 only.
    let mut spectrum = vec![0.0f32; window_size];
    spectrum[8] = 1.0; // bin 4 real

    shifter.transform(&mut spectrum);

    // Bin 8 (= 4 * 2) should now have energy.
    let mag_8 = spectrum[16].hypot(spectrum[17]);
    // Bin 4 should be near zero (shifted away).
    let mag_4 = spectrum[8].hypot(spectrum[9]);
    assert!(
        mag_8 > 0.5,
        "bin 8 should have energy after octave-up: {mag_8}"
    );
    assert!(
        mag_4 < 0.1,
        "bin 4 should be mostly empty after octave-up: {mag_4}"
    );
}

#[test]
fn mix_blends_dry_wet() {
    let window_size = 64;
    let hop_size = 16;
    let mut shifter = SpectralPitchShifter::new(window_size, hop_size);
    shifter.set_shift_semitones(12.0);
    shifter.set_mix(0.0); // fully dry

    let mut spectrum = vec![0.0f32; window_size];
    spectrum[8] = 1.0;
    let original = spectrum.clone();

    shifter.transform(&mut spectrum);

    // mix=0 → output should equal original.
    for (i, (&a, &b)) in original.iter().zip(spectrum.iter()).enumerate() {
        assert_within!(a, b, 1e-6, "mix=0 bin {i}: expected {a}, got {b}");
    }
}

#[test]
fn region_preserves_phase_coherence() {
    // A peak with sidelobes shifted by a fifth: all bins in the region
    // get the same complex rotation, so inter-bin phase relationships
    // from the analysis are preserved in the output.
    let window_size = 128;
    let hop_size = 32;
    let mut shifter = SpectralPitchShifter::new(window_size, hop_size);
    shifter.set_shift_semitones(7.0); // perfect fifth, ratio ≈ 1.498
    shifter.set_mono(true);

    // Simulate a windowed sinusoid: peak at bin 10 with sidelobes.
    let mut spectrum = vec![0.0f32; window_size];
    // Bin 10: magnitude 1.0, phase 0.3
    spectrum[20] = 0.3f32.cos(); // re
    spectrum[21] = 0.3f32.sin(); // im
    // Bin 9: magnitude 0.3, phase 0.5
    spectrum[18] = 0.3 * 0.5f32.cos();
    spectrum[19] = 0.3 * 0.5f32.sin();
    // Bin 11: magnitude 0.3, phase -0.2
    spectrum[22] = 0.3 * (-0.2f32).cos();
    spectrum[23] = 0.3 * (-0.2f32).sin();

    // Input phase differences between sidelobes and peak.
    let input_diff_9_10 = principal_argument(0.5 - 0.3);
    let input_diff_11_10 = principal_argument(-0.2 - 0.3);

    // Run a frame.
    shifter.transform(&mut spectrum);

    // Target bin ≈ round(10 * 1.498) = 15.  Region shifts by +5.
    // Bins 9,10,11 → bins 14,15,16.
    let phase_of = |bin: usize| -> f32 {
        spectrum[2 * bin + 1].atan2(spectrum[2 * bin])
    };

    let p15 = phase_of(15);
    let p14 = phase_of(14);
    let p16 = phase_of(16);

    // The rotation is the same for all bins in the region, so the
    // output inter-bin phase differences should equal the input ones.
    let output_diff_14_15 = principal_argument(p14 - p15);
    let output_diff_16_15 = principal_argument(p16 - p15);

    assert_within!(
        input_diff_9_10, output_diff_14_15, 1e-4,
        "phase diff 14-15 should match input diff 9-10"
    );
    assert_within!(
        input_diff_11_10, output_diff_16_15, 1e-4,
        "phase diff 16-15 should match input diff 11-10"
    );
}

#[test]
fn reset_clears_phase_state() {
    let mut shifter = SpectralPitchShifter::new(64, 16);
    shifter.set_shift_semitones(7.0);

    let mut spectrum = vec![0.0f32; 64];
    spectrum[8] = 1.0;

    // Test poly mode (per-bin accumulator).
    shifter.transform(&mut spectrum);
    assert!(shifter.phase_accumulator.iter().any(|&p| p != 0.0));

    shifter.reset();
    assert!(shifter.phase_accumulator.iter().all(|&p| p == 0.0));
    assert!(shifter.prev_phase.iter().all(|&p| p == 0.0));

    // Test mono mode (synth_phase).
    shifter.set_mono(true);
    spectrum.fill(0.0);
    spectrum[8] = 1.0;
    shifter.transform(&mut spectrum);
    assert!(shifter.synth_phase.iter().any(|&p| p != 0.0));

    shifter.reset();
    assert!(shifter.synth_phase.iter().all(|&p| p == 0.0));
}

// ── End-to-end audio tests (T-0260) ────────────────────────────────────

/// Helper: generate audio, window, FFT, transform, IFFT, overlap-add.
/// Returns the reconstructed output signal.
fn pitch_shift_audio(
    signal: &[f32],
    window_size: usize,
    overlap: usize,
    semitones: f32,
    mix: f32,
) -> Vec<f32> {
    use patches_dsp::fft::RealPackedFft;

    let hop = window_size / overlap;
    let fft = RealPackedFft::new(window_size);
    let mut shifter = SpectralPitchShifter::new(window_size, hop);
    shifter.set_shift_semitones(semitones);
    shifter.set_mix(mix);

    // Hann window
    let hann: Vec<f32> = (0..window_size)
        .map(|i| {
            let n = i as f32 / window_size as f32;
            0.5 * (1.0 - (2.0 * PI * n).cos())
        })
        .collect();

    let out_len = signal.len();
    let mut output = vec![0.0f32; out_len];
    let mut norm = vec![0.0f32; out_len];

    let mut pos = 0isize;
    while (pos as usize) + window_size <= signal.len() + window_size {
        let mut frame = vec![0.0f32; window_size];
        for i in 0..window_size {
            let idx = pos + i as isize;
            if idx >= 0 && (idx as usize) < signal.len() {
                frame[i] = signal[idx as usize] * hann[i];
            }
        }

        fft.forward(&mut frame);
        shifter.transform(&mut frame);
        fft.inverse(&mut frame);

        // Overlap-add with synthesis window
        for i in 0..window_size {
            let idx = pos + i as isize;
            if idx >= 0 && (idx as usize) < out_len {
                let oi = idx as usize;
                output[oi] += frame[i] * hann[i];
                norm[oi] += hann[i] * hann[i];
            }
        }

        pos += hop as isize;
    }

    // Normalise by WOLA factor
    for i in 0..out_len {
        if norm[i] > 1e-10 {
            output[i] /= norm[i];
        }
    }
    output
}

/// 440 Hz sine shifted +12 semitones should produce ~880 Hz.
#[test]
fn pitch_shift_octave_up_audio() {
    use patches_dsp::fft::RealPackedFft;
    use crate::test_support::dominant_bin;

    let sample_rate = 48_000.0;
    let window_size = 1024;
    let overlap = 4;
    let duration = 8192;

    let signal: Vec<f32> = (0..duration)
        .map(|i| (2.0 * PI * 440.0 / sample_rate * i as f32).sin())
        .collect();

    let output = pitch_shift_audio(&signal, window_size, overlap, 12.0, 1.0);

    // Analyse output with FFT — skip transient at start
    let analysis_start = window_size * 2;
    let fft_size = 2048;
    let fft = RealPackedFft::new(fft_size);
    let mut buf = vec![0.0f32; fft_size];
    let copy_len = fft_size.min(output.len() - analysis_start);
    buf[..copy_len].copy_from_slice(&output[analysis_start..analysis_start + copy_len]);
    fft.forward(&mut buf);

    let peak = dominant_bin(&buf, fft_size);

    let expected_bin = (880.0 * fft_size as f32 / sample_rate).round() as usize;
    let bin_diff = (peak as isize - expected_bin as isize).unsigned_abs();
    assert!(
        bin_diff <= 2,
        "octave up: peak at bin {peak} (expected ~{expected_bin}, 880 Hz)"
    );
}

/// Identity shift (0 semitones) should preserve the signal.
#[test]
fn pitch_shift_identity_audio() {
    let sample_rate = 48_000.0;
    let window_size = 1024;
    let overlap = 4;
    let duration = 8192;

    let signal: Vec<f32> = (0..duration)
        .map(|i| (2.0 * PI * 440.0 / sample_rate * i as f32).sin())
        .collect();

    let output = pitch_shift_audio(&signal, window_size, overlap, 0.0, 1.0);

    // Compare steady-state region (skip transients)
    let start = window_size * 2;
    let end = duration - window_size;
    let mut sum_sq_signal = 0.0f64;
    let mut sum_sq_error = 0.0f64;
    for i in start..end {
        let s = signal[i] as f64;
        let e = (output[i] - signal[i]) as f64;
        sum_sq_signal += s * s;
        sum_sq_error += e * e;
    }
    let rms_signal = (sum_sq_signal / (end - start) as f64).sqrt();
    let rms_error = (sum_sq_error / (end - start) as f64).sqrt();
    let error_ratio = rms_error / rms_signal;
    assert!(
        error_ratio < 0.1,
        "identity shift error ratio {error_ratio:.4} should be < 0.1"
    );
}

/// Mix=0.0 should return the original signal.
#[test]
fn pitch_shift_mix_zero_audio() {
    let sample_rate = 48_000.0;
    let window_size = 1024;
    let overlap = 4;
    let duration = 8192;

    let signal: Vec<f32> = (0..duration)
        .map(|i| (2.0 * PI * 440.0 / sample_rate * i as f32).sin())
        .collect();

    let output = pitch_shift_audio(&signal, window_size, overlap, 12.0, 0.0);

    // With mix=0, output should match input in the steady-state region
    let start = window_size * 2;
    let end = duration - window_size;
    let mut sum_sq_signal = 0.0f64;
    let mut sum_sq_error = 0.0f64;
    for i in start..end {
        let s = signal[i] as f64;
        let e = (output[i] - signal[i]) as f64;
        sum_sq_signal += s * s;
        sum_sq_error += e * e;
    }
    let rms_signal = (sum_sq_signal / (end - start) as f64).sqrt();
    let rms_error = (sum_sq_error / (end - start) as f64).sqrt();
    let error_ratio = rms_error / rms_signal;
    assert!(
        error_ratio < 1e-3,
        "mix=0 error ratio {error_ratio:.6} should be < 1e-3"
    );
}

#[test]
fn multiple_peaks_shift_independently() {
    // Two peaks at different frequencies should each shift to their
    // own target bin without interfering.
    let window_size = 128;
    let hop_size = 32;
    let mut shifter = SpectralPitchShifter::new(window_size, hop_size);
    shifter.set_shift_semitones(12.0); // octave up, ratio = 2.0
    shifter.set_mono(true);

    let mut spectrum = vec![0.0f32; window_size];
    // Peak at bin 8.
    spectrum[16] = 1.0;
    // Peak at bin 20.
    spectrum[40] = 0.8;

    shifter.transform(&mut spectrum);

    // Bin 8 → target 16, bin 20 → target 40.
    let mag_16 = spectrum[32].hypot(spectrum[33]);
    let mag_40 = spectrum[2 * 40].hypot(spectrum[2 * 40 + 1]);

    assert!(
        mag_16 > 0.5,
        "target of bin 8 should have energy: {mag_16}"
    );
    assert!(
        mag_40 > 0.4,
        "target of bin 20 should have energy: {mag_40}"
    );

    // Original positions should be mostly empty.
    let mag_8 = spectrum[16].hypot(spectrum[17]);
    let mag_20 = spectrum[40].hypot(spectrum[41]);
    // bin 8 in the output IS the target of the first peak (16→32,
    // but bin 8 = spectrum[16..17] which is now target of bin 8).
    // Actually bin 8 in output = spectrum[16] which is target bin 16's
    // data.  Let me check the original bins:
    // Original bin 8 is at spectrum[16].  Target for peak at bin 8 is
    // bin 16 (spectrum[32]).  So spectrum[16] should be near zero
    // (no peak targets it).  But wait — bin 16 in the output
    // IS the target of peak 8.  Hmm, let me re-examine.
    // Target 16 is at spectrum index 2*16 = 32.  So spectrum[16] is
    // bin 8 in output.  Nothing targets bin 8, so it should be zero.
    assert!(
        mag_8 < 0.1,
        "original bin 8 position should be empty: {mag_8}"
    );
    assert!(
        mag_20 < 0.1,
        "original bin 20 position should be empty: {mag_20}"
    );
}

// ── Gap tests (E092 / ticket 0545 companion) ───────────────────────────

/// Variant of [`pitch_shift_audio`] that also toggles mono mode and formant
/// preservation. The existing helper is kept intact so pre-existing tests
/// are unchanged.
fn pitch_shift_audio_ext(
    signal: &[f32],
    window_size: usize,
    overlap: usize,
    semitones: f32,
    mix: f32,
    mono: bool,
    preserve_formants: bool,
) -> Vec<f32> {
    use patches_dsp::fft::RealPackedFft;

    let hop = window_size / overlap;
    let fft = RealPackedFft::new(window_size);
    let mut shifter = SpectralPitchShifter::new(window_size, hop);
    shifter.set_shift_semitones(semitones);
    shifter.set_mix(mix);
    shifter.set_mono(mono);
    shifter.set_preserve_formants(preserve_formants);

    let hann: Vec<f32> = (0..window_size)
        .map(|i| {
            let n = i as f32 / window_size as f32;
            0.5 * (1.0 - (2.0 * PI * n).cos())
        })
        .collect();

    let out_len = signal.len();
    let mut output = vec![0.0f32; out_len];
    let mut norm = vec![0.0f32; out_len];

    let mut pos = 0isize;
    while (pos as usize) + window_size <= signal.len() + window_size {
        let mut frame = vec![0.0f32; window_size];
        for i in 0..window_size {
            let idx = pos + i as isize;
            if idx >= 0 && (idx as usize) < signal.len() {
                frame[i] = signal[idx as usize] * hann[i];
            }
        }

        fft.forward(&mut frame);
        shifter.transform(&mut frame);
        fft.inverse(&mut frame);

        for i in 0..window_size {
            let idx = pos + i as isize;
            if idx >= 0 && (idx as usize) < out_len {
                let oi = idx as usize;
                output[oi] += frame[i] * hann[i];
                norm[oi] += hann[i] * hann[i];
            }
        }

        pos += hop as isize;
    }

    for i in 0..out_len {
        if norm[i] > 1e-10 {
            output[i] /= norm[i];
        }
    }
    output
}

/// Stationary 100 Hz sine through a ratio-1.2 (≈ +3.156 semitones) shift over
/// multiple hops should not produce a phase-flip spike at grain boundaries.
/// Adjacent-sample differences in the reconstructed output are bounded above
/// by a shape consistent with a pure sine at the shifted frequency.
#[test]
fn grain_boundary_continuity_no_phase_flip_spike() {
    let sr = 48_000.0_f32;
    let window_size = 1024;
    let overlap = 4;
    let hop = window_size / overlap;
    // Exact semitones for ratio 1.2
    let semitones = 12.0 * (1.2_f32).log2();

    // At least warmup + 3 hops of steady state.
    let duration = window_size * 4;
    let input_freq_hz = 100.0_f32;
    let signal: Vec<f32> = (0..duration)
        .map(|i| (2.0 * PI * input_freq_hz / sr * i as f32).sin())
        .collect();

    let output = pitch_shift_audio_ext(&signal, window_size, overlap, semitones, 1.0, false, false);

    // Skip initial warmup where overlap-add is building up.
    let steady_start = window_size * 2;
    let steady_end = duration - window_size;
    let mut max_diff = 0.0_f32;
    for i in steady_start + 1..steady_end {
        let d = (output[i] - output[i - 1]).abs();
        if d > max_diff {
            max_diff = d;
        }
    }
    // The shifted tone is at ~120 Hz; per-sample derivative bound is
    // 2π·120/48000 ≈ 0.0157. Use 0.05 as a generous ceiling: a phase-flip
    // spike at hop boundaries would easily exceed 0.1.
    assert!(
        max_diff <= 0.05,
        "grain-boundary max adjacent-sample diff {max_diff:.4} exceeds 0.05 \
         (hop={hop}, window={window_size}) — phase-flip at grain boundary?"
    );
}

fn peak_biquad_coeffs(f0: f32, q: f32, gain_db: f32, sr: f32) -> (f32, f32, f32, f32, f32) {
    let a = 10.0_f32.powf(gain_db / 40.0);
    let w = 2.0 * PI * f0 / sr;
    let alpha = w.sin() / (2.0 * q);
    let cos_w = w.cos();
    let b0 = 1.0 + alpha * a;
    let b1 = -2.0 * cos_w;
    let b2 = 1.0 - alpha * a;
    let a0 = 1.0 + alpha / a;
    let a1 = -2.0 * cos_w;
    let a2 = 1.0 - alpha / a;
    (b0 / a0, b1 / a0, b2 / a0, a1 / a0, a2 / a0)
}

fn biquad_tdf2(input: &[f32], c: (f32, f32, f32, f32, f32)) -> Vec<f32> {
    let (b0, b1, b2, a1, a2) = c;
    let mut s1 = 0.0_f32;
    let mut s2 = 0.0_f32;
    input
        .iter()
        .map(|&x| {
            let y = b0 * x + s1;
            s1 = b1 * x - a1 * y + s2;
            s2 = b2 * x - a2 * y;
            y
        })
        .collect()
}

fn xorshift_noise(n: usize, seed: u64) -> Vec<f32> {
    let mut s = seed.max(1);
    (0..n)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            // Map to [-1, 1)
            ((s as u32) as f32 / u32::MAX as f32) * 2.0 - 1.0
        })
        .collect()
}

/// Smoothed magnitude envelope of a signal segment. Returns the bin index of
/// the envelope peak in the steady-state FFT.
fn envelope_peak_bin(signal: &[f32], fft_size: usize) -> usize {
    use patches_dsp::fft::RealPackedFft;
    let fft = RealPackedFft::new(fft_size);
    let mut buf = vec![0.0_f32; fft_size];
    let len = signal.len().min(fft_size);
    buf[..len].copy_from_slice(&signal[..len]);
    fft.forward(&mut buf);

    let half_n = fft_size / 2 + 1;
    let mags: Vec<f32> = (0..half_n)
        .map(|k| {
            if k == 0 {
                buf[0].abs()
            } else if k == fft_size / 2 {
                buf[1].abs()
            } else {
                buf[2 * k].hypot(buf[2 * k + 1])
            }
        })
        .collect();

    // Moving-average smoothing with the same shape the shifter uses.
    let width = (half_n / 32).max(4);
    let mut env = vec![0.0_f32; half_n];
    for (i, e) in env.iter_mut().enumerate() {
        let start = i.saturating_sub(width);
        let end = (i + width).min(half_n);
        let sum: f32 = mags[start..end].iter().sum();
        *e = sum / (end - start) as f32;
    }

    // Skip DC / near-DC bins where the smoothing picks up broadband noise.
    let mut best_bin = width;
    let mut best_val = env[width];
    for (i, &v) in env.iter().enumerate().skip(width) {
        if v > best_val {
            best_val = v;
            best_bin = i;
        }
    }
    best_bin
}

/// `preserve_formants = true` keeps the input signal's spectral envelope
/// peak anchored. `preserve_formants = false` moves the envelope peak with
/// the pitch shift.
#[test]
fn formant_preservation_anchors_envelope_peak() {
    let sr = 48_000.0_f32;
    let window_size = 1024;
    let overlap = 4;
    let fft_size = 4096;
    let semitones = 9.0_f32; // ratio ≈ 1.682 — large enough to be unambiguous
    let ratio = 2f32.powf(semitones / 12.0);
    let formant_hz = 3000.0_f32;
    let duration = window_size * 12;

    // Broadband noise filtered through a resonant peak biquad at formant_hz.
    let noise = xorshift_noise(duration, 0x00DD_BA11_CAFE_BE42_u64);
    let coeffs = peak_biquad_coeffs(formant_hz, 6.0, 24.0, sr);
    let signal = biquad_tdf2(&noise, coeffs);

    // Measure input envelope peak bin over the steady-state region.
    let steady_start = window_size * 2;
    let steady_len = fft_size;
    assert!(steady_start + steady_len <= duration);
    let input_peak = envelope_peak_bin(&signal[steady_start..steady_start + steady_len], fft_size);

    // With formant preservation the envelope peak should remain near the
    // input's formant. Tolerance reflects the smoothing window width.
    let preserved =
        pitch_shift_audio_ext(&signal, window_size, overlap, semitones, 1.0, false, true);
    let preserved_peak = envelope_peak_bin(
        &preserved[steady_start..steady_start + steady_len],
        fft_size,
    );
    let preserved_delta = preserved_peak.abs_diff(input_peak);

    // Without formant preservation the envelope peak should shift with the
    // pitch shift (i.e. move to input_peak * ratio).
    let shifted =
        pitch_shift_audio_ext(&signal, window_size, overlap, semitones, 1.0, false, false);
    let shifted_peak = envelope_peak_bin(
        &shifted[steady_start..steady_start + steady_len],
        fft_size,
    );
    let expected_shifted = (input_peak as f32 * ratio) as usize;
    let shifted_delta = shifted_peak.abs_diff(expected_shifted);

    // Preserved peak closer to input peak than to the shifted position; and
    // shifted peak closer to the shifted position than to the input peak.
    let preserved_vs_shifted_target = preserved_peak.abs_diff(expected_shifted);
    assert!(
        preserved_delta < preserved_vs_shifted_target,
        "preserve=true envelope peak {preserved_peak} should be closer to input \
         peak {input_peak} than to shifted target {expected_shifted} \
         (Δ_input={preserved_delta}, Δ_shifted={preserved_vs_shifted_target})"
    );
    let shifted_vs_input = shifted_peak.abs_diff(input_peak);
    assert!(
        shifted_delta < shifted_vs_input,
        "preserve=false envelope peak {shifted_peak} should follow the shift \
         to ~{expected_shifted} rather than stay at {input_peak} \
         (Δ_shifted={shifted_delta}, Δ_input={shifted_vs_input})"
    );
}

/// Mono (region-based) and poly (per-bin) shifters on a stationary tone
/// produce output with the same dominant-frequency bin.
///
/// The two paths are not bit-identical, but for a clean tone the spectral
/// peak should land in the same bin after one grain of settling.
#[test]
fn mono_poly_parity_on_stationary_tone() {
    use patches_dsp::fft::RealPackedFft;
    use crate::test_support::dominant_bin;

    let sr = 48_000.0_f32;
    let window_size = 1024;
    let overlap = 4;
    let semitones = 7.0_f32; // ratio ≈ 1.498
    let duration = window_size * 12;

    let signal: Vec<f32> = (0..duration)
        .map(|i| (2.0 * PI * 440.0 / sr * i as f32).sin())
        .collect();

    let mono_out = pitch_shift_audio_ext(&signal, window_size, overlap, semitones, 1.0, true, false);
    let poly_out = pitch_shift_audio_ext(&signal, window_size, overlap, semitones, 1.0, false, false);

    // Analyse steady-state region (skip warmup of ~2 windows).
    let analysis_start = window_size * 3;
    let fft_size = 4096;
    let fft = RealPackedFft::new(fft_size);

    let mut mono_buf = vec![0.0_f32; fft_size];
    let mut poly_buf = vec![0.0_f32; fft_size];
    let copy_len = fft_size.min(duration - analysis_start);
    mono_buf[..copy_len].copy_from_slice(&mono_out[analysis_start..analysis_start + copy_len]);
    poly_buf[..copy_len].copy_from_slice(&poly_out[analysis_start..analysis_start + copy_len]);
    fft.forward(&mut mono_buf);
    fft.forward(&mut poly_buf);

    let mono_peak = dominant_bin(&mono_buf, fft_size);
    let poly_peak = dominant_bin(&poly_buf, fft_size);
    let bin_diff = mono_peak.abs_diff(poly_peak);
    assert!(
        bin_diff <= 2,
        "mono peak bin {mono_peak} vs poly peak bin {poly_peak}: \
         expected agreement within 2 bins, got {bin_diff}"
    );
}
