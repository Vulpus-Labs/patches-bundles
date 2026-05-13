//! `VPolyLadder` — 16-voice polyphonic sibling of [`crate::vladder::VLadder`].
//!
//! Shares the ladder kernel with the mono version; wrapper differs
//! only in port kind and per-voice state fan-out.
//!
//! # Inputs
//!
//! | Port        | Kind | Description                             |
//! |-------------|------|-----------------------------------------|
//! | `in`        | poly | Audio input per voice                   |
//! | `cutoff_cv` | poly | V/oct offset added to `cutoff` per voice |
//!
//! # Outputs
//!
//! | Port  | Kind | Description              |
//! |-------|------|--------------------------|
//! | `out` | poly | Filtered signal per voice |
//!
//! # Parameters
//!
//! | Name        | Type  | Range              | Default   | Description                      |
//! |-------------|-------|--------------------|-----------|----------------------------------|
//! | `variant`   | enum  | `sharp`/`smooth`   | `smooth`  | Filter voicing (shared)          |
//! | `cutoff`    | float | 20.0 -- 20000.0    | `1000.0`  | Base cutoff in Hz                |
//! | `resonance` | float | 0.0 -- 1.0         | `0.0`     | Feedback amount; self-osc near 1 |
//! | `drive`     | float | 0.0 -- 4.0         | `1.0`     | Input gain into the tanh stage   |

use patches_sdk::module_params;
use patches_sdk::param_frame::ParamView;
use patches_sdk::{
    AudioEnvironment, CablePool, CountAxis, InputPort, InstanceId, Module, ModuleDescriptor,
    ModuleDescriptorTemplate, OutputPort, ParameterKind, ParameterTemplate, PolyInput,
    PolyOutput, PortTemplate,
};
use patches_sdk::{StructuralParams, BuildError};
use patches_dsp::{LadderCoeffs, LadderVariant, PolyLadderKernel};

use crate::vladder::VLadderVariant;

module_params! {
    VPolyLadderParams {
        variant:   Enum<VLadderVariant>,
        cutoff:    Float,
        resonance: Float,
        drive:     Float,
    }
}

const CUTOFF_MIN: f32 = 20.0;
const CUTOFF_MAX: f32 = 20_000.0;
const DRIVE_MAX: f32 = 4.0;

pub struct VPolyLadder {
    instance_id: InstanceId,
    descriptor: ModuleDescriptor,
    sample_rate: f32,
    interval_recip: f32,
    variant: VLadderVariant,
    cutoff: f32,
    resonance: f32,
    drive: f32,
    kernel: PolyLadderKernel,
    in_audio: PolyInput,
    in_cutoff_cv: PolyInput,
    out_audio: PolyOutput,
}

impl VPolyLadder {
    fn coeffs_for(&self, cv_voct: f32) -> LadderCoeffs {
        let eff = (self.cutoff * (2.0f32).powf(cv_voct)).clamp(CUTOFF_MIN, self.sample_rate * 0.45);
        let lv: LadderVariant = self.variant.into();
        LadderCoeffs::new(eff, self.sample_rate, self.resonance, self.drive, lv)
    }

    fn apply_static(&mut self) {
        let c = self.coeffs_for(0.0);
        self.kernel.set_static(c);
    }
}

impl Module for VPolyLadder {
    fn template() -> ModuleDescriptorTemplate {
        const T: ModuleDescriptorTemplate = ModuleDescriptorTemplate {
            name: "VPolyLadder",
            axes: &[CountAxis::CHANNELS],
            global_inputs: &[PortTemplate::poly("in"), PortTemplate::poly("cutoff_cv")],
            per_axis_inputs: &[],
            global_outputs: &[PortTemplate::poly("out")],
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
            kernel: PolyLadderKernel::new_static(coeffs),
            in_audio: PolyInput::default(),
            in_cutoff_cv: PolyInput::default(),
            out_audio: PolyOutput::default(),
        }
    })}

    fn update_validated_parameters(&mut self, p: &ParamView<'_>) {
        self.variant = p.get(params::variant);
        self.cutoff = p.get(params::cutoff).clamp(CUTOFF_MIN, CUTOFF_MAX);
        self.resonance = p.get(params::resonance).clamp(0.0, 1.0);
        self.drive = p.get(params::drive).clamp(0.0, DRIVE_MAX);
        self.kernel.set_variant(self.variant.into());
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
        self.in_audio = PolyInput::from_ports(inputs, 0);
        self.in_cutoff_cv = PolyInput::from_ports(inputs, 1);
        self.out_audio = PolyOutput::from_ports(outputs, 0);
        if !self.in_cutoff_cv.is_connected() {
            self.apply_static();
        }
    }

    fn process(&mut self, pool: &mut CablePool<'_>) {
        if !self.out_audio.is_connected() {
            return;
        }
        let audio = if self.in_audio.is_connected() {
            pool.read_poly(&self.in_audio)
        } else {
            [0.0f32; 16]
        };
        let ramp = self.in_cutoff_cv.is_connected();
        let out = self.kernel.tick_all(&audio, ramp);
        pool.write_poly(&self.out_audio, out);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn wants_periodic(&self) -> bool { true }

    fn periodic_update(&mut self, pool: &CablePool<'_>) {
        if !self.in_cutoff_cv.is_connected() {
            return;
        }
        let cv = pool.read_poly(&self.in_cutoff_cv);
        for (i, &v) in cv.iter().enumerate() {
            let c = self.coeffs_for(v);
            self.kernel.begin_ramp_voice(i, c, self.interval_recip);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use patches_sdk::test_support::{params, ModuleHarness};

    #[test]
    fn descriptor_shape() {
        let h = ModuleHarness::build::<VPolyLadder>(&[]);
        let d = h.descriptor();
        assert_eq!(d.module_name, "VPolyLadder");
        assert_eq!(d.inputs.len(), 2);
        assert_eq!(d.outputs.len(), 1);
    }

    #[test]
    fn stable_under_max_drive_and_resonance() {
        let mut h = ModuleHarness::build::<VPolyLadder>(params![
            "variant" => VLadderVariant::Sharp,
            "cutoff" => 2_000.0_f32,
            "resonance" => 1.0_f32,
            "drive" => DRIVE_MAX,
        ]);
        let mut input = [0.0f32; 16];
        for n in 0..4_096 {
            let v = if (n / 48) % 2 == 0 { 1.0 } else { -1.0 };
            for (i, slot) in input.iter_mut().enumerate() {
                *slot = v * (0.5 + 0.03 * i as f32);
            }
            h.set_poly("in", input);
            h.tick();
            let out = h.read_poly("out");
            for (i, y) in out.iter().enumerate() {
                assert!(y.is_finite(), "non-finite voice {i} at n={n}");
            }
        }
    }
}
