//! `VOtaPolyVcf` — 16-voice polyphonic sibling of [`crate::vota_vcf::VOtaVcf`].
//!
//! Shares the OTA-ladder kernel with the mono version; wrapper differs
//! only in port kind and per-voice state fan-out. The `GLOBAL_DRIFT`
//! backplane slot is read once per periodic update and applied to every
//! voice — drift is globally correlated by design.
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
    ModuleDescriptorTemplate, MonoInput, OutputPort, ParameterKind, ParameterTemplate,
    PolyInput, PolyOutput, PortTemplate, GLOBAL_DRIFT,
};
use patches_sdk::{StructuralParams, BuildError};
use patches_dsp::{OtaLadderCoeffs, OtaPoles, PolyOtaLadderKernel};

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

const CUTOFF_MIN: f32 = 20.0;
const CUTOFF_MAX: f32 = 20_000.0;
const DRIVE_MAX: f32 = 4.0;
const MAX_DRIFT_CENTS: f32 = 25.0;

pub struct VOtaPolyVcf {
    instance_id: InstanceId,
    descriptor: ModuleDescriptor,
    sample_rate: f32,
    interval_recip: f32,
    poles: VOtaPoles,
    cutoff: f32,
    resonance: f32,
    drive: f32,
    drift_amount: f32,
    kernel: PolyOtaLadderKernel,
    in_audio: PolyInput,
    in_cutoff_cv: PolyInput,
    in_global_drift: MonoInput,
    out_audio: PolyOutput,
}

impl VOtaPolyVcf {
    fn coeffs_for(&self, cv_voct: f32, drift_sample: f32) -> OtaLadderCoeffs {
        let drift_voct = drift_sample * self.drift_amount * (MAX_DRIFT_CENTS / 1200.0);
        let total_voct = cv_voct + drift_voct;
        let eff = (self.cutoff * (2.0f32).powf(total_voct))
            .clamp(CUTOFF_MIN, self.sample_rate * 0.45);
        let k = self.resonance * OtaPoles::Four.k_max();
        OtaLadderCoeffs::new(eff, self.sample_rate, k, self.drive)
    }

    fn apply_static(&mut self) {
        let c = self.coeffs_for(0.0, 0.0);
        self.kernel.set_static(c);
    }
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

    fn prepare(env: &AudioEnvironment, descriptor: ModuleDescriptor, instance_id: InstanceId, _structural: &StructuralParams) -> Result<Self, BuildError> { Ok({
        let poles = VOtaPoles::Four;
        let cutoff = 1_000.0;
        let resonance = 0.0;
        let drive = 1.0;
        let drift_amount = 0.0;
        let coeffs = OtaLadderCoeffs::new(cutoff, env.sample_rate, 0.0, drive);
        Self {
            instance_id,
            descriptor,
            sample_rate: env.sample_rate,
            interval_recip: 1.0 / env.periodic_update_interval as f32,
            poles,
            cutoff,
            resonance,
            drive,
            drift_amount,
            kernel: PolyOtaLadderKernel::new_static(coeffs, poles.into()),
            in_audio: PolyInput::default(),
            in_cutoff_cv: PolyInput::default(),
            in_global_drift: MonoInput::backplane(GLOBAL_DRIFT),
            out_audio: PolyOutput::default(),
        }
    })}

    fn update_validated_parameters(&mut self, p: &ParamView<'_>) {
        self.poles = p.get(params::poles);
        self.cutoff = p.get(params::cutoff).clamp(CUTOFF_MIN, CUTOFF_MAX);
        self.resonance = p.get(params::resonance).clamp(0.0, 1.0);
        self.drive = p.get(params::drive).clamp(0.0, DRIVE_MAX);
        self.drift_amount = p.get(params::drift_amount).clamp(0.0, 1.0);
        self.kernel.set_poles(self.poles.into());
        if !self.in_cutoff_cv.is_connected() && self.drift_amount == 0.0 {
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
        if !self.in_cutoff_cv.is_connected() && self.drift_amount == 0.0 {
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
        let ramp = self.in_cutoff_cv.is_connected() || self.drift_amount > 0.0;
        let out = self.kernel.tick_all(&audio, ramp);
        pool.write_poly(&self.out_audio, out);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn wants_periodic(&self) -> bool { true }

    fn periodic_update(&mut self, pool: &CablePool<'_>) {
        let cv_connected = self.in_cutoff_cv.is_connected();
        let drift_active = self.drift_amount > 0.0;
        if !cv_connected && !drift_active {
            return;
        }
        let cv = if cv_connected {
            pool.read_poly(&self.in_cutoff_cv)
        } else {
            [0.0f32; 16]
        };
        let drift = if drift_active {
            pool.read_mono(&self.in_global_drift)
        } else {
            0.0
        };
        for (i, &v) in cv.iter().enumerate() {
            let c = self.coeffs_for(v, drift);
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
