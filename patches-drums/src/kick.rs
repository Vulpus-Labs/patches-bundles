/// 808-style kick drum synthesiser.
///
/// A sine oscillator with a fast pitch sweep from a configurable start
/// frequency down to a settable base pitch, shaped by an exponential
/// amplitude decay envelope, with optional tanh saturation for grit and
/// a transient click layer.
///
/// # Inputs
///
/// | Port       | Kind | Description                                          |
/// |------------|------|------------------------------------------------------|
/// | `trigger`  | mono | Rising edge triggers                                 |
/// | `voct`     | mono | V/oct pitch CV; overrides `sweep` start frequency if connected |
/// | `velocity` | mono | Velocity (0.0–1.0); latched on trigger, scales output amplitude. Defaults to 1.0 when disconnected |
///
/// # Outputs
///
/// | Port  | Kind | Description |
/// |-------|------|-------------|
/// | `out` | mono | Kick signal |
///
/// # Parameters
///
/// | Name         | Type  | Range       | Default | Description                       |
/// |--------------|-------|-------------|---------|-----------------------------------|
/// | `pitch`      | float | 20–200 Hz   | 55      | Base pitch of the kick            |
/// | `sweep`      | float | 0–5000 Hz   | 2500    | Starting frequency of pitch sweep |
/// | `sweep_time` | float | 0.001–0.5 s | 0.04    | Duration of pitch sweep           |
/// | `decay`      | float | 0.01–2.0 s  | 0.5     | Amplitude decay time              |
/// | `drive`      | float | 0.0–1.0     | 0.0     | Saturation amount                 |
/// | `click`      | float | 0.0–1.0     | 0.3     | Transient click intensity         |
use patches_dsp::fast_exp2;

/// C0 reference frequency for 1V/oct pitch input. Mirrors
/// `patches_modules::common::frequency::C0_FREQ` (kept local so this
/// crate does not pull patches-modules).
const C0_FREQ: f32 = 16.351_598;

use patches_sdk::{
    AudioEnvironment, CablePool, InputPort, InstanceId, Module, ModuleDescriptor,
    MonoInput, MonoOutput, OutputPort, ParameterKind,
};
use patches_sdk::modules::{CountAxis, ModuleDescriptorTemplate, ParameterTemplate, PortTemplate};
use patches_sdk::{StructuralParams, BuildError};
use patches_sdk::cables::TriggerInput;
use patches_sdk::param_frame::ParamView;
use patches_sdk::module_params;
use crate::primitives::{DecayEnvelope, PitchSweep, saturate};
use patches_dsp::{MonoPhaseAccumulator, fast_sine};

module_params! {
    Kick {
        pitch:      Float,
        sweep:      Float,
        sweep_time: Float,
        decay:      Float,
        drive:      Float,
        click:      Float,
    }
}

pub struct Kick {
    instance_id: InstanceId,
    descriptor: ModuleDescriptor,
    sample_rate_reciprocal: f32,
    // Parameters
    pitch: f32,
    sweep_start: f32,
    sweep_time: f32,
    decay_time: f32,
    drive: f32,
    click: f32,
    voct_connected: bool,
    latched_velocity: f32,
    // DSP state
    osc: MonoPhaseAccumulator,
    pitch_sweep: PitchSweep,
    amp_env: DecayEnvelope,
    click_env: DecayEnvelope,
    // Ports
    in_trigger: TriggerInput,
    voct_in: MonoInput,
    in_velocity: MonoInput,
    out_audio: MonoOutput,
}

