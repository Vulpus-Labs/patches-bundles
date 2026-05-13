//! Vintage stereo BBD multi-tap delay.
//!
//! Two parallel [`crate::vbbd::VBbd`]-style chains (L and R), each with
//! N taps built on [`crate::bbd::Bbd`] (4096-stage preset) and bracketed
//! by an NE570-style compander. Per-tap delay/gain/feedback parameters
//! are shared between the L and R chains; per-side decorrelation comes
//! from independent BBD jitter seeds. Optional `pingpong` per tap
//! cross-routes feedback (L tap's feedback drives R tap's input on the
//! next tick and vice versa).
//!
//! A mono input arriving via mono→stereo broadcast (ADR 0059 §2)
//! produces identical L/R unless `pingpong` or non-zero `jitter` adds
//! decorrelation. A true stereo input is processed channel-independent.
//!
//! # Inputs
//!
//! | Port | Kind | Description |
//! |------|------|-------------|
//! | `in` | stereo | Stereo audio input |
//! | `drywet_cv` | mono | Additive CV for dry/wet |
//! | `delay_cv[i]` | mono | Multiplicative CV for delay time (i in 0..N-1, N = channels) |
//! | `gain_cv[i]` | mono | Additive CV for tap gain |
//! | `fb_cv[i]` | mono | Additive CV for feedback |
//!
//! # Outputs
//!
//! | Port | Kind | Description |
//! |------|------|-------------|
//! | `out` | stereo | Wet/dry mixed stereo output |
//!
//! # Parameters
//!
//! | Name | Type | Range | Default | Description |
//! |------|------|-------|---------|-------------|
//! | `dry_wet` | float | 0.0--1.0 | `0.5` | Dry/wet mix |
//! | `jitter` | float | 0.0--1.0 | `0.0` | BBD clock jitter amount |
//! | `delay_ms[i]` | float | 1.0--340.0 | `40.0` | Delay time in ms (per tap) |
//! | `gain[i]` | float | 0.0--1.0 | `1.0` | Tap gain |
//! | `feedback[i]` | float | 0.0--0.95 | `0.0` | Self-feedback (or cross-feedback if `pingpong[i]`) |
//! | `pingpong[i]` | bool | -- | `false` | Cross-route tap feedback L↔R |

use patches_sdk::module_params;
use patches_sdk::param_frame::ParamView;
use patches_sdk::{
    AudioEnvironment, AxisId, CablePool, CountAxis, InputPort, InstanceId, Module,
    ModuleDescriptor, ModuleDescriptorTemplate, MonoInput, OutputPort, ParameterKind,
    ParameterTemplate, PortTemplate, StereoInput, StereoOutput,
};
use patches_sdk::{StructuralParams, BuildError};
use patches_dsp::approximate::fast_tanh;

use crate::vbbd::{Tap, DELAY_MS_MAX, DELAY_MS_MIN, FEEDBACK_MAX};

module_params! {
    VStereoBbd {
        dry_wet:  Float,
        jitter:   Float,
        delay_ms: FloatArray,
        gain:     FloatArray,
        feedback: FloatArray,
        pingpong: BoolArray,
    }
}

pub struct VStereoBbd {
    instance_id: InstanceId,
    descriptor: ModuleDescriptor,
    taps: usize,

    taps_l: Vec<Tap>,
    taps_r: Vec<Tap>,

    dry_wet: f32,
    delay_ms: Vec<f32>,
    gains: Vec<f32>,
    feedbacks: Vec<f32>,
    pingpong: Vec<bool>,

    in_stereo: StereoInput,
    drywet_cv: MonoInput,
    out_stereo: StereoOutput,
    delay_cv: Vec<MonoInput>,
    gain_cv: Vec<MonoInput>,
    fb_cv: Vec<MonoInput>,
}

impl Module for VStereoBbd {
    fn template() -> ModuleDescriptorTemplate {
        const T: ModuleDescriptorTemplate = ModuleDescriptorTemplate {
            name: "VStereoBbd",
            axes: &[CountAxis::CHANNELS],
            global_inputs: &[PortTemplate::stereo("in"), PortTemplate::mono("drywet_cv")],
            per_axis_inputs: &[
                (AxisId::CHANNELS, PortTemplate::mono("delay_cv")),
                (AxisId::CHANNELS, PortTemplate::mono("gain_cv")),
                (AxisId::CHANNELS, PortTemplate::mono("fb_cv")),
            ],
            global_outputs: &[PortTemplate::stereo("out")],
            per_axis_outputs: &[],
            realtime_params: &[
                ParameterTemplate { name: params::dry_wet.as_str(), kind: ParameterKind::Float { min: 0.0, max: 1.0, default: 0.5 } },
                ParameterTemplate { name: params::jitter.as_str(),  kind: ParameterKind::Float { min: 0.0, max: 1.0, default: 0.0 } },
            ],
            structural_params: &[],
            per_axis_realtime_params: &[
                (AxisId::CHANNELS, ParameterTemplate { name: params::delay_ms.as_str(), kind: ParameterKind::Float { min: DELAY_MS_MIN, max: DELAY_MS_MAX, default: 40.0 } }),
                (AxisId::CHANNELS, ParameterTemplate { name: params::gain.as_str(),     kind: ParameterKind::Float { min: 0.0, max: 1.0, default: 1.0 } }),
                (AxisId::CHANNELS, ParameterTemplate { name: params::feedback.as_str(), kind: ParameterKind::Float { min: 0.0, max: FEEDBACK_MAX, default: 0.0 } }),
                (AxisId::CHANNELS, ParameterTemplate { name: params::pingpong.as_str(), kind: ParameterKind::Bool { default: false } }),
            ],
            per_axis_structural_params: &[],
        };
        T
    }

