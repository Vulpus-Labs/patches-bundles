//! Pure DSP core for [`VFlangerStereo`](super::VFlangerStereo). Two
//! independent BBD chains share one triangle LFO; the right channel
//! sweeps with the inverted LFO, producing a wide pseudo-stereo comb
//! without losing mono compatibility.
//!
//! The flanger chain itself, the `OnePoleLpf` primitive, the constants
//! (`DELAY_MIN_S/MAX_S`, `LF_BYPASS_HZ`, `FB_MAX`), and the triangle
//! LFO tick all live in [`crate::vflanger::core`] — both flanger
//! modules share that single source of truth.

use crate::vflanger::core::{
    tri_lfo_tick, Channel, DELAY_MAX_S, DELAY_MIN_S, FB_MAX,
};

pub struct VFlangerStereoCore {
    sample_rate: f32,

    left: Channel,
    right: Channel,

    lfo_phase: f32,

    /// `smoothing_interval - 1` for both BBDs (same at a given SR).
    mod_interval_mask: u32,
    mod_counter: u32,

    rate_hz: f32,
    depth: f32,
    manual_s: f32,
    feedback: f32,
    mix: f32,
    lf_bypass: bool,
}

impl VFlangerStereoCore {
    pub fn new(sample_rate: f32) -> Self {
        let left = Channel::new(sample_rate);
        let mod_interval_mask = left.smoothing_interval() - 1;
        Self {
            sample_rate,
            left,
            right: Channel::new(sample_rate),
            lfo_phase: 0.0,
            mod_interval_mask,
            mod_counter: 0,
            rate_hz: 0.5,
            depth: 0.5,
            manual_s: 0.002,
            feedback: 0.0,
            mix: 0.5,
            lf_bypass: true,
        }
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
        self.left.bbd_mut().set_jitter_amount(amount);
        self.right.bbd_mut().set_jitter_amount(amount);
    }

    pub fn set_jitter_seed_base(&mut self, base: u32) {
        self.left.bbd_mut().set_jitter_seed(base);
        self.right.bbd_mut().set_jitter_seed(base.wrapping_add(1));
    }

    #[allow(clippy::too_many_arguments)]
    pub fn process(
        &mut self,
        l_in: f32,
        r_in: f32,
        both_connected: bool,
        rate_offset: f32,
        depth_offset: f32,
        manual_offset: f32,
        fb_offset: f32,
    ) -> (f32, f32) {
        // Mono-safe: left and right see the same source, so the
        // inverse-LFO trick produces anti-phase modulation rather than
        // truly independent chains.
        let mono = if both_connected {
            0.5 * (l_in + r_in)
        } else {
            l_in + r_in
        };

        let rate = (self.rate_hz * (1.0 + rate_offset.clamp(-1.0, 1.0))).max(0.01);
        let tri = tri_lfo_tick(&mut self.lfo_phase, rate, self.sample_rate);

        let depth = (self.depth + depth_offset).clamp(0.0, 1.0);
        let manual = (self.manual_s + manual_offset * 0.001).clamp(DELAY_MIN_S, DELAY_MAX_S);
        let span = 0.5 * (DELAY_MAX_S - DELAY_MIN_S) * depth;
        let delay_l = (manual + span * tri).clamp(DELAY_MIN_S, DELAY_MAX_S);
        let delay_r = (manual - span * tri).clamp(DELAY_MIN_S, DELAY_MAX_S);

        let fb = (self.feedback + fb_offset).clamp(-FB_MAX, FB_MAX);

        if self.mod_counter & self.mod_interval_mask == 0 {
            self.left.bbd_mut().set_delay_seconds(delay_l);
            self.right.bbd_mut().set_delay_seconds(delay_r);
        }
        self.mod_counter = self.mod_counter.wrapping_add(1);

        let y_l = self.left.process(mono, fb, self.mix, self.lf_bypass);
        let y_r = self.right.process(mono, fb, self.mix, self.lf_bypass);
        (y_l, y_r)
    }
}
