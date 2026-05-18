//! Shared analog-emulation primitives for `patches-vintage`.
//!
//! Hosts the triangle LFO and the one-pole HP/LP idioms used by the
//! BBD modules (chorus, flanger, BBD delays, reverb damping). The
//! point of consolidation is exactly the precision pitfall flagged in
//! E150: three sibling modules each re-derived
//! `4 * (phase - (phase+0.5).floor()).abs() - 1` and
//! `1 - exp(-TAU·fc/sr)` and one of them already drifted (the November
//! sweep caught `Tap::fb_hp_r` using a Taylor approximation instead of
//! the exact exponential).
//!
//! These primitives stay local to `patches-vintage` per E150's
//! "Out of scope" note. If a second bundle wants the same shapes,
//! lift then.

use std::f32::consts::TAU;

// ── Triangle LFO ────────────────────────────────────────────────────────────

/// Strict triangle LFO in `[-1, +1]`. Phase is wrapped to `[0, 1)`
/// internally; the triangle is generated via
/// `4 * |phase - (phase + 0.5).floor()| - 1` and clamped against
/// floating-point residual so callers never see a stray sample
/// just outside the unit range.
#[derive(Default, Clone, Copy)]
pub(crate) struct TriangleLfo {
    pub(crate) phase: f32,
    increment: f32,
}

impl TriangleLfo {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Set the per-sample phase increment from `rate_hz` at `sample_rate`.
    pub(crate) fn set_rate(&mut self, rate_hz: f32, sample_rate: f32) {
        self.increment = rate_hz / sample_rate;
    }

    /// Advance the phase and return the triangle sample in `[-1, +1]`.
    #[inline]
    pub(crate) fn tick(&mut self) -> f32 {
        self.phase += self.increment;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        let tri = 4.0 * (self.phase - (self.phase + 0.5).floor()).abs() - 1.0;
        tri.clamp(-1.0, 1.0)
    }
}

// ── One-pole low-pass ───────────────────────────────────────────────────────

/// One-pole low-pass smoother: `y[n] = y[n-1] + α (x[n] - y[n-1])`
/// where `α = 1 - exp(-2π fc / sr)`. DC gain is unity. Used as the
/// post-BBD reconstruction filter in vchorus and as the LF/HF split
/// and recon LPF in the vflanger pair.
#[derive(Default, Clone, Copy)]
pub(crate) struct OnePoleLpf {
    alpha: f32,
    y: f32,
}

impl OnePoleLpf {
    /// Coefficient for a one-pole LPF at `cutoff_hz` running at
    /// `sample_rate`. Useful when the caller maintains its own state
    /// array (e.g. vreverb's per-line damping cascade) instead of
    /// owning an `OnePoleLpf` value.
    #[inline]
    pub(crate) fn alpha_for(cutoff_hz: f32, sample_rate: f32) -> f32 {
        1.0 - (-TAU * cutoff_hz / sample_rate).exp()
    }

    pub(crate) fn set_cutoff(&mut self, cutoff_hz: f32, sample_rate: f32) {
        self.alpha = Self::alpha_for(cutoff_hz, sample_rate);
    }

    #[inline]
    pub(crate) fn process(&mut self, x: f32) -> f32 {
        self.y += self.alpha * (x - self.y);
        self.y
    }
}

// ── One-pole high-pass (DC blocker) ─────────────────────────────────────────

/// One-pole high-pass / DC blocker: `y[n] = x[n] - x[n-1] + r · y[n-1]`
/// where `r = exp(-2π fc / sr)`. DC gain is zero. Matches the HP shape
/// originally inlined into `Tap::filter_feedback` in vbbd.
#[derive(Default, Clone, Copy)]
pub(crate) struct OnePoleHpf {
    r: f32,
    x_prev: f32,
    y_prev: f32,
}

impl OnePoleHpf {
    pub(crate) fn set_cutoff(&mut self, cutoff_hz: f32, sample_rate: f32) {
        self.r = (-TAU * cutoff_hz / sample_rate).exp();
    }

