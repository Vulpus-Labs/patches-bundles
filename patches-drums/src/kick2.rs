/// Struck-resonator kick drum (ADR 0002).
///
/// Thin shell over [`StruckResonatorVoice`](crate::primitives::StruckResonatorVoice):
/// the voice owns the resonator + excitation + attack-FM spine; this module
/// adds the parameter descriptor, port wiring, and v/oct handling. Pitch
/// behaviour from two explicit FM paths (see
/// [ADR 0002 §"Nonlinearity and pitch droop"](https://github.com/Vulpus-Labs/patches-bundles/blob/main/patches-drums/adrs/0002-bridged-t-resonator-family.md#nonlinearity-and-pitch-droop)):
///
/// - **Self-FM** (`drive`): amplitude-coupled pitch lift via the
///   resonator's `lp` tap. Also scales the output saturator amount.
/// - **Attack-FM** (`attack`): short trigger-locked pulse adds a brief
///   strike lift to the cutoff.
///
/// Sister to [`crate::kick::Kick`]; the two coexist.
///
/// # Inputs
///
/// | Port       | Kind | Description                                          |
/// |------------|------|------------------------------------------------------|
/// | `trigger`  | mono | Rising edge strikes the resonator                    |
/// | `voct`     | mono | V/oct pitch CV; overrides `tune` when connected      |
/// | `velocity` | mono | Velocity (0.0–1.0); latched on trigger, scales output. Defaults to 1.0 when disconnected |
///
/// # Outputs
///
/// | Port  | Kind | Description |
/// |-------|------|-------------|
/// | `out` | mono | Kick signal |
///
/// # Parameters
///
/// | Name        | Type  | Range       | Default     | Description                                |
/// |-------------|-------|-------------|-------------|--------------------------------------------|
/// | `tune`      | float | 20–200 Hz   | 55          | Resonator centre frequency                 |
/// | `q`         | float | 5.0–80.0    | 50          | Resonator Q (decay follows)                |
/// | `pulse_ms`  | float | 0.1–10.0    | 2.0         | Excitation duration                        |
/// | `drive`     | float | 0.0–1.0     | 0.3         | Self-FM depth + output saturator amount    |
/// | `attack`    | float | 0.0–1.0     | 0.5         | Attack-FM depth                            |
/// | `pulse_shape` | enum | dirac / exp_decay / half_sine / filtered_click | half_sine | Structural; excitation shape |
use patches_dsp::fast_exp2;
use patches_sdk::cables::TriggerInput;
use patches_sdk::module_params;
use patches_sdk::modules::{CountAxis, ModuleDescriptorTemplate, ParameterTemplate, PortTemplate};
use patches_sdk::param_frame::ParamView;
use patches_sdk::{
    AudioEnvironment, BuildError, CablePool, InputPort, InstanceId, Module, ModuleDescriptor,
    MonoInput, MonoOutput, OutputPort, ParameterKind, StructuralParams,
};

use crate::primitives::{PulseShape, StruckResonatorVoice};

const C0_FREQ: f32 = 16.351_598;

patches_sdk::params_enum! {
    pub enum Kick2PulseShape {
        Dirac => "dirac",
        ExpDecay => "exp_decay",
        HalfSine => "half_sine",
        FilteredClick => "filtered_click",
    }
}

impl Kick2PulseShape {
    fn to_primitive(self) -> PulseShape {
        match self {
            Self::Dirac => PulseShape::Dirac,
            Self::ExpDecay => PulseShape::ExpDecay,
            Self::HalfSine => PulseShape::HalfSine,
            Self::FilteredClick => PulseShape::FilteredClick,
        }
    }
}

fn read_pulse_shape(s: &StructuralParams) -> Kick2PulseShape {
    s.get_int("pulse_shape", 0)
        .and_then(|i| u32::try_from(i).ok())
        .and_then(|i| Kick2PulseShape::try_from(i).ok())
        .unwrap_or(Kick2PulseShape::HalfSine)
}

module_params! {
    Kick2 {
        tune: Float,
        q: Float,
        pulse_ms: Float,
        drive: Float,
        attack: Float,
    }
}

pub struct Kick2 {
    instance_id: InstanceId,
    descriptor: ModuleDescriptor,
    tune: f32,
    voct_connected: bool,
    voice: StruckResonatorVoice,
    in_trigger: TriggerInput,
    voct_in: MonoInput,
    in_velocity: MonoInput,
    out_audio: MonoOutput,
}

