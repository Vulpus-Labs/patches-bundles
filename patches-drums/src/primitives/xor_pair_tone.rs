/// XOR-pair inharmonic ratios. Same six values as
/// [`super::metallic::METALLIC_RATIOS`](super::metallic) — paired as
/// `[0,1]`, `[2,3]`, `[4,5]` so the three intermod products span three
/// distinct difference-frequency regions.
const XOR_PAIR_RATIOS: [f32; 6] = [1.0, 1.4471, 1.6170, 1.9265, 2.5028, 2.6637];

/// Inharmonic generator: six square oscillators arranged as three pairs;
/// each pair's outputs are multiplied (bipolar XOR = signed product),
/// the three products are summed and normalised. Denser, coarser
/// spectrum than [`super::MetallicTone`] because each pair carries
/// intermod energy at the sum and difference of its two inputs'
/// frequencies that neither input had alone.
pub struct XorPairTone {
    phases: [f32; 6],
    increments: [f32; 6],
    sample_rate: f32,
    sr_recip: f32,
}

impl XorPairTone {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            phases: [0.0; 6],
            increments: [0.0; 6],
            sample_rate,
            sr_recip: 1.0 / sample_rate,
        }
    }

    /// Set the base frequency. Per-oscillator increments are
    /// `base_hz * XOR_PAIR_RATIOS[i] / sr`, clamped to ≤ 0.499.
    pub fn set_frequency(&mut self, base_hz: f32) {
        for (inc, &ratio) in self.increments.iter_mut().zip(&XOR_PAIR_RATIOS) {
            *inc = (base_hz * ratio / self.sample_rate).min(0.499);
        }
    }

    /// Reset all oscillator phases.
    pub fn reset(&mut self) {
        self.phases = [0.0; 6];
    }

    /// Trigger: reset phases only.
    pub fn trigger(&mut self) {
        self.reset();
    }

    /// Process one sample. Three pair products summed and normalised by
    /// `/3.0` — same approximate output amplitude as `MetallicTone`'s
    /// six-square `/6.0` average so module shells can swap generators
    /// without re-balancing.
    pub fn tick(&mut self) -> f32 {
        let mut sq = [0.0f32; 6];
        for (i, (phase, &inc)) in self.phases.iter_mut().zip(&self.increments).enumerate() {
            sq[i] = if *phase < 0.5 { 1.0 } else { -1.0 };
            *phase += inc;
            if *phase >= 1.0 {
                *phase -= 1.0;
            }
        }
        let p_a = sq[0] * sq[1];
        let p_b = sq[2] * sq[3];
        let p_c = sq[4] * sq[5];
        (p_a + p_b + p_c) / 3.0
    }

    /// Process one sample with per-partial frequency modulation.
    /// `mod_depth` is in Hz, `mod_phase` is a slow LFO phase in [0, 1).
    /// When `mod_depth` is exactly zero this collapses to [`Self::tick`].
    pub fn tick_with_modulation(&mut self, mod_depth: f32, mod_phase: f32) -> f32 {
        if mod_depth == 0.0 {
            return self.tick();
        }
        let mod_base = patches_dsp::fast_sine(mod_phase) * mod_depth * self.sr_recip;
        let mut sq = [0.0f32; 6];
        for (i, (phase, &base_inc)) in self.phases.iter_mut().zip(&self.increments).enumerate() {
            sq[i] = if *phase < 0.5 { 1.0 } else { -1.0 };

            let mod_offset = mod_base * XOR_PAIR_RATIOS[i];
            // `inc` is non-negative after clamp, so phase wraps only at the
            // upper bound — no `*phase < 0.0` rebase needed.
            let inc = (base_inc + mod_offset).clamp(0.0, 0.499);
            *phase += inc;
            if *phase >= 1.0 {
                *phase -= 1.0;
            }
        }
        let p_a = sq[0] * sq[1];
        let p_b = sq[2] * sq[3];
        let p_c = sq[4] * sq[5];
        (p_a + p_b + p_c) / 3.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::MetallicTone;
    use crate::test_support::{freq_to_bin, magnitude_spectrum};

    const SR: f32 = 44100.0;

    #[test]
    fn xor_pair_tone_produces_output_after_trigger() {
        let mut xt = XorPairTone::new(SR);
        xt.set_frequency(400.0);
        xt.trigger();

        let mut sum_sq = 0.0f32;
        for _ in 0..1000 {
            let v = xt.tick();
            sum_sq += v * v;
        }
        let rms = (sum_sq / 1000.0).sqrt();
        assert!(rms > 0.1, "xor pair tone should produce output, rms = {rms}");
    }

    #[test]
    fn xor_pair_tone_output_bounded() {
        let mut xt = XorPairTone::new(SR);
        xt.set_frequency(800.0);
        xt.trigger();

        for _ in 0..5000 {
            let v = xt.tick();
            assert!(
                (-1.0..=1.0).contains(&v),
                "xor pair tone output out of [-1, 1]: {v}"
            );
        }
    }

    #[test]
    fn xor_pair_tone_differs_from_metallic_tone() {
        let mut xt = XorPairTone::new(SR);
        xt.set_frequency(600.0);
        xt.trigger();
        let mut mt = MetallicTone::new(SR);
        mt.set_frequency(600.0);
        mt.trigger();

        let mut diff_sum = 0.0f32;
        for _ in 0..4096 {
            diff_sum += (xt.tick() - mt.tick()).abs();
        }
        let avg_diff = diff_sum / 4096.0;
        assert!(
            avg_diff > 0.05,
            "xor pair tone should differ from metallic tone, avg diff = {avg_diff}"
        );
    }

    #[test]
    fn xor_pair_tone_intermod_product_present() {
        // Pair A = [1.0, 1.4471] at base 500 Hz → difference frequency
        // 500 * 0.4471 ≈ 223.55 Hz. XOR product carries energy near
        // this bin; MetallicTone's direct sum does not.
        let fft_size = 4096;
        let base = 500.0f32;
        let diff_hz = base * (XOR_PAIR_RATIOS[1] - XOR_PAIR_RATIOS[0]);

        let mut xt = XorPairTone::new(SR);
        xt.set_frequency(base);
        xt.trigger();
        let xor_samples: Vec<f32> = (0..fft_size).map(|_| xt.tick()).collect();

        let mut mt = MetallicTone::new(SR);
        mt.set_frequency(base);
        mt.trigger();
        let metal_samples: Vec<f32> = (0..fft_size).map(|_| mt.tick()).collect();

        let xor_spec = magnitude_spectrum(&xor_samples, fft_size);
        let metal_spec = magnitude_spectrum(&metal_samples, fft_size);

        let center = freq_to_bin(diff_hz, SR, fft_size);
        let lo = center.saturating_sub(2);
        let hi = (center + 3).min(xor_spec.len());

        let xor_band: f32 = xor_spec[lo..hi].iter().map(|m| m * m).sum();
        let metal_band: f32 = metal_spec[lo..hi].iter().map(|m| m * m).sum();

        assert!(
            xor_band > 4.0 * metal_band,
            "xor pair tone should carry difference-frequency energy near {diff_hz} Hz absent from metallic tone: xor={xor_band}, metallic={metal_band}"
        );
        assert!(
            xor_band > 0.01,
            "xor difference-frequency band too quiet to count: {xor_band}"
        );
    }

    #[test]
    fn tick_with_modulation_zero_depth_matches_tick() {
        let mut a = XorPairTone::new(SR);
        a.set_frequency(500.0);
        a.trigger();
        let mut b = XorPairTone::new(SR);
        b.set_frequency(500.0);
        b.trigger();
        for n in 0..256 {
            let ya = a.tick();
            let yb = b.tick_with_modulation(0.0, n as f32 * 0.01);
            assert_eq!(ya, yb, "n={n}");
        }
    }

    #[test]
    fn xor_pair_tone_reset_zeros_phases() {
        let mut xt = XorPairTone::new(SR);
        xt.set_frequency(400.0);
        xt.trigger();
        for _ in 0..100 {
            xt.tick();
        }
        xt.reset();
        assert_eq!(xt.phases, [0.0; 6]);
    }
}
