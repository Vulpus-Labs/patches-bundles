/// Closed hi-hat synthesiser.
///
/// Metallic tone from six inharmonic square oscillators mixed with
/// highpass-filtered white noise, shaped by a short decay envelope.
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
/// | Port  | Kind | Description       |
/// |-------|------|-------------------|
/// | `out` | mono | Closed hat signal |
///
/// # Parameters
///
/// | Name     | Type  | Range         | Default | Description                     |
/// |----------|-------|---------------|---------|---------------------------------|
/// | `pitch`  | float | 100–8000 Hz   | 400     | Base frequency of metallic tone |
/// | `decay`  | float | 0.005–0.2 s   | 0.04    | Amplitude decay time            |
/// | `tone`   | float | 0.0–1.0       | 0.5     | Metallic vs noise mix           |
/// | `filter` | float | 2000–16000 Hz | 8000    | Noise highpass cutoff           |
use patches_sdk::{
    AudioEnvironment, CablePool, InputPort, InstanceId, Module, ModuleDescriptor,
    MonoInput, MonoOutput, OutputPort, ParameterKind,
};
use patches_sdk::modules::{CountAxis, ModuleDescriptorTemplate, ParameterTemplate, PortTemplate};
use patches_sdk::{StructuralParams, BuildError};
use patches_sdk::cables::TriggerInput;
use patches_sdk::param_frame::ParamView;
use patches_sdk::module_params;
use crate::primitives::{DecayEnvelope, MetallicTone};
use patches_dsp::{SvfKernel, svf_f, q_to_damp, xorshift64};

module_params! {
    HiHat {
        pitch:  Float,
        decay:  Float,
        tone:   Float,
        filter: Float,
    }
}

pub struct ClosedHiHat {
    instance_id: InstanceId,
    descriptor: ModuleDescriptor,
    sample_rate: f32,
    pitch: f32,
    decay_time: f32,
    tone: f32,
    filter_freq: f32,
    latched_velocity: f32,
    metallic: MetallicTone,
    amp_env: DecayEnvelope,
    hp_filter: SvfKernel,
    prng_state: u64,
    in_trigger: TriggerInput,
    in_velocity: MonoInput,
    out_audio: MonoOutput,
}

impl Module for ClosedHiHat {
    fn template() -> ModuleDescriptorTemplate {
        const T: ModuleDescriptorTemplate = ModuleDescriptorTemplate {
            name: "ClosedHiHat",
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
                    kind: ParameterKind::Float { min: 100.0, max: 8000.0, default: 400.0 },
                },
                ParameterTemplate {
                    name: params::decay.as_str(),
                    kind: ParameterKind::Float { min: 0.005, max: 0.2, default: 0.04 },
                },
                ParameterTemplate {
                    name: params::tone.as_str(),
                    kind: ParameterKind::Float { min: 0.0, max: 1.0, default: 0.5 },
                },
                ParameterTemplate {
                    name: params::filter.as_str(),
                    kind: ParameterKind::Float { min: 2000.0, max: 16000.0, default: 8000.0 },
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
        amp_env.set_decay(0.04);
        let mut metallic = MetallicTone::new(sr);
        metallic.set_frequency(400.0);
        let f = svf_f(8000.0, sr);
        let d = q_to_damp(0.3);
        Self {
            instance_id,
            descriptor,
            sample_rate: sr,
            pitch: 400.0,
            decay_time: 0.04,
            tone: 0.5,
            filter_freq: 8000.0,
            latched_velocity: 1.0,
            metallic,
            amp_env,
            hp_filter: SvfKernel::new_static(f, d),
            prng_state: instance_id.as_u64() + 1,
            in_trigger: TriggerInput::default(),
            in_velocity: MonoInput::default(),
            out_audio: MonoOutput::default(),
        }
    })}

    fn update_validated_parameters(&mut self, p: &ParamView<'_>) {
        let v = p.get(params::pitch);
        self.pitch = v;
        self.metallic.set_frequency(self.pitch);
        let v = p.get(params::decay);
        self.decay_time = v;
        self.amp_env.set_decay(self.decay_time);
        let v = p.get(params::tone);
        self.tone = v;
        let v = p.get(params::filter);
        self.filter_freq = v;
        let f = svf_f(self.filter_freq, self.sample_rate);
        let d = q_to_damp(0.3);
        self.hp_filter = SvfKernel::new_static(f, d);
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
            self.metallic.trigger();
        }

        let amp = self.amp_env.tick(trigger_rose);

        let metal = self.metallic.tick();
        let white = xorshift64(&mut self.prng_state);
        let (_lp, hp, _bp) = self.hp_filter.tick(white);

        let mix = metal * self.tone + hp * (1.0 - self.tone);
        let output = mix * amp;

        pool.write_mono(&self.out_audio, output * self.latched_velocity);
    }

    fn as_any(&self) -> &dyn std::any::Any { self }
}

