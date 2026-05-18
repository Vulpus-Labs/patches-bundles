//! Topology-preserving (TPT / zero-delay-feedback) state-variable filter.
//!
//! Standard Zavalishin ZDF SVF from "The Art of VA Filter Design" Ch. 5.
//! Implicit-feedback equation solved analytically once per coefficient
//! update; per-sample state advance is trapezoidal. Unconditionally stable
//! in normalised form, tracks `fc` accurately under any per-sample `fc`
//! trajectory — both properties Chamberlin SVF lacks, both needed by the
//! struck-resonator family's amplitude → frequency FM (see
//! [`super::bridged_t`]).
//!
//! Recurrence per sample:
//! ```text
//! v3 = x - ic2eq
//! bp = a1 * ic1eq + a2 * v3
//! lp = ic2eq + a2 * ic1eq + a3 * v3
//! ic1eq = 2 * bp - ic1eq
//! ic2eq = 2 * lp - ic2eq
//! hp = x - k * bp - lp
//! ```
//! with `g = tan(π · fc / sr)`, `k = 1 / Q`, and `a1 = 1 / (1 + g(g + k))`,
//! `a2 = g · a1`, `a3 = g · a2`.
//!
//! Crate-local for now: the gate for promotion to `patches_dsp` is a second
//! consumer.

use std::f32::consts::PI;

/// Minimum classical Q the filter will accept. The implicit-equation
/// denominator `1 + g(g + 1/Q)` stays bounded as long as `Q` does not
/// collapse the `k = 1/Q` term to infinity, so we clamp `Q ≥ 0.5`
/// (matches the lowest meaningful resonance for a bandpass biquad).
const Q_MIN: f32 = 0.5;

/// Reference: Plaits's `AnalogBassDrum` constrains `g` to `[0, 0.4]`.
/// Equivalent to a cutoff ceiling of `atan(0.4) / π ≈ 0.122 · sr` —
/// well below Nyquist, where TPT pre-warp still tracks accurately.
const G_MAX: f32 = 0.4;

pub(crate) struct TptSvf {
    sample_rate: f32,
    g: f32,
    k: f32,
    a1: f32,
    a2: f32,
    a3: f32,
    ic1eq: f32,
    ic2eq: f32,
}

impl TptSvf {
    pub(crate) fn new(sample_rate: f32) -> Self {
        let mut s = Self {
            sample_rate,
            g: 0.0,
            k: 1.0,
            a1: 1.0,
            a2: 0.0,
            a3: 0.0,
            ic1eq: 0.0,
            ic2eq: 0.0,
        };
        s.set_f_q(1000.0, 1.0);
        s
    }

    pub(crate) fn set_f_q(&mut self, fc_hz: f32, q: f32) {
        let q = q.max(Q_MIN);
        let fc = fc_hz.max(1.0);
        let g = (PI * fc / self.sample_rate).tan().min(G_MAX);
        let k = 1.0 / q;
        let a1 = 1.0 / (1.0 + g * (g + k));
        self.g = g;
        self.k = k;
        self.a1 = a1;
        self.a2 = g * a1;
        self.a3 = g * self.a2;
    }

    pub(crate) fn reset_state(&mut self) {
        self.ic1eq = 0.0;
        self.ic2eq = 0.0;
    }

    #[inline]
    pub(crate) fn tick(&mut self, x: f32) -> (f32, f32, f32) {
        let v3 = x - self.ic2eq;
        let bp = self.a1 * self.ic1eq + self.a2 * v3;
        let lp = self.ic2eq + self.a2 * self.ic1eq + self.a3 * v3;
        let ic1 = 2.0 * bp - self.ic1eq;
        let ic2 = 2.0 * lp - self.ic2eq;
        self.ic1eq = sanitize(ic1);
        self.ic2eq = sanitize(ic2);
        let hp = x - self.k * bp - lp;
        (lp, hp, bp)
    }
}

#[inline]
fn sanitize(v: f32) -> f32 {
    if v.is_finite() { v } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{dominant_bin, freq_to_bin, magnitude_spectrum};
    use patches_dsp::xorshift64;

    const SR: f32 = 44100.0;

    fn impulse_response(svf: &mut TptSvf, len: usize) -> Vec<f32> {
        let mut out = Vec::with_capacity(len);
        let (_, _, bp) = svf.tick(1.0);
        out.push(bp);
        for _ in 1..len {
            let (_, _, bp) = svf.tick(0.0);
            out.push(bp);
        }
        out
    }

    fn rms(samples: &[f32]) -> f32 {
        let s: f32 = samples.iter().map(|x| x * x).sum();
        (s / samples.len() as f32).sqrt()
    }

    #[test]
    fn linear_ring_matches_tune() {
        let mut svf = TptSvf::new(SR);
        svf.set_f_q(200.0, 50.0);
        let buf = impulse_response(&mut svf, 4200);
        let tail = &buf[100..4196];
        let spec = magnitude_spectrum(tail, 4096);
        let peak = dominant_bin(&spec);
        let expected = freq_to_bin(200.0, SR, 4096);
        assert!(peak.abs_diff(expected) <= 2, "peak {peak}, expected {expected}");
    }

    #[test]
    fn decay_couples_to_q() {
        let mut lo = TptSvf::new(SR);
        lo.set_f_q(200.0, 20.0);
        let mut hi = TptSvf::new(SR);
        hi.set_f_q(200.0, 60.0);
        let lo_buf = impulse_response(&mut lo, 4100);
        let hi_buf = impulse_response(&mut hi, 4100);
        let lo_rms = rms(&lo_buf[3900..4100]);
        let hi_rms = rms(&hi_buf[3900..4100]);
        assert!(hi_rms > lo_rms, "lo={lo_rms}, hi={hi_rms}");
    }

    #[test]
    fn high_q_stable_under_audio_rate_modulation() {
        let mut svf = TptSvf::new(SR);
        let mut rng: u64 = 0xDEAD_BEEF_CAFE_F00D;
        let len = SR as usize;
        let mut peak = 0.0f32;
        for n in 0..len {
            let r = xorshift64(&mut rng).abs();
            let fc = 100.0 + 4900.0 * r;
            svf.set_f_q(fc, 60.0);
            let x = if n == 0 { 1.0 } else { 0.0 };
            let (_, _, bp) = svf.tick(x);
            assert!(bp.is_finite(), "n={n}: bp = {bp}");
            peak = peak.max(bp.abs());
        }
        assert!(peak < 1.0e3, "audio-rate FM should not blow up, peak={peak}");
    }

    #[test]
    fn idle_zero_input_stays_silent() {
        let mut svf = TptSvf::new(SR);
        svf.set_f_q(80.0, 70.0);
        let mut peak = 0.0f32;
        for _ in 0..(SR as usize * 10) {
            let (_, _, bp) = svf.tick(0.0);
            peak = peak.max(bp.abs());
        }
        assert!(peak == 0.0, "idle silence broken, peak={peak}");
    }
}
