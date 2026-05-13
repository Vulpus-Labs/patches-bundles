//! Convolution reverb module.
//!
//! Uses uniform partitioned overlap-save convolution running on a dedicated
//! processing thread. Defines [`ConvolutionReverb`] (mono) and
//! [`StereoConvReverb`] (stereo).
//!
//! # Inputs (ConvolutionReverb / mono)
//!
//! | Port | Kind | Description |
//! |------|------|-------------|
//! | `in` | mono | Audio input |
//! | `mix` | mono | Dry/wet CV (0--1 added to parameter) |
//!
//! # Outputs (ConvolutionReverb / mono)
//!
//! | Port | Kind | Description |
//! |------|------|-------------|
//! | `out` | mono | Audio output |
//!
//! # Inputs (StereoConvReverb / stereo)
//!
//! | Port | Kind | Description |
//! |------|------|-------------|
//! | `in` | stereo | Stereo audio input |
//! | `mix` | mono | Dry/wet CV (0--1 added to parameter) |
//!
//! # Outputs (StereoConvReverb / stereo)
//!
//! | Port | Kind | Description |
//! |------|------|-------------|
//! | `out` | stereo | Stereo audio output |
//!
//! # Parameters (both variants)
//!
//! | Name | Type | Range | Default | Description |
//! |------|------|-------|---------|-------------|
//! | `mix` | float | 0.0--1.0 | `1.0` | Dry/wet mix |
//! | `ir` | enum | room/hall/plate/file | `room` | Impulse response variant |
//! | `ir_path` | structural str | -- | `""` | File path for `ir: file` variant |
//!
//! # Real-time safety
//!
//! IR resolution (file I/O, synthetic generation), convolver construction, and
//! processing thread management all happen off the audio thread. On initial
//! build these run synchronously on the control thread (via `Module::prepare`
//! and `apply_unpacked_params`). For parameter updates to surviving modules
//! ([`update_validated_parameters`]), an [`ir_loader::IrLoader`] background
//! thread handles the heavy work. The audio thread only stashes a request
//! and polls for results in [`periodic_update`].

use patches_sdk::build_error::BuildError;
use patches_sdk::cable_pool::CablePool;
use patches_sdk::parameter_map::ParameterMap;
use patches_sdk::param_frame::ParamView;
use patches_sdk::{
    AudioEnvironment, InputPort, InstanceId,
    ModuleDescriptor, MonoInput, MonoOutput, OutputPort, ParameterKind,
};
use patches_sdk::modules::{CountAxis, ModuleDescriptorTemplate, ParameterTemplate, PortTemplate};
use patches_sdk::{StructuralParams};

use patches_fft_harness::partitioned_convolution::NonUniformConvolver;

mod core;
mod ir_loader;
mod params;
mod stereo;

pub use stereo::StereoConvReverb;

use core::ConvReverbCore;
use core::params as core_params;
use params::{BLOCK_SIZE, IR_FILE_EXTENSIONS, IrVariant, MAX_TIER_BLOCK_SIZE};

// ---------------------------------------------------------------------------
// Module: ConvolutionReverb (Mono)
// ---------------------------------------------------------------------------

/// Mono convolution reverb.
///
/// See [module-level documentation](self) for port and parameter tables.
pub struct ConvolutionReverb {
    instance_id: InstanceId,
    descriptor: ModuleDescriptor,

    // Ports
    in_audio: MonoInput,
    in_mix: MonoInput,
    out_audio: MonoOutput,

    core: ConvReverbCore,
    /// Pre-decoded IR loaded from the `ir_path` structural param (if any).
    /// `apply_unpacked_params` consumes this on first call to install the
    /// convolver synchronously.
    pre_fft_ir: Option<Vec<f32>>,
    /// Sticky witness: `true` iff `prepare` decoded a non-empty IR from the
    /// `ir_path` structural param. Survives `apply_unpacked_params` so
    /// integration tests can assert the structural pipeline reached
    /// `Module::prepare` (ADR 0060, ticket 0746).
    prepared_with_ir_path: bool,
}

// SAFETY: see ConvReverbCore.
unsafe impl Send for ConvolutionReverb {}

