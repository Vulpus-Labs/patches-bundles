//! Phase-vocoder spectral pitch shifter (Laroche & Dolson region-based).
//!
//! Shifts pitch by identifying spectral peaks, partitioning bins into regions
//! of influence, and shifting entire regions as blocks with a single complex
//! rotation per region.  This preserves inter-bin phase coherence (identity
//! phase locking) without requiring per-bin phase interpolation.
//!
//! Based on: Laroche & Dolson, "New Phase-Vocoder Techniques for
//! Pitch-Shifting, Harmonizing and Other Exotic Effects" (1999) and
//! US Patent US6549884B1 (expired 2019).
//!
//! Operates on the packed real FFT format produced by [`RealPackedFft`]:
//!
//! ```text
//!   [0]     = DC (real)
//!   [1]     = Nyquist (real)
//!   [2k]    = bin k real,  k = 1 .. N/2-1
//!   [2k+1]  = bin k imag
//! ```
//!
//! Call [`SpectralPitchShifter::transform`] on each windowed, FFT'd frame
//! between the forward and inverse FFT steps.

use std::f32::consts::PI;

const TWO_PI: f32 = 2.0 * PI;

/// Phase-vocoder pitch shifter for packed real FFT spectra.
///
/// Construct once per window/hop configuration. Call [`transform`](Self::transform)
/// on each frame's packed spectrum (between forward and inverse FFT).
///
/// # Parameters (set between frames)
///
/// - **shift_ratio**: frequency multiplier (2.0 = octave up, 0.5 = octave down).
///   Use [`set_shift_semitones`](Self::set_shift_semitones) for musical intervals.
/// - **mix**: dry/wet blend (0.0 = original, 1.0 = fully shifted).
/// - **preserve_formants**: when true, applies spectral envelope correction to
///   avoid the "chipmunk effect" on upward shifts.
pub struct SpectralPitchShifter {
    /// Number of frequency bins: N/2 + 1 (DC through Nyquist inclusive).
    half_n: usize,
    /// `2π · hop_size / window_size` — expected phase advance per bin per hop.
    phase_scale: f32,

    // Parameters
    shift_ratio: f32,
    mix: f32,
    preserve_formants: bool,
    mono: bool,

    // Shared phase tracking
    prev_phase: Vec<f32>,

    // Region-based (mono) state
    synth_phase: Vec<f32>,
    peaks: Vec<usize>,

    // Per-bin (poly) state
    phase_deviation: Vec<f32>,
    phase_accumulator: Vec<f32>,

    // Shared working buffers (pre-allocated, avoid per-frame allocation)
    analysis_re: Vec<f32>,
    analysis_im: Vec<f32>,
    shifted_re: Vec<f32>,
    shifted_im: Vec<f32>,
    magnitude: Vec<f32>,
    phase: Vec<f32>,
    shifted_mag: Vec<f32>,
    original_spectrum: Vec<f32>,
    envelope_buf: Vec<f32>,
    shifted_envelope_buf: Vec<f32>,
}

impl SpectralPitchShifter {
    /// Create a pitch shifter for the given window and hop sizes.
    ///
    /// `window_size` must match the [`RealPackedFft`] length. `hop_size` is
    /// typically `window_size / overlap_factor`.
    pub fn new(window_size: usize, hop_size: usize) -> Self {
        let half_n = (window_size >> 1) + 1;
        Self {
            half_n,
            phase_scale: TWO_PI * hop_size as f32 / window_size as f32,
            shift_ratio: 1.0,
            mix: 1.0,
            preserve_formants: false,
            mono: false,
            prev_phase: vec![0.0; half_n],
            synth_phase: vec![0.0; half_n],
            peaks: Vec::with_capacity(half_n / 4),
            phase_deviation: vec![0.0; half_n],
            phase_accumulator: vec![0.0; half_n],
            analysis_re: vec![0.0; half_n],
            analysis_im: vec![0.0; half_n],
            shifted_re: vec![0.0; half_n],
            shifted_im: vec![0.0; half_n],
            magnitude: vec![0.0; half_n],
            phase: vec![0.0; half_n],
            shifted_mag: vec![0.0; half_n],
            original_spectrum: vec![0.0; window_size],
            envelope_buf: vec![0.0; half_n],
            shifted_envelope_buf: vec![0.0; half_n],
        }
    }

