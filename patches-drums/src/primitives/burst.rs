/// Generates a sequence of short noise bursts with configurable spacing.
///
/// Used for clap synthesis. On trigger, produces `burst_count` short bursts
/// separated by `burst_spacing_samples`, with each burst slightly quieter
/// than the previous.
pub struct BurstGenerator {
    sample_rate: f32,
    burst_count: usize,
    burst_spacing: usize,
    burst_decay: f32,
    // Runtime state
    active: bool,
    current_burst: usize,
    sample_counter: usize,
    burst_level: f32,
}

impl BurstGenerator {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            sample_rate,
            burst_count: 4,
            burst_spacing: (0.005 * sample_rate) as usize,
            burst_decay: 0.7,
            active: false,
            current_burst: 0,
            sample_counter: 0,
            burst_level: 0.0,
        }
    }

    /// Configure burst parameters.
    /// - `burst_count`: number of bursts (1..=8)
    /// - `burst_spacing_secs`: time in seconds between burst onsets
    /// - `burst_decay`: amplitude multiplier per burst (e.g. 0.7)
    pub fn set_params(&mut self, burst_count: usize, burst_spacing_secs: f32, burst_decay: f32) {
        self.burst_count = burst_count.clamp(1, 8);
        self.burst_spacing = ((burst_spacing_secs * self.sample_rate) as usize).max(1);
        self.burst_decay = burst_decay.clamp(0.0, 1.0);
    }

    /// Reset state.
    pub fn reset(&mut self) {
        self.active = false;
        self.current_burst = 0;
        self.sample_counter = 0;
        self.burst_level = 0.0;
    }

    /// Process one sample. Takes a noise input and returns the gated/enveloped
    /// output. Returns 0.0 when not in a burst.
    /// `triggered` should be `true` on the sample where a rising edge was
    /// detected (e.g. from `TriggerInput::tick()`).
    pub fn tick(&mut self, triggered: bool, noise_sample: f32) -> f32 {
        if triggered {
            self.active = true;
            self.current_burst = 0;
            self.sample_counter = 0;
            self.burst_level = 1.0;
        }

        if !self.active {
            return 0.0;
        }

        // Each burst lasts burst_spacing samples. Within a burst, the first
        // half is "on" and the second half is "off" (silence between bursts).
        let burst_on_samples = self.burst_spacing / 2;
        let in_burst = self.sample_counter < burst_on_samples;

        let output = if in_burst {
            noise_sample * self.burst_level
        } else {
            0.0
        };

        self.sample_counter += 1;
        if self.sample_counter >= self.burst_spacing {
            self.sample_counter = 0;
            self.current_burst += 1;
            self.burst_level *= self.burst_decay;
            if self.current_burst >= self.burst_count {
                self.active = false;
            }
        }

        output
    }

    /// Returns true if the burst sequence is currently active.
    pub fn is_active(&self) -> bool {
        self.active
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::assert_within;

    const SR: f32 = 44100.0;

    #[test]
    fn burst_generator_produces_correct_burst_count() {
        // 100 samples at SR = 100/44100 secs
        let spacing_secs = 100.0 / SR;
        let mut bg = BurstGenerator::new(SR);
        bg.set_params(3, spacing_secs, 0.8);

        let total_expected_samples = 3 * 100;
        bg.tick(true, 1.0);

        let mut active_count = 1; // started active
        for i in 0..1000 {
            bg.tick(false, 1.0);
            if !bg.is_active() {
                active_count = i + 1;
                break;
            }
        }
        // Should be active for exactly burst_count * burst_spacing - 1 samples
        // (3 * 100 = 300, minus 1 for the trigger sample)
        assert!(
            active_count <= total_expected_samples,
            "burst sequence ran too long: {active_count} > {total_expected_samples}"
        );
    }

    #[test]
    fn burst_generator_spacing_creates_gaps() {
        let spacing_samples = 200_usize;
        let spacing_secs = spacing_samples as f32 / SR;
        let mut bg = BurstGenerator::new(SR);
        bg.set_params(2, spacing_secs, 1.0);

        // Trigger
        let v = bg.tick(true, 1.0);
        assert!(v.abs() > 0.0, "first sample should be non-zero");

        // Collect output for 2 bursts worth
        let mut output = Vec::new();
        output.push(v);
        for _ in 1..(spacing_samples * 2) {
            output.push(bg.tick(false, 1.0));
        }

        // Second half of first burst should be silent
        let silent_start = spacing_samples / 2;
        let silent_end = spacing_samples;
        for (i, &v) in output.iter().enumerate().take(silent_end).skip(silent_start) {
            assert_within!(
                0.0, v, 1e-6,
                "gap between bursts should be silent at sample {i}"
            );
        }
    }

    #[test]
    fn burst_generator_inactive_before_trigger() {
        let mut bg = BurstGenerator::new(SR);
        bg.set_params(4, 100.0 / SR, 0.7);

        let v = bg.tick(false, 1.0);
        assert_within!(0.0, v, 1e-6, "should be silent before trigger");
        assert!(!bg.is_active());
    }
}