/// Open hi-hat synthesiser.
///
/// Same metallic tone engine as closed hi-hat but with a longer decay range.
/// Includes a `choke` input so a closed hi-hat trigger can cut it short.
///
/// # Inputs
///
/// | Port       | Kind | Description                                                                                      |
/// |------------|------|--------------------------------------------------------------------------------------------------|
/// | `trigger`  | mono | Rising edge triggers                                                                             |
/// | `choke`    | mono | Rising edge chokes (cuts) the sound                                                              |
/// | `velocity` | mono | Velocity (0.0–1.0); latched on trigger, scales output amplitude. Defaults to 1.0 when disconnected |
///
/// # Outputs
///
/// | Port  | Kind | Description     |
/// |-------|------|-----------------|
/// | `out` | mono | Open hat signal |
///
/// # Parameters
///
/// | Name     | Type  | Range         | Default | Description                     |
/// |----------|-------|---------------|---------|---------------------------------|
/// | `pitch`  | float | 100–8000 Hz   | 400     | Base frequency of metallic tone |
/// | `decay`  | float | 0.05–4.0 s    | 0.5     | Amplitude decay time            |
/// | `tone`   | float | 0.0–1.0       | 0.5     | Metallic vs noise mix           |
/// | `filter` | float | 2000–16000 Hz | 8000    | Noise highpass cutoff           |
pub struct OpenHiHat {
    instance_id: InstanceId,
    descriptor: ModuleDescriptor,
    sample_rate: f32,
    pitch: f32,
    decay_time: f32,
    tone: f32,
    filter_freq: f32,
    latched_velocity: f32,
    metallic: MetallicTone,
    amp_env: DecayEnvelope,
    hp_filter: SvfKernel,
    prng_state: u64,
    in_trigger: TriggerInput,
    in_choke: TriggerInput,
    in_velocity: MonoInput,
    out_audio: MonoOutput,
}

