//! Pure DSP core for [`VChorus`](super::VChorus): no module protocol,
//! no ports, no parameter map. Testable in isolation.

use patches_sdk::params_enum;
use patches_dsp::noise::xorshift64;

use crate::bbd::{Bbd, BbdDevice};
use crate::primitives::{OnePoleLpf, TriangleLfo};

params_enum! {
    pub enum Variant {
        Bright => "bright",
        Dark => "dark",
    }
}

params_enum! {
    pub enum Mode {
        Off => "off",
        One => "one",
        Two => "two",
        Both => "both",
    }
}

/// Per-variant, per-mode LFO rate and delay sweep.
#[derive(Clone, Copy, Debug)]
struct ModeTable {
    rate_hz: f32,
    delay_min_s: f32,
    delay_max_s: f32,
}

impl ModeTable {
    #[inline]
    fn center(&self) -> f32 {
        0.5 * (self.delay_min_s + self.delay_max_s)
    }

    #[inline]
    fn depth(&self) -> f32 {
        0.5 * (self.delay_max_s - self.delay_min_s)
    }
}

fn mode_table(variant: Variant, mode: Mode) -> ModeTable {
    match (variant, mode) {
        // Off and One share timings on both variants; depth is zeroed at
        // runtime when `mode == Off`.
        (Variant::Bright, Mode::One) | (Variant::Bright, Mode::Off) => ModeTable {
            rate_hz: 0.513,
            delay_min_s: 0.00166,
            delay_max_s: 0.00535,
        },
        (Variant::Bright, Mode::Two) => ModeTable {
            rate_hz: 0.863,
            delay_min_s: 0.00166,
            delay_max_s: 0.00535,
        },
        (Variant::Bright, Mode::Both) => ModeTable {
            rate_hz: 9.75,
            delay_min_s: 0.00330,
            delay_max_s: 0.00370,
        },
        (Variant::Dark, Mode::One) | (Variant::Dark, Mode::Off) => ModeTable {
            rate_hz: 0.5,
            delay_min_s: 0.00166,
            delay_max_s: 0.00535,
        },
        // `(Dark, Both)` is not a real hardware mode. Both `mode` and
        // `variant` are realtime params, so descriptor validation
        // can't reject the combination — we silently coerce to
        // `Mode::Two` rather than panicking. Documented in the module
        // parameter table; a future structural-param refactor (ADR
        // 0060) could make this a true bind-time error.
        (Variant::Dark, Mode::Two) | (Variant::Dark, Mode::Both) => ModeTable {
            rate_hz: 0.83,
            delay_min_s: 0.00166,
            delay_max_s: 0.00535,
        },
    }
}

/// Pure-DSP core of the VChorus module. Owns the BBDs, reconstruction
/// filters, LFO phase and hiss PRNG; no knowledge of ports or params.
pub struct VChorusCore {
    sample_rate: f32,

    variant: Variant,
    mode: Mode,
    hiss_amount: f32,

    bbd_l: Bbd,
    bbd_r: Bbd,
    lpf_l: OnePoleLpf,
    lpf_r: OnePoleLpf,

    lfo: TriangleLfo,
    noise_state: u64,

    /// `smoothing_interval - 1`; gates delay updates to the BBDs via
    /// `counter & mask == 0`.
    mod_interval_mask: u32,
    mod_counter: u32,
}

impl VChorusCore {
    pub fn new(sample_rate: f32, noise_seed: u64) -> Self {
        let bbd_l = Bbd::new(&BbdDevice::BBD_256, sample_rate);
        let bbd_r = Bbd::new(&BbdDevice::BBD_256, sample_rate);
        let interval = bbd_l.smoothing_interval();
        debug_assert!(
            interval.is_power_of_two(),
            "BBD smoothing_interval must be a power of two (got {interval})"
        );
        let mod_interval_mask = interval - 1;
        let mut me = Self {
            sample_rate,
            variant: Variant::Bright,
            mode: Mode::One,
            hiss_amount: 1.0,
            bbd_l,
            bbd_r,
            lpf_l: OnePoleLpf::default(),
            lpf_r: OnePoleLpf::default(),
            lfo: TriangleLfo::new(),
            noise_state: noise_seed,
            mod_interval_mask,
            mod_counter: 0,
        };
        me.apply_variant_filters();
        me
    }

    pub fn set_variant(&mut self, v: Variant) {
        if v != self.variant {
            self.variant = v;
            self.apply_variant_filters();
        }
    }

    pub fn set_mode(&mut self, m: Mode) {
        self.mode = m;
    }