    /// Set pitch shift in semitones (+12 = octave up, -12 = octave down, +7 = perfect fifth).
    pub fn set_shift_semitones(&mut self, semitones: f32) {
        self.shift_ratio = (2.0f32).powf(semitones / 12.0);
    }

    /// Set pitch shift as a raw frequency ratio (2.0 = octave up, 0.5 = octave down).
    pub fn set_shift_ratio(&mut self, ratio: f32) {
        self.shift_ratio = ratio;
    }

    /// Set dry/wet mix. Clamped to `[0, 1]`.
    pub fn set_mix(&mut self, mix: f32) {
        self.mix = mix.clamp(0.0, 1.0);
    }

    /// Enable or disable formant preservation.
    pub fn set_preserve_formants(&mut self, preserve: bool) {
        self.preserve_formants = preserve;
    }

    /// Enable mono mode (region-based Laroche & Dolson shifting).
    ///
    /// - **mono = true**: shifts entire spectral regions as blocks with a
    ///   single complex rotation per peak.  Best for monophonic input
    ///   (eliminates phasiness artefacts).
    /// - **mono = false** (default): per-bin resampling with independent
    ///   phase propagation.  Better for polyphonic input where dense peaks
    ///   make region boundaries audible.
    pub fn set_mono(&mut self, mono: bool) {
        self.mono = mono;
    }

    /// Reset phase tracking state. Call when starting a new stream.
    pub fn reset(&mut self) {
        self.prev_phase.fill(0.0);
        self.synth_phase.fill(0.0);
        self.phase_accumulator.fill(0.0);
    }

    /// Transform a packed spectrum in-place.
    ///
    /// `spectrum` must have length `window_size` (the value passed to [`new`](Self::new))
    /// and contain the output of [`RealPackedFft::forward`].
    pub fn transform(&mut self, spectrum: &mut [f32]) {
        let half_n = self.half_n;
        let shift_ratio = self.shift_ratio;

        // Save original for mixing.
        if self.mix < 1.0 {
            self.original_spectrum[..spectrum.len()].copy_from_slice(spectrum);
        }

        // 1. Unpack packed real FFT into separate real/imag arrays.
        self.unpack(spectrum);

        // 2. Extract magnitude and phase.
        for k in 0..half_n {
            self.magnitude[k] = self.analysis_re[k].hypot(self.analysis_im[k]);
            self.phase[k] = self.analysis_im[k].atan2(self.analysis_re[k]);
        }

        // 3. Spectral envelope for formant preservation.
        if self.preserve_formants {
            spectral_envelope_into(&self.magnitude, half_n, &mut self.envelope_buf);
        }

        // 4. Shift — dispatch to mono (region-based) or poly (per-bin).
        //    Both paths produce output in shifted_re / shifted_im.
        if self.mono {
            self.shift_region_based();
        } else {
            self.shift_per_bin();
        }

        // 5. Update previous phase for next frame.
        self.prev_phase.copy_from_slice(&self.phase);

        // 6. Formant correction.
        if self.preserve_formants {
            self.apply_formant_correction(shift_ratio);
        }

        // 7. Pack back into packed real FFT format.
        self.pack(spectrum);

        // 8. Complex-domain dry/wet mix.
        if self.mix < 1.0 {
            mix_complex_spectra(&self.original_spectrum, spectrum, self.mix, half_n);
        }
    }

    /// Region-based shifting (Laroche & Dolson).
    ///
    /// Identifies spectral peaks, partitions bins into regions, and shifts
    /// each region as a block with a single complex rotation.  Best for
    /// monophonic input.
    fn shift_region_based(&mut self) {
        let half_n = self.half_n;
        let shift_ratio = self.shift_ratio;

        self.detect_peaks();

        self.shifted_re.fill(0.0);
        self.shifted_im.fill(0.0);

        let num_peaks = self.peaks.len();
        for i in 0..num_peaks {
            let p = self.peaks[i];

            let left = if i == 0 {
                0
            } else {
                (self.peaks[i - 1] + p).div_ceil(2)
            };
            let right = if i == num_peaks - 1 {
                half_n
            } else {
                (p + self.peaks[i + 1]).div_ceil(2)
            };

            let target = (p as f32 * shift_ratio).round() as isize;
            let delta = target - p as isize;

            let inst_freq = self.phase_scale * p as f32
                + principal_argument(
                    self.phase[p] - self.prev_phase[p] - self.phase_scale * p as f32,
                );

            let omega_output = inst_freq * shift_ratio;

            let target_usize = target.clamp(0, half_n as isize - 1) as usize;
            self.synth_phase[target_usize] += omega_output;
            let phi_s = self.synth_phase[target_usize];
            self.synth_phase[target_usize] = principal_argument(phi_s);

            let rotation = phi_s - self.phase[p];
            let (sin_r, cos_r) = rotation.sin_cos();

            for k in left..right {
                let target_k = k as isize + delta;
                if target_k >= 0 && (target_k as usize) < half_n {
                    let tk = target_k as usize;
                    let re = self.analysis_re[k];
                    let im = self.analysis_im[k];
                    self.shifted_re[tk] += cos_r * re - sin_r * im;
                    self.shifted_im[tk] += sin_r * re + cos_r * im;
                }
            }
        }
    }

