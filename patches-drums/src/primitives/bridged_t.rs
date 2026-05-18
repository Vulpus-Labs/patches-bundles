//! Bridged-T struck-resonator primitive.
//!
//! TPT-SVF wrapper with a soft-clipped bp output and per-sample
//! frequency-modulation input. Drives the high-Q ringing on impulse
//! excitation that the [`E002`](../../../adrs/0002-bridged-t-resonator-family.md)
//! family is built around. Exposes the SVF's `lp` tap from the previous
//! tick so the voice (Kick2/Tom2) can compute self-FM.
//!
//! Pitch droop is **not** implemented inside this primitive — the voice
//! tracks `lp` between ticks, runs it through Plaits's `Diode` shaper to
//! get the self-FM offset, optionally adds an attack-FM pulse, and passes
//! the resulting `fm_offset` to [`BridgedT::tick`]. The earlier draft of
//! this primitive tried to make pitch droop emerge from a feedback
//! saturator on a Chamberlin SVF; that does not work physically (the
//! complex-pole angle is set by `f` alone in the Chamberlin recurrence
//! and the saturator only changes effective Q + harmonic content). See
//! [ADR 0002 §"Nonlinearity and pitch droop"](../../../adrs/0002-bridged-t-resonator-family.md#nonlinearity-and-pitch-droop).
//!
//! The saturator sits on the **output**, not in the integrator loop. TPT
//! SVF's implicit feedback resolution does not tolerate an in-loop
//! nonlinearity without a Newton solve, and post-output saturation gives
//! the harmonic dirt we want without that complication.

use super::saturate;
use super::tpt_svf::TptSvf;

pub struct BridgedT {
    svf: TptSvf,
    sample_rate: f32,
    tune_base: f32,
    q: f32,
    clip: f32,
    lp_prev: f32,
}

impl BridgedT {
    /// `q` is the classical resonator Q.
    pub fn new(sample_rate: f32, tune_hz: f32, q: f32) -> Self {
        let mut svf = TptSvf::new(sample_rate);
        svf.set_f_q(tune_hz, q);
        Self {
            svf,
            sample_rate,
            tune_base: tune_hz,
            q,
            clip: 0.0,
            lp_prev: 0.0,
        }
    }

    pub fn set_tune(&mut self, tune_hz: f32) {
        self.tune_base = tune_hz;
    }

    pub fn set_q(&mut self, q: f32) {
        self.q = q;
    }

    pub fn set_clip(&mut self, clip: f32) {
        self.clip = clip.clamp(0.0, 1.0);
    }

    pub fn reset_state(&mut self) {
        self.svf.reset_state();
        self.lp_prev = 0.0;
    }

    /// Cached `lp` tap from the previous [`tick`] call. Used by the voice
    /// to compute self-FM. Returns `0.0` before the first tick or after a
    /// `reset_state`.
    #[inline]
    pub fn lp(&self) -> f32 {
        self.lp_prev
    }

