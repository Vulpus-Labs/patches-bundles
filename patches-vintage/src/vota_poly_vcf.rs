//! `VOtaPolyVcf` — 16-voice polyphonic sibling of [`crate::vota_vcf::VOtaVcf`].
//!
//! Shares the OTA-ladder kernel with the mono version; wrapper differs
//! only in port kind and per-voice state fan-out. The `GLOBAL_DRIFT`
//! backplane slot is read once per periodic update and applied to every
//! voice — drift is globally correlated by design.
//!
//! Module plumbing lives in [`crate::vintage_filter`] (shared with the
//! other three ladder/VCF modules).
//!
//! # Inputs
//!
//! | Port        | Kind | Description                              |
//! |-------------|------|------------------------------------------|
//! | `in`        | poly | Audio input per voice                    |
//! | `cutoff_cv` | poly | V/oct offset added to `cutoff` per voice |
//!
//! # Outputs
//!
//! | Port  | Kind | Description               |
//! |-------|------|---------------------------|
//! | `out` | poly | Filtered signal per voice |
//!
//! # Parameters
//!
//! | Name           | Type  | Range            | Default  | Description                        |
//! |----------------|-------|------------------|----------|------------------------------------|
//! | `poles`        | enum  | `two`/`four`     | `four`   | Output slope (shared)              |
//! | `cutoff`       | float | 20.0 -- 20000.0  | `1000.0` | Base cutoff in Hz                  |
//! | `resonance`    | float | 0.0 -- 1.0       | `0.0`    | Feedback amount; self-osc near 1   |
//! | `drive`        | float | 0.0 -- 4.0       | `1.0`    | Input gain before stage tanh       |
//! | `drift_amount` | float | 0.0 -- 1.0       | `0.0`    | Scales `GLOBAL_DRIFT` into cutoff  |

use patches_sdk::module_params;
use patches_sdk::param_frame::ParamView;
use patches_sdk::{
    AudioEnvironment, CablePool, CountAxis, InputPort, InstanceId, Module, ModuleDescriptor,
    ModuleDescriptorTemplate, OutputPort, ParameterKind, ParameterTemplate, PortTemplate,
};
use patches_sdk::{StructuralParams, BuildError};
use patches_dsp::{OtaPoles, PolyOtaLadderKernel};

use crate::vintage_filter::{
    VintageVcfPolyCore, CUTOFF_MAX, CUTOFF_MIN, DRIVE_MAX,
};
use crate::vota_vcf::VOtaPoles;

module_params! {
    VOtaPolyVcfParams {
        poles:        Enum<VOtaPoles>,
        cutoff:       Float,
        resonance:    Float,
        drive:        Float,
        drift_amount: Float,
    }
}

pub struct VOtaPolyVcf {
    instance_id: InstanceId,
    descriptor: ModuleDescriptor,
    core: VintageVcfPolyCore<PolyOtaLadderKernel>,
}

impl Module for VOtaPolyVcf {
    fn template() -> ModuleDescriptorTemplate {
        const T: ModuleDescriptorTemplate = ModuleDescriptorTemplate {
            name: "VOtaPolyVcf",
            axes: &[CountAxis::CHANNELS],
            global_inputs: &[PortTemplate::poly("in"), PortTemplate::poly("cutoff_cv")],
            per_axis_inputs: &[],
            global_outputs: &[PortTemplate::poly("out")],
            per_axis_outputs: &[],
            realtime_params: &[
                ParameterTemplate { name: params::poles.as_str(),        kind: ParameterKind::Enum { variants: VOtaPoles::VARIANTS, default: "four" } },
                ParameterTemplate { name: params::cutoff.as_str(),       kind: ParameterKind::Float { min: CUTOFF_MIN, max: CUTOFF_MAX, default: 1_000.0 } },
                ParameterTemplate { name: params::resonance.as_str(),    kind: ParameterKind::Float { min: 0.0, max: 1.0, default: 0.0 } },
                ParameterTemplate { name: params::drive.as_str(),        kind: ParameterKind::Float { min: 0.0, max: DRIVE_MAX, default: 1.0 } },
                ParameterTemplate { name: params::drift_amount.as_str(), kind: ParameterKind::Float { min: 0.0, max: 1.0, default: 0.0 } },
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
                true,
                OtaPoles::from(VOtaPoles::Four),
                1_000.0,
                0.0,
                1.0,
            ),
        })
    }

    fn update_validated_parameters(&mut self, p: &ParamView<'_>) {
        let poles: VOtaPoles = p.get(params::poles);
        self.core.set_params(
            poles.into(),
            p.get(params::cutoff).clamp(CUTOFF_MIN, CUTOFF_MAX),
            p.get(params::resonance).clamp(0.0, 1.0),
            p.get(params::drive).clamp(0.0, DRIVE_MAX),
            p.get(params::drift_amount).clamp(0.0, 1.0),
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
        let h = ModuleHarness::build::<VOtaPolyVcf>(&[]);
        let d = h.descriptor();
        assert_eq!(d.module_name, "VOtaPolyVcf");
        assert_eq!(d.inputs.len(), 2);
        assert_eq!(d.outputs.len(), 1);
    }

    #[test]
    fn stable_under_max_drive_and_resonance() {
        for poles in [VOtaPoles::Two, VOtaPoles::Four] {
            let mut h = ModuleHarness::build::<VOtaPolyVcf>(params![
                "poles" => poles,
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
                    assert!(y.is_finite(), "voice {i} @ n={n} poles={poles:?}");
                }
            }
        }
    }
}
