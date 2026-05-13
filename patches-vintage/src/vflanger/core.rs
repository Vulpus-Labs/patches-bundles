//! Pure DSP core for [`VFlanger`](super::VFlanger). No ports, no
//! parameter map. Testable standalone.
//!
//! Models the Boss BF-2B signal flow: input splits into a low-frequency
//! bypass (preserved flat) and a high-frequency band that is fed through
//! a companded BBD delay modulated by a triangle LFO. Wet output is
//! summed with the dry HF band (~50/50, the characteristic flanger
//! comb) and the untouched LF band.

use crate::bbd::{Bbd, BbdDevice};
use crate::compander::{CompanderParams, Compressor, Expander};

/// BBD delay window in seconds. Matches a 1024-stage BBD clocked
/// between roughly 200 kHz and 30 kHz — the range a BF-2-style
/// flanger covers.
const DELAY_MIN_S: f32 = 0.0003;
const DELAY_MAX_S: f32 = 0.010;
/// LF/HF crossover cutoff when the BF-2B-style bass bypass is enabled.
const LF_BYPASS_HZ: f32 = 150.0;
/// Resonance is positive-only on the hardware but the model accepts
/// signed values so a patch can invert the comb for a hollow tone.
const FB_MAX: f32 = 0.93;

#[derive(Default, Clone, Copy)]
struct OnePoleLpf {
    a: f32,
    y: f32,
}

impl OnePoleLpf {
    fn set_cutoff(&mut self, cutoff_hz: f32, sample_rate: f32) {
        let x = (-std::f32::consts::TAU * cutoff_hz / sample_rate).exp();
        self.a = 1.0 - x;
    }
    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        self.y += self.a * (x - self.y);
        self.y
    }
}

pub struct VFlangerCore {
    sample_rate: f32,

    bbd: Bbd,
    comp: Compressor,
    exp: Expander,
    recon_lpf: OnePoleLpf,
    lf_split: OnePoleLpf,

    lfo_phase: f32,

    // Last wet sample — carried for the feedback path.
    fb_state: f32,

    /// `smoothing_interval - 1` for the BBD — a power-of-two mask
    /// lets us gate `set_delay_seconds` with a single AND.
    mod_interval_mask: u32,
    mod_counter: u32,

    // Parameters.
    rate_hz: f32,
    depth: f32,
    manual_s: f32,
    feedback: f32,
    mix: f32,
    lf_bypass: bool,
}

impl VFlangerCore {
    pub fn new(sample_rate: f32) -> Self {
        let bbd = Bbd::new(&BbdDevice::BBD_1024, sample_rate);
        let mod_interval_mask = bbd.smoothing_interval() - 1;
        let mut me = Self {
            sample_rate,
            bbd,
            comp: Compressor::new(CompanderParams::NE570_DEFAULT, sample_rate),
            exp: Expander::new(CompanderParams::NE570_DEFAULT, sample_rate),
            recon_lpf: OnePoleLpf::default(),
            lf_split: OnePoleLpf::default(),
            lfo_phase: 0.0,
            fb_state: 0.0,
            mod_interval_mask,
            mod_counter: 0,
            rate_hz: 0.5,
            depth: 0.5,
            manual_s: 0.002,
            feedback: 0.0,
            mix: 0.5,
            lf_bypass: true,
        };
        me.recon_lpf.set_cutoff(8_000.0, sample_rate);
        me.lf_split.set_cutoff(LF_BYPASS_HZ, sample_rate);
        me
    }

    pub fn set_rate(&mut self, r: f32) {
        self.rate_hz = r.clamp(0.05, 12.0);
    }
    pub fn set_depth(&mut self, d: f32) {
        self.depth = d.clamp(0.0, 1.0);
    }
    pub fn set_manual(&mut self, ms: f32) {
        self.manual_s = (ms * 0.001).clamp(DELAY_MIN_S, DELAY_MAX_S);
    }
    pub fn set_feedback(&mut self, f: f32) {
        self.feedback = f.clamp(-FB_MAX, FB_MAX);
    }
    pub fn set_mix(&mut self, m: f32) {
        self.mix = m.clamp(0.0, 1.0);
    }
    pub fn set_lf_bypass(&mut self, on: bool) {
        self.lf_bypass = on;
    }

    pub fn set_jitter(&mut self, amount: f32) {
        self.bbd.set_jitter_amount(amount);
    }

    pub fn set_jitter_seed(&mut self, seed: u32) {
        self.bbd.set_jitter_seed(seed);
    }

    pub fn process(
        &mut self,
        x: f32,
        rate_offset: f32,
        depth_offset: f32,
        manual_offset: f32,
        fb_offset: f32,
    ) -> f32 {
        // ── LFO (triangle in [-1, +1]) ──────────────────────────────
        let rate = (self.rate_hz * (1.0 + rate_offset.clamp(-1.0, 1.0))).max(0.01);
        self.lfo_phase += rate / self.sample_rate;
        if self.lfo_phase >= 1.0 {
            self.lfo_phase -= 1.0;
        }
        let tri = 4.0 * (self.lfo_phase - (self.lfo_phase + 0.5).floor()).abs() - 1.0;

        // Depth scales the sweep around the manual centre. The total
        // window is clamped to the BBD-usable range.
        let depth = (self.depth + depth_offset).clamp(0.0, 1.0);
        let manual = (self.manual_s + manual_offset * 0.001).clamp(DELAY_MIN_S, DELAY_MAX_S);
        let span = 0.5 * (DELAY_MAX_S - DELAY_MIN_S) * depth;
        let delay = (manual + span * tri).clamp(DELAY_MIN_S, DELAY_MAX_S);
        if self.mod_counter & self.mod_interval_mask == 0 {
            self.bbd.set_delay_seconds(delay);
        }
        self.mod_counter = self.mod_counter.wrapping_add(1);

        // ── LF/HF split ─────────────────────────────────────────────
        // HPF is always applied to the BBD path — matches the ~100 Hz
        // input HPF on a real BF-2 and keeps low-note comb notches out
        // of the effect. Dry LF is reinjected only when `lf_bypass` is
        // on (BF-2B behaviour); otherwise it is simply rolled off, as
        // on the plain BF-2.
        let lf = self.lf_split.process(x);
        let hf = x - lf;

        // ── Flanger core: HF + feedback → comp → BBD → exp → LPF ────
        let fb = (self.feedback + fb_offset).clamp(-FB_MAX, FB_MAX);
        let drive = hf + fb * self.fb_state;
        let comp = self.comp.process(drive);
        let bbd_out = self.bbd.process(comp);
        let expanded = self.exp.process(bbd_out);
        let wet = self.recon_lpf.process(expanded);

        self.fb_state = patches_dsp::flush_denormal(wet);

        let dry_lf = if self.lf_bypass { lf } else { 0.0 };
        dry_lf + (1.0 - self.mix) * hf + self.mix * wet
    }
}