    /// Per-bin resampling (standard phase vocoder).
    ///
    /// Each output bin independently reads from a fractional source position,
    /// interpolating magnitude and phase deviation.  No phase locking — each
    /// bin's phase propagates independently.  Better for polyphonic input.
    fn shift_per_bin(&mut self) {
        let half_n = self.half_n;
        let shift_ratio = self.shift_ratio;

        // Phase deviations.
        for k in 0..half_n {
            let expected = self.prev_phase[k] + self.phase_scale * k as f32;
            self.phase_deviation[k] = principal_argument(self.phase[k] - expected);
        }

        // Resample bins with interpolation.
        for k in 0..half_n {
            let source = k as f32 / shift_ratio;
            if source >= (half_n - 1) as f32 {
                self.shifted_re[k] = 0.0;
                self.shifted_im[k] = 0.0;
                self.phase_accumulator[k] = 0.0;
            } else {
                let mag = cubic(&self.magnitude, source);
                let interp_dev = cubic(&self.phase_deviation, source);
                let advance = self.phase_scale * k as f32;
                let ph =
                    self.phase_accumulator[k] + advance + shift_ratio * interp_dev;
                self.phase_accumulator[k] = principal_argument(ph);
                let (sin_p, cos_p) = ph.sin_cos();
                self.shifted_re[k] = mag * cos_p;
                self.shifted_im[k] = mag * sin_p;
            }
        }
    }

    // -- internal helpers ----------------------------------------------------

    /// Unpack packed real FFT format into separate real/imag arrays.
    fn unpack(&mut self, spectrum: &[f32]) {
        self.analysis_re[0] = spectrum[0];
        self.analysis_im[0] = 0.0;

        let last = self.half_n - 1;
        self.analysis_re[last] = spectrum[1];
        self.analysis_im[last] = 0.0;

        for k in 1..last {
            self.analysis_re[k] = spectrum[2 * k];
            self.analysis_im[k] = spectrum[2 * k + 1];
        }
    }

    /// Pack shifted real/imag arrays back into packed real FFT format.
    fn pack(&self, spectrum: &mut [f32]) {
        spectrum[0] = self.shifted_re[0];
        spectrum[1] = self.shifted_re[self.half_n - 1];

        let last = self.half_n - 1;
        for k in 1..last {
            spectrum[2 * k] = self.shifted_re[k];
            spectrum[2 * k + 1] = self.shifted_im[k];
        }
    }

    /// Detect spectral peaks.  Interior bins use a 4-neighbour criterion
    /// (must exceed 2 bins on each side); edge bins use 2-neighbour.
    fn detect_peaks(&mut self) {
        let half_n = self.half_n;
        self.peaks.clear();

        for k in 1..half_n - 1 {
            let m = self.magnitude[k];
            // Must exceed immediate neighbours.
            if m <= self.magnitude[k - 1] || m <= self.magnitude[k + 1] {
                continue;
            }
            // Interior bins: also check second neighbours.
            if k >= 2 && m <= self.magnitude[k - 2] {
                continue;
            }
            if k + 2 < half_n && m <= self.magnitude[k + 2] {
                continue;
            }
            self.peaks.push(k);
        }
    }

