//! Stereo convolution reverb module.

use std::sync::Arc;

use patches_sdk::build_error::BuildError;
use patches_sdk::cable_pool::CablePool;
use patches_sdk::parameter_map::ParameterMap;
use patches_sdk::param_frame::ParamView;
use patches_sdk::{
    AudioEnvironment, InputPort, InstanceId,
    ModuleDescriptor, MonoInput, OutputPort, ParameterKind, StereoInput, StereoOutput,
};
use patches_sdk::modules::{CountAxis, ModuleDescriptorTemplate, ParameterTemplate, PortTemplate};
use patches_sdk::{StructuralParams};

use patches_fft_harness::partitioned_convolution::NonUniformConvolver;

use super::params::{
    self, BLOCK_SIZE, IR_FILE_EXTENSIONS, IrVariant, MAX_TIER_BLOCK_SIZE, SharedParams,
};
use super::core::params as core_params;
use super::processor::{build_processor, ProcessorKit};
use super::{read_ir_variant, ConvReverbCore};

/// Stereo convolution reverb -- two independent convolvers (L/R) sharing
/// parameters. Stereo impulse files use left/right channels directly; mono
/// impulse files duplicate to both channels. Synthetic IRs use decorrelated
/// noise per channel for natural stereo width.
///
/// See [module-level documentation](super) for port and parameter tables.
pub struct StereoConvReverb {
    pub(super) instance_id: InstanceId,
    pub(super) descriptor: ModuleDescriptor,

    // Ports
    pub(super) in_stereo: StereoInput,
    pub(super) in_mix: MonoInput,
    pub(super) out_stereo: StereoOutput,

    pub(super) core: ConvReverbCore,
}

// SAFETY: see ConvolutionReverb (super).
unsafe impl Send for StereoConvReverb {}

impl patches_sdk::Module for StereoConvReverb {
    fn template() -> ModuleDescriptorTemplate {
        const T: ModuleDescriptorTemplate = ModuleDescriptorTemplate {
            name: "StereoConvReverb",
            axes: &[CountAxis::CHANNELS],
            global_inputs: &[
                PortTemplate::stereo("in"),
                PortTemplate::mono("mix"),
            ],
            per_axis_inputs: &[],
            global_outputs: &[PortTemplate::stereo("out")],
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

        let ir_pair: Option<(Vec<f32>, Vec<f32>)> = match (variant, ir_path) {
            (IrVariant::File, Some(path)) => Some(
                patches_io::read_stereo(
                    std::path::Path::new(path),
                    sample_rate as f64,
                )
                .map_err(|e| BuildError::Custom {
                    module: "StereoConvReverb",
                    message: format!("failed to load '{path}': {e}"),
                    origin: None,
                })?,
            ),
            (IrVariant::File, None) => None,
            (v, _) => Some(params::generate_stereo_variant_ir(v, sample_rate)
                .expect("non-File variant always synthesises an IR pair")),
        };

        let shared = Arc::new(SharedParams::new());
        let kits: Vec<Option<ProcessorKit>> = match ir_pair {
            Some((ir_l, ir_r)) => {
                let conv_l = NonUniformConvolver::new(&ir_l, BLOCK_SIZE, MAX_TIER_BLOCK_SIZE);
                let conv_r = NonUniformConvolver::new(&ir_r, BLOCK_SIZE, MAX_TIER_BLOCK_SIZE);
                vec![
                    Some(build_processor(conv_l, Arc::clone(&shared), "patches-conv-reverb-l")),
                    Some(build_processor(conv_r, Arc::clone(&shared), "patches-conv-reverb-r")),
                ]
            }
            None => vec![None, None],
        };

        Ok(Self {
            instance_id,
            descriptor,
            in_stereo: StereoInput::default(),
            in_mix: MonoInput::default(),
            out_stereo: StereoOutput::default(),
            core: ConvReverbCore::from_kits(kits, shared, 1.0),
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
        self.in_stereo = StereoInput::from_ports(inputs, 0);
        self.in_mix = MonoInput::from_ports(inputs, 1);
        self.out_stereo = StereoOutput::from_ports(outputs, 0);
    }

    fn process(&mut self, pool: &mut CablePool<'_>) {
        let (input_l, input_r) = pool.read_stereo(&self.in_stereo);

        let out_l = match &mut self.core.kits[0] {
            Some(kit) => { kit.overlap_buffer.write(input_l); kit.overlap_buffer.read() }
            None => input_l,
        };
        let out_r = match &mut self.core.kits[1] {
            Some(kit) => { kit.overlap_buffer.write(input_r); kit.overlap_buffer.read() }
            None => input_r,
        };

        pool.write_stereo(&self.out_stereo, out_l, out_r);
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
