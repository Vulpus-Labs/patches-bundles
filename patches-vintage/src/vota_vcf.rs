//! `VOtaVcf` — R3109/IR3109-style 4-pole OTA-C lowpass.
//!
//! Mono wrapper around [`patches_dsp::OtaLadderKernel`]. Per-stage `tanh`
//! saturation (rather than a global pre-feedback tanh) yields softer,
//! more distributed distortion and a cleaner self-oscillation than the
//! Moog-style transistor ladder in [`crate::vladder::VLadder`].
//!
//! Switchable output slope: 12 dB/oct (2-pole tap) or 24 dB/oct (4-pole
//! tap). The resonance feedback loop always wraps all four stages, so
//! the filter rings and self-oscillates identically in either mode —
//! only the output slope changes.
//!
//! Cutoff is modulated by the engine-level `GLOBAL_DRIFT` backplane slot
//! scaled by the `drift_amount` parameter. `drift_amount = 0.0` is
//! bit-identical to a drift-free build.
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
//! | Name           | Type  | Range            | Default  | Description                      |
//! |----------------|-------|------------------|----------|----------------------------------|
//! | `poles`        | enum  | `two`/`four`     | `four`   | Output slope (12 or 24 dB/oct)   |
//! | `cutoff`       | float | 20.0 -- 20000.0  | `1000.0` | Base cutoff in Hz                |
//! | `resonance`    | float | 0.0 -- 1.0       | `0.0`    | Feedback amount; self-osc near 1 |
//! | `drive`        | float | 0.0 -- 4.0       | `1.0`    | Input gain before stage tanh     |
//! | `drift_amount` | float | 0.0 -- 1.0       | `0.0`    | Scales `GLOBAL_DRIFT` into cutoff |

use patches_sdk::module_params;
use patches_sdk::param_frame::ParamView;
use patches_sdk::{
    params_enum, AudioEnvironment, CablePool, CountAxis, InputPort, InstanceId, Module,
    ModuleDescriptor, ModuleDescriptorTemplate, MonoInput, MonoOutput, OutputPort,
    ParameterKind, ParameterTemplate, PortTemplate, GLOBAL_DRIFT,
};
use patches_sdk::{StructuralParams, BuildError};
use patches_dsp::{OtaLadderCoeffs, OtaLadderKernel, OtaPoles};

params_enum! {
    pub enum VOtaPoles {
        Two => "two",
        Four => "four",
    }
}

impl From<VOtaPoles> for OtaPoles {
    fn from(p: VOtaPoles) -> Self {
        match p {
            VOtaPoles::Two => OtaPoles::Two,
            VOtaPoles::Four => OtaPoles::Four,
        }
    }
}

