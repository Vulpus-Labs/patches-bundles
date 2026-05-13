//! NE570-style 2:1 log/exp compander primitive.
//!
//! Two halves: [`Compressor`] (2:1 log encode) and [`Expander`] (1:2
//! exp decode). Topology per the NE570 datasheet: full-wave rectifier
//! → one-pole averaging filter with asymmetric attack/release → variable
//! gain cell. Runtime-settable [`CompanderParams`] so future consumers
//! can fit different chips (NE570, MN3102-internal, …).
//!
//! Not consumed by `VChorus` — neither Juno-60 nor Juno-106 compand
//! their chorus. First consumer will be a vintage-BBD-delay module
//! (CE-2 / Small-Clone topology: compressor → BBD → expander).
//!
//! All state is scalar; `process` performs no allocations.

#[cfg(test)]
mod tests;

/// Tunable compander parameters.
#[derive(Clone, Copy, Debug)]
pub struct CompanderParams {
    pub attack_s: f32,
    pub release_s: f32,
    /// Reference RMS level at which gain = 1.0. The NE570 spec is
    /// centred at 100 mV RMS across the internal rectifier; we expose
    /// it as a linear level against unit full-scale.
    pub ref_level: f32,
}

impl CompanderParams {
    /// NE570 data-sheet default: ~5 ms attack, ~100 ms release, reference
    /// level matched to the chip's nominal 100 mV RMS operating point
    /// (NE570 datasheet, Philips/NXP, figure "Compressor (typical)").
    pub const NE570_DEFAULT: Self = Self {
        attack_s: 0.005,
        release_s: 0.100,
        ref_level: 0.1,
    };
}

/// Shared rectifier-averager used by both halves.
#[derive(Clone, Copy, Debug)]
struct LevelFollower {
    level: f32,
    attack_a: f32,
    release_a: f32,
    ref_level: f32,
}

impl LevelFollower {
    fn new(params: CompanderParams, sample_rate: f32) -> Self {
        Self {
            level: 0.0,
            attack_a: one_pole_coef(params.attack_s, sample_rate),
            release_a: one_pole_coef(params.release_s, sample_rate),
            ref_level: params.ref_level.max(1.0e-6),
        }
    }

    #[inline]
    fn follow(&mut self, input: f32) -> f32 {
        let rect = input.abs();
        // Full-wave rectifier into an asymmetric one-pole averager —
        // fast on the way up, slow on the way down, per NE570 topology.
        let a = if rect > self.level {
            self.attack_a
        } else {
            self.release_a
        };
        self.level += a * (rect - self.level);
        self.level
    }

    fn reset(&mut self) {
        self.level = 0.0;
    }
}

/// 2:1 log-domain compressor.
pub struct Compressor {
    follower: LevelFollower,
}

impl Compressor {
    pub fn new(params: CompanderParams, sample_rate: f32) -> Self {
        Self { follower: LevelFollower::new(params, sample_rate) }
    }

    pub fn process(&mut self, input: f32) -> f32 {
        let level = self.follower.follow(input);
        // 2:1 compressor gain: g = sqrt(ref / level). Clamped against
        // silence to avoid a huge boost on cold start.
        let gain = (self.follower.ref_level
            / level.max(self.follower.ref_level * 1.0e-3))
            .sqrt();
        input * gain
    }

    pub fn reset(&mut self) {
        self.follower.reset();
    }
}

/// 1:2 log-domain expander — the mirror of [`Compressor`].
pub struct Expander {
    follower: LevelFollower,
}

impl Expander {
    pub fn new(params: CompanderParams, sample_rate: f32) -> Self {
        Self { follower: LevelFollower::new(params, sample_rate) }
    }

    pub fn process(&mut self, input: f32) -> f32 {
        let level = self.follower.follow(input);
        // 1:2 expander gain: g = sqrt(level / ref). Unity at ref_level,
        // matches the compressor so round-trip gain ≈ 1 at steady state.
        let gain = (level
            / self.follower.ref_level.max(1.0e-6))
            .sqrt();
        input * gain
    }

    pub fn reset(&mut self) {
        self.follower.reset();
    }
}

/// One-pole lowpass coefficient from a time constant (seconds).
#[inline]
fn one_pole_coef(tau_s: f32, sample_rate: f32) -> f32 {
    if tau_s <= 0.0 {
        return 1.0;
    }
    1.0 - (-1.0 / (tau_s * sample_rate)).exp()
}