    fn prepare(
        env: &AudioEnvironment,
        descriptor: ModuleDescriptor,
        instance_id: InstanceId,
        _structural: &StructuralParams,
    ) -> Result<Self, BuildError> {
        let sr = env.sample_rate;
        let interval = env.periodic_update_interval;
        let taps = descriptor.shape.channels;
        let seed_base = (instance_id.as_u64() ^ 0xBBD0_0020) as u32;
        let taps_l: Vec<Tap> = (0..taps)
            .map(|i| {
                let mut t = Tap::new(sr, interval);
                t.bbd.set_jitter_seed(seed_base.wrapping_add(i as u32));
                t
            })
            .collect();
        let taps_r: Vec<Tap> = (0..taps)
            .map(|i| {
                let mut t = Tap::new(sr, interval);
                t.bbd
                    .set_jitter_seed(seed_base.wrapping_add(0x8000_0000 ^ i as u32));
                t
            })
            .collect();

        Ok(Self {
            instance_id,
            descriptor,
            taps,
            taps_l,
            taps_r,
            dry_wet: 0.5,
            delay_ms: vec![40.0; taps],
            gains: vec![1.0; taps],
            feedbacks: vec![0.0; taps],
            pingpong: vec![false; taps],
            in_stereo: StereoInput::default(),
            drywet_cv: MonoInput::default(),
            out_stereo: StereoOutput::default(),
            delay_cv: vec![MonoInput::default(); taps],
            gain_cv: vec![MonoInput::default(); taps],
            fb_cv: vec![MonoInput::default(); taps],
        })
    }

    fn update_validated_parameters(&mut self, p: &ParamView<'_>) {
        self.dry_wet = p.get(params::dry_wet).clamp(0.0, 1.0);
        let jitter = p.get(params::jitter).clamp(0.0, 1.0);
        for i in 0..self.taps {
            let idx = i as u16;
            self.delay_ms[i] = p.get(params::delay_ms.at(idx)).clamp(DELAY_MS_MIN, DELAY_MS_MAX);
            self.gains[i] = p.get(params::gain.at(idx)).clamp(0.0, 1.0);
            self.feedbacks[i] = p.get(params::feedback.at(idx)).clamp(0.0, FEEDBACK_MAX);
            self.pingpong[i] = p.get(params::pingpong.at(idx));
            self.taps_l[i].bbd.set_jitter_amount(jitter);
            self.taps_r[i].bbd.set_jitter_amount(jitter);
        }
    }

    fn descriptor(&self) -> &ModuleDescriptor {
        &self.descriptor
    }

    fn instance_id(&self) -> InstanceId {
        self.instance_id
    }

    fn set_ports(&mut self, inputs: &[InputPort], outputs: &[OutputPort]) {
        let n = self.taps;
        self.in_stereo = StereoInput::from_ports(inputs, 0);
        self.drywet_cv = MonoInput::from_ports(inputs, 1);
        for i in 0..n {
            self.delay_cv[i] = MonoInput::from_ports(inputs, 2 + i);
            self.gain_cv[i] = MonoInput::from_ports(inputs, 2 + n + i);
            self.fb_cv[i] = MonoInput::from_ports(inputs, 2 + 2 * n + i);
        }
        self.out_stereo = StereoOutput::from_ports(outputs, 0);
    }