impl Module for Kick {
    fn template() -> ModuleDescriptorTemplate {
        const T: ModuleDescriptorTemplate = ModuleDescriptorTemplate {
            name: "Kick",
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
                    name: params::pitch.as_str(),
                    kind: ParameterKind::Float { min: 20.0, max: 200.0, default: 55.0 },
                },
                ParameterTemplate {
                    name: params::sweep.as_str(),
                    kind: ParameterKind::Float { min: 0.0, max: 5000.0, default: 2500.0 },
                },
                ParameterTemplate {
                    name: params::sweep_time.as_str(),
                    kind: ParameterKind::Float { min: 0.001, max: 0.5, default: 0.04 },
                },
                ParameterTemplate {
                    name: params::decay.as_str(),
                    kind: ParameterKind::Float { min: 0.01, max: 2.0, default: 0.5 },
                },
                ParameterTemplate {
                    name: params::drive.as_str(),
                    kind: ParameterKind::Float { min: 0.0, max: 1.0, default: 0.0 },
                },
                ParameterTemplate {
                    name: params::click.as_str(),
                    kind: ParameterKind::Float { min: 0.0, max: 1.0, default: 0.3 },
                },
            ],
            structural_params: &[],
            per_axis_realtime_params: &[],
            per_axis_structural_params: &[],
        };
        T
    }

    fn prepare(audio_environment: &AudioEnvironment, descriptor: ModuleDescriptor, instance_id: InstanceId, _structural: &StructuralParams) -> Result<Self, BuildError> { Ok({
        let sr = audio_environment.sample_rate;
        let mut amp_env = DecayEnvelope::new(sr);
        amp_env.set_decay(0.5);
        let mut click_env = DecayEnvelope::new(sr);
        click_env.set_decay(0.003);
        let mut pitch_sweep = PitchSweep::new(sr);
        pitch_sweep.set_params(2500.0, 55.0, 0.04);
        Self {
            instance_id,
            descriptor,
            sample_rate_reciprocal: sr.recip(),
            pitch: 55.0,
            voct_connected: false,
            latched_velocity: 1.0,
            sweep_start: 2500.0,
            sweep_time: 0.04,
            decay_time: 0.5,
            drive: 0.0,
            click: 0.3,
            osc: MonoPhaseAccumulator::new(),
            pitch_sweep,
            amp_env,
            click_env,
            in_trigger: TriggerInput::default(),
            voct_in: MonoInput::default(),
            in_velocity: MonoInput::default(),
            out_audio: MonoOutput::default(),
        }
    })}

    fn update_validated_parameters(&mut self, p: &ParamView<'_>) {
        let v = p.get(params::pitch);
        self.pitch = v;
        let v = p.get(params::sweep);
        self.sweep_start = v;
        let v = p.get(params::sweep_time);
        self.sweep_time = v;
        let v = p.get(params::decay);
        self.decay_time = v;
        self.amp_env.set_decay(self.decay_time);
        let v = p.get(params::drive);
        self.drive = v;
        let v = p.get(params::click);
        self.click = v;
        self.pitch_sweep.set_params(self.sweep_start, self.pitch, self.sweep_time);
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
            self.pitch_sweep.set_params(self.sweep_start, self.pitch, self.sweep_time);
        }
        self.out_audio = MonoOutput::from_ports(outputs, 0);
    }

    fn process(&mut self, pool: &mut CablePool<'_>) {
        let trigger_rose = self.in_trigger.tick(pool).is_some();

        if trigger_rose {
            self.latched_velocity = if self.in_velocity.connected {
                pool.read_mono(&self.in_velocity)
            } else {
                1.0
            };
            self.osc.reset();
            self.pitch_sweep.trigger();
        }

        let freq = self.pitch_sweep.tick();
        let amp = self.amp_env.tick(trigger_rose);
        let click_amp = self.click_env.tick(trigger_rose);

        // Set oscillator frequency
        let increment = freq * self.sample_rate_reciprocal;
        self.osc.set_increment(increment);

        // Sine oscillator
        let phase = self.osc.phase;
        let sine = fast_sine(phase);
        let two_phase = (phase + phase).fract();
        let three_phase = (two_phase + phase).fract();
        let click_signal = (fast_sine(two_phase) + fast_sine(three_phase)) * 0.5;
        self.osc.advance();

        // Mix sine body with click transient (higher harmonics)
        let signal = sine * amp + click_signal * click_amp * self.click * 0.3;

        // Apply saturation
        let output = if self.drive > 0.0 {
            saturate(signal, self.drive)
        } else {
            signal
        };

        pool.write_mono(&self.out_audio, output * self.latched_velocity);
    }
    fn wants_periodic(&self) -> bool { true }

    fn periodic_update(&mut self, pool: &CablePool<'_>) {
        if !self.voct_connected {
            return;
        }
        let start_hz = C0_FREQ * fast_exp2(pool.read_mono(&self.voct_in));
        let ratio = self.pitch / self.sweep_start;
        let end_hz = start_hz * ratio;
        self.pitch_sweep.set_params(start_hz, end_hz, self.sweep_time);
    }

    fn as_any(&self) -> &dyn std::any::Any { self }
}

