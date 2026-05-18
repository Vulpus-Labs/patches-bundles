//! Excitation pulse generator for the struck-resonator family (ADR 0002).
//!
//! Produces a short shaped pulse (1–5 ms typical) that strikes a downstream
//! [`super::BridgedT`]. A Dirac would ring at the right frequency but sound
//! thin — the analog circuits deliver a short asymmetric pulse and the shape
//! carries much of the voice's weight, thump, click, or snap.
//!
//! ## Dispatch
//!
//! Per-sample branching on the shape enum is avoided by dispatching once in
//! [`Excitation::tick`] via a function pointer cached at shape-change time.
//! The hot per-sample path is therefore branch-free over `shape`.
//!
//! ## Energy
//!
//! The four shapes have different total energy per pulse for the same
//! `pulse_ms`. Consumers wanting comparable loudness across a shape change
//! should add a per-shape gain trim; this primitive does not normalise.

use std::f32::consts::PI;

use patches_dsp::xorshift64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PulseShape {
    Dirac,
    ExpDecay,
    HalfSine,
    FilteredClick,
}

pub struct Excitation {
    sample_rate: f32,
    shape: PulseShape,
    pulse_ms: f32,
    lp_hz: f32,
    length_samples: u32,
    counter: u32,
    decay_coeff: f32,
    half_sine_step: f32,
    lp_alpha: f32,
    lp_state: f32,
    prng: u64,
    seed: u64,
    tick_fn: fn(&mut Excitation) -> f32,
}

impl Excitation {
    pub fn new(sample_rate: f32, instance_seed: u64) -> Self {
        let seed = instance_seed.wrapping_add(1);
        let mut s = Self {
            sample_rate,
            shape: PulseShape::Dirac,
            pulse_ms: 2.0,
            lp_hz: 4000.0,
            length_samples: 0,
            counter: u32::MAX,
            decay_coeff: 0.0,
            half_sine_step: 0.0,
            lp_alpha: 0.0,
            lp_state: 0.0,
            prng: seed,
            seed,
            tick_fn: tick_dirac,
        };
        s.recompute();
        s
    }

    pub fn set_shape(&mut self, shape: PulseShape) {
        self.shape = shape;
        self.recompute();
    }

    pub fn set_pulse_ms(&mut self, ms: f32) {
        self.pulse_ms = ms.max(0.0);
        self.recompute();
    }

    pub fn set_lp_hz(&mut self, hz: f32) {
        self.lp_hz = hz.max(1.0);
        self.recompute_lp();
    }

    pub fn trigger(&mut self) {
        self.counter = 0;
        self.lp_state = 0.0;
        self.prng = self.seed;
    }

    #[inline]
    pub fn tick(&mut self) -> f32 {
        (self.tick_fn)(self)
    }

    pub fn is_active(&self) -> bool {
        self.counter < self.length_samples
    }

    fn recompute(&mut self) {
        let len = (self.pulse_ms * 0.001 * self.sample_rate).max(1.0) as u32;
        self.length_samples = match self.shape {
            PulseShape::Dirac => 1,
            PulseShape::ExpDecay => {
                let tau_samples = (self.pulse_ms * 0.001 * self.sample_rate).max(1.0);
                self.decay_coeff = (-1.0 / tau_samples).exp();
                (tau_samples * 5.0) as u32
            }
            PulseShape::HalfSine => {
                self.half_sine_step = PI / len as f32;
                len
            }
            PulseShape::FilteredClick => {
                self.recompute_lp();
                len
            }
        };
        self.tick_fn = match self.shape {
            PulseShape::Dirac => tick_dirac,
            PulseShape::ExpDecay => tick_exp_decay,
            PulseShape::HalfSine => tick_half_sine,
            PulseShape::FilteredClick => tick_filtered_click,
        };
        self.counter = self.length_samples;
    }

    fn recompute_lp(&mut self) {
        let omega = 2.0 * PI * self.lp_hz / self.sample_rate;
        self.lp_alpha = 1.0 - (-omega).exp();
    }
}