impl Module for Kick2 {
    fn template() -> ModuleDescriptorTemplate {
        const T: ModuleDescriptorTemplate = ModuleDescriptorTemplate {
            name: "Kick2",
            axes: &[CountAxis::CHANNELS],
            global_inputs: &[
                PortTemplate::trigger("trigger"),
                PortTemplate::mono("voct"),
                PortTemplate::mono("velocity"),
            ],
            per_axis_inputs: &[],
            global_outputs: &[PortTemplate::mono("out")],
            per_axis_outputs: &[],
            realtime_params: &[
                ParameterTemplate {
                    name: params::tune.as_str(),
                    kind: ParameterKind::Float { min: 20.0, max: 200.0, default: 55.0 },
                },
                ParameterTemplate {
                    name: params::q.as_str(),
                    kind: ParameterKind::Float { min: 5.0, max: 80.0, default: 50.0 },
                },
                ParameterTemplate {
                    name: params::pulse_ms.as_str(),
                    kind: ParameterKind::Float { min: 0.1, max: 10.0, default: 2.0 },
                },
                ParameterTemplate {
                    name: params::drive.as_str(),
                    kind: ParameterKind::Float { min: 0.0, max: 1.0, default: 0.3 },
                },
                ParameterTemplate {
                    name: params::attack.as_str(),
                    kind: ParameterKind::Float { min: 0.0, max: 1.0, default: 0.5 },
                },
            ],
            structural_params: &[ParameterTemplate {
                name: "pulse_shape",
                kind: ParameterKind::Enum {
                    variants: Kick2PulseShape::VARIANTS,
                    default: "half_sine",
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
            55.0,
            50.0,
            read_pulse_shape(structural).to_primitive(),
            2.0,
            instance_id.as_u64(),
        );
        voice.set_drive(0.3);
        voice.set_attack(0.5);
        Ok(Self {
            instance_id,
            descriptor,
            tune: 55.0,
            voct_connected: false,
            voice,
            in_trigger: TriggerInput::default(),
            voct_in: MonoInput::default(),
            in_velocity: MonoInput::default(),
            out_audio: MonoOutput::default(),
        })
    }

    fn update_validated_parameters(&mut self, p: &ParamView<'_>) {
        self.tune = p.get(params::tune);
        if !self.voct_connected {
            self.voice.set_tune(self.tune);
        }
        self.voice.set_q(p.get(params::q));
        self.voice.set_pulse_ms(p.get(params::pulse_ms));
        self.voice.set_drive(p.get(params::drive));
        self.voice.set_attack(p.get(params::attack));
    }

    fn descriptor(&self) -> &ModuleDescriptor { &self.descriptor }
    fn instance_id(&self) -> InstanceId { self.instance_id }

    fn set_ports(&mut self, inputs: &[InputPort], outputs: &[OutputPort]) {
        self.in_trigger = TriggerInput::from_ports(inputs, 0);
        self.voct_in = MonoInput::from_ports(inputs, 1);
        self.in_velocity = MonoInput::from_ports(inputs, 2);
        let was_connected = self.voct_connected;
        self.voct_connected = self.voct_in.is_connected();
        if was_connected && !self.voct_connected {
            self.voice.set_tune(self.tune);
        }
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

    fn wants_periodic(&self) -> bool { true }

    fn periodic_update(&mut self, pool: &CablePool<'_>) {
        if !self.voct_connected {
            return;
        }
        let hz = C0_FREQ * fast_exp2(pool.read_mono(&self.voct_in));
        self.voice.set_tune(hz);
    }

    fn as_any(&self) -> &dyn std::any::Any { self }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{dominant_bin, magnitude_spectrum};
    use patches_sdk::test_support::ModuleHarness;
    use patches_sdk::ParameterValue;

    fn make(args: &[(&str, ParameterValue)]) -> ModuleHarness {
        let mut h = ModuleHarness::build::<Kick2>(args);
        h.disconnect_inputs(&["voct", "velocity"]);
        h
    }

    fn fire(h: &mut ModuleHarness) {
        h.set_mono("trigger", 1.0);
        h.tick();
        h.set_mono("trigger", 0.0);
    }

    #[test]
    fn trigger_produces_audible_output() {
        let mut h = make(&[]);
        fire(&mut h);
        let rms = h.measure_rms(2000, "out");
        assert!(rms > 0.01, "rms = {rms}");
    }

    #[test]
    fn output_decays() {
        let mut h = make(&[
            ("q", ParameterValue::Float(10.0)),
            ("drive", ParameterValue::Float(0.0)),
            ("attack", ParameterValue::Float(0.0)),
        ]);
        fire(&mut h);
        for _ in 0..22050 {
            h.tick();
        }
        let rms = h.measure_rms(2000, "out");
        assert!(rms < 0.005, "rms = {rms}");
    }

    #[test]
    fn tune_affects_pitch() {
        fn peak_bin(tune: f32) -> usize {
            let mut h = make(&[
                ("tune", ParameterValue::Float(tune)),
                ("q", ParameterValue::Float(50.0)),
                ("drive", ParameterValue::Float(0.0)),
                ("attack", ParameterValue::Float(0.0)),
            ]);
            fire(&mut h);
            let s = h.run_mono(4096, "out");
            let spec = magnitude_spectrum(&s, 4096);
            dominant_bin(&spec)
        }
        let lo = peak_bin(40.0);
        let hi = peak_bin(80.0);
        assert!(hi > lo, "lo bin {lo}, hi bin {hi}");
    }

    #[test]
    fn q_affects_decay_length() {
        fn rms_at(q: f32) -> f32 {
            let mut h = make(&[
                ("q", ParameterValue::Float(q)),
                ("drive", ParameterValue::Float(0.0)),
                ("attack", ParameterValue::Float(0.0)),
            ]);
            fire(&mut h);
            for _ in 0..6000 {
                h.tick();
            }
            h.measure_rms(2000, "out")
        }
        let lo = rms_at(20.0);
        let hi = rms_at(70.0);
        assert!(hi > lo, "lo={lo}, hi={hi}");
    }

    fn early_late_hz(h: &mut ModuleHarness) -> (f32, f32) {
        fire(h);
        let s = h.run_mono(6096, "out");
        let mut early = [0.0; 1024];
        let take = early.len().min(s.len());
        early[..take].copy_from_slice(&s[..take]);
        let early_spec = magnitude_spectrum(&early, 1024);
        let late_spec = magnitude_spectrum(&s[2000..6096], 4096);
        let early_bin = dominant_bin(&early_spec);
        let late_bin = dominant_bin(&late_spec);
        let sr = 44100.0;
        (
            early_bin as f32 * sr / 1024.0,
            late_bin as f32 * sr / 4096.0,
        )
    }

    #[test]
    fn pitch_droop_with_attack_fm() {
        let mut h = make(&[
            ("tune", ParameterValue::Float(100.0)),
            ("q", ParameterValue::Float(50.0)),
            ("drive", ParameterValue::Float(0.0)),
            ("attack", ParameterValue::Float(0.8)),
        ]);
        let (early_hz, late_hz) = early_late_hz(&mut h);
        assert!(
            early_hz > late_hz + 20.0,
            "attack=0.8 should lift strike pitch above settled: early {early_hz} Hz, late {late_hz} Hz"
        );
    }

    #[test]
    fn pitch_droop_with_self_fm() {
        let mut h = make(&[
            ("tune", ParameterValue::Float(100.0)),
            ("q", ParameterValue::Float(70.0)),
            ("drive", ParameterValue::Float(1.0)),
            ("attack", ParameterValue::Float(0.0)),
        ]);
        let (early_hz, late_hz) = early_late_hz(&mut h);
        assert!(
            early_hz > late_hz + 10.0,
            "drive=1.0 self-FM should lift strike pitch: early {early_hz} Hz, late {late_hz} Hz"
        );
    }

    #[test]
    fn no_droop_when_fm_off() {
        let mut h = make(&[
            ("tune", ParameterValue::Float(100.0)),
            ("q", ParameterValue::Float(60.0)),
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
            ("attack", ParameterValue::Float(0.0)),
            ("drive", ParameterValue::Float(0.0)),
        ]);
        fire(&mut h_full);
        let rms_full = h_full.measure_rms(2000, "out");

        let mut h_half = ModuleHarness::build::<Kick2>(&[
            ("attack", ParameterValue::Float(0.0)),
            ("drive", ParameterValue::Float(0.0)),
        ]);
        h_half.disconnect_input("voct");
        h_half.set_mono("velocity", 0.5);
        fire(&mut h_half);
        let rms_half = h_half.measure_rms(2000, "out");

        let ratio = rms_half / rms_full;
        assert!(
            (ratio - 0.5).abs() < 0.1,
            "ratio = {ratio}"
        );
    }

    #[test]
    fn velocity_disconnected_defaults_to_full() {
        let mut h = make(&[]);
        fire(&mut h);
        let rms = h.measure_rms(2000, "out");
        assert!(rms > 0.01, "rms = {rms}");
    }

    #[test]
    fn voct_overrides_tune() {
        let voct_for_120 = (120.0f32 / C0_FREQ).log2();

        let mut h_voct = ModuleHarness::build::<Kick2>(&[
            ("tune", ParameterValue::Float(40.0)),
            ("q", ParameterValue::Float(60.0)),
            ("drive", ParameterValue::Float(0.0)),
            ("attack", ParameterValue::Float(0.0)),
        ]);
        h_voct.disconnect_input("velocity");
        h_voct.set_mono("voct", voct_for_120);

        let mut h_no = ModuleHarness::build::<Kick2>(&[
            ("tune", ParameterValue::Float(40.0)),
            ("q", ParameterValue::Float(60.0)),
            ("drive", ParameterValue::Float(0.0)),
            ("attack", ParameterValue::Float(0.0)),
        ]);
        h_no.disconnect_inputs(&["voct", "velocity"]);

        for _ in 0..64 { h_voct.tick(); }
        for _ in 0..64 { h_no.tick(); }

        fire(&mut h_voct);
        fire(&mut h_no);

        let s_voct = h_voct.run_mono(4096, "out");
        let s_no = h_no.run_mono(4096, "out");
        let bin_voct = dominant_bin(&magnitude_spectrum(&s_voct, 4096));
        let bin_no = dominant_bin(&magnitude_spectrum(&s_no, 4096));
        assert!(
            bin_voct > bin_no,
            "voct override should raise pitch: voct bin {bin_voct}, no_voct bin {bin_no}"
        );
    }
}
