//! Pure DSP core for [`VFlanger`](super::VFlanger) and the building
//! blocks shared with [`vflanger_stereo`](crate::vflanger_stereo).
//!
//! Models the Boss BF-2B signal flow: input splits into a low-frequency
//! bypass (preserved flat) and a high-frequency band that is fed through
//! a companded BBD delay modulated by a triangle LFO. Wet output is
//! summed with the dry HF band (~50/50, the characteristic flanger
//! comb) and the untouched LF band.
//!
//! `Channel` owns one BBD chain (compander + BBD + reconstruction LPF +
//! LF/HF split + feedback state). `VFlanger` holds a single `Channel`;
//! `VFlangerStereo` holds two, sharing this same chain definition.
//!
//! Per-module seed salt registry (XORed with `instance_id` so two
//! instances of *different* modules with the same id stay
//! decorrelated):
//!   `0xBBD0_0001` vbbd, `0xBBD0_0010` vchorus, `0xBBD0_0020` vstereobbd,
//!   `0xBBD0_0030` vflanger_stereo, `0xBBD0_0040` vreverb,
//!   `0xBBD0_0050` vflanger.

use crate::bbd::{Bbd, BbdDevice};
use crate::compander::{CompanderParams, Compressor, Expander};
use crate::primitives::{OnePoleLpf, TriangleLfo};

/// BBD delay window in seconds. Matches a 1024-stage BBD clocked
/// between roughly 200 kHz and 30 kHz — the range a BF-2-style
/// flanger covers.
pub(crate) const DELAY_MIN_S: f32 = 0.0003;
pub(crate) const DELAY_MAX_S: f32 = 0.010;
/// LF/HF crossover cutoff when the BF-2B-style bass bypass is enabled.
pub(crate) const LF_BYPASS_HZ: f32 = 150.0;
/// Resonance is positive-only on the hardware but the model accepts
/// signed values so a patch can invert the comb for a hollow tone.
pub(crate) const FB_MAX: f32 = 0.93;

/// One flanger channel: HPF/LPF split → HF + feedback → compand → BBD
/// → expand → reconstruction LPF → LF reinjection. The mono module
/// owns one of these; the stereo module owns two.
pub(crate) struct Channel {
    bbd: Bbd,
    comp: Compressor,
    exp: Expander,
    recon: OnePoleLpf,
    lf_split: OnePoleLpf,
    fb_state: f32,
}

impl Channel {
    pub(crate) fn new(sample_rate: f32) -> Self {
        let mut c = Self {
            bbd: Bbd::new(&BbdDevice::BBD_1024, sample_rate),
            comp: Compressor::new(CompanderParams::NE570_DEFAULT, sample_rate),
            exp: Expander::new(CompanderParams::NE570_DEFAULT, sample_rate),
            recon: OnePoleLpf::default(),
            lf_split: OnePoleLpf::default(),
            fb_state: 0.0,
        };
        c.recon.set_cutoff(8_000.0, sample_rate);
        c.lf_split.set_cutoff(LF_BYPASS_HZ, sample_rate);
        c
    }

    pub(crate) fn bbd_mut(&mut self) -> &mut Bbd {
        &mut self.bbd
    }

    pub(crate) fn smoothing_interval(&self) -> u32 {
        self.bbd.smoothing_interval()
    }

    #[inline]
    pub(crate) fn process(&mut self, x: f32, fb: f32, mix: f32, lf_bypass: bool) -> f32 {
        // HPF is always applied to the BBD path — matches the ~100 Hz
        // input HPF on a real BF-2 and keeps low-note comb notches out
        // of the effect. Dry LF is reinjected only when `lf_bypass` is
        // on (BF-2B behaviour); otherwise it is simply rolled off, as
        // on the plain BF-2.
        let lf = self.lf_split.process(x);
        let hf = x - lf;
        let drive = hf + fb * self.fb_state;
        let compressed = self.comp.process(drive);
        let bbd_out = self.bbd.process(compressed);
        let expanded = self.exp.process(bbd_out);
        let wet = self.recon.process(expanded);
        self.fb_state = patches_dsp::flush_denormal(wet);
        let dry_lf = if lf_bypass { lf } else { 0.0 };
        dry_lf + (1.0 - mix) * hf + mix * wet
    }
}

pub struct VFlangerCore {
    sample_rate: f32,

    channel: Channel,

    lfo: TriangleLfo,

    /// `smoothing_interval - 1` for the BBD — a power-of-two mask
    /// lets us gate `set_delay_seconds` with a single AND.
    mod_interval_mask: u32,
    mod_counter: u32,

    rate_hz: f32,
    depth: f32,
    manual_s: f32,
    feedback: f32,
    mix: f32,
    lf_bypass: bool,
}

impl VFlangerCore {
    pub fn new(sample_rate: f32) -> Self {
        let channel = Channel::new(sample_rate);
        let mod_interval_mask = channel.smoothing_interval() - 1;
        Self {
            sample_rate,
            channel,
            lfo: TriangleLfo::new(),
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
        self.channel.bbd_mut().set_jitter_amount(amount);
    }

    pub fn set_jitter_seed(&mut self, seed: u32) {
        self.channel.bbd_mut().set_jitter_seed(seed);
    }

    pub fn process(
        &mut self,
        x: f32,
        rate_offset: f32,
        depth_offset: f32,
        manual_offset: f32,
        fb_offset: f32,
    ) -> f32 {
        let rate = (self.rate_hz * (1.0 + rate_offset.clamp(-1.0, 1.0))).max(0.01);
        self.lfo.set_rate(rate, self.sample_rate);
        let tri = self.lfo.tick();

        // Depth scales the sweep around the manual centre. The total
        // window is clamped to the BBD-usable range.
        let depth = (self.depth + depth_offset).clamp(0.0, 1.0);
        let manual = (self.manual_s + manual_offset * 0.001).clamp(DELAY_MIN_S, DELAY_MAX_S);
        let span = 0.5 * (DELAY_MAX_S - DELAY_MIN_S) * depth;
        let delay = (manual + span * tri).clamp(DELAY_MIN_S, DELAY_MAX_S);
        if self.mod_counter & self.mod_interval_mask == 0 {
            self.channel.bbd_mut().set_delay_seconds(delay);
        }
        self.mod_counter = self.mod_counter.wrapping_add(1);

        let fb = (self.feedback + fb_offset).clamp(-FB_MAX, FB_MAX);
        self.channel.process(x, fb, self.mix, self.lf_bypass)
    }
}
