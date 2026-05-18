//! Vintage BBD flanger module (Boss BF-2B reference).
//!
//! Single 1024-stage BBD ([`crate::bbd::Bbd`] with the
//! [`crate::bbd::BbdDevice::BBD_1024`] preset, matching the stage count of a
//! BF-2B-class low-voltage BBD)
//! bracketed by an NE570-style compander, modulated by a triangle LFO.
//! A switchable lowpass bypass (~150 Hz) keeps bass fundamentals out of
//! the comb — the defining BF-2B trait versus a plain BF-2.
//!
//! Signal flow:
//!
//! ```text
//!   in ─┬─── LPF(150 Hz) ────────────────────────────┐
//!       └─> HPF ─┬─> compressor ─> BBD ─> expander ─> LPF ─> wet
//!                └────────────────────────────────────┘  │
//!                      (feedback path, signed)           │
//!   out = lf + 0.5 * (hf_dry + wet)  ←─────────────────── │
//! ```
//!
//! # Inputs
//!
//! | Port | Kind | Description |
//! |------|------|-------------|
//! | `in` | mono | Audio input |
//! | `rate_cv` | mono | Additive CV offset for LFO rate |
//! | `depth_cv` | mono | Additive CV offset for sweep depth |
//! | `manual_cv` | mono | Additive CV (ms) for centre delay |
//! | `feedback_cv` | mono | Additive CV for resonance/feedback |
//!
//! # Outputs
//!
//! | Port | Kind | Description |
//! |------|------|-------------|
//! | `out` | mono | Flanged output |
//!
//! # Parameters
//!
//! | Name | Type | Range | Default | Description |
//! |------|------|-------|---------|-------------|
//! | `rate_hz` | float | 0.05--12.0 | `0.5` | Triangle LFO rate |
//! | `depth` | float | 0.0--1.0 | `0.5` | Sweep depth around centre |
//! | `manual_ms` | float | 0.3--8.0 | `2.0` | Centre delay in ms |
//! | `feedback` | float | -0.93--0.93 | `0.3` | Resonance (signed; negative inverts the comb) |
//! | `mix` | float | 0.0--1.0 | `0.5` | Dry/wet balance on the HF path; `0.5` is the classic flanger comb |
//! | `lf_bypass` | bool | on/off | `on` | BF-2B low-frequency bypass (BBD path is always HPF'd at 150 Hz) |

use patches_sdk::module_params;
use patches_sdk::param_frame::ParamView;
use patches_sdk::{
    AudioEnvironment, CablePool, CountAxis, InputPort, InstanceId, Module, ModuleDescriptor,
    ModuleDescriptorTemplate, MonoInput, MonoOutput, OutputPort, ParameterKind,
    ParameterTemplate, PortTemplate,
};
use patches_sdk::{StructuralParams, BuildError};

mod core;

#[cfg(test)]
mod tests;

pub use self::core::VFlangerCore;

module_params! {
    VFlanger {
        rate_hz:   Float,
        depth:     Float,
        manual_ms: Float,
        feedback:  Float,
        mix:       Float,
        lf_bypass: Bool,
        jitter:    Float,
    }
}

pub struct VFlanger {
    instance_id: InstanceId,
    descriptor: ModuleDescriptor,
    core: VFlangerCore,

    in_port: MonoInput,
    rate_cv: MonoInput,
    depth_cv: MonoInput,
    manual_cv: MonoInput,
    fb_cv: MonoInput,
    out_port: MonoOutput,
}

impl Module for VFlanger {
    fn template() -> ModuleDescriptorTemplate {
        const T: ModuleDescriptorTemplate = ModuleDescriptorTemplate {
            name: "VFlanger",
            axes: &[CountAxis::CHANNELS],
            global_inputs: &[
                PortTemplate::mono("in"),
                PortTemplate::mono("rate_cv"),
                PortTemplate::mono("depth_cv"),
                PortTemplate::mono("manual_cv"),
                PortTemplate::mono("feedback_cv"),
            ],
            per_axis_inputs: &[],
            global_outputs: &[PortTemplate::mono("out")],
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
                let mut c = VFlangerCore::new(env.sample_rate);
                // Per-module seed salt — keep distinct from sibling BBD-based
                // modules so two instances with the same `instance_id`
                // (different modules) decorrelate their jitter streams.
                //   0x0001 vbbd, 0x0010 vchorus, 0x0020 vstereobbd,
                //   0x0030 vflanger_stereo, 0x0040 vreverb, 0x0050 vflanger.
                c.set_jitter_seed((instance_id.as_u64() ^ 0xBBD0_0050) as u32);
                c
            },
            in_port: MonoInput::default(),
            rate_cv: MonoInput::default(),
            depth_cv: MonoInput::default(),
            manual_cv: MonoInput::default(),
            fb_cv: MonoInput::default(),
            out_port: MonoOutput::default(),
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
        self.in_port = MonoInput::from_ports(inputs, 0);
        self.rate_cv = MonoInput::from_ports(inputs, 1);
        self.depth_cv = MonoInput::from_ports(inputs, 2);
        self.manual_cv = MonoInput::from_ports(inputs, 3);
        self.fb_cv = MonoInput::from_ports(inputs, 4);
        self.out_port = MonoOutput::from_ports(outputs, 0);
    }

    fn process(&mut self, pool: &mut CablePool<'_>) {
        let x = pool.read_mono(&self.in_port);
        let r = pool.read_mono(&self.rate_cv);
        let d = pool.read_mono(&self.depth_cv);
        let m = pool.read_mono(&self.manual_cv);
        let fb = pool.read_mono(&self.fb_cv);
        let y = self.core.process(x, r, d, m, fb);
        pool.write_mono(&self.out_port, y);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
