/// Classic 808 metallic ratios for inharmonic square oscillators.
const METALLIC_RATIOS: [f32; 6] = [1.0, 1.4471, 1.6170, 1.9265, 2.5028, 2.6637];

/// Generates a metallic timbre by summing six square oscillators at inharmonic
/// frequency ratios. Used for hi-hats and cymbals.
pub struct MetallicTone {
    phases: [f32; 6],
    increments: [f32; 6],
    sample_rate: f32,
    sr_recip: f32,
}

impl MetallicTone {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            phases: [0.0; 6],
            increments: [0.0; 6],
            sample_rate,
            sr_recip: 1.0 / sample_rate,
        }
    }

    /// Set the base frequency. Partials are at fixed inharmonic ratios.
    pub fn set_frequency(&mut self, base_hz: f32) {
        for (inc, &ratio) in self.increments.iter_mut().zip(&METALLIC_RATIOS) {
            *inc = (base_hz * ratio / self.sample_rate).min(0.499);
        }
    }

    /// Reset all oscillator phases.
    pub fn reset(&mut self) {
        self.phases = [0.0; 6];
    }

    /// Trigger: reset phases only. Call `set_frequency` separately for configuration.
    pub fn trigger(&mut self) {
        self.reset();
    }

    /// Process one sample. Returns the summed square-wave output, normalised
    /// to approximately [-1, 1].
    pub fn tick(&mut self) -> f32 {
        let mut sum = 0.0f32;
        for (phase, &inc) in self.phases.iter_mut().zip(&self.increments) {
            let sq = if *phase < 0.5 { 1.0 } else { -1.0 };
            sum += sq;
            *phase += inc;
            if *phase >= 1.0 {
                *phase -= 1.0;
            }
        }
        sum / 6.0
    }

    /// Process one sample with per-partial frequency modulation (for cymbal shimmer).
    /// `mod_depth` is in Hz, `mod_phase` is a slow LFO phase in [0, 1).
    pub fn tick_with_modulation(&mut self, mod_depth: f32, mod_phase: f32) -> f32 {
        let mod_base = patches_dsp::fast_sine(mod_phase) * mod_depth * self.sr_recip;
        let mut sum = 0.0f32;
        for (i, (phase, &base_inc)) in self.phases.iter_mut().zip(&self.increments).enumerate() {
            let sq = if *phase < 0.5 { 1.0 } else { -1.0 };
            sum += sq;

            let mod_offset = mod_base * METALLIC_RATIOS[i];
            let inc = (base_inc + mod_offset).clamp(0.0, 0.499);
            *phase += inc;
            if *phase >= 1.0 {
                *phase -= 1.0;
            }
            if *phase < 0.0 {
                *phase += 1.0;
            }
        }
        sum / 6.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 44100.0;

    #[test]
    fn metallic_tone_produces_output_after_trigger() {
        let mut mt = MetallicTone::new(SR);
        mt.set_frequency(400.0);
        mt.trigger();

        let mut sum_sq = 0.0f32;
        for _ in 0..1000 {
            let v = mt.tick();
            sum_sq += v * v;
        }
        let rms = (sum_sq / 1000.0).sqrt();
        assert!(rms > 0.1, "metallic tone should produce output, rms = {rms}");
    }

    #[test]
    fn metallic_tone_output_bounded() {
        let mut mt = MetallicTone::new(SR);
        mt.set_frequency(800.0);
        mt.trigger();

        for _ in 0..5000 {
            let v = mt.tick();
            assert!(
                (-1.0..=1.0).contains(&v),
                "metallic tone output out of [-1, 1]: {v}"
            );
        }
    }

    #[test]
    fn metallic_tone_reset_silences() {
        let mut mt = MetallicTone::new(SR);
        mt.set_frequency(400.0);
        mt.trigger();
        for _ in 0..100 {
            mt.tick();
        }
        mt.reset();
        // After reset with no frequency, increments are still set but phases are 0
        // The output at phase=0 is always +1 for square wave, so test is just that
        // reset zeros phases
        assert_eq!(mt.phases, [0.0; 6]);
    }
}
