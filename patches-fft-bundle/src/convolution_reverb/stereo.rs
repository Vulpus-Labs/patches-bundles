//! Stereo convolution reverb module.

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

use super::params::{BLOCK_SIZE, IR_FILE_EXTENSIONS, IrVariant, MAX_TIER_BLOCK_SIZE};
use super::core::params as core_params;
use super::ConvReverbCore;

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
    /// Pre-decoded IR loaded from the `ir_path` structural param (if any).
    pub(super) pre_fft_ir: Option<Vec<f32>>,
}

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
                let (left, right) = patches_io::read_stereo(
                    std::path::Path::new(p),
                    audio_environment.sample_rate as f64,
                )
                .map_err(|e| BuildError::Custom {
                    module: "StereoConvReverb",
                    message: format!("failed to load '{p}': {e}"),
                    origin: None,
                })?;
                let left_pre =
                    NonUniformConvolver::serialize_pre_fft(&left, BLOCK_SIZE, MAX_TIER_BLOCK_SIZE);
                let right_pre =
                    NonUniformConvolver::serialize_pre_fft(&right, BLOCK_SIZE, MAX_TIER_BLOCK_SIZE);
                let mut packed = Vec::with_capacity(1 + left_pre.len() + right_pre.len());
                packed.push(left_pre.len() as f32);
                packed.extend_from_slice(&left_pre);
                packed.extend_from_slice(&right_pre);
                Some(packed)
            }
            _ => None,
        };
        Ok(Self {
            instance_id,
            descriptor,
            in_stereo: StereoInput::default(),
            in_mix: MonoInput::default(),
            out_stereo: StereoOutput::default(),
            core: ConvReverbCore::new(true, audio_environment.sample_rate),
            pre_fft_ir,
        })
    }

    fn apply_unpacked_params(&mut self, params: &ParameterMap) -> Result<(), BuildError> {
        self.core.update_parameters(params, "StereoConvReverb", self.pre_fft_ir.take())
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

        let out_l = if let Some(ref mut ol) = self.core.overlap_buffers[0] {
            ol.write(input_l);
            ol.read()
        } else {
            input_l
        };

        let out_r = if let Some(ref mut or) = self.core.overlap_buffers[1] {
            or.write(input_r);
            or.read()
        } else {
            input_r
        };

        pool.write_stereo(&self.out_stereo, out_l, out_r);
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