    /// Run one sample. `fm_offset` is a normalised multiplier added to
    /// the base tune (`f = tune_base · (1 + fm_offset)`); the SVF
    /// coefficients are recomputed every tick. Clamped internally so
    /// `f` stays under `0.4 · sr` (matches Plaits's `CONSTRAIN`).
    /// Output is `saturate(bp, clip)` — saturator on the output tap,
    /// not in the feedback path.
    #[inline]
    pub fn tick(&mut self, x: f32, fm_offset: f32) -> f32 {
        let fc = (self.tune_base * (1.0 + fm_offset)).max(1.0);
        let fc = fc.min(0.4 * self.sample_rate);
        self.svf.set_f_q(fc, self.q);
        let (lp, _hp, bp) = self.svf.tick(x);
        self.lp_prev = lp;
        if self.clip > 0.0 {
            saturate(bp, self.clip)
        } else {
            bp
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{band_energy, dominant_bin, freq_to_bin, magnitude_spectrum};

    const SR: f32 = 44100.0;

    fn impulse_response(bt: &mut BridgedT, len: usize) -> Vec<f32> {
        let mut out = Vec::with_capacity(len);
        out.push(bt.tick(1.0, 0.0));
        for _ in 1..len {
            out.push(bt.tick(0.0, 0.0));
        }
        out
    }

    fn rms(samples: &[f32]) -> f32 {
        let s: f32 = samples.iter().map(|x| x * x).sum();
        (s / samples.len() as f32).sqrt()
    }

    #[test]
    fn linear_ring_matches_tune() {
        let mut bt = BridgedT::new(SR, 100.0, 50.0);
        let buf = impulse_response(&mut bt, 4200);
        let tail = &buf[100..4196];
        let spec = magnitude_spectrum(tail, 4096);
        let peak = dominant_bin(&spec);
        let expected = freq_to_bin(100.0, SR, 4096);
        assert!(peak.abs_diff(expected) <= 3, "peak {peak}, expected {expected}");
    }

    #[test]
    fn decay_couples_to_q() {
        let mut lo = BridgedT::new(SR, 200.0, 20.0);
        let mut hi = BridgedT::new(SR, 200.0, 60.0);
        let lo_buf = impulse_response(&mut lo, 4100);
        let hi_buf = impulse_response(&mut hi, 4100);
        let lo_rms = rms(&lo_buf[3900..4100]);
        let hi_rms = rms(&hi_buf[3900..4100]);
        assert!(hi_rms > lo_rms, "lo={lo_rms}, hi={hi_rms}");
    }

    #[test]
    fn fm_offset_shifts_frequency() {
        let mut bt = BridgedT::new(SR, 200.0, 50.0);
        let mut buf = Vec::with_capacity(4200);
        buf.push(bt.tick(1.0, 0.5));
        for _ in 1..4200 {
            buf.push(bt.tick(0.0, 0.5));
        }
        let spec = magnitude_spectrum(&buf[100..4196], 4096);
        let peak = dominant_bin(&spec);
        let expected = freq_to_bin(300.0, SR, 4096);
        assert!(peak.abs_diff(expected) <= 3, "peak {peak}, expected {expected}");
    }

    #[test]
    fn idle_zero_input_stays_silent() {
        let mut bt = BridgedT::new(SR, 80.0, 70.0);
        bt.set_clip(0.5);
        let mut peak = 0.0f32;
        for _ in 0..(SR as usize * 10) {
            let y = bt.tick(0.0, 0.0);
            peak = peak.max(y.abs());
        }
        assert!(peak == 0.0, "idle peak {peak}");
    }

    #[test]
    fn clip_zero_matches_bypass() {
        let mut a = BridgedT::new(SR, 250.0, 40.0);
        let mut b = BridgedT::new(SR, 250.0, 40.0);
        b.set_clip(0.0);
        for n in 0..2000 {
            let x = if n == 0 { 1.0 } else { 0.0 };
            let ya = a.tick(x, 0.0);
            let yb = b.tick(x, 0.0);
            assert!((ya - yb).abs() < 1e-7, "n={n}: {ya} vs {yb}");
        }
    }

    #[test]
    fn clip_engages_saturator_with_harmonics() {
        let tune = 200.0_f32;
        let drive = |clip: f32| -> (f32, f32, usize) {
            let mut bt = BridgedT::new(SR, tune, 50.0);
            bt.set_clip(clip);
            let mut buf = Vec::with_capacity(6000);
            buf.push(bt.tick(10.0, 0.0));
            for _ in 1..6000 {
                buf.push(bt.tick(0.0, 0.0));
            }
            let spec = magnitude_spectrum(&buf[1000..5096], 4096);
            let peak = dominant_bin(&spec);
            let fund = band_energy(&spec, SR, 4096, 150.0, 250.0);
            let third = band_energy(&spec, SR, 4096, 540.0, 660.0);
            (third, fund, peak)
        };
        let (third_clean, fund_clean, peak_clean) = drive(0.0);
        let (third_clip, fund_clip, peak_clip) = drive(0.9);
        let expected_bin = freq_to_bin(tune, SR, 4096);
        assert!(peak_clean.abs_diff(expected_bin) <= 3, "clean peak {peak_clean}");
        assert!(peak_clip.abs_diff(expected_bin) <= 3, "clipped peak {peak_clip}");
        let ratio_clean = third_clean / fund_clean;
        let ratio_clip = third_clip / fund_clip;
        assert!(
            ratio_clip > ratio_clean * 2.0,
            "clean ratio {ratio_clean}, clipped ratio {ratio_clip}"
        );
    }
}