#[cfg(test)]
mod tests {
    use patches_sdk::ParameterValue;
    use super::*;
    use patches_sdk::test_support::ModuleHarness;

    fn make_kick() -> ModuleHarness {
        let mut h = ModuleHarness::build::<Kick>(&[
            ("pitch", ParameterValue::Float(55.0)),
            ("sweep", ParameterValue::Float(2500.0)),
            ("sweep_time", ParameterValue::Float(0.04)),
            ("decay", ParameterValue::Float(0.5)),
            ("drive", ParameterValue::Float(0.0)),
            ("click", ParameterValue::Float(0.3)),
        ]);
        h.disconnect_inputs(&["voct", "velocity"]);
        h
    }

    #[test]
    fn trigger_produces_non_silent_output() {
        let mut h = make_kick();
        h.set_mono("trigger", 1.0);
        h.tick();
        h.set_mono("trigger", 0.0);
        let rms = h.measure_rms(2000, "out");
        assert!(rms > 0.01, "kick should produce audible output, rms = {rms}");
    }

    #[test]
    fn output_decays_to_near_zero() {
        let mut h = ModuleHarness::build::<Kick>(&[
            ("pitch", ParameterValue::Float(55.0)),
            ("decay", ParameterValue::Float(0.1)),
            ("sweep", ParameterValue::Float(2500.0)),
            ("sweep_time", ParameterValue::Float(0.04)),
            ("drive", ParameterValue::Float(0.0)),
            ("click", ParameterValue::Float(0.3)),
        ]);
        h.disconnect_inputs(&["voct", "velocity"]);

        // Trigger
        h.set_mono("trigger", 1.0);
        h.tick();
        h.set_mono("trigger", 0.0);

        // Let it decay for 0.5s (well past 0.1s decay)
        for _ in 0..22050 {
            h.tick();
        }

        // Last 100 samples should be near zero
        let rms = h.measure_rms(100, "out");
        assert!(rms < 0.01, "kick should decay to near zero, rms = {rms}");
    }

    /// Higher pitch should produce more zero-crossings per unit time.
    /// Crossing count is a coarse frequency proxy — only the ordering is
    /// asserted, not absolute values.
    #[test]
    fn pitch_parameter_affects_output() {
        fn crossings_at_pitch(pitch: f32) -> usize {
            let mut h = ModuleHarness::build::<Kick>(&[
                ("pitch", ParameterValue::Float(pitch)),
                ("sweep", ParameterValue::Float(pitch)), // no sweep
                ("sweep_time", ParameterValue::Float(0.001)),
                ("decay", ParameterValue::Float(0.5)),
                ("drive", ParameterValue::Float(0.0)),
                ("click", ParameterValue::Float(0.0)),
            ]);
            h.disconnect_inputs(&["voct", "velocity"]);
            h.set_mono("trigger", 1.0);
            h.tick();
            h.set_mono("trigger", 0.0);
            let s = h.run_mono(1000, "out");
            s.windows(2).filter(|w| (w[0] >= 0.0) != (w[1] >= 0.0)).count()
        }
        let low = crossings_at_pitch(40.0);
        let high = crossings_at_pitch(120.0);
        assert!(high > low, "higher pitch should have more zero crossings: low={low}, high={high}");
    }

