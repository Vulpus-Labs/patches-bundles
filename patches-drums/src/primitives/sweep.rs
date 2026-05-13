/// Exponential pitch sweep from a start frequency to an end frequency.
///
/// Used for kick and tom body pitch envelopes. After `sweep_time_secs` the
/// output frequency settles at `end_hz`.
pub struct PitchSweep {
    start_hz: f32,
    current_hz: f32,
    end_hz: f32,
    sweep_coeff: f32,
    sample_rate: f32,
}

impl PitchSweep {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            start_hz: 55.0,
            current_hz: 55.0,
            end_hz: 55.0,
            sweep_coeff: 1.0,
            sample_rate,
        }
    }

    /// Configure the sweep. `start_hz` is the initial frequency on trigger,
    /// `end_hz` is the settling frequency, and `sweep_time_secs` is the time
    /// to reach ~99% of the way from start to end.
    ///
    /// This is configuration only — it does not reset `current_hz`.
    /// Call `trigger()` to start the sweep.
    pub fn set_params(&mut self, start_hz: f32, end_hz: f32, sweep_time_secs: f32) {
        self.start_hz = start_hz;
        self.end_hz = end_hz;
        let samples = sweep_time_secs * self.sample_rate;
        if samples > 0.0 && start_hz > end_hz {
            // Exponential decay of (current - end) toward 0
            self.sweep_coeff = (-4.605 / samples).exp(); // ~-40dB = ~1% remaining
        } else {
            self.sweep_coeff = 0.0;
        }
    }

    /// Reset state.
    pub fn reset(&mut self) {
        self.current_hz = self.end_hz;
    }

    /// Trigger the sweep (resets current frequency to start).
    pub fn trigger(&mut self) {
        self.current_hz = self.start_hz;
    }

    /// Tick the sweep and return current frequency in Hz.
    pub fn tick(&mut self) -> f32 {
        // Exponentially approach end_hz
        let diff = self.current_hz - self.end_hz;
        self.current_hz = self.end_hz + diff * self.sweep_coeff;
        self.current_hz
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::assert_within;

    const SR: f32 = 44100.0;

    #[test]
    fn pitch_sweep_starts_at_start_freq() {
        let mut sweep = PitchSweep::new(SR);
        sweep.set_params(2500.0, 55.0, 0.04);
        sweep.trigger();

        // On trigger, should return start freq
        let hz = sweep.tick();
        assert_within!(2500.0, hz, 50.0);
    }

    #[test]
    fn pitch_sweep_settles_at_end_freq() {
        let mut sweep = PitchSweep::new(SR);
        sweep.set_params(2500.0, 55.0, 0.04);
        sweep.trigger();
        sweep.tick();

        // After many samples, should settle near end freq
        for _ in 0..10000 {
            sweep.tick();
        }
        let hz = sweep.tick();
        assert!(
            (hz - 55.0).abs() < 1.0,
            "sweep should settle near 55 Hz, got {hz}"
        );
    }

    #[test]
    fn pitch_sweep_monotonically_decreasing() {
        let mut sweep = PitchSweep::new(SR);
        sweep.set_params(2500.0, 55.0, 0.04);
        sweep.trigger();
        let mut prev = sweep.tick();

        for _ in 0..5000 {
            let hz = sweep.tick();
            assert!(hz <= prev + 0.01, "sweep should decrease: {hz} > {prev}");
            prev = hz;
        }
    }
}