impl ConvolutionReverb {
    /// Test/observation hook (ADR 0060): `true` when `prepare` decoded a
    /// non-empty IR from the `ir_path` structural param. Sticky — set once
    /// at `prepare` time, survives `apply_unpacked_params`.
    pub fn prepared_with_ir_path(&self) -> bool {
        self.prepared_with_ir_path
    }
}

impl patches_sdk::Module for ConvolutionReverb {
    fn template() -> ModuleDescriptorTemplate {
        const T: ModuleDescriptorTemplate = ModuleDescriptorTemplate {
            name: "ConvReverb",
            axes: &[CountAxis::CHANNELS],
            global_inputs: &[
                PortTemplate::mono("in"),
                PortTemplate::mono("mix"),
            ],
            per_axis_inputs: &[],
            global_outputs: &[PortTemplate::mono("out")],
            per_axis_outputs: &[],
            realtime_params: &[
                ParameterTemplate {
                    name: core_params::mix.as_str(),
                    kind: ParameterKind::Float { min: 0.0, max: 1.0, default: 1.0 },
                },
                ParameterTemplate {
                    name: core_params::ir.as_str(),
                    kind: ParameterKind::Enum { variants: IrVariant::VARIANTS, default: "room" },
                },
            ],
            structural_params: &[
                ParameterTemplate {
                    name: "ir_path",
                    kind: ParameterKind::File { extensions: IR_FILE_EXTENSIONS },
                },
            ],
            per_axis_realtime_params: &[],
            per_axis_structural_params: &[],
        };
        T
    }

    fn prepare(
        audio_environment: &AudioEnvironment,
        descriptor: ModuleDescriptor,
        instance_id: InstanceId,
        structural: &StructuralParams,
    ) -> Result<Self, BuildError> {
        let pre_fft_ir = match structural.get_string("ir_path", 0) {
            Some(p) if !p.is_empty() => {
                let samples = patches_io::read_mono(
                    std::path::Path::new(p),
                    audio_environment.sample_rate as f64,
                )
                .map_err(|e| BuildError::Custom {
                    module: "ConvReverb",
                    message: format!("failed to load '{p}': {e}"),
                    origin: None,
                })?;
                Some(NonUniformConvolver::serialize_pre_fft(
                    &samples, BLOCK_SIZE, MAX_TIER_BLOCK_SIZE,
                ))
            }
            _ => None,
        };
        let prepared_with_ir_path = pre_fft_ir.as_ref().is_some_and(|v| !v.is_empty());
        Ok(Self {
            instance_id,
            descriptor,
            in_audio: MonoInput::default(),
            in_mix: MonoInput::default(),
            out_audio: MonoOutput::default(),
            core: ConvReverbCore::new(false, audio_environment.sample_rate),
            pre_fft_ir,
            prepared_with_ir_path,
        })
    }

    fn apply_unpacked_params(&mut self, params: &ParameterMap) -> Result<(), BuildError> {
        self.core.update_parameters(params, "ConvReverb", self.pre_fft_ir.take())
    }

    fn update_validated_parameters(&mut self, params: &ParamView<'_>) {
        self.core.update_validated_parameters(params);
    }

    fn descriptor(&self) -> &ModuleDescriptor {
        &self.descriptor
    }

    fn instance_id(&self) -> InstanceId {
        self.instance_id
    }

    fn set_ports(&mut self, inputs: &[InputPort], outputs: &[OutputPort]) {
        self.in_audio = MonoInput::from_ports(inputs, 0);
        self.in_mix = MonoInput::from_ports(inputs, 1);
        self.out_audio = MonoOutput::from_ports(outputs, 0);
    }

    fn process(&mut self, pool: &mut CablePool<'_>) {
        let input = pool.read_mono(&self.in_audio);
        if let Some(ref mut overlap_buffer) = self.core.overlap_buffers[0] {
            overlap_buffer.write(input);
            let output = overlap_buffer.read();
            pool.write_mono(&self.out_audio, output);
        } else {
            // Passthrough if processor not yet started.
            pool.write_mono(&self.out_audio, input);
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn wants_periodic(&self) -> bool { true }

    fn periodic_update(&mut self, pool: &CablePool<'_>) {
        self.core.poll_loader();

        if self.in_mix.is_connected() {
            let mix_cv = pool.read_mono(&self.in_mix);
            self.core.update_shared_mix(mix_cv);
        }
    }
}

#[cfg(test)]
mod tests;