    #[test]
    fn velocity_scales_output() {
        let mut h_full = make_kick();
        h_full.set_mono("trigger", 1.0);
        h_full.tick();
        h_full.set_mono("trigger", 0.0);
        let rms_full = h_full.measure_rms(2000, "out");

        let mut h_half = ModuleHarness::build::<Kick>(&[
            ("pitch", ParameterValue::Float(55.0)),
            ("sweep", ParameterValue::Float(2500.0)),
            ("sweep_time", ParameterValue::Float(0.04)),
            ("decay", ParameterValue::Float(0.5)),
            ("drive", ParameterValue::Float(0.0)),
            ("click", ParameterValue::Float(0.3)),
        ]);
        h_half.disconnect_input("voct");
        h_half.set_mono("velocity", 0.5);
        h_half.set_mono("trigger", 1.0);
        h_half.tick();
        h_half.set_mono("trigger", 0.0);
        let rms_half = h_half.measure_rms(2000, "out");

        let ratio = rms_half / rms_full;
        assert!(
            (ratio - 0.5).abs() < 0.1,
            "half velocity should roughly halve output: ratio = {ratio}"
        );
    }

    #[test]
    fn velocity_disconnected_defaults_to_full() {
        let mut h = make_kick();
        h.set_mono("trigger", 1.0);
        h.tick();
        h.set_mono("trigger", 0.0);
        let rms = h.measure_rms(2000, "out");
        assert!(rms > 0.01, "disconnected velocity should give full output, rms = {rms}");
    }

    #[test]
    fn voct_overrides_sweep_start() {
        // voct value that maps to ~5000 Hz sweep start
        let voct_for_5000 = (5000.0f32 / 16.351_598).log2();

        // Kick with low sweep param but voct overriding to high sweep start
        let mut h_voct = ModuleHarness::build::<Kick>(&[
            ("pitch", ParameterValue::Float(55.0)),
            ("sweep", ParameterValue::Float(500.0)),
            ("sweep_time", ParameterValue::Float(0.04)),
            ("decay", ParameterValue::Float(0.5)),
            ("drive", ParameterValue::Float(0.0)),
            ("click", ParameterValue::Float(0.0)),
        ]);
        h_voct.disconnect_input("velocity");
        h_voct.set_mono("voct", voct_for_5000);

        // Same kick without voct — sweep starts at 500 Hz
        let mut h_no_voct = ModuleHarness::build::<Kick>(&[
            ("pitch", ParameterValue::Float(55.0)),
            ("sweep", ParameterValue::Float(500.0)),
            ("sweep_time", ParameterValue::Float(0.04)),
            ("decay", ParameterValue::Float(0.5)),
            ("drive", ParameterValue::Float(0.0)),
            ("click", ParameterValue::Float(0.0)),
        ]);
        h_no_voct.disconnect_inputs(&["voct", "velocity"]);

        // Let periodic update run before triggering
        for _ in 0..64 { h_voct.tick(); }
        for _ in 0..64 { h_no_voct.tick(); }

        // Trigger both
        h_voct.set_mono("trigger", 1.0);
        h_voct.tick();
        h_voct.set_mono("trigger", 0.0);
        h_no_voct.set_mono("trigger", 1.0);
        h_no_voct.tick();
        h_no_voct.set_mono("trigger", 0.0);

        // Measure first 200 samples — the sweep transient
        let voct_samples = h_voct.run_mono(200, "out");
        let no_voct_samples = h_no_voct.run_mono(200, "out");

        let count_crossings = |s: &[f32]| -> usize {
            s.windows(2).filter(|w| (w[0] >= 0.0) != (w[1] >= 0.0)).count()
        };

        let voct_crossings = count_crossings(&voct_samples);
        let no_voct_crossings = count_crossings(&no_voct_samples);

        // Higher sweep start from voct should produce more crossings in the transient
        assert!(
            voct_crossings > no_voct_crossings,
            "voct sweep start at 5000 Hz should have more transient crossings than 500 Hz: voct={voct_crossings}, no_voct={no_voct_crossings}"
        );
    }
}
