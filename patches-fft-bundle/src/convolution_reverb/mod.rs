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
//! | `mix` | float | 0.0--1.0 | `1.0` | Dry/wet mix (realtime) |
//! | `ir` | structural enum | room/hall/plate/file | `room` | Impulse response variant |
//! | `ir_path` | structural str | -- | `""` | File path for `ir: file` variant |
//!
//! # Real-time safety
//!
//! IR resolution (file I/O, synthetic generation), convolver construction,
//! and processing-thread spawn all happen on the control thread inside
//! [`Module::prepare`]. Both `ir` and `ir_path` are structural (ADR 0060),
//! so any change rebuilds the module via the planner — there is no
//! audio-thread reload path. The audio thread only reads/writes overlap
//! buffers and updates the `mix` atomic.

use std::sync::Arc;

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
mod params;
mod processor;
mod stereo;

pub use stereo::StereoConvReverb;

use core::ConvReverbCore;
use core::params as core_params;
use params::{BLOCK_SIZE, IR_FILE_EXTENSIONS, IrVariant, MAX_TIER_BLOCK_SIZE, SharedParams};
use processor::build_processor;

// ---------------------------------------------------------------------------
// Shared structural-param resolution
// ---------------------------------------------------------------------------

/// Read the `ir` structural enum out of a `StructuralParams` blob. Falls
/// back to the descriptor default (`Room`) on missing or out-of-range.
pub(super) fn read_ir_variant(structural: &StructuralParams) -> IrVariant {
    structural
        .get_int("ir", 0)
        .and_then(|i| u32::try_from(i).ok())
        .and_then(|i| IrVariant::try_from(i).ok())
        .unwrap_or(IrVariant::Room)
}

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
    /// Sticky witness: `true` iff `prepare` decoded a non-empty IR from the
    /// `ir_path` structural param. Survives the lifetime of the module so
    /// integration tests can assert the structural pipeline reached
    /// `Module::prepare` (ADR 0060, ticket 0746).
    prepared_with_ir_path: bool,
}

// SAFETY: ConvolutionReverb is constructed on the control thread and sent
// once to the audio thread (via Module: Send), where it remains for its
// lifetime. `OverlapBuffer` (inside `ProcessorKit`) is `!Send` as a lint
// against casual cross-thread use; single ownership transfer at plan
// activation is safe.
unsafe impl Send for ConvolutionReverb {}

impl ConvolutionReverb {
    /// Test/observation hook (ADR 0060): `true` when `prepare` decoded a
    /// non-empty IR from the `ir_path` structural param.
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
            ],
            structural_params: &[
                ParameterTemplate {
                    name: core_params::ir.as_str(),
                    kind: ParameterKind::Enum { variants: IrVariant::VARIANTS, default: "room" },
                },
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
        let sample_rate = audio_environment.sample_rate;
        let variant = read_ir_variant(structural);
        let ir_path = structural.get_string("ir_path", 0).filter(|p| !p.is_empty());
        let prepared_with_ir_path = ir_path.is_some();

        let ir_samples: Option<Vec<f32>> = match (variant, ir_path) {
            (IrVariant::File, Some(path)) => Some(
                patches_io::read_mono(
                    std::path::Path::new(path),
                    sample_rate as f64,
                )
                .map_err(|e| BuildError::Custom {
                    module: "ConvReverb",
                    message: format!("failed to load '{path}': {e}"),
                    origin: None,
                })?,
            ),
            (IrVariant::File, None) => None,
            (v, _) => Some(params::generate_variant_ir(v, sample_rate)
                .expect("non-File variant always synthesises an IR")),
        };

        let shared = Arc::new(SharedParams::new());
        let kit = ir_samples.map(|ir| {
            let convolver = NonUniformConvolver::new(&ir, BLOCK_SIZE, MAX_TIER_BLOCK_SIZE);
            build_processor(convolver, Arc::clone(&shared), "patches-conv-reverb")
        });

        Ok(Self {
            instance_id,
            descriptor,
            in_audio: MonoInput::default(),
            in_mix: MonoInput::default(),
            out_audio: MonoOutput::default(),
            core: ConvReverbCore::from_kits(vec![kit], shared, 1.0),
            prepared_with_ir_path,
        })
    }

    fn apply_unpacked_params(&mut self, params: &ParameterMap) -> Result<(), BuildError> {
        if let Some(patches_sdk::parameter_map::ParameterValue::Float(v)) = params.get("mix", 0) {
            self.core.set_base_mix(*v);
        }
        Ok(())
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
        let output = match &mut self.core.kits[0] {
            Some(kit) => {
                kit.overlap_buffer.write(input);
                kit.overlap_buffer.read()
            }
            None => input,
        };
        pool.write_mono(&self.out_audio, output);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn wants_periodic(&self) -> bool { true }

    fn periodic_update(&mut self, pool: &CablePool<'_>) {
        if self.in_mix.is_connected() {
            let mix_cv = pool.read_mono(&self.in_mix);
            self.core.update_shared_mix(mix_cv);
        }
    }
}

#[cfg(test)]
mod tests;
