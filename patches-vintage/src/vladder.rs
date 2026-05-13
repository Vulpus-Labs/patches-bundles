//! `VLadder` — vintage 4-pole ZDF ladder low-pass with `sharp` / `smooth` voicings.
//!
//! Mono wrapper around [`patches_dsp::LadderKernel`]. The `cutoff_cv`
//! input carries a V/oct offset summed externally from envelopes, LFOs
//! and key-tracking; the module adds nothing on top.
//!
//! # Inputs
//!
//! | Port        | Kind | Description                          |
//! |-------------|------|--------------------------------------|
//! | `in`        | mono | Audio input                          |
//! | `cutoff_cv` | mono | V/oct offset added to base `cutoff`  |
//!
//! # Outputs
//!
//! | Port  | Kind | Description     |
//! |-------|------|-----------------|
//! | `out` | mono | Filtered signal |
//!
//! # Parameters
//!
//! | Name        | Type  | Range            | Default   | Description                      |
//! |-------------|-------|------------------|-----------|----------------------------------|
//! | `variant`   | enum  | `sharp`/`smooth` | `smooth` | Filter voicing                     |
//! | `cutoff`    | float | 20.0 -- 20000.0  | `1000.0`  | Base cutoff in Hz                |
//! | `resonance` | float | 0.0 -- 1.0       | `0.0`     | Feedback amount; self-osc near 1 |
//! | `drive`     | float | 0.0 -- 4.0       | `1.0`     | Input gain into the tanh stage   |

use patches_sdk::module_params;
use patches_sdk::param_frame::ParamView;
use patches_sdk::{
    params_enum, AudioEnvironment, CablePool, CountAxis, InputPort, InstanceId, Module,
    ModuleDescriptor, ModuleDescriptorTemplate, MonoInput, MonoOutput, OutputPort,
    ParameterKind, ParameterTemplate, PortTemplate,
};
use patches_sdk::{StructuralParams, BuildError};
use patches_dsp::{LadderCoeffs, LadderKernel, LadderVariant};

params_enum! {
    pub enum VLadderVariant {
        Sharp => "sharp",
        Smooth => "smooth",
    }
}

impl From<VLadderVariant> for LadderVariant {
    fn from(v: VLadderVariant) -> Self {
        match v {
            VLadderVariant::Sharp => LadderVariant::Sharp,
            VLadderVariant::Smooth => LadderVariant::Smooth,
        }
    }
}

module_params! {
    VLadderParams {
        variant:   Enum<VLadderVariant>,
        cutoff:    Float,
        resonance: Float,
        drive:     Float,
    }
}

const CUTOFF_MIN: f32 = 20.0;
const CUTOFF_MAX: f32 = 20_000.0;
const DRIVE_MAX: f32 = 4.0;

pub struct VLadder {
    instance_id: InstanceId,
    descriptor: ModuleDescriptor,
    sample_rate: f32,
    interval_recip: f32,
    variant: VLadderVariant,
    cutoff: f32,
    resonance: f32,
    drive: f32,
    kernel: LadderKernel,
    in_audio: MonoInput,
    in_cutoff_cv: MonoInput,
    out_audio: MonoOutput,
}

impl VLadder {
    fn coeffs(&self, cv_voct: f32) -> LadderCoeffs {
        let effective = (self.cutoff * (2.0f32).powf(cv_voct)).clamp(CUTOFF_MIN, self.sample_rate * 0.45);
        LadderCoeffs::new(effective, self.sample_rate, self.resonance, self.drive, self.variant.into())
    }

    fn apply_static(&mut self) {
        let c = self.coeffs(0.0);
        self.kernel.set_static(c);
    }
}

impl Module for VLadder {
    fn template() -> ModuleDescriptorTemplate {
        const T: ModuleDescriptorTemplate = ModuleDescriptorTemplate {
            name: "VLadder",
            axes: &[CountAxis::CHANNELS],
            global_inputs: &[PortTemplate::mono("in"), PortTemplate::mono("cutoff_cv")],
            per_axis_inputs: &[],
            global_outputs: &[PortTemplate::mono("out")],
            per_axis_outputs: &[],
            realtime_params: &[
                ParameterTemplate { name: params::variant.as_str(),   kind: ParameterKind::Enum { variants: VLadderVariant::VARIANTS, default: "smooth" } },
                ParameterTemplate { name: params::cutoff.as_str(),    kind: ParameterKind::Float { min: CUTOFF_MIN, max: CUTOFF_MAX, default: 1_000.0 } },
                ParameterTemplate { name: params::resonance.as_str(), kind: ParameterKind::Float { min: 0.0, max: 1.0, default: 0.0 } },
                ParameterTemplate { name: params::drive.as_str(),     kind: ParameterKind::Float { min: 0.0, max: DRIVE_MAX, default: 1.0 } },
            ],
            structural_params: &[],
            per_axis_realtime_params: &[],
            per_axis_structural_params: &[],
        };
        T
    }

