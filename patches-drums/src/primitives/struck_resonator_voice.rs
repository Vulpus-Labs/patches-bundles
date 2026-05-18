//! Shared process-loop spine for struck-resonator voices (Kick2, Tom2).
//!
//! Owns the [`BridgedT`] resonator, the [`Excitation`] pulse generator,
//! and the attack-FM pulse used to lift pitch at strike. Per-sample tick
//! computes self-FM from the resonator's previous-tick `lp` tap (Plaits
//! `Diode` shaper), adds attack-FM, drives `BridgedT::tick` with the
//! resulting offset, and returns the velocity-scaled bp output.
//!
//! Modules wrap this voice and add their parameter descriptors, port
//! wiring, and any voice-specific behaviour (Kick2 adds v/oct).
//!
//! `Claves2` deliberately does not use this spine — its two-stage cascade
//! does not match the single-resonator shape and the asymmetry is the
//! design point.

use super::{BridgedT, Excitation, PulseShape};

/// Plaits `AnalogBassDrum` reference depths.
const SELF_FM_REF: f32 = 0.08;
const ATTACK_FM_REF: f32 = 1.7;
const FM_PULSE_SECS: f32 = 6.0e-3;
const FM_PULSE_FILTER_SECS: f32 = 0.1e-3;

pub struct StruckResonatorVoice {
    resonator: BridgedT,
    excitation: Excitation,
    attack_fm: AttackFmPulse,
    drive: f32,
    attack: f32,
    latched_velocity: f32,
}

impl StruckResonatorVoice {
    pub fn new(
        sample_rate: f32,
        tune_hz: f32,
        q: f32,
        pulse_shape: PulseShape,
        pulse_ms: f32,
        instance_seed: u64,
    ) -> Self {
        let resonator = BridgedT::new(sample_rate, tune_hz, q);
        let mut excitation = Excitation::new(sample_rate, instance_seed);
        excitation.set_shape(pulse_shape);
        excitation.set_pulse_ms(pulse_ms);
        Self {
            resonator,
            excitation,
            attack_fm: AttackFmPulse::new(sample_rate),
            drive: 0.0,
            attack: 0.0,
            latched_velocity: 1.0,
        }
    }

    pub fn set_tune(&mut self, hz: f32) {
        self.resonator.set_tune(hz);
    }

    pub fn set_q(&mut self, q: f32) {
        self.resonator.set_q(q);
    }

    pub fn set_pulse_ms(&mut self, ms: f32) {
        self.excitation.set_pulse_ms(ms);
    }

    /// Sets the self-FM depth **and** the resonator output-saturator
    /// amount. Per ADR 0002, the saturator tracks `drive` so the
    /// amplitude-coupled droop and the harmonic dirt move together.
    pub fn set_drive(&mut self, drive: f32) {
        self.drive = drive;
        self.resonator.set_clip(drive);
    }

    pub fn set_attack(&mut self, attack: f32) {
        self.attack = attack;
    }

    pub fn trigger(&mut self, velocity: f32) {
        self.latched_velocity = velocity;
        self.resonator.reset_state();
        self.excitation.trigger();
        self.attack_fm.trigger();
    }

    #[inline]
    pub fn tick(&mut self) -> f32 {
        let punch = 0.7 + diode(10.0 * self.resonator.lp() - 1.0);
        let self_fm = self.drive * SELF_FM_REF * punch;
        let attack_fm = self.attack * ATTACK_FM_REF * self.attack_fm.tick();
        let fm_offset = self_fm + attack_fm;
        let pulse = self.excitation.tick();
        let ring = self.resonator.tick(pulse, fm_offset);
        ring * self.latched_velocity
    }
}

/// 6 ms rectangular pulse through a one-pole LP (~10 kHz, 0.1 ms tc).
/// Per Plaits's `fm_pulse` / `fm_pulse_lp_` in `AnalogBassDrum`.
struct AttackFmPulse {
    remaining: u32,
    duration: u32,
    lp: f32,
    lp_alpha: f32,
}

impl AttackFmPulse {
    fn new(sample_rate: f32) -> Self {
        let duration = (FM_PULSE_SECS * sample_rate) as u32;
        let filter_samples = (FM_PULSE_FILTER_SECS * sample_rate).max(1.0);
        Self {
            remaining: 0,
            duration,
            lp: 0.0,
            lp_alpha: 1.0 / filter_samples,
        }
    }

    fn trigger(&mut self) {
        self.remaining = self.duration;
        self.lp = 0.0;
    }

    #[inline]
    fn tick(&mut self) -> f32 {
        let raw = if self.remaining > 0 {
            self.remaining -= 1;
            1.0
        } else {
            0.0
        };
        self.lp += self.lp_alpha * (raw - self.lp);
        self.lp
    }
}

/// Plaits's `Diode` half-wave shaper. Pass-through for positive input,
/// soft-clipped scaled-down version for negative input.
#[inline]
fn diode(x: f32) -> f32 {
    if x >= 0.0 {
        x
    } else {
        let scaled = 2.0 * x;
        0.7 * scaled / (1.0 + scaled.abs())
    }
}