module_params! {
    VOtaVcfParams {
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
/// Max cents of cutoff detune at `drift_amount = 1.0, drift = ±1.0`.
const MAX_DRIFT_CENTS: f32 = 25.0;

pub struct VOtaVcf {
    instance_id: InstanceId,
    descriptor: ModuleDescriptor,
    sample_rate: f32,
    interval_recip: f32,
    poles: VOtaPoles,
    cutoff: f32,
    resonance: f32,
    drive: f32,
    drift_amount: f32,
    kernel: OtaLadderKernel,
    in_audio: MonoInput,
    in_cutoff_cv: MonoInput,
    in_global_drift: MonoInput,
    out_audio: MonoOutput,
}

impl VOtaVcf {
    fn coeffs(&self, cv_voct: f32, drift_sample: f32) -> OtaLadderCoeffs {
        let drift_voct = drift_sample * self.drift_amount * (MAX_DRIFT_CENTS / 1200.0);
        let total_voct = cv_voct + drift_voct;
        let effective = (self.cutoff * (2.0f32).powf(total_voct))
            .clamp(CUTOFF_MIN, self.sample_rate * 0.45);
        let k = self.resonance * OtaPoles::Four.k_max();
        OtaLadderCoeffs::new(effective, self.sample_rate, k, self.drive)
    }

    fn apply_static(&mut self) {
        let c = self.coeffs(0.0, 0.0);
        self.kernel.set_static(c);
    }
}

impl Module for VOtaVcf {
    fn template() -> ModuleDescriptorTemplate {
        const T: ModuleDescriptorTemplate = ModuleDescriptorTemplate {
            name: "VOtaVcf",
            axes: &[CountAxis::CHANNELS],
            global_inputs: &[PortTemplate::mono("in"), PortTemplate::mono("cutoff_cv")],
            per_axis_inputs: &[],
            global_outputs: &[PortTemplate::mono("out")],
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
            kernel: OtaLadderKernel::new_static(coeffs, poles.into()),
            in_audio: MonoInput::default(),
            in_cutoff_cv: MonoInput::default(),
            in_global_drift: MonoInput::backplane(GLOBAL_DRIFT),
            out_audio: MonoOutput::default(),
        }
    })}

    fn update_validated_parameters(&mut self, p: &ParamView<'_>) {
        self.poles = p.get(params::poles);
        self.cutoff = p.get(params::cutoff).clamp(CUTOFF_MIN, CUTOFF_MAX);
        self.resonance = p.get(params::resonance).clamp(0.0, 1.0);
        self.drive = p.get(params::drive).clamp(0.0, DRIVE_MAX);
        self.drift_amount = p.get(params::drift_amount).clamp(0.0, 1.0);
        self.kernel.set_poles(self.poles.into());
        // Coefficients depend on cutoff_cv and drift, both read each periodic
        // update. Fall back to static apply only when neither modulates the
        // cutoff.
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
        self.in_audio = MonoInput::from_ports(inputs, 0);
        self.in_cutoff_cv = MonoInput::from_ports(inputs, 1);
        self.out_audio = MonoOutput::from_ports(outputs, 0);
        if !self.in_cutoff_cv.is_connected() && self.drift_amount == 0.0 {
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
        let cv_connected = self.in_cutoff_cv.is_connected();
        let drift_active = self.drift_amount > 0.0;
        if !cv_connected && !drift_active {
            return;
        }
        let cv = if cv_connected {
            pool.read_mono(&self.in_cutoff_cv)
        } else {
            0.0
        };
        let drift = if drift_active {
            pool.read_mono(&self.in_global_drift)
        } else {
            0.0
        };
        let c = self.coeffs(cv, drift);
        self.kernel.begin_ramp(c, self.interval_recip);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use patches_sdk::test_support::{params, ModuleHarness};

    #[test]
    fn descriptor_shape() {
        let h = ModuleHarness::build::<VOtaVcf>(&[]);
        let d = h.descriptor();
        assert_eq!(d.module_name, "VOtaVcf");
        assert_eq!(d.inputs.len(), 2);
        assert_eq!(d.outputs.len(), 1);
    }

    #[test]
    fn self_oscillates_at_max_resonance_4pole() {
        let mut h = ModuleHarness::build::<VOtaVcf>(params![
            "poles" => VOtaPoles::Four,
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
        assert!(peak > 0.05, "4-pole failed to self-oscillate: peak={peak}");
    }

    #[test]
    fn self_oscillates_at_max_resonance_2pole() {
        let mut h = ModuleHarness::build::<VOtaVcf>(params![
            "poles" => VOtaPoles::Two,
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
        assert!(peak > 0.05, "2-pole failed to self-oscillate: peak={peak}");
    }

    #[test]
    fn stable_under_max_drive_and_resonance() {
        for poles in [VOtaPoles::Two, VOtaPoles::Four] {
            let mut h = ModuleHarness::build::<VOtaVcf>(params![
                "poles" => poles,
                "cutoff" => 2_000.0_f32,
                "resonance" => 1.0_f32,
                "drive" => DRIVE_MAX,
            ]);
            for n in 0..8_192 {
                let x = if (n / 48) % 2 == 0 { 1.0 } else { -1.0 };
                h.set_mono("in", x);
                h.tick();
                let y = h.read_mono("out");
                assert!(y.is_finite(), "non-finite @ n={n} poles={poles:?}");
            }
        }
    }
}