    fn prepare(env: &AudioEnvironment, descriptor: ModuleDescriptor, instance_id: InstanceId, _structural: &StructuralParams) -> Result<Self, BuildError> { Ok({
        let variant = VLadderVariant::Smooth;
        let cutoff = 1_000.0;
        let resonance = 0.0;
        let drive = 1.0;
        let coeffs = LadderCoeffs::new(cutoff, env.sample_rate, resonance, drive, variant.into());
        Self {
            instance_id,
            descriptor,
            sample_rate: env.sample_rate,
            interval_recip: 1.0 / env.periodic_update_interval as f32,
            variant,
            cutoff,
            resonance,
            drive,
            kernel: LadderKernel::new_static(coeffs),
            in_audio: MonoInput::default(),
            in_cutoff_cv: MonoInput::default(),
            out_audio: MonoOutput::default(),
        }
    })}

    fn update_validated_parameters(&mut self, p: &ParamView<'_>) {
        self.variant = p.get(params::variant);
        self.cutoff = p.get(params::cutoff).clamp(CUTOFF_MIN, CUTOFF_MAX);
        self.resonance = p.get(params::resonance).clamp(0.0, 1.0);
        self.drive = p.get(params::drive).clamp(0.0, DRIVE_MAX);
        if !self.in_cutoff_cv.is_connected() {
            self.apply_static();
        }
    }

    fn descriptor(&self) -> &ModuleDescriptor {
        &self.descriptor
    }

    fn instance_id(&self) -> InstanceId {
        self.instance_id
    }

    fn set_ports(&mut self, inputs: &[InputPort], outputs: &[OutputPort]) {
        self.in_audio = MonoInput::from_ports(inputs, 0);
        self.in_cutoff_cv = MonoInput::from_ports(inputs, 1);
        self.out_audio = MonoOutput::from_ports(outputs, 0);
        if !self.in_cutoff_cv.is_connected() {
            self.apply_static();
        }
    }

    fn process(&mut self, pool: &mut CablePool<'_>) {
        let x = pool.read_mono(&self.in_audio);
        let y = self.kernel.tick(x);
        pool.write_mono(&self.out_audio, y);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn wants_periodic(&self) -> bool { true }

    fn periodic_update(&mut self, pool: &CablePool<'_>) {
        if !self.in_cutoff_cv.is_connected() {
            return;
        }
        let cv = pool.read_mono(&self.in_cutoff_cv);
        let c = self.coeffs(cv);
        self.kernel.begin_ramp(c, self.interval_recip);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use patches_sdk::test_support::{params, ModuleHarness};

    #[test]
    fn descriptor_shape() {
        let h = ModuleHarness::build::<VLadder>(&[]);
        let d = h.descriptor();
        assert_eq!(d.module_name, "VLadder");
        assert_eq!(d.inputs.len(), 2);
        assert_eq!(d.outputs.len(), 1);
    }

    #[test]
    fn self_oscillates_at_max_resonance() {
        let mut h = ModuleHarness::build::<VLadder>(params![
            "variant" => VLadderVariant::Sharp,
            "cutoff" => 500.0_f32,
            "resonance" => 1.0_f32,
            "drive" => 1.0_f32,
        ]);
        for _ in 0..128 {
            h.set_mono("in", 0.2);
            h.tick();
        }
        let mut peak = 0.0f32;
        for _ in 0..4_800 {
            h.set_mono("in", 0.0);
            h.tick();
            peak = peak.max(h.read_mono("out").abs());
        }
        assert!(peak > 0.05, "ladder failed to self-oscillate: peak={peak}");
    }

    #[test]
    fn stable_under_max_drive_and_resonance() {
        let mut h = ModuleHarness::build::<VLadder>(params![
            "variant" => VLadderVariant::Sharp,
            "cutoff" => 2_000.0_f32,
            "resonance" => 1.0_f32,
            "drive" => DRIVE_MAX,
        ]);
        for n in 0..8_192 {
            let x = if (n / 48) % 2 == 0 { 1.0 } else { -1.0 };
            h.set_mono("in", x);
            h.tick();
            let y = h.read_mono("out");
            assert!(y.is_finite(), "non-finite output at n={n}");
        }
    }
}
