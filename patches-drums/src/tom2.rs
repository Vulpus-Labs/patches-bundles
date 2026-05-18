/// Struck-resonator tom drum (ADR 0002).
///
/// Thin shell over [`StruckResonatorVoice`](crate::primitives::StruckResonatorVoice).
/// Same spine as [`crate::kick2::Kick2`]; differs only in parameter defaults
/// (higher tune, lower Q, shorter pulse, stronger default `drive` so the
/// amplitude-coupled glide is audible at defaults — the tom's signature)
/// and the absence of a v/oct port. See
/// [ADR 0002 §"Nonlinearity and pitch droop"](https://github.com/Vulpus-Labs/patches-bundles/blob/main/patches-drums/adrs/0002-bridged-t-resonator-family.md#nonlinearity-and-pitch-droop).
///
/// Sister to [`crate::tom::Tom`]; the two coexist.
///
/// # Inputs
///
/// | Port       | Kind | Description                                          |
/// |------------|------|------------------------------------------------------|
/// | `trigger`  | mono | Rising edge strikes the resonator                    |
/// | `velocity` | mono | Velocity (0.0–1.0); latched on trigger, scales output. Defaults to 1.0 when disconnected |
///
/// # Outputs
///
/// | Port  | Kind | Description |
/// |-------|------|-------------|
/// | `out` | mono | Tom signal  |
///
/// # Parameters
///
/// | Name        | Type  | Range       | Default     | Description                                |
/// |-------------|-------|-------------|-------------|--------------------------------------------|
/// | `tune`      | float | 40–500 Hz   | 150         | Resonator centre frequency                 |
/// | `q`         | float | 5.0–60.0    | 30          | Resonator Q (decay follows)                |
/// | `pulse_ms`  | float | 0.1–8.0     | 1.5         | Excitation duration                        |
/// | `drive`     | float | 0.0–1.0     | 0.5         | Self-FM depth + output saturator amount    |
/// | `attack`    | float | 0.0–1.0     | 0.7         | Attack-FM depth                            |
/// | `pulse_shape` | enum | dirac / exp_decay / half_sine / filtered_click | exp_decay | Structural; excitation shape |
use patches_sdk::cables::TriggerInput;
use patches_sdk::module_params;
use patches_sdk::modules::{CountAxis, ModuleDescriptorTemplate, ParameterTemplate, PortTemplate};
use patches_sdk::param_frame::ParamView;
use patches_sdk::{
    AudioEnvironment, BuildError, CablePool, InputPort, InstanceId, Module, ModuleDescriptor,
    MonoInput, MonoOutput, OutputPort, ParameterKind, StructuralParams,
};

use crate::primitives::{PulseShape, StruckResonatorVoice};

patches_sdk::params_enum! {
    pub enum Tom2PulseShape {
        Dirac => "dirac",
        ExpDecay => "exp_decay",
        HalfSine => "half_sine",
        FilteredClick => "filtered_click",
    }
}

impl Tom2PulseShape {
    fn to_primitive(self) -> PulseShape {
        match self {
            Self::Dirac => PulseShape::Dirac,
            Self::ExpDecay => PulseShape::ExpDecay,
            Self::HalfSine => PulseShape::HalfSine,
            Self::FilteredClick => PulseShape::FilteredClick,
        }
    }
}

fn read_pulse_shape(s: &StructuralParams) -> Tom2PulseShape {
    s.get_int("pulse_shape", 0)
        .and_then(|i| u32::try_from(i).ok())
        .and_then(|i| Tom2PulseShape::try_from(i).ok())
        .unwrap_or(Tom2PulseShape::ExpDecay)
}

module_params! {
    Tom2 {
        tune: Float,
        q: Float,
        pulse_ms: Float,
        drive: Float,
        attack: Float,
    }
}

pub struct Tom2 {
    instance_id: InstanceId,
    descriptor: ModuleDescriptor,
    voice: StruckResonatorVoice,
    in_trigger: TriggerInput,
    in_velocity: MonoInput,
    out_audio: MonoOutput,
}

