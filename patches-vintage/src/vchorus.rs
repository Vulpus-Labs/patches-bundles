//! Stereo vintage BBD chorus module.
//!
//! Two BBD delay lines ([`crate::bbd::Bbd`] with the 256-stage preset) fed
//! from a mono sum of the left/right inputs, modulated by a shared
//! triangle LFO. The right-channel modulation is the inverse of the
//! left, reproducing the mono-compatibility trick used by the Juno-60
//! / Juno-106 hardware references.
//!
//! Two voicings are exposed as the `variant` parameter:
//!
//! - `bright` (Juno-60 reference): three modes (`one`, `two`, `both`),
//!   ~9 kHz reconstruction LPF, hotter wet level, `off` fully bypasses.
//! - `dark` (Juno-106 reference): two modes (`one`, `two`), ~7 kHz
//!   reconstruction LPF, matched wet level, `off` passes through the
//!   BBD with zero LFO depth. Selecting `both` falls back to `two`.
//!
//! # Inputs
//!
//! | Port | Kind | Description |
//! |------|------|-------------|
//! | `in` | stereo | Stereo audio input |
//! | `rate_cv` | mono | Additive CV offset for LFO rate |
//! | `depth_cv` | mono | Additive CV offset for LFO depth |
//!
//! # Outputs
//!
//! | Port | Kind | Description |
//! |------|------|-------------|
//! | `out` | stereo | Stereo output (dry + wet) |
//!
//! # Parameters
//!
//! | Name | Type | Range | Default | Description |
//! |------|------|-------|---------|-------------|
//! | `variant` | enum | `bright`/`dark` | `bright` | Voicing |
//! | `mode` | enum | `off`/`one`/`two`/`both` | `one` | Chorus mode (`both` is bright-only; on `dark` it coerces to mode II) |
//! | `hiss` | float | 0.0--1.0 | `1.0` | Wet-path hiss amount |

use patches_sdk::module_params;
use patches_sdk::param_frame::ParamView;
use patches_sdk::{
    AudioEnvironment, CablePool, CountAxis, InputPort, InstanceId, Module, ModuleDescriptor,
    ModuleDescriptorTemplate, MonoInput, OutputPort, ParameterKind,
    ParameterTemplate, PortTemplate, StereoInput, StereoOutput,
};
use patches_sdk::{StructuralParams, BuildError};

mod core;

#[cfg(test)]
mod tests;

pub use self::core::{Mode, VChorusCore, Variant};

module_params! {
    VChorus {
        variant: Enum<Variant>,
        mode:    Enum<Mode>,
        hiss:    Float,
        jitter:  Float,
    }
}

/// Vintage BBD chorus. See the module-level documentation.
pub struct VChorus {
    instance_id: InstanceId,
    descriptor: ModuleDescriptor,
    core: VChorusCore,

    in_stereo: StereoInput,
    rate_cv: MonoInput,
    depth_cv: MonoInput,
    out_stereo: StereoOutput,
}

impl Module for VChorus {
    fn template() -> ModuleDescriptorTemplate {
        const T: ModuleDescriptorTemplate = ModuleDescriptorTemplate {
            name: "VChorus",
            axes: &[CountAxis::CHANNELS],
            global_inputs: &[
                PortTemplate::stereo("in"),
                PortTemplate::mono("rate_cv"),
                PortTemplate::mono("depth_cv"),
            ],
            per_axis_inputs: &[],
            global_outputs: &[PortTemplate::stereo("out")],
            per_axis_outputs: &[],
            realtime_params: &[
                ParameterTemplate {
                    name: params::variant.as_str(),
                    kind: ParameterKind::Enum { variants: Variant::VARIANTS, default: "bright" },
                },
                ParameterTemplate {
                    name: params::mode.as_str(),
                    kind: ParameterKind::Enum { variants: Mode::VARIANTS, default: "one" },
                },
                ParameterTemplate {
                    name: params::hiss.as_str(),
                    kind: ParameterKind::Float { min: 0.0, max: 1.0, default: 1.0 },
                },
                ParameterTemplate {
                    name: params::jitter.as_str(),
                    kind: ParameterKind::Float { min: 0.0, max: 1.0, default: 0.0 },
                },
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
            // xorshift64 requires a non-zero seed.
            core: {
                let mut c = VChorusCore::new(env.sample_rate, instance_id.as_u64().wrapping_add(1));
                c.set_jitter_seed_base((instance_id.as_u64() ^ 0xBBD0_0010) as u32);
                c
            },
            in_stereo: StereoInput::default(),
            rate_cv: MonoInput::default(),
            depth_cv: MonoInput::default(),
            out_stereo: StereoOutput::default(),
        }
    })}

    fn update_validated_parameters(&mut self, p: &ParamView<'_>) {
        self.core.set_variant(p.get(params::variant));
        self.core.set_mode(p.get(params::mode));
        self.core.set_hiss(p.get(params::hiss));
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
        self.out_stereo = StereoOutput::from_ports(outputs, 0);
    }

    fn process(&mut self, pool: &mut CablePool<'_>) {
        let (l_in, r_in) = pool.read_stereo(&self.in_stereo);
        // The Juno-106 mono-compatibility trick depends on whether the two
        // halves carry distinct material. Mono-source broadcast signals this
        // explicitly; otherwise treat any connected stereo cable as "stereo".
        let both_connected =
            self.in_stereo.is_connected() && !self.in_stereo.broadcast_from_mono;
        let rate_offset = pool.read_mono(&self.rate_cv);
        let depth_offset = pool.read_mono(&self.depth_cv);

        let (ol, or) = self
            .core
            .process(l_in, r_in, both_connected, rate_offset, depth_offset);
        pool.write_stereo(&self.out_stereo, ol, or);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
