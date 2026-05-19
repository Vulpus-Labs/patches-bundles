/// Single-stage exponential decay envelope.
///
/// Simpler than `AdsrCore` for drum sounds that only need attack-decay behaviour.
/// When `triggered` is true the level resets to 1.0 and decays exponentially
/// toward zero. The caller is responsible for edge detection (via `TriggerInput`).
///
/// The exponential multiply asymptotes to zero but never reaches it. `tick`
/// snaps `level` to exactly `0.0` once it drops below
/// [`SILENCE_THRESHOLD`] (≈ −140 dBFS), so downstream voices can early-out
/// on [`Self::is_silent`] and skip their entire process body. Snap is
/// monotone-decreasing — existing decay tests are unaffected.
pub struct DecayEnvelope {
    level: f32,
    decay_coeff: f32,
    sample_rate: f32,
}

/// Level below which the envelope snaps to zero (−140 dBFS, inaudible).
const SILENCE_THRESHOLD: f32 = 1.0e-7;

impl DecayEnvelope {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            level: 0.0,
            decay_coeff: 1.0,
            sample_rate,
        }
    }

    /// Set the decay time in seconds. The envelope reaches ~-60 dB after this time.
    pub fn set_decay(&mut self, decay_secs: f32) {
        // exp(-6.9078 / (decay_secs * sr)) gives ~-60dB at decay_secs
        let samples = decay_secs * self.sample_rate;
        if samples > 0.0 {
            self.decay_coeff = (-6.907_755 / samples).exp();
        } else {
            self.decay_coeff = 0.0;
        }
    }

    /// Reset all state to idle.
    pub fn reset(&mut self) {
        self.level = 0.0;
    }

    /// Process one sample. Returns envelope level in [0, 1].
    /// `triggered` should be `true` on the sample where a rising edge was
    /// detected (e.g. from `TriggerInput::tick()`).
    pub fn tick(&mut self, triggered: bool) -> f32 {
        if triggered {
            self.level = 1.0;
        } else {
            self.level *= self.decay_coeff;
            if self.level < SILENCE_THRESHOLD {
                self.level = 0.0;
            }
        }

        self.level
    }

    /// `true` when [`Self::tick`] has snapped to exactly `0.0`. Voices use
    /// this to skip their per-sample work when the envelope has decayed
    /// past audibility.
    #[inline]
    pub fn is_silent(&self) -> bool {
        self.level == 0.0
    }

    /// Immediately silence the envelope (used for hi-hat choke).
    pub fn choke(&mut self) {
        self.level = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::assert_within;

    const SR: f32 = 44100.0;

    #[test]
    fn decay_envelope_trigger_resets_to_one() {
        let mut env = DecayEnvelope::new(SR);
        env.set_decay(0.1);

        // Before trigger, level is 0
        let v = env.tick(false);
        assert_within!(0.0, v, 1e-6);

        // Trigger
        let v = env.tick(true);
        assert_within!(1.0, v, 1e-6);
    }

    #[test]
    fn decay_envelope_decays_over_time() {
        let mut env = DecayEnvelope::new(SR);
        let decay_time = 0.1;
        env.set_decay(decay_time);

        // Trigger
        env.tick(true);

        // After decay_time seconds, should be near zero (~-60dB = ~0.001)
        let decay_samples = (decay_time * SR) as usize;
        for _ in 0..decay_samples {
            env.tick(false);
        }
        let v = env.tick(false);
        assert!(v < 0.01, "after decay time, level should be near zero, got {v}");
    }

    #[test]
    fn decay_envelope_retrigger() {
        let mut env = DecayEnvelope::new(SR);
        env.set_decay(0.05);

        // Trigger and let decay halfway
        env.tick(true);
        for _ in 0..1000 {
            env.tick(false);
        }
        let mid_level = env.tick(false);
        assert!(mid_level < 1.0 && mid_level > 0.0, "should be mid-decay: {mid_level}");

        // Retrigger should reset to 1.0
        let v = env.tick(true);
        assert_within!(1.0, v, 1e-6);
    }

    #[test]
    fn decay_envelope_snaps_to_zero_below_threshold() {
        let mut env = DecayEnvelope::new(SR);
        env.set_decay(0.05);
        env.tick(true);
        // Run far past the −140 dBFS threshold. Analytical level
        // reaches 1e-7 around 0.05 × log10(1e-7) / log10(0.001) ≈ 5×
        // decay-time = 0.25 s ≈ 11k samples; 30k is comfortably past.
        for _ in 0..30_000 {
            env.tick(false);
        }
        assert!(env.is_silent());
        // The snap is exact: subsequent ticks return exactly 0.0.
        for _ in 0..100 {
            assert_eq!(env.tick(false), 0.0);
        }
    }

    #[test]
    fn decay_envelope_monotonically_decreasing() {
        let mut env = DecayEnvelope::new(SR);
        env.set_decay(0.2);
        env.tick(true);

        let mut prev = 1.0f32;
        for _ in 0..5000 {
            let v = env.tick(false);
            assert!(v <= prev + 1e-7, "decay should be monotonically decreasing: {v} > {prev}");
            prev = v;
        }
    }
}
