//! `VPolyLadder` — 16-voice polyphonic sibling of [`crate::vladder::VLadder`].
//!
//! Shares the ladder kernel with the mono version; wrapper differs
//! only in port kind and per-voice state fan-out.
//!
//! Module plumbing lives in [`crate::vintage_filter`] (shared with the
//! other three ladder/VCF modules).
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
    ModuleDescriptorTemplate, OutputPort, ParameterKind, ParameterTemplate, PortTemplate,
};
use patches_sdk::{StructuralParams, BuildError};
use patches_dsp::{LadderVariant, PolyLadderKernel};

use crate::vintage_filter::{
    VintageVcfPolyCore, CUTOFF_MAX, CUTOFF_MIN, DRIVE_MAX,
};
use crate::vladder::VLadderVariant;

module_params! {
    VPolyLadderParams {
        variant:   Enum<VLadderVariant>,
        cutoff:    Float,
        resonance: Float,
        drive:     Float,
    }
}

pub struct VPolyLadder {
    instance_id: InstanceId,
    descriptor: ModuleDescriptor,
    core: VintageVcfPolyCore<PolyLadderKernel>,
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

    fn prepare(
        env: &AudioEnvironment,
        descriptor: ModuleDescriptor,
        instance_id: InstanceId,
        _structural: &StructuralParams,
    ) -> Result<Self, BuildError> {
        Ok(Self {
            instance_id,
            descriptor,
            core: VintageVcfPolyCore::new(
                env,
                false,
                LadderVariant::from(VLadderVariant::Smooth),
                1_000.0,
                0.0,
                1.0,
            ),
        })
    }

    fn update_validated_parameters(&mut self, p: &ParamView<'_>) {
        let variant: VLadderVariant = p.get(params::variant);
        self.core.set_params(
            variant.into(),
            p.get(params::cutoff).clamp(CUTOFF_MIN, CUTOFF_MAX),
            p.get(params::resonance).clamp(0.0, 1.0),
            p.get(params::drive).clamp(0.0, DRIVE_MAX),
            0.0,
        );
    }

    fn descriptor(&self) -> &ModuleDescriptor {
        &self.descriptor
    }

    fn instance_id(&self) -> InstanceId {
        self.instance_id
    }

    fn set_ports(&mut self, inputs: &[InputPort], outputs: &[OutputPort]) {
        self.core.set_ports(inputs, outputs);
    }

    fn process(&mut self, pool: &mut CablePool<'_>) {
        self.core.process(pool);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn wants_periodic(&self) -> bool { true }

    fn periodic_update(&mut self, pool: &CablePool<'_>) {
        self.core.periodic_update(pool);
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
