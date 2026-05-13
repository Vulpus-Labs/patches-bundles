/// Claves synthesiser.
///
/// A short, bright, resonant click produced by exciting a high-Q bandpass SVF
/// with a single-sample impulse and shaping with a fast decay envelope.
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
/// | Port  | Kind | Description   |
/// |-------|------|---------------|
/// | `out` | mono | Claves signal |
///
/// # Parameters
///
/// | Name    | Type  | Range        | Default | Description              |
/// |---------|-------|--------------|---------|--------------------------|
/// | `pitch` | float | 200–5000 Hz  | 2500    | Resonant frequency       |
/// | `decay` | float | 0.01–0.5 s   | 0.06    | Amplitude decay time     |
/// | `reson` | float | 0.3–1.0      | 0.85    | Bandpass resonance / ring |
use patches_sdk::{
    AudioEnvironment, CablePool, InputPort, InstanceId, Module, ModuleDescriptor,
    MonoInput, MonoOutput, OutputPort, ParameterKind,
};
use patches_sdk::modules::{CountAxis, ModuleDescriptorTemplate, ParameterTemplate, PortTemplate};
use patches_sdk::{StructuralParams, BuildError};
use patches_sdk::cables::TriggerInput;
use patches_sdk::param_frame::ParamView;
use patches_sdk::module_params;
use crate::primitives::DecayEnvelope;
use patches_dsp::{SvfKernel, svf_f, q_to_damp};

module_params! {
    Claves {
        pitch: Float,
        decay: Float,
        reson: Float,
    }
}

pub struct Claves {
    instance_id: InstanceId,
    descriptor: ModuleDescriptor,
    sample_rate: f32,
    pitch: f32,
    decay_time: f32,
    reson: f32,
    latched_velocity: f32,
    bp_filter: SvfKernel,
    amp_env: DecayEnvelope,
    impulse_pending: bool,
    in_trigger: TriggerInput,
    in_velocity: MonoInput,
    out_audio: MonoOutput,
}

impl Module for Claves {
    fn template() -> ModuleDescriptorTemplate {
        const T: ModuleDescriptorTemplate = ModuleDescriptorTemplate {
            name: "Claves",
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
                    kind: ParameterKind::Float { min: 200.0, max: 5000.0, default: 2500.0 },
                },
                ParameterTemplate {
                    name: params::decay.as_str(),
                    kind: ParameterKind::Float { min: 0.01, max: 0.5, default: 0.06 },
                },
                ParameterTemplate {
                    name: params::reson.as_str(),
                    kind: ParameterKind::Float { min: 0.3, max: 1.0, default: 0.85 },
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
        amp_env.set_decay(0.06);
        let f = svf_f(2500.0, sr);
        let d = q_to_damp(0.85);
        Self {
            instance_id,
            descriptor,
            sample_rate: sr,
            pitch: 2500.0,
            decay_time: 0.06,
            reson: 0.85,
            latched_velocity: 1.0,
            bp_filter: SvfKernel::new_static(f, d),
            amp_env,
            impulse_pending: false,
            in_trigger: TriggerInput::default(),
            in_velocity: MonoInput::default(),
            out_audio: MonoOutput::default(),
        }
    })}

    fn update_validated_parameters(&mut self, p: &ParamView<'_>) {
        let v = p.get(params::pitch);
        self.pitch = v;
        let v = p.get(params::decay);
        self.decay_time = v;
        self.amp_env.set_decay(self.decay_time);
        let v = p.get(params::reson);
        self.reson = v;
        let f = svf_f(self.pitch, self.sample_rate);
        let d = q_to_damp(self.reson);
        self.bp_filter = SvfKernel::new_static(f, d);
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
            self.impulse_pending = true;
            self.bp_filter.reset_state();
        }

        let amp = self.amp_env.tick(trigger_rose);

        // Feed impulse (or zero) into resonant bandpass
        let input = if self.impulse_pending {
            self.impulse_pending = false;
            1.0
        } else {
            0.0
        };

        let (_lp, _hp, bp) = self.bp_filter.tick(input);
        let output = bp * amp;

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
        let mut h = ModuleHarness::build::<Claves>(&[]);
        h.disconnect_input("velocity");
        h.set_mono("trigger", 1.0);
        h.tick();
        h.set_mono("trigger", 0.0);
        let rms = h.measure_rms(500, "out");
        assert!(rms > 0.001, "claves should produce output, rms = {rms}");
    }

    #[test]
    fn pitch_tracking() {
        let mut h_low = ModuleHarness::build::<Claves>(&[
            ("pitch", ParameterValue::Float(500.0)),
            ("reson", ParameterValue::Float(0.9)),
        ]);
        h_low.disconnect_input("velocity");
        let mut h_high = ModuleHarness::build::<Claves>(&[
            ("pitch", ParameterValue::Float(4000.0)),
            ("reson", ParameterValue::Float(0.9)),
        ]);
        h_high.disconnect_input("velocity");

        h_low.set_mono("trigger", 1.0);
        h_low.tick();
        h_low.set_mono("trigger", 0.0);
        h_high.set_mono("trigger", 1.0);
        h_high.tick();
        h_high.set_mono("trigger", 0.0);

        let low_samples = h_low.run_mono(500, "out");
        let high_samples = h_high.run_mono(500, "out");

        let count_crossings = |s: &[f32]| -> usize {
            s.windows(2).filter(|w| (w[0] >= 0.0) != (w[1] >= 0.0)).count()
        };

        assert!(
            count_crossings(&high_samples) > count_crossings(&low_samples),
            "higher pitch claves should have more zero crossings"
        );
    }

    #[test]
    fn output_decays() {
        let mut h = ModuleHarness::build::<Claves>(&[
            ("decay", ParameterValue::Float(0.02)),
        ]);
        h.disconnect_input("velocity");
        h.set_mono("trigger", 1.0);
        h.tick();
        h.set_mono("trigger", 0.0);

        for _ in 0..4410 {
            h.tick();
        }
        let rms = h.measure_rms(100, "out");
        assert!(rms < 0.01, "claves should decay, rms = {rms}");
    }
}