fn tick_dirac(e: &mut Excitation) -> f32 {
    if e.counter == 0 {
        e.counter = 1;
        1.0
    } else {
        0.0
    }
}

fn tick_exp_decay(e: &mut Excitation) -> f32 {
    if e.counter >= e.length_samples {
        return 0.0;
    }
    let n = e.counter;
    e.counter = n.saturating_add(1);
    let tau_samples = (e.pulse_ms * 0.001 * e.sample_rate).max(1.0);
    let amp = (-(n as f32) / tau_samples).exp();
    if amp < 1.0e-4 {
        e.counter = e.length_samples;
        0.0
    } else {
        amp
    }
}

fn tick_half_sine(e: &mut Excitation) -> f32 {
    if e.counter >= e.length_samples {
        return 0.0;
    }
    let phase = e.half_sine_step * e.counter as f32;
    e.counter = e.counter.saturating_add(1);
    phase.sin()
}

fn tick_filtered_click(e: &mut Excitation) -> f32 {
    if e.counter >= e.length_samples {
        return 0.0;
    }
    e.counter = e.counter.saturating_add(1);
    let white = xorshift64(&mut e.prng);
    e.lp_state += e.lp_alpha * (white - e.lp_state);
    e.lp_state
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 44100.0;

    #[test]
    fn dirac_sample_zero() {
        let mut e = Excitation::new(SR, 1);
        e.set_shape(PulseShape::Dirac);
        e.trigger();
        let first = e.tick();
        assert!((first - 1.0).abs() < 1e-7, "first sample = {first}");
        for n in 1..100 {
            let v = e.tick();
            assert!(v == 0.0, "n={n}: {v}");
        }
    }

    #[test]
    fn exp_decay_monotonic() {
        let mut e = Excitation::new(SR, 7);
        e.set_shape(PulseShape::ExpDecay);
        e.set_pulse_ms(5.0);
        e.trigger();
        let mut prev = e.tick();
        for _ in 1..200 {
            let v = e.tick();
            assert!(v <= prev + 1e-7, "{v} > {prev}");
            prev = v;
        }
    }

    #[test]
    fn half_sine_peaks_middle() {
        let mut e = Excitation::new(SR, 3);
        e.set_shape(PulseShape::HalfSine);
        e.set_pulse_ms(4.0);
        e.trigger();
        let len = (4.0 * 0.001 * SR) as usize;
        let mut buf = Vec::with_capacity(len);
        for _ in 0..len {
            buf.push(e.tick());
        }
        let mid = len / 2;
        let peak_idx = buf
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .map(|(i, _)| i)
            .unwrap_or(0);
        assert!(
            (peak_idx as isize - mid as isize).abs() <= 2,
            "peak at {peak_idx} vs mid {mid}"
        );
        let after = e.tick();
        assert!(after == 0.0, "after pulse: {after}");
    }

    #[test]
    fn filtered_click_band_energy() {
        use crate::test_support::{band_energy, magnitude_spectrum};
        let mut e = Excitation::new(SR, 11);
        e.set_shape(PulseShape::FilteredClick);
        e.set_pulse_ms(2.0);
        e.set_lp_hz(500.0);
        e.trigger();
        let mut buf = vec![0.0; 4096];
        for slot in buf.iter_mut() {
            *slot = e.tick();
        }
        let spec = magnitude_spectrum(&buf, 4096);
        let lo = band_energy(&spec, SR, 4096, 100.0, 1000.0);
        let hi = band_energy(&spec, SR, 4096, 4000.0, 10000.0);
        assert!(lo > hi * 4.0, "lo={lo}, hi={hi}");
    }

    #[test]
    fn active_gating() {
        let mut e = Excitation::new(SR, 5);
        e.set_shape(PulseShape::HalfSine);
        e.set_pulse_ms(2.0);
        assert!(!e.is_active());
        e.trigger();
        assert!(e.is_active());
        let len = (2.0 * 0.001 * SR) as usize + 1;
        for _ in 0..len {
            e.tick();
        }
        assert!(!e.is_active());
    }
}
