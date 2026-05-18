//! Stereo BBD flanger.
//!
//! Two [`crate::bbd::Bbd`] chains share one triangle LFO; the right
//! channel is swept with the inverted LFO. A mono input routed via
//! mono→stereo broadcast (ADR 0059 §2) produces an anti-phase comb
//! across L/R (wide but mono-safe); a true stereo input is summed to
//! mono before the BBD chains, so the output spread comes from the
//! modulation, not the source.
//!
//! # Inputs
//!
//! | Port | Kind | Description |
//! |------|------|-------------|
//! | `in` | stereo | Stereo audio input |
//! | `rate_cv` | mono | Additive CV offset for LFO rate |
//! | `depth_cv` | mono | Additive CV offset for sweep depth |
//! | `manual_cv` | mono | Additive CV (ms) for centre delay |
//! | `feedback_cv` | mono | Additive CV for resonance/feedback |
//!
//! # Outputs
//!
//! | Port | Kind | Description |
//! |------|------|-------------|
//! | `out` | stereo | Stereo output |
//!
//! # Parameters
//!
//! | Name | Type | Range | Default | Description |
//! |------|------|-------|---------|-------------|
//! | `rate_hz` | float | 0.05--12.0 | `0.5` | Triangle LFO rate |
//! | `depth` | float | 0.0--1.0 | `0.5` | Sweep depth around centre |
//! | `manual_ms` | float | 0.3--8.0 | `2.0` | Centre delay in ms |
//! | `feedback` | float | -0.93--0.93 | `0.3` | Resonance (signed) |
//! | `mix` | float | 0.0--1.0 | `0.5` | Dry/wet on the HF path |
//! | `lf_bypass` | bool | on/off | `on` | 150 Hz bass bypass |

use patches_sdk::module_params;
use patches_sdk::param_frame::ParamView;
use patches_sdk::{
    AudioEnvironment, CablePool, CountAxis, InputPort, InstanceId, Module, ModuleDescriptor,
    ModuleDescriptorTemplate, MonoInput, OutputPort, ParameterKind, ParameterTemplate,
    PortTemplate, StereoInput, StereoOutput,
};
use patches_sdk::{StructuralParams, BuildError};

mod core;

pub use self::core::VFlangerStereoCore;

module_params! {
    VFlangerStereo {
        rate_hz:   Float,
        depth:     Float,
        manual_ms: Float,
        feedback:  Float,
        mix:       Float,
        lf_bypass: Bool,
        jitter:    Float,
    }
}

pub struct VFlangerStereo {
    instance_id: InstanceId,
    descriptor: ModuleDescriptor,
    core: VFlangerStereoCore,

    in_stereo: StereoInput,
    rate_cv: MonoInput,
    depth_cv: MonoInput,
    manual_cv: MonoInput,
    fb_cv: MonoInput,
    out_stereo: StereoOutput,
}

impl Module for VFlangerStereo {
    fn template() -> ModuleDescriptorTemplate {
        const T: ModuleDescriptorTemplate = ModuleDescriptorTemplate {
            name: "VFlangerStereo",
            axes: &[CountAxis::CHANNELS],
            global_inputs: &[
                PortTemplate::stereo("in"),
                PortTemplate::mono("rate_cv"),
                PortTemplate::mono("depth_cv"),
                PortTemplate::mono("manual_cv"),
                PortTemplate::mono("feedback_cv"),
            ],
            per_axis_inputs: &[],
            global_outputs: &[PortTemplate::stereo("out")],
            per_axis_outputs: &[],
            realtime_params: &[
                ParameterTemplate { name: params::rate_hz.as_str(),   kind: ParameterKind::Float { min: 0.05, max: 12.0, default: 0.5 } },
                ParameterTemplate { name: params::depth.as_str(),     kind: ParameterKind::Float { min: 0.0,  max: 1.0,  default: 0.5 } },
                ParameterTemplate { name: params::manual_ms.as_str(), kind: ParameterKind::Float { min: 0.3,  max: 8.0,  default: 2.0 } },
                ParameterTemplate { name: params::feedback.as_str(),  kind: ParameterKind::Float { min: -0.93, max: 0.93, default: 0.3 } },
                ParameterTemplate { name: params::mix.as_str(),       kind: ParameterKind::Float { min: 0.0,  max: 1.0,  default: 0.5 } },
                ParameterTemplate { name: params::lf_bypass.as_str(), kind: ParameterKind::Bool { default: true } },
                ParameterTemplate { name: params::jitter.as_str(),    kind: ParameterKind::Float { min: 0.0,  max: 1.0,  default: 0.0 } },
            ],
            structural_params: &[],
            per_axis_realtime_params: &[],
            per_axis_structural_params: &[],
        };
        T
    }

