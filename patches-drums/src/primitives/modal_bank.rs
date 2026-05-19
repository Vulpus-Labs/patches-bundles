//! Parallel bank of [`BridgedT`] resonators with per-partial Q for the modal
//! metal voices (Cymbal2, ClosedHiHat2, OpenHiHat2) introduced in
//! [E003](../../../epics/E003-modal-metal-voices.md).
//!
//! Six `BridgedT` instances ring in parallel at inharmonic ratios of a base
//! frequency, each with its own Q and per-partial output gain. A single
//! excitation sample is fanned out to every resonator, and the gain-weighted
//! sum of bandpass taps is the bank output (divided by 6 to keep typical
//! amplitudes near `±1` when partial gains sum to ≈ 1, mirroring
//! [`MetallicTone`](super::MetallicTone)).
//!
//! Per-partial Q is the point of the primitive: in a struck metal high
//! partials radiate energy faster than low ones, so the spectral envelope
//! darkens through the tail. A single outer envelope cannot reproduce that —
//! each partial needs its own decay rate. See [ADR 0002 §"Linear core" and
//! §"Excitation shape"](../../../adrs/0002-bridged-t-resonator-family.md).

use patches_dsp::fast_sine;

use super::BridgedT;

/// Classic 808 metallic ratios — shared with [`MetallicTone`](super::MetallicTone).
const DEFAULT_RATIOS: [f32; 6] = [1.0, 1.4471, 1.6170, 1.9265, 2.5028, 2.6637];

/// Decreasing-Q default profile: low partials ring longer than high ones.
const DEFAULT_Q_PROFILE: [f32; 6] = [60.0, 50.0, 45.0, 40.0, 35.0, 30.0];

pub struct ModalBank {
    resonators: [BridgedT; 6],
    ratios: [f32; 6],
    gains: [f32; 6],
    base_freq: f32,
}

impl ModalBank {
    pub fn new(
        sample_rate: f32,
        base_hz: f32,
        ratios: [f32; 6],
        q_profile: [f32; 6],
        gains: [f32; 6],
    ) -> Self {
        let base = base_hz.max(1.0);
        let resonators = [
            BridgedT::new(sample_rate, base * ratios[0], q_profile[0]),
            BridgedT::new(sample_rate, base * ratios[1], q_profile[1]),
            BridgedT::new(sample_rate, base * ratios[2], q_profile[2]),
            BridgedT::new(sample_rate, base * ratios[3], q_profile[3]),
            BridgedT::new(sample_rate, base * ratios[4], q_profile[4]),
            BridgedT::new(sample_rate, base * ratios[5], q_profile[5]),
        ];
        Self {
            resonators,
            ratios,
            gains,
            base_freq: base,
        }
    }

    pub fn with_default_metal_profile(sample_rate: f32, base_hz: f32) -> Self {
        Self::new(sample_rate, base_hz, DEFAULT_RATIOS, DEFAULT_Q_PROFILE, [1.0; 6])
    }

    /// Classic 808 inharmonic ratios used by [`MetallicTone`](super::MetallicTone).
    pub const fn default_ratios() -> [f32; 6] {
        DEFAULT_RATIOS
    }

    /// Decreasing-Q profile used by [`Self::with_default_metal_profile`].
    pub const fn default_q_profile() -> [f32; 6] {
        DEFAULT_Q_PROFILE
    }

    pub fn set_base_freq(&mut self, hz: f32) {
        self.base_freq = hz.max(1.0);
        for (r, &ratio) in self.resonators.iter_mut().zip(&self.ratios) {
            r.set_tune(self.base_freq * ratio);
        }
    }

    pub fn set_ratios(&mut self, ratios: [f32; 6]) {
        self.ratios = ratios;
        for (r, &ratio) in self.resonators.iter_mut().zip(&self.ratios) {
            r.set_tune(self.base_freq * ratio);
        }
    }

    pub fn set_q_profile(&mut self, qs: [f32; 6]) {
        for (r, &q) in self.resonators.iter_mut().zip(&qs) {
            r.set_q(q);
        }
    }

    pub fn set_gains(&mut self, gains: [f32; 6]) {
        self.gains = gains;
    }

    pub fn set_clip(&mut self, clip: f32) {
        for r in self.resonators.iter_mut() {
            r.set_clip(clip);
        }
    }

    pub fn reset_state(&mut self) {
        for r in self.resonators.iter_mut() {
            r.reset_state();
        }
    }

    #[inline]
    pub fn tick(&mut self, excitation: f32) -> f32 {
        let mut sum = 0.0f32;
        for (r, &g) in self.resonators.iter_mut().zip(&self.gains) {
            sum += r.tick(excitation, 0.0) * g;
        }
        sum / 6.0
    }

