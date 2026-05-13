/// 808-style tom synthesiser.
///
/// Shares the kick's basic architecture (sine oscillator + pitch sweep +
/// amplitude decay) but with a higher pitch range, shorter sweep, and a
/// subtle noise layer for attack texture.
///
/// # Inputs
///
/// | Port       | Kind | Description                                                                                      |
/// |------------|------|--------------------------------------------------------------------------------------------------|
/// | `trigger`  | mono | Rising edge triggers                                                                             |
/// | `velocity` | mono | Velocity (0.0–1.0); latched on trigger, scales output amplitude. Defaults to 1.0 when disconnected |
///
/// # Outputs
///
/// | Port  | Kind | Description |
/// |-------|------|-------------|
/// | `out` | mono | Tom signal  |
///
/// # Parameters
///
/// | Name         | Type  | Range       | Default | Description              |
/// |--------------|-------|-------------|---------|--------------------------|
/// | `pitch`      | float | 40–500 Hz   | 120     | Base pitch               |
/// | `sweep`      | float | 0–2000 Hz   | 400     | Pitch sweep start offset |
/// | `sweep_time` | float | 0.001–0.3 s | 0.03    | Pitch sweep duration     |
/// | `decay`      | float | 0.05–2.0 s  | 0.3     | Amplitude decay time     |
/// | `noise`      | float | 0.0–1.0     | 0.15    | Noise layer amount       |
use patches_sdk::{
    AudioEnvironment, CablePool, InputPort, InstanceId, Module, ModuleDescriptor,
    MonoInput, MonoOutput, OutputPort, ParameterKind,
};
use patches_sdk::modules::{CountAxis, ModuleDescriptorTemplate, ParameterTemplate, PortTemplate};
use patches_sdk::{StructuralParams, BuildError};
use patches_sdk::cables::TriggerInput;
use patches_sdk::module_params;
use patches_sdk::param_frame::ParamView;
use crate::primitives::{DecayEnvelope, PitchSweep};

module_params! {
    Tom {
        pitch:      Float,
        sweep:      Float,
        sweep_time: Float,
        decay:      Float,
        noise:      Float,
    }
}
use patches_dsp::{MonoPhaseAccumulator, xorshift64};

pub struct Tom {
    instance_id: InstanceId,
    descriptor: ModuleDescriptor,
    sample_rate: f32,
    // Parameters
    pitch: f32,
    sweep_start: f32,
    sweep_time: f32,
    decay_time: f32,
    noise_amt: f32,
    latched_velocity: f32,
    // DSP state
    osc: MonoPhaseAccumulator,
    pitch_sweep: PitchSweep,
    amp_env: DecayEnvelope,
    noise_env: DecayEnvelope,
    prng_state: u64,
    // Ports
    in_trigger: TriggerInput,
    in_velocity: MonoInput,
    out_audio: MonoOutput,
}

impl Module for Tom {
    fn template() -> ModuleDescriptorTemplate {
        const T: ModuleDescriptorTemplate = ModuleDescriptorTemplate {
            name: "Tom",
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
                    name: params::pitch.as_str(),
                    kind: ParameterKind::Float { min: 40.0, max: 500.0, default: 120.0 },
                },
                ParameterTemplate {
                    name: params::sweep.as_str(),
                    kind: ParameterKind::Float { min: 0.0, max: 2000.0, default: 400.0 },
                },
                ParameterTemplate {
                    name: params::sweep_time.as_str(),
                    kind: ParameterKind::Float { min: 0.001, max: 0.3, default: 0.03 },
                },
                ParameterTemplate {
                    name: params::decay.as_str(),
                    kind: ParameterKind::Float { min: 0.05, max: 2.0, default: 0.3 },
                },
                ParameterTemplate {
                    name: params::noise.as_str(),
                    kind: ParameterKind::Float { min: 0.0, max: 1.0, default: 0.15 },
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
        amp_env.set_decay(0.3);
        let mut noise_env = DecayEnvelope::new(sr);
        noise_env.set_decay(0.01);
        let mut pitch_sweep = PitchSweep::new(sr);
        pitch_sweep.set_params(520.0, 120.0, 0.03);
        Self {
            instance_id,
            descriptor,
            sample_rate: sr,
            pitch: 120.0,
            sweep_start: 400.0,
            sweep_time: 0.03,
            decay_time: 0.3,
            noise_amt: 0.15,
            latched_velocity: 1.0,
            osc: MonoPhaseAccumulator::new(),
            pitch_sweep,
            amp_env,
            noise_env,
            prng_state: instance_id.as_u64() + 1,
            in_trigger: TriggerInput::default(),
            in_velocity: MonoInput::default(),
            out_audio: MonoOutput::default(),
        }
    })}

    fn update_validated_parameters(&mut self, p: &ParamView<'_>) {
        self.pitch = p.get(params::pitch);
        self.sweep_start = p.get(params::sweep);
        self.sweep_time = p.get(params::sweep_time);
        self.decay_time = p.get(params::decay);
        self.amp_env.set_decay(self.decay_time);
        self.noise_amt = p.get(params::noise);
        self.pitch_sweep.set_params(self.pitch + self.sweep_start, self.pitch, self.sweep_time);
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
        let noise_amp = self.noise_env.tick(trigger_rose);

        // Sine oscillator
        let increment = freq / self.sample_rate;
        self.osc.set_increment(increment);
        let phase = self.osc.phase;
        let sine = (phase * std::f32::consts::TAU).sin();
        self.osc.advance();

        // Noise attack texture
        let white = xorshift64(&mut self.prng_state);
        let noise = white * noise_amp * self.noise_amt;

        let output = (sine * amp) + noise;

        pool.write_mono(&self.out_audio, output * self.latched_velocity);
    }

    fn as_any(&self) -> &dyn std::any::Any { self }
}