    #[inline]
    pub(crate) fn process(&mut self, x: f32) -> f32 {
        let y = x - self.x_prev + self.r * self.y_prev;
        self.x_prev = x;
        self.y_prev = y;
        y
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    const SR: f32 = 48_000.0;

    // ── Triangle LFO ────────────────────────────────────────────────────────

    #[test]
    fn triangle_lfo_peak_at_plus_minus_one() {
        let mut lfo = TriangleLfo::new();
        lfo.set_rate(1.0, SR);
        let mut max = f32::MIN;
        let mut min = f32::MAX;
        for _ in 0..SR as usize + 1 {
            let y = lfo.tick();
            if y > max {
                max = y;
            }
            if y < min {
                min = y;
            }
        }
        assert!((max - 1.0).abs() < 1e-3, "max should be ~+1, got {max}");
        assert!((min + 1.0).abs() < 1e-3, "min should be ~-1, got {min}");
    }

    #[test]
    fn triangle_lfo_period_matches_one_over_rate() {
        let rate = 50.0_f32;
        let mut lfo = TriangleLfo::new();
        lfo.set_rate(rate, SR);
        // Run one full period and confirm phase returned ~to start.
        let samples_per_period = (SR / rate) as usize;
        for _ in 0..samples_per_period {
            lfo.tick();
        }
        // After one period, phase should be ~0 again (modulo FP).
        assert!(lfo.phase < 0.01 || lfo.phase > 0.99,
            "phase didn't wrap after one period: {}", lfo.phase);
    }

    #[test]
    fn triangle_lfo_halves_are_monotonic() {
        // Use a fast-enough rate that the per-sample step is well
        // above FP noise (~4·rate/sr per sample).
        let rate = 50.0_f32;
        let mut lfo = TriangleLfo::new();
        lfo.set_rate(rate, SR);
        let mut prev = lfo.tick();
        let mut rising = true;
        let mut flips = 0;
        // Two full periods → exactly four flips (two peaks, two troughs).
        let samples = (2.0 * SR / rate) as usize;
        for _ in 0..samples {
            let y = lfo.tick();
            if rising && y < prev - 1e-4 {
                rising = false;
                flips += 1;
            } else if !rising && y > prev + 1e-4 {
                rising = true;
                flips += 1;
            }
            prev = y;
        }
        assert_eq!(flips, 4, "expected 4 monotonicity flips across 2 periods, got {flips}");
    }

    // ── One-pole LP ─────────────────────────────────────────────────────────

    #[test]
    fn one_pole_lpf_dc_gain_unity() {
        let mut lpf = OnePoleLpf::default();
        lpf.set_cutoff(1_000.0, SR);
        for _ in 0..10_000 {
            lpf.process(1.0);
        }
        let y = lpf.process(1.0);
        assert!((y - 1.0).abs() < 1e-3, "DC gain should be 1.0, got {y}");
    }

    #[test]
    fn one_pole_lpf_minus_3db_at_cutoff() {
        let cutoff = 1_000.0_f32;
        let mut lpf = OnePoleLpf::default();
        lpf.set_cutoff(cutoff, SR);
        // Drive a sine at cutoff; measure RMS of output / input.
        let mut sum_in = 0.0_f64;
        let mut sum_out = 0.0_f64;
        for n in 0..(SR as usize) {
            let t = n as f32 / SR;
            let x = (TAU * cutoff * t).sin();
            // Warm up — skip first 0.1s.
            let y = lpf.process(x);
            if n >= (SR as usize / 10) {
                sum_in += (x as f64).powi(2);
                sum_out += (y as f64).powi(2);
            }
        }
        let ratio = (sum_out / sum_in).sqrt() as f32;
        let db = 20.0 * ratio.log10();
        // Standard one-pole −3 dB ≈ at fc; allow ±1 dB tolerance for FP/edges.
        assert!((-4.0..=-2.0).contains(&db),
            "−3 dB at cutoff expected, got {db:.2} dB");
    }

    // ── One-pole HP ─────────────────────────────────────────────────────────

    #[test]
    fn one_pole_hpf_dc_gain_zero() {
        let mut hpf = OnePoleHpf::default();
        hpf.set_cutoff(100.0, SR);
        for _ in 0..10_000 {
            hpf.process(1.0);
        }
        let y = hpf.process(1.0);
        assert!(y.abs() < 1e-3, "DC gain should be 0, got {y}");
    }

    #[test]
    fn one_pole_hpf_complementary_to_lpf_at_matched_cutoff() {
        // At very high frequency: HPF passes (gain ~1), LPF rolls off.
        // At DC: HPF blocks (gain 0), LPF passes (gain 1).
        // Check passband behaviour: drive at 8 kHz with fc = 200 Hz.
        let cutoff = 200.0_f32;
        let probe = 8_000.0_f32;
        let mut hpf = OnePoleHpf::default();
        hpf.set_cutoff(cutoff, SR);
        let mut sum_in = 0.0_f64;
        let mut sum_out = 0.0_f64;
        for n in 0..(SR as usize) {
            let t = n as f32 / SR;
            let x = (TAU * probe * t).sin();
            let y = hpf.process(x);
            if n >= (SR as usize / 10) {
                sum_in += (x as f64).powi(2);
                sum_out += (y as f64).powi(2);
            }
        }
        let ratio = (sum_out / sum_in).sqrt() as f32;
        let db = 20.0 * ratio.log10();
        // Well above cutoff: HP near unity. Allow generous tolerance —
        // the DC-blocker shape has subtle phase + magnitude features
        // distinct from a textbook LPF complement.
        assert!(db > -1.0, "HP at 40× cutoff should be ~0 dB, got {db:.2} dB");
    }
}