    fn prepare(
        env: &AudioEnvironment,
        descriptor: ModuleDescriptor,
        instance_id: InstanceId, _structural: &StructuralParams,
    ) -> Result<Self, BuildError> { Ok({
        Self {
            instance_id,
            descriptor,
            core: {
                let mut c = VFlangerStereoCore::new(env.sample_rate);
                // Per-module seed salt. Full registry lives in
                // `vflanger::core` module docstring.
                c.set_jitter_seed_base((instance_id.as_u64() ^ 0xBBD0_0030) as u32);
                c
            },
            in_stereo: StereoInput::default(),
            rate_cv: MonoInput::default(),
            depth_cv: MonoInput::default(),
            manual_cv: MonoInput::default(),
            fb_cv: MonoInput::default(),
            out_stereo: StereoOutput::default(),
        }
    })}

    fn update_validated_parameters(&mut self, p: &ParamView<'_>) {
        self.core.set_rate(p.get(params::rate_hz));
        self.core.set_depth(p.get(params::depth));
        self.core.set_manual(p.get(params::manual_ms));
        self.core.set_feedback(p.get(params::feedback));
        self.core.set_mix(p.get(params::mix));
        self.core.set_lf_bypass(p.get(params::lf_bypass));
        self.core.set_jitter(p.get(params::jitter));
    }

    fn descriptor(&self) -> &ModuleDescriptor {
        &self.descriptor
    }

    fn instance_id(&self) -> InstanceId {
        self.instance_id
    }

    fn set_ports(&mut self, inputs: &[InputPort], outputs: &[OutputPort]) {
        self.in_stereo = StereoInput::from_ports(inputs, 0);
        self.rate_cv = MonoInput::from_ports(inputs, 1);
        self.depth_cv = MonoInput::from_ports(inputs, 2);
        self.manual_cv = MonoInput::from_ports(inputs, 3);
        self.fb_cv = MonoInput::from_ports(inputs, 4);
        self.out_stereo = StereoOutput::from_ports(outputs, 0);
    }

    fn process(&mut self, pool: &mut CablePool<'_>) {
        let (l, r) = pool.read_stereo(&self.in_stereo);
        let both =
            self.in_stereo.is_connected() && !self.in_stereo.broadcast_from_mono;
        let ro = pool.read_mono(&self.rate_cv);
        let d = pool.read_mono(&self.depth_cv);
        let m = pool.read_mono(&self.manual_cv);
        let fb = pool.read_mono(&self.fb_cv);
        let (yl, yr) = self.core.process(l, r, both, ro, d, m, fb);
        pool.write_stereo(&self.out_stereo, yl, yr);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use patches_sdk::test_support::{params, ModuleHarness};
    use patches_sdk::{AudioEnvironment, ModuleShape};

    const SR: f32 = 48_000.0;
    const ENV: AudioEnvironment = AudioEnvironment {
        sample_rate: SR,
        poly_voices: 16,
        periodic_update_interval: 32,
        hosted: false,
    };

    fn shape() -> ModuleShape {
        ModuleShape { channels: 1 }
    }

    #[test]
    fn descriptor_shape() {
        let h = ModuleHarness::build::<VFlangerStereo>(&[]);
        let d = h.descriptor();
        assert_eq!(d.module_name, "VFlangerStereo");
        assert_eq!(d.inputs.len(), 5);
        assert_eq!(d.outputs.len(), 1);
    }

    #[test]
    fn l_r_decorrelate_under_modulation() {
        let mut h = ModuleHarness::build_full::<VFlangerStereo>(
            params![
                "rate_hz" => 1.0_f32,
                "depth" => 0.9_f32,
                "manual_ms" => 3.0_f32,
                "feedback" => 0.0_f32,
                "mix" => 0.5_f32,
                "lf_bypass" => false,
            ],
            ENV,
            shape(),
        );
        let n = (SR * 0.5) as usize;
        let mut l = Vec::with_capacity(n);
        let mut r = Vec::with_capacity(n);
        for i in 0..n {
            let t = i as f32 / SR;
            let x = 0.3 * (std::f32::consts::TAU * 440.0 * t).sin();
            h.set_stereo("in", x, x);
            h.tick();
            let (lo, ro) = h.read_stereo("out");
            l.push(lo);
            r.push(ro);
        }
        let ml = l.iter().sum::<f32>() / n as f32;
        let mr = r.iter().sum::<f32>() / n as f32;
        let (mut num, mut dl, mut dr) = (0.0_f32, 0.0_f32, 0.0_f32);
        for i in 0..n {
            let a = l[i] - ml;
            let b = r[i] - mr;
            num += a * b;
            dl += a * a;
            dr += b * b;
        }
        let c = num / (dl * dr).sqrt();
        assert!(c < 0.98, "L/R too correlated under deep modulation: {c}");
    }

    #[test]
    fn stable_at_high_feedback() {
        let mut h = ModuleHarness::build_full::<VFlangerStereo>(
            params![
                "rate_hz" => 0.5_f32,
                "depth" => 0.8_f32,
                "feedback" => 0.9_f32,
            ],
            ENV,
            shape(),
        );
        for i in 0..((SR * 0.5) as usize) {
            let t = i as f32 / SR;
            let x = 0.3 * (std::f32::consts::TAU * 220.0 * t).sin();
            h.set_stereo("in", x, x);
            h.tick();
            let (lo, ro) = h.read_stereo("out");
            assert!(lo.is_finite());
            assert!(ro.is_finite());
        }
    }
}