    /// Per-partial frequency-modulated tick. `mod_depth_hz` is a peak
    /// frequency excursion in Hz applied to the fundamental; partial `i`
    /// receives `fast_sine(mod_phase) * mod_depth_hz * ratios[i]`. Contract
    /// matches [`MetallicTone::tick_with_modulation`](super::MetallicTone::tick_with_modulation)
    /// so cymbal shimmer routes through unchanged.
    #[inline]
    pub fn tick_with_modulation(
        &mut self,
        excitation: f32,
        mod_depth_hz: f32,
        mod_phase: f32,
    ) -> f32 {
        let mod_norm = fast_sine(mod_phase) * mod_depth_hz / self.base_freq;
        let mut sum = 0.0f32;
        for (i, (r, &g)) in self.resonators.iter_mut().zip(&self.gains).enumerate() {
            let fm_offset = mod_norm * self.ratios[i];
            sum += r.tick(excitation, fm_offset) * g;
        }
        sum / 6.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{band_energy, dominant_bin, freq_to_bin, magnitude_spectrum};

    const SR: f32 = 44100.0;

    fn strike(bank: &mut ModalBank, len: usize) -> Vec<f32> {
        let mut buf = Vec::with_capacity(len);
        buf.push(bank.tick(1.0));
        for _ in 1..len {
            buf.push(bank.tick(0.0));
        }
        buf
    }

    fn hf_ratio(samples: &[f32]) -> f32 {
        let spec = magnitude_spectrum(samples, samples.len());
        let total = band_energy(&spec, SR, samples.len(), 20.0, 20_000.0);
        let hi = band_energy(&spec, SR, samples.len(), 4_000.0, 10_000.0);
        hi / total.max(1e-12)
    }

    #[test]
    fn tail_darkens_with_default_profile() {
        let mut bank = ModalBank::with_default_metal_profile(SR, 400.0);
        let buf = strike(&mut bank, 8192);
        let early_ratio = hf_ratio(&buf[0..2048]);
        let late_ratio = hf_ratio(&buf[4096..8192]);
        assert!(
            late_ratio < early_ratio,
            "tail should darken: early={early_ratio}, late={late_ratio}"
        );
    }

    #[test]
    fn pitch_tracks_base_freq() {
        let mut bank = ModalBank::with_default_metal_profile(SR, 200.0);
        bank.set_base_freq(800.0);
        let buf = strike(&mut bank, 4096);
        let spec = magnitude_spectrum(&buf, 4096);
        let peak = dominant_bin(&spec);
        let expected = freq_to_bin(800.0, SR, 4096);
        assert!(
            peak.abs_diff(expected) <= 3,
            "peak {peak}, expected {expected}"
        );
    }

    #[test]
    fn per_partial_q_shapes_tail() {
        // Base 2 kHz puts the six partials across ~2–5.3 kHz, straddling the
        // LF/HF bands below so the per-partial Q profile is observable in
        // the tail spectrum. Compare late-window HF-share between a
        // decreasing-Q bank and a uniform-Q bank: decreasing Q must darken
        // the tail (lower HF share) relative to uniform Q.
        let base = 2_000.0;
        let mut decreasing = ModalBank::with_default_metal_profile(SR, base);
        let mut flat = ModalBank::new(SR, base, DEFAULT_RATIOS, [40.0; 6], [1.0; 6]);
        let dec_buf = strike(&mut decreasing, 8192);
        let flat_buf = strike(&mut flat, 8192);

        let hf_share = |buf: &[f32]| -> f32 {
            let spec = magnitude_spectrum(buf, buf.len());
            let lo = band_energy(&spec, SR, buf.len(), 1_500.0, 3_500.0);
            let hi = band_energy(&spec, SR, buf.len(), 3_500.0, 7_000.0);
            hi / (lo + hi).max(1e-12)
        };

        let dec_late = hf_share(&dec_buf[4096..8192]);
        let flat_late = hf_share(&flat_buf[4096..8192]);
        assert!(
            dec_late < flat_late,
            "decreasing-Q should fade HF faster than flat-Q: dec={dec_late}, flat={flat_late}"
        );
    }

    #[test]
    fn reset_silences() {
        let mut bank = ModalBank::with_default_metal_profile(SR, 400.0);
        for _ in 0..1000 {
            bank.tick(1.0);
        }
        bank.reset_state();
        for n in 0..SR as usize {
            let v = bank.tick(0.0);
            assert!(v == 0.0, "n={n}: {v}");
        }
    }

    #[test]
    fn modulation_engages() {
        let mut a = ModalBank::with_default_metal_profile(SR, 400.0);
        let mut b = ModalBank::with_default_metal_profile(SR, 400.0);
        let len = 4096usize;
        let mut buf_a = Vec::with_capacity(len);
        let mut buf_b = Vec::with_capacity(len);
        let lfo_inc = 3.0 / SR;
        let mut phase = 0.0f32;
        // First sample strikes; subsequent samples just sustain ringing.
        buf_a.push(a.tick(1.0));
        buf_b.push(b.tick_with_modulation(1.0, 20.0, phase));
        phase += lfo_inc;
        for _ in 1..len {
            buf_a.push(a.tick(0.0));
            buf_b.push(b.tick_with_modulation(0.0, 20.0, phase));
            phase += lfo_inc;
            if phase >= 1.0 {
                phase -= 1.0;
            }
        }
        let diff: f32 =
            buf_a.iter().zip(&buf_b).map(|(x, y)| (x - y).abs()).sum::<f32>() / len as f32;
        assert!(diff > 1e-5, "modulation should change output: avg diff {diff}");
    }
}
