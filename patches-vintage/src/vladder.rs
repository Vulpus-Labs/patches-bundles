//! `VLadder` — vintage 4-pole ZDF ladder low-pass with `sharp` / `smooth` voicings.
//!
//! Mono wrapper around [`patches_dsp::LadderKernel`]. The `cutoff_cv`
//! input carries a V/oct offset summed externally from envelopes, LFOs
//! and key-tracking; the module adds nothing on top.
//!
//! Module plumbing (ports, periodic updates, `apply_static`/ramp
//! cadence) lives in [`crate::vintage_filter`] and is shared with
//! [`crate::vpoly_ladder`], [`crate::vota_vcf`],
//! [`crate::vota_poly_vcf`].
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
    ModuleDescriptor, ModuleDescriptorTemplate, OutputPort, ParameterKind, ParameterTemplate,
    PortTemplate,
};
use patches_sdk::{StructuralParams, BuildError};
use patches_dsp::{LadderKernel, LadderVariant};

use crate::vintage_filter::{
    VintageVcfMonoCore, CUTOFF_MAX, CUTOFF_MIN, DRIVE_MAX,
};

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

pub struct VLadder {
    instance_id: InstanceId,
    descriptor: ModuleDescriptor,
    core: VintageVcfMonoCore<LadderKernel>,
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

    fn prepare(
        env: &AudioEnvironment,
        descriptor: ModuleDescriptor,
        instance_id: InstanceId,
        _structural: &StructuralParams,
    ) -> Result<Self, BuildError> {
        Ok(Self {
            instance_id,
            descriptor,
            core: VintageVcfMonoCore::new(
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