impl Module for Tom2 {
    fn template() -> ModuleDescriptorTemplate {
        const T: ModuleDescriptorTemplate = ModuleDescriptorTemplate {
            name: "Tom2",
            axes: &[CountAxis::CHANNELS],
            global_inputs: &[
                PortTemplate::trigger("trigger"),
                PortTemplate::mono("velocity"),
            ],
            per_axis_inputs: &[],
            global_outputs: &[PortTemplate::mono("out")],
            per_axis_outputs: &[],
            realtime_params: &[
                ParameterTemplate {
                    name: params::tune.as_str(),
                    kind: ParameterKind::Float { min: 40.0, max: 500.0, default: 150.0 },
                },
                ParameterTemplate {
                    name: params::q.as_str(),
                    kind: ParameterKind::Float { min: 5.0, max: 60.0, default: 30.0 },
                },
                ParameterTemplate {
                    name: params::pulse_ms.as_str(),
                    kind: ParameterKind::Float { min: 0.1, max: 8.0, default: 1.5 },
                },
                ParameterTemplate {
                    name: params::drive.as_str(),
                    kind: ParameterKind::Float { min: 0.0, max: 1.0, default: 0.5 },
                },
                ParameterTemplate {
                    name: params::attack.as_str(),
                    kind: ParameterKind::Float { min: 0.0, max: 1.0, default: 0.7 },
                },
            ],
            structural_params: &[ParameterTemplate {
                name: "pulse_shape",
                kind: ParameterKind::Enum {
                    variants: Tom2PulseShape::VARIANTS,
                    default: "exp_decay",
                },
            }],
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
        let sr = audio_environment.sample_rate;
        let mut voice = StruckResonatorVoice::new(
            sr,
            150.0,
            30.0,
            read_pulse_shape(structural).to_primitive(),
            1.5,
            instance_id.as_u64(),
        );
        voice.set_drive(0.5);
        voice.set_attack(0.7);
        Ok(Self {
            instance_id,
            descriptor,
            voice,
            in_trigger: TriggerInput::default(),
            in_velocity: MonoInput::default(),
            out_audio: MonoOutput::default(),
        })
    }

    fn update_validated_parameters(&mut self, p: &ParamView<'_>) {
        self.voice.set_tune(p.get(params::tune));
        self.voice.set_q(p.get(params::q));
        self.voice.set_pulse_ms(p.get(params::pulse_ms));
        self.voice.set_drive(p.get(params::drive));
        self.voice.set_attack(p.get(params::attack));
    }

    fn descriptor(&self) -> &ModuleDescriptor { &self.descriptor }
    fn instance_id(&self) -> InstanceId { self.instance_id }

    fn set_ports(&mut self, inputs: &[InputPort], outputs: &[OutputPort]) {
        self.in_trigger = TriggerInput::from_ports(inputs, 0);
        self.in_velocity = MonoInput::from_ports(inputs, 1);
        self.out_audio = MonoOutput::from_ports(outputs, 0);
    }

    fn process(&mut self, pool: &mut CablePool<'_>) {
        let trigger_rose = self.in_trigger.tick(pool).is_some();
        if trigger_rose {
            let velocity = if self.in_velocity.connected {
                pool.read_mono(&self.in_velocity)
            } else {
                1.0
            };
            self.voice.trigger(velocity);
        }
        let out = self.voice.tick();
        pool.write_mono(&self.out_audio, out);
    }

    fn as_any(&self) -> &dyn std::any::Any { self }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{dominant_bin, magnitude_spectrum, windowed_rms};
    use patches_sdk::test_support::ModuleHarness;
    use patches_sdk::ParameterValue;

    fn make(args: &[(&str, ParameterValue)]) -> ModuleHarness {
        let mut h = ModuleHarness::build::<Tom2>(args);
        h.disconnect_input("velocity");
        h
    }

    fn fire(h: &mut ModuleHarness) {
        h.set_mono("trigger", 1.0);
        h.tick();
        h.set_mono("trigger", 0.0);
    }

    fn early_late_hz(h: &mut ModuleHarness) -> (f32, f32) {
        fire(h);
        let s = h.run_mono(6096, "out");
        let mut early = [0.0; 1024];
        let take = early.len().min(s.len());
        early[..take].copy_from_slice(&s[..take]);
        let early_spec = magnitude_spectrum(&early, 1024);
        let late_spec = magnitude_spectrum(&s[2000..6096], 4096);
        let sr = 44100.0;
        (
            dominant_bin(&early_spec) as f32 * sr / 1024.0,
            dominant_bin(&late_spec) as f32 * sr / 4096.0,
        )
    }

    #[test]
    fn trigger_produces_output() {
        let mut h = make(&[]);
        fire(&mut h);
        let rms = h.measure_rms(2000, "out");
        assert!(rms > 0.01, "rms = {rms}");
    }

    #[test]
    fn pitch_tracking() {
        fn peak(tune: f32) -> usize {
            let mut h = make(&[
                ("tune", ParameterValue::Float(tune)),
                ("q", ParameterValue::Float(40.0)),
                ("drive", ParameterValue::Float(0.0)),
                ("attack", ParameterValue::Float(0.0)),
            ]);
            fire(&mut h);
            let s = h.run_mono(4096, "out");
            dominant_bin(&magnitude_spectrum(&s, 4096))
        }
        let lo = peak(80.0);
        let hi = peak(300.0);
        assert!(hi > lo, "lo={lo}, hi={hi}");
    }

    #[test]
    fn output_decays() {
        let mut h = make(&[
            ("drive", ParameterValue::Float(0.0)),
            ("attack", ParameterValue::Float(0.0)),
        ]);
        fire(&mut h);
        for _ in 0..20000 {
            h.tick();
        }
        let rms = h.measure_rms(2000, "out");
        assert!(rms < 0.005, "rms = {rms}");
    }

    #[test]
    fn envelope_monotonic() {
        let mut h = make(&[
            ("tune", ParameterValue::Float(150.0)),
            ("q", ParameterValue::Float(40.0)),
            ("drive", ParameterValue::Float(0.0)),
            ("attack", ParameterValue::Float(0.0)),
        ]);
        fire(&mut h);
        let s = h.run_mono(8192, "out");
        let rms = windowed_rms(&s, 512);
        let mut prev = rms[1];
        for (i, &r) in rms.iter().enumerate().skip(2) {
            assert!(
                r <= prev * 1.05,
                "tom2 envelope not monotonic at block {i}: {r} > {prev}"
            );
            prev = r;
        }
    }

    #[test]
    fn pitch_droop_with_attack_fm() {
        let mut h = make(&[
            ("tune", ParameterValue::Float(150.0)),
            ("q", ParameterValue::Float(40.0)),
            ("drive", ParameterValue::Float(0.0)),
            ("attack", ParameterValue::Float(0.8)),
        ]);
        let (early_hz, late_hz) = early_late_hz(&mut h);
        // 1024-sample early window (23 ms) is split between the 6 ms FM
        // pulse and 17 ms of settled ring, so the dominant bin sits a
        // partial bin above the late value. ≥ 15 Hz (~third of a bin) is
        // the detection floor.
        assert!(
            early_hz > late_hz + 15.0,
            "attack lift: early {early_hz} Hz, late {late_hz} Hz"
        );
    }

    #[test]
    fn no_droop_when_fm_off() {
        let mut h = make(&[
            ("tune", ParameterValue::Float(150.0)),
            ("q", ParameterValue::Float(40.0)),
            ("drive", ParameterValue::Float(0.0)),
            ("attack", ParameterValue::Float(0.0)),
        ]);
        let (early_hz, late_hz) = early_late_hz(&mut h);
        assert!(
            (early_hz - late_hz).abs() < 50.0,
            "fm off: early {early_hz}, late {late_hz} should match"
        );
    }

    #[test]
    fn velocity_scales_output() {
        let mut h_full = make(&[
            ("drive", ParameterValue::Float(0.0)),
            ("attack", ParameterValue::Float(0.0)),
        ]);
        fire(&mut h_full);
        let rms_full = h_full.measure_rms(2000, "out");

        let mut h_half = ModuleHarness::build::<Tom2>(&[
            ("drive", ParameterValue::Float(0.0)),
            ("attack", ParameterValue::Float(0.0)),
        ]);
        h_half.set_mono("velocity", 0.5);
        fire(&mut h_half);
        let rms_half = h_half.measure_rms(2000, "out");

        let ratio = rms_half / rms_full;
        assert!(
            (ratio - 0.5).abs() < 0.1,
            "ratio = {ratio}"
        );
    }
}