    fn process(&mut self, pool: &mut CablePool<'_>) {
        let (in_l, in_r) = pool.read_stereo(&self.in_stereo);

        let mut wet_l = 0.0_f32;
        let mut wet_r = 0.0_f32;

        // Snapshot prior fb_state so swaps for pingpong taps stay
        // symmetric within this tick.
        for i in 0..self.taps {
            let fb_l_in = self.taps_l[i].fb_state;
            let fb_r_in = self.taps_r[i].fb_state;

            let comp_l = self.taps_l[i].comp.process(fast_tanh(in_l));
            let comp_r = self.taps_r[i].comp.process(fast_tanh(in_r));

            let with_fb_l = fast_tanh(comp_l + fb_l_in);
            let with_fb_r = fast_tanh(comp_r + fb_r_in);

            let bbd_l = self.taps_l[i].bbd.process(with_fb_l);
            let bbd_r = self.taps_r[i].bbd.process(with_fb_r);

            let tap_l = self.taps_l[i].exp.process(bbd_l);
            let tap_r = self.taps_r[i].exp.process(bbd_r);

            let eff_gain = (self.gains[i] + pool.read_mono(&self.gain_cv[i])).clamp(0.0, 1.0);
            wet_l += tap_l * eff_gain;
            wet_r += tap_r * eff_gain;

            let eff_fb =
                (self.feedbacks[i] + pool.read_mono(&self.fb_cv[i])).clamp(0.0, FEEDBACK_MAX);
            let fb_l_filt = self.taps_l[i].filter_feedback(bbd_l);
            let fb_r_filt = self.taps_r[i].filter_feedback(bbd_r);
            let (l, r) = if self.pingpong[i] {
                (fb_r_filt * eff_fb, fb_l_filt * eff_fb)
            } else {
                (fb_l_filt * eff_fb, fb_r_filt * eff_fb)
            };
            self.taps_l[i].fb_state = patches_dsp::flush_denormal(l);
            self.taps_r[i].fb_state = patches_dsp::flush_denormal(r);
        }

        let eff_dw = (self.dry_wet + pool.read_mono(&self.drywet_cv)).clamp(0.0, 1.0);
        let out_l = in_l + eff_dw * (wet_l - in_l);
        let out_r = in_r + eff_dw * (wet_r - in_r);
        pool.write_stereo(&self.out_stereo, out_l, out_r);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn wants_periodic(&self) -> bool {
        true
    }

    fn periodic_update(&mut self, pool: &CablePool<'_>) {
        for i in 0..self.taps {
            let cv = pool.read_mono(&self.delay_cv[i]).clamp(-1.0, 2.0);
            let delay_s = (self.delay_ms[i] * (1.0 + cv) * 0.001)
                .clamp(DELAY_MS_MIN * 0.001, DELAY_MS_MAX * 0.001);
            self.taps_l[i].bbd.set_delay_seconds(delay_s);
            self.taps_r[i].bbd.set_delay_seconds(delay_s);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use patches_sdk::parameter_map::{ParameterMap, ParameterValue};
    use patches_sdk::test_support::{params, ModuleHarness};
    use patches_sdk::{AudioEnvironment, ModuleShape};

    const SR: f32 = 48_000.0;
    const ENV: AudioEnvironment = AudioEnvironment {
        sample_rate: SR,
        poly_voices: 16,
        periodic_update_interval: 32,
        hosted: false,
    };

    fn shape(taps: usize) -> ModuleShape {
        ModuleShape { channels: taps }
    }

    #[test]
    fn descriptor_shape() {
        let h = ModuleHarness::build::<VStereoBbd>(&[]);
        let d = h.descriptor();
        assert_eq!(d.module_name, "VStereoBbd");
        assert_eq!(d.outputs.len(), 1);
    }

    #[test]
    fn dry_wet_zero_passes_only_dry() {
        let mut h = ModuleHarness::build_full::<VStereoBbd>(
            params!["dry_wet" => 0.0_f32],
            ENV,
            shape(1),
        );
        h.set_stereo("in", 0.7, -0.3);
        h.tick();
        let (l, r) = h.read_stereo("out");
        assert_eq!(l, 0.7);
        assert_eq!(r, -0.3);
    }

    #[test]
    fn output_bounded_under_sustained_input() {
        let mut h = ModuleHarness::build_full::<VStereoBbd>(params![], ENV, shape(2));
        let mut pm = ParameterMap::new();
        pm.insert_param("dry_wet", 0, ParameterValue::Float(1.0));
        pm.insert_param("delay_ms", 0, ParameterValue::Float(5.0));
        pm.insert_param("delay_ms", 1, ParameterValue::Float(15.0));
        pm.insert_param("feedback", 0, ParameterValue::Float(0.9));
        pm.insert_param("feedback", 1, ParameterValue::Float(0.9));
        pm.insert_param("gain", 0, ParameterValue::Float(1.0));
        pm.insert_param("gain", 1, ParameterValue::Float(1.0));
        h.update_params_map(&pm);
        h.disconnect_input("drywet_cv");
        for i in 0..2 {
            h.disconnect_input_at("delay_cv", i);
            h.disconnect_input_at("gain_cv", i);
            h.disconnect_input_at("fb_cv", i);
        }
        for i in 0..20_000 {
            let t = i as f32 / SR;
            let x = 0.5 * (std::f32::consts::TAU * 440.0 * t).sin();
            h.set_stereo("in", x, -x);
            h.tick();
            let (l, r) = h.read_stereo("out");
            assert!(l.is_finite() && l.abs() < 5.0, "L diverged at i={i}: {l}");
            assert!(r.is_finite() && r.abs() < 5.0, "R diverged at i={i}: {r}");
        }
    }
}