    pub fn set_hiss(&mut self, h: f32) {
        self.hiss_amount = h.clamp(0.0, 1.0);
    }

    pub fn set_jitter(&mut self, amount: f32) {
        self.bbd_l.set_jitter_amount(amount);
        self.bbd_r.set_jitter_amount(amount);
    }

    pub fn set_jitter_seed_base(&mut self, base: u32) {
        self.bbd_l.set_jitter_seed(base);
        self.bbd_r.set_jitter_seed(base.wrapping_add(1));
    }

    #[inline]
    fn reconstruction_cutoff(variant: Variant) -> f32 {
        match variant {
            Variant::Bright => 9_000.0,
            Variant::Dark => 7_000.0,
        }
    }

    #[inline]
    fn dry_wet(variant: Variant) -> (f32, f32) {
        // Approximate summing-resistor ratios on the hardware:
        // bright ≈ 1:1.15 (wet hotter); dark ≈ 1:1.
        match variant {
            Variant::Bright => (1.0, 1.15),
            Variant::Dark => (1.0, 1.0),
        }
    }

    #[inline]
    fn hiss_floor(variant: Variant) -> f32 {
        // dark is ~6–8 dB quieter at matched hiss=1.0.
        match variant {
            Variant::Bright => 0.0020,
            Variant::Dark => 0.0010,
        }
    }

    #[inline]
    fn bypasses_when_off(variant: Variant) -> bool {
        matches!(variant, Variant::Bright)
    }

    fn apply_variant_filters(&mut self) {
        let cutoff = Self::reconstruction_cutoff(self.variant);
        self.lpf_l.set_cutoff(cutoff, self.sample_rate);
        self.lpf_r.set_cutoff(cutoff, self.sample_rate);
    }

    /// Process one stereo sample. `both_connected` tells the core
    /// whether both inputs carry signal (so the L+R sum should be
    /// halved to avoid doubling when a mono source is routed to one
    /// side only).
    pub fn process(
        &mut self,
        l_in: f32,
        r_in: f32,
        both_connected: bool,
        rate_offset: f32,
        depth_offset: f32,
    ) -> (f32, f32) {
        // ── Dry path ────────────────────────────────────────────────
        let mono_in = if both_connected {
            0.5 * (l_in + r_in)
        } else {
            l_in + r_in
        };

        // Short-circuit `off` on the bright variant: the hardware
        // routes the signal around the BBD entirely.
        if matches!(self.mode, Mode::Off) && Self::bypasses_when_off(self.variant) {
            return (l_in, r_in);
        }

        // ── LFO ─────────────────────────────────────────────────────
        let table = mode_table(self.variant, self.mode);
        let rate_offset = rate_offset.clamp(-1.0, 1.0);
        let rate_hz = (table.rate_hz * (1.0 + rate_offset)).max(0.01);
        let depth_offset = depth_offset.clamp(-1.0, 1.0);
        let depth_scale = if matches!(self.mode, Mode::Off) {
            0.0
        } else {
            (1.0 + depth_offset).clamp(0.0, 2.0)
        };

        self.lfo.set_rate(rate_hz, self.sample_rate);
        let lfo = self.lfo.tick();

        let depth = table.depth() * depth_scale;
        let center = table.center();
        let min_d = (center - table.depth()).max(1.0e-4);
        let max_d = center + table.depth();
        let delay_l = (center + depth * lfo).clamp(min_d, max_d);
        let delay_r = (center - depth * lfo).clamp(min_d, max_d);
        if self.mod_counter & self.mod_interval_mask == 0 {
            self.bbd_l.set_delay_seconds(delay_l);
            self.bbd_r.set_delay_seconds(delay_r);
        }
        self.mod_counter = self.mod_counter.wrapping_add(1);

        // ── Hiss injection (pre-BBD: filtered + modulated) ──────────
        let floor = Self::hiss_floor(self.variant) * self.hiss_amount;
        let n_l = xorshift64(&mut self.noise_state) * floor;
        let n_r = xorshift64(&mut self.noise_state) * floor;

        // ── BBD + reconstruction LPF ────────────────────────────────
        let wet_l_raw = self.bbd_l.process(mono_in + n_l);
        let wet_r_raw = self.bbd_r.process(mono_in + n_r);
        let wet_l = self.lpf_l.process(wet_l_raw);
        let wet_r = self.lpf_r.process(wet_r_raw);

        // ── Dry/wet sum ─────────────────────────────────────────────
        let (gd, gw) = Self::dry_wet(self.variant);
        (gd * l_in + gw * wet_l, gd * r_in + gw * wet_r)
    }
}