#[cfg(test)]
mod tests {
    use patches_sdk::ParameterValue;
    use super::*;
    use patches_sdk::test_support::ModuleHarness;

    #[test]
    fn trigger_produces_output() {
        let mut h = ModuleHarness::build::<Tom>(&[]);
        h.disconnect_input("velocity");
        h.set_mono("trigger", 1.0);
        h.tick();
        h.set_mono("trigger", 0.0);
        let rms = h.measure_rms(2000, "out");
        assert!(rms > 0.01, "tom should produce output, rms = {rms}");
    }

    #[test]
    fn pitch_tracking() {
        let mut h_low = ModuleHarness::build::<Tom>(&[
            ("pitch", ParameterValue::Float(60.0)),
            ("sweep", ParameterValue::Float(0.0)),
            ("noise", ParameterValue::Float(0.0)),
        ]);
        h_low.disconnect_input("velocity");
        let mut h_high = ModuleHarness::build::<Tom>(&[
            ("pitch", ParameterValue::Float(300.0)),
            ("sweep", ParameterValue::Float(0.0)),
            ("noise", ParameterValue::Float(0.0)),
        ]);
        h_high.disconnect_input("velocity");

        h_low.set_mono("trigger", 1.0);
        h_low.tick();
        h_low.set_mono("trigger", 0.0);
        h_high.set_mono("trigger", 1.0);
        h_high.tick();
        h_high.set_mono("trigger", 0.0);

        let low_samples = h_low.run_mono(1000, "out");
        let high_samples = h_high.run_mono(1000, "out");

        let count_crossings = |s: &[f32]| -> usize {
            s.windows(2).filter(|w| (w[0] >= 0.0) != (w[1] >= 0.0)).count()
        };

        assert!(
            count_crossings(&high_samples) > count_crossings(&low_samples),
            "higher pitch tom should have more zero crossings"
        );
    }

    #[test]
    fn output_decays() {
        let mut h = ModuleHarness::build::<Tom>(&[
            ("decay", ParameterValue::Float(0.05)),
            ("noise", ParameterValue::Float(0.0)),
        ]);
        h.disconnect_input("velocity");
        h.set_mono("trigger", 1.0);
        h.tick();
        h.set_mono("trigger", 0.0);

        for _ in 0..22050 {
            h.tick();
        }
        let rms = h.measure_rms(100, "out");
        assert!(rms < 0.01, "tom should decay, rms = {rms}");
    }

    #[test]
    fn fundamental_bin_matches_pitch() {
        use crate::test_support::{dominant_bin, freq_to_bin, magnitude_spectrum};
        // sweep=0 holds pitch steady; long decay keeps the tail audible.
        let pitch_hz = 200.0;
        let mut h = ModuleHarness::build::<Tom>(&[
            ("pitch", ParameterValue::Float(pitch_hz)),
            ("sweep", ParameterValue::Float(0.0)),
            ("decay", ParameterValue::Float(2.0)),
            ("noise", ParameterValue::Float(0.0)),
        ]);
        h.disconnect_input("velocity");
        h.set_mono("trigger", 1.0);
        h.tick();
        h.set_mono("trigger", 0.0);
        // Skip the sweep window (~30 ms) and the noise attack.
        for _ in 0..1500 {
            h.tick();
        }
        let samples = h.run_mono(4096, "out");
        let spec = magnitude_spectrum(&samples, 4096);
        let peak = dominant_bin(&spec);
        let expected = freq_to_bin(pitch_hz, 44100.0, 4096);
        // ±3 bins ≈ ±32 Hz at 4096/44100. Pitch-sweep tail may still drift a bit.
        let diff = peak.abs_diff(expected);
        assert!(
            diff <= 3,
            "tom fundamental bin = {peak}, expected {expected} (pitch {pitch_hz} Hz)"
        );
    }

    #[test]
    fn envelope_decays_monotonically() {
        use crate::test_support::windowed_rms;
        let mut h = ModuleHarness::build::<Tom>(&[
            ("pitch", ParameterValue::Float(120.0)),
            ("sweep", ParameterValue::Float(0.0)),
            ("decay", ParameterValue::Float(0.5)),
            ("noise", ParameterValue::Float(0.0)),
        ]);
        h.disconnect_input("velocity");
        h.set_mono("trigger", 1.0);
        h.tick();
        h.set_mono("trigger", 0.0);
        let samples = h.run_mono(8192, "out");
        let rms = windowed_rms(&samples, 512);
        // Skip the first block (initial attack); require overall non-increasing
        // trend with a small tolerance for sine-phase jitter in short windows.
        let mut prev = rms[1];
        for (i, &r) in rms.iter().enumerate().skip(2) {
            assert!(
                r <= prev * 1.05,
                "tom envelope not monotonic at block {i}: {r} > {prev}"
            );
            prev = r;
        }
    }
}