impl Module for OpenHiHat {
    fn template() -> ModuleDescriptorTemplate {
        const T: ModuleDescriptorTemplate = ModuleDescriptorTemplate {
            name: "OpenHiHat",
            axes: &[CountAxis::CHANNELS],
            global_inputs: &[
                PortTemplate::trigger("trigger"),
                PortTemplate::trigger("choke"),
                PortTemplate::mono("velocity"),
            ],
            per_axis_inputs: &[],
            global_outputs: &[PortTemplate::mono("out")],
            per_axis_outputs: &[],
            realtime_params: &[
                ParameterTemplate {
                    name: params::pitch.as_str(),
                    kind: ParameterKind::Float { min: 100.0, max: 8000.0, default: 400.0 },
                },
                ParameterTemplate {
                    name: params::decay.as_str(),
                    kind: ParameterKind::Float { min: 0.05, max: 4.0, default: 0.5 },
                },
                ParameterTemplate {
                    name: params::tone.as_str(),
                    kind: ParameterKind::Float { min: 0.0, max: 1.0, default: 0.5 },
                },
                ParameterTemplate {
                    name: params::filter.as_str(),
                    kind: ParameterKind::Float { min: 2000.0, max: 16000.0, default: 8000.0 },
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
        let mut metallic = MetallicTone::new(sr);
        metallic.set_frequency(400.0);
        let f = svf_f(8000.0, sr);
        let d = q_to_damp(0.3);
        Self {
            instance_id,
            descriptor,
            sample_rate: sr,
            pitch: 400.0,
            decay_time: 0.5,
            tone: 0.5,
            filter_freq: 8000.0,
            latched_velocity: 1.0,
            metallic,
            amp_env,
            hp_filter: SvfKernel::new_static(f, d),
            prng_state: instance_id.as_u64() + 1,
            in_trigger: TriggerInput::default(),
            in_choke: TriggerInput::default(),
            in_velocity: MonoInput::default(),
            out_audio: MonoOutput::default(),
        }
    })}

    fn update_validated_parameters(&mut self, p: &ParamView<'_>) {
        let v = p.get(params::pitch);
        self.pitch = v;
        self.metallic.set_frequency(self.pitch);
        let v = p.get(params::decay);
        self.decay_time = v;
        self.amp_env.set_decay(self.decay_time);
        let v = p.get(params::tone);
        self.tone = v;
        let v = p.get(params::filter);
        self.filter_freq = v;
        let f = svf_f(self.filter_freq, self.sample_rate);
        let d = q_to_damp(0.3);
        self.hp_filter = SvfKernel::new_static(f, d);
    }

    fn descriptor(&self) -> &ModuleDescriptor { &self.descriptor }
    fn instance_id(&self) -> InstanceId { self.instance_id }

    fn set_ports(&mut self, inputs: &[InputPort], outputs: &[OutputPort]) {
        self.in_trigger = TriggerInput::from_ports(inputs, 0);
        self.in_choke = TriggerInput::from_ports(inputs, 1);
        self.in_velocity = MonoInput::from_ports(inputs, 2);
        self.out_audio = MonoOutput::from_ports(outputs, 0);
    }

    fn process(&mut self, pool: &mut CablePool<'_>) {
        let trigger_rose = self.in_trigger.tick(pool).is_some();
        let choke_rose = self.in_choke.tick(pool).is_some();

        if trigger_rose {
            self.latched_velocity = if self.in_velocity.connected {
                pool.read_mono(&self.in_velocity)
            } else {
                1.0
            };
            self.metallic.trigger();
        }

        if choke_rose {
            self.amp_env.choke();
        }

        let amp = self.amp_env.tick(trigger_rose);

        let metal = self.metallic.tick();
        let white = xorshift64(&mut self.prng_state);
        let (_lp, hp, _bp) = self.hp_filter.tick(white);

        let mix = metal * self.tone + hp * (1.0 - self.tone);
        let output = mix * amp;

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
    fn closed_hihat_trigger_produces_output() {
        let mut h = ModuleHarness::build::<ClosedHiHat>(&[]);
        h.disconnect_input("velocity");
        h.set_mono("trigger", 1.0);
        h.tick();
        h.set_mono("trigger", 0.0);
        let rms = h.measure_rms(500, "out");
        assert!(rms > 0.001, "closed hihat should produce output, rms = {rms}");
    }

    #[test]
    fn closed_hihat_velocity_scales_output() {
        let mut h_full = ModuleHarness::build::<ClosedHiHat>(&[]);
        h_full.disconnect_input("velocity");
        h_full.set_mono("trigger", 1.0);
        h_full.tick();
        h_full.set_mono("trigger", 0.0);
        let rms_full = h_full.measure_rms(500, "out");

        let mut h_half = ModuleHarness::build::<ClosedHiHat>(&[]);
        h_half.set_mono("velocity", 0.5);
        h_half.set_mono("trigger", 1.0);
        h_half.tick();
        h_half.set_mono("trigger", 0.0);
        let rms_half = h_half.measure_rms(500, "out");

        let ratio = rms_half / rms_full;
        assert!(
            (ratio - 0.5).abs() < 0.1,
            "half velocity should roughly halve output: ratio = {ratio}"
        );
    }

    #[test]
    fn closed_hihat_short_decay() {
        let mut h = ModuleHarness::build::<ClosedHiHat>(&[
            ("decay", ParameterValue::Float(0.01)),
        ]);
        h.disconnect_input("velocity");
        h.set_mono("trigger", 1.0);
        h.tick();
        h.set_mono("trigger", 0.0);
        // After 0.1s (well past 0.01s decay)
        for _ in 0..4410 {
            h.tick();
        }
        let rms = h.measure_rms(100, "out");
        assert!(rms < 0.01, "closed hihat should decay quickly, rms = {rms}");
    }

    #[test]
    fn open_hihat_trigger_produces_output() {
        let mut h = ModuleHarness::build::<OpenHiHat>(&[]);
        h.disconnect_input("velocity");
        h.set_mono("trigger", 1.0);
        h.tick();
        h.set_mono("trigger", 0.0);
        let rms = h.measure_rms(2000, "out");
        assert!(rms > 0.001, "open hihat should produce output, rms = {rms}");
    }

    #[test]
    fn open_hihat_choke_silences() {
        let mut h = ModuleHarness::build::<OpenHiHat>(&[
            ("decay", ParameterValue::Float(2.0)),
        ]);
        h.disconnect_input("velocity");
        // Trigger
        h.set_mono("trigger", 1.0);
        h.tick();
        h.set_mono("trigger", 0.0);

        // Let it ring for a bit
        for _ in 0..500 {
            h.tick();
        }

        // Verify it's still producing output
        let rms_before = h.measure_rms(100, "out");
        assert!(rms_before > 0.001, "should still be ringing before choke");

        // Choke
        h.set_mono("choke", 1.0);
        h.tick();
        h.set_mono("choke", 0.0);

        // Should be silent
        let rms_after = h.measure_rms(100, "out");
        assert!(rms_after < 0.001, "should be silent after choke, rms = {rms_after}");
    }

    #[test]
    fn open_hihat_longer_than_closed() {
        let mut h_closed = ModuleHarness::build::<ClosedHiHat>(&[
            ("decay", ParameterValue::Float(0.04)),
        ]);
        h_closed.disconnect_input("velocity");
        let mut h_open = ModuleHarness::build::<OpenHiHat>(&[
            ("decay", ParameterValue::Float(0.5)),
        ]);
        h_open.disconnect_input("velocity");

        // Trigger both
        h_closed.set_mono("trigger", 1.0);
        h_closed.tick();
        h_closed.set_mono("trigger", 0.0);
        h_open.set_mono("trigger", 1.0);
        h_open.tick();
        h_open.set_mono("trigger", 0.0);

        // Measure RMS at 0.1s
        for _ in 0..4410 {
            h_closed.tick();
            h_open.tick();
        }

        let rms_closed = h_closed.measure_rms(200, "out");
        let rms_open = h_open.measure_rms(200, "out");
        assert!(
            rms_open > rms_closed,
            "open hat should ring longer: open={rms_open}, closed={rms_closed}"
        );
    }

    #[test]
    fn closed_hihat_spectrum_is_hf_dominant() {
        use crate::test_support::{band_energy, magnitude_spectrum};
        let mut h = ModuleHarness::build::<ClosedHiHat>(&[
            ("decay", ParameterValue::Float(0.2)),
        ]);
        h.disconnect_input("velocity");
        h.set_mono("trigger", 1.0);
        h.tick();
        h.set_mono("trigger", 0.0);
        let samples = h.run_mono(4096, "out");
        let spec = magnitude_spectrum(&samples, 4096);
        let lf = band_energy(&spec, 44100.0, 4096, 20.0, 500.0);
        let hf = band_energy(&spec, 44100.0, 4096, 2000.0, 20000.0);
        assert!(
            hf > 4.0 * lf,
            "closed hihat should be HF-dominant: hf={hf}, lf={lf}"
        );
    }

    #[test]
    fn open_hihat_spectrum_is_hf_dominant() {
        use crate::test_support::{band_energy, magnitude_spectrum};
        let mut h = ModuleHarness::build::<OpenHiHat>(&[
            ("decay", ParameterValue::Float(0.5)),
        ]);
        h.disconnect_input("velocity");
        h.set_mono("trigger", 1.0);
        h.tick();
        h.set_mono("trigger", 0.0);
        let samples = h.run_mono(4096, "out");
        let spec = magnitude_spectrum(&samples, 4096);
        let lf = band_energy(&spec, 44100.0, 4096, 20.0, 500.0);
        let hf = band_energy(&spec, 44100.0, 4096, 2000.0, 20000.0);
        assert!(
            hf > 4.0 * lf,
            "open hihat should be HF-dominant: hf={hf}, lf={lf}"
        );
    }

    #[test]
    fn open_hihat_decay_exceeds_closed_ratio() {
        // Compare windowed-RMS decay envelopes on equivalent time scales.
        let mut h_closed = ModuleHarness::build::<ClosedHiHat>(&[
            ("decay", ParameterValue::Float(0.05)),
        ]);
        h_closed.disconnect_input("velocity");
        let mut h_open = ModuleHarness::build::<OpenHiHat>(&[
            ("decay", ParameterValue::Float(0.4)),
        ]);
        h_open.disconnect_input("velocity");

        h_closed.set_mono("trigger", 1.0);
        h_closed.tick();
        h_closed.set_mono("trigger", 0.0);
        h_open.set_mono("trigger", 1.0);
        h_open.tick();
        h_open.set_mono("trigger", 0.0);

        // At 0.2s, closed should be effectively silent while open still rings.
        for _ in 0..8820 {
            h_closed.tick();
            h_open.tick();
        }
        let rms_c = h_closed.measure_rms(500, "out");
        let rms_o = h_open.measure_rms(500, "out");
        assert!(
            rms_o > rms_c * 20.0,
            "open hat should ring far longer than closed: closed={rms_c}, open={rms_o}"
        );
    }
}