    /// Apply formant correction: rescale shifted magnitudes so the spectral
    /// envelope matches the original, then adjust complex bins accordingly.
    fn apply_formant_correction(&mut self, shift_ratio: f32) {
        let half_n = self.half_n;

        // Extract shifted magnitudes.
        for k in 0..half_n {
            self.shifted_mag[k] = self.shifted_re[k].hypot(self.shifted_im[k]);
        }

        // Compute shifted envelope and correct magnitudes.
        apply_formant_envelope(
            &mut self.shifted_mag,
            &self.envelope_buf,
            &mut self.shifted_envelope_buf,
            half_n,
            shift_ratio,
        );

        // Rescale complex bins to match corrected magnitudes.
        // Guard threshold set well above f32 denormal range to avoid
        // extreme scale factors from near-zero denominators.
        const MAG_FLOOR: f32 = 1e-10;
        for k in 0..half_n {
            let current = self.shifted_re[k].hypot(self.shifted_im[k]);
            if current > MAG_FLOOR {
                let scale = self.shifted_mag[k] / current;
                self.shifted_re[k] *= scale;
                self.shifted_im[k] *= scale;
            }
        }
    }
}

// -- free functions ----------------------------------------------------------

/// Wrap phase into `(-π, π]`.
fn principal_argument(phase: f32) -> f32 {
    phase - TWO_PI * (phase / TWO_PI).round()
}

/// Linear interpolation into `data` at a fractional index.
fn lerp(data: &[f32], index: f32) -> f32 {
    if data.is_empty() || index < 0.0 {
        return 0.0;
    }
    let floor = index as usize;
    if floor + 1 >= data.len() {
        return data[data.len() - 1];
    }
    let frac = index - floor as f32;
    data[floor] * (1.0 - frac) + data[floor + 1] * frac
}

/// Cubic (Catmull-Rom) interpolation into `data` at a fractional index.
///
/// Uses 4 neighbouring samples for a smoother curve than linear.  Falls back
/// to linear at the edges where 4-point support is unavailable.
fn cubic(data: &[f32], index: f32) -> f32 {
    if index < 0.0 {
        return 0.0;
    }
    let n = data.len();
    let i = index as usize;
    if i >= n - 1 {
        return data[n - 1];
    }
    let t = index - i as f32;

    // Need indices i-1, i, i+1, i+2.  Clamp at boundaries.
    let y0 = data[i.saturating_sub(1)];
    let y1 = data[i];
    let y2 = data[(i + 1).min(n - 1)];
    let y3 = data[(i + 2).min(n - 1)];

    // Catmull-Rom spline.
    let a = -0.5 * y0 + 1.5 * y1 - 1.5 * y2 + 0.5 * y3;
    let b = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
    let c = -0.5 * y0 + 0.5 * y2;
    let d = y1;

    ((a * t + b) * t + c) * t + d
}

/// Smoothed spectral envelope (moving average over magnitude).
/// Writes into the provided `out` buffer (must be at least `half_n` long).
fn spectral_envelope_into(magnitude: &[f32], half_n: usize, out: &mut [f32]) {
    let width = (half_n / 32).max(4);
    for (i, env) in out.iter_mut().enumerate().take(half_n) {
        let start = i.saturating_sub(width);
        let end = (i + width).min(half_n);
        let sum: f32 = magnitude[start..end].iter().sum();
        *env = sum / (end - start) as f32;
    }
}

/// Apply formant correction: rescale `shifted_mag` so its spectral envelope
/// matches the original `envelope` (looked up at the pre-shift bin position).
fn apply_formant_envelope(
    shifted_mag: &mut [f32],
    original_envelope: &[f32],
    shifted_envelope: &mut [f32],
    half_n: usize,
    shift_ratio: f32,
) {
    spectral_envelope_into(shifted_mag, half_n, shifted_envelope);
    for k in 0..half_n {
        let env_source = k as f32 * shift_ratio;
        if env_source < (original_envelope.len() - 1) as f32 {
            let target = lerp(original_envelope, env_source);
            let current = shifted_envelope[k];
            if current > 1e-10 {
                shifted_mag[k] *= target / current;
            }
        }
    }
}

/// Linearly blend two packed spectra in the complex domain.
fn mix_complex_spectra(dry: &[f32], wet: &mut [f32], wet_amount: f32, half_n: usize) {
    let dry_amount = 1.0 - wet_amount;

    // DC and Nyquist (real only).
    wet[0] = dry_amount * dry[0] + wet_amount * wet[0];
    wet[1] = dry_amount * dry[1] + wet_amount * wet[1];

    // Interior complex bins.
    let last = half_n - 1;
    for k in 1..last {
        let idx = 2 * k;
        wet[idx] = dry_amount * dry[idx] + wet_amount * wet[idx];
        wet[idx + 1] = dry_amount * dry[idx + 1] + wet_amount * wet[idx + 1];
    }
}

#[cfg(test)]
mod tests;
