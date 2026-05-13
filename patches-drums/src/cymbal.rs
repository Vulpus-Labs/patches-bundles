/// Cymbal synthesiser (crash/ride).
///
/// Uses the same metallic tone engine as hi-hats but with a higher frequency
/// range, longer decay, and a "shimmer" parameter that adds slow LFO
/// modulation to the partial frequencies.
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
/// | `out` | mono | Cymbal signal |
///
/// # Parameters
///
/// | Name      | Type  | Range         | Default | Description                        |
/// |-----------|-------|---------------|---------|------------------------------------|
/// | `pitch`   | float | 200–10000 Hz  | 600     | Base frequency of metallic tone    |
/// | `decay`   | float | 0.2–8.0 s     | 2.0     | Amplitude decay time               |
/// | `tone`    | float | 0.0–1.0       | 0.5     | Metallic vs noise mix              |
/// | `filter`  | float | 2000–16000 Hz | 6000    | Noise highpass cutoff              |
/// | `shimmer` | float | 0.0–1.0       | 0.2     | Partial frequency modulation depth |
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
    Cymbal {
        pitch:   Float,
        decay:   Float,
        tone:    Float,
        filter:  Float,
        shimmer: Float,
    }
}

pub struct Cymbal {
    instance_id: InstanceId,
    descriptor: ModuleDescriptor,
    sample_rate: f32,
    pitch: f32,
    decay_time: f32,
    tone: f32,
    filter_freq: f32,
    shimmer: f32,
    mod_depth: f32,
    latched_velocity: f32,
    metallic: MetallicTone,
    amp_env: DecayEnvelope,
    hp_filter: SvfKernel,
    prng_state: u64,
    lfo_phase: f32,
    lfo_increment: f32,
    in_trigger: TriggerInput,
    in_velocity: MonoInput,
    out_audio: MonoOutput,
}

impl Module for Cymbal {
    fn template() -> ModuleDescriptorTemplate {
        const T: ModuleDescriptorTemplate = ModuleDescriptorTemplate {
            name: "Cymbal",
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
                    kind: ParameterKind::Float { min: 200.0, max: 10000.0, default: 600.0 },
                },
                ParameterTemplate {
                    name: params::decay.as_str(),
                    kind: ParameterKind::Float { min: 0.2, max: 8.0, default: 2.0 },
                },
                ParameterTemplate {
                    name: params::tone.as_str(),
                    kind: ParameterKind::Float { min: 0.0, max: 1.0, default: 0.5 },
                },
                ParameterTemplate {
                    name: params::filter.as_str(),
                    kind: ParameterKind::Float { min: 2000.0, max: 16000.0, default: 6000.0 },
                },
                ParameterTemplate {
                    name: params::shimmer.as_str(),
                    kind: ParameterKind::Float { min: 0.0, max: 1.0, default: 0.2 },
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
        amp_env.set_decay(2.0);
        let mut metallic = MetallicTone::new(sr);
        metallic.set_frequency(600.0);
        let f = svf_f(6000.0, sr);
        let d = q_to_damp(0.3);
        // Shimmer LFO at ~3 Hz
        let lfo_increment = 3.0 / sr;
        Self {
            instance_id,
            descriptor,
            sample_rate: sr,
            pitch: 600.0,
            decay_time: 2.0,
            tone: 0.5,
            filter_freq: 6000.0,
            shimmer: 0.2,
            mod_depth: 0.2 * 20.0,
            latched_velocity: 1.0,
            metallic,
            amp_env,
            hp_filter: SvfKernel::new_static(f, d),
            prng_state: instance_id.as_u64() + 1,
            lfo_phase: 0.0,
            lfo_increment,
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
        let v = p.get(params::shimmer);
        self.shimmer = v;
        self.mod_depth = self.shimmer * 20.0;
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
            self.lfo_phase = 0.0;
        }

        let amp = self.amp_env.tick(trigger_rose);

        // Metallic tone with shimmer modulation
        let metal = self.metallic.tick_with_modulation(self.mod_depth, self.lfo_phase);

        // Advance LFO
        self.lfo_phase += self.lfo_increment;
        if self.lfo_phase >= 1.0 {
            self.lfo_phase -= 1.0;
        }

        // Highpass-filtered noise
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
    fn trigger_produces_output() {
        let mut h = ModuleHarness::build::<Cymbal>(&[]);
        h.disconnect_input("velocity");
        h.set_mono("trigger", 1.0);
        h.tick();
        h.set_mono("trigger", 0.0);
        let rms = h.measure_rms(5000, "out");
        assert!(rms > 0.001, "cymbal should produce output, rms = {rms}");
    }

    #[test]
    fn long_decay() {
        let mut h = ModuleHarness::build::<Cymbal>(&[
            ("decay", ParameterValue::Float(4.0)),
        ]);
        h.disconnect_input("velocity");
        h.set_mono("trigger", 1.0);
        h.tick();
        h.set_mono("trigger", 0.0);

        // At 1s in, should still be audible
        for _ in 0..44100 {
            h.tick();
        }
        let rms = h.measure_rms(1000, "out");
        assert!(rms > 0.001, "cymbal with 4s decay should still ring at 1s, rms = {rms}");
    }

    #[test]
    fn shimmer_produces_modulation() {
        // With shimmer=0 and shimmer=1, output should differ
        let mut h_no = ModuleHarness::build::<Cymbal>(&[
            ("shimmer", ParameterValue::Float(0.0)),
            ("tone", ParameterValue::Float(1.0)), // all metallic
        ]);
        h_no.disconnect_input("velocity");
        let mut h_yes = ModuleHarness::build::<Cymbal>(&[
            ("shimmer", ParameterValue::Float(1.0)),
            ("tone", ParameterValue::Float(1.0)),
        ]);
        h_yes.disconnect_input("velocity");

        h_no.set_mono("trigger", 1.0);
        h_no.tick();
        h_no.set_mono("trigger", 0.0);
        h_yes.set_mono("trigger", 1.0);
        h_yes.tick();
        h_yes.set_mono("trigger", 0.0);

        let s_no = h_no.run_mono(2000, "out");
        let s_yes = h_yes.run_mono(2000, "out");

        // They should differ (shimmer modulates frequencies)
        let diff: f32 = s_no.iter().zip(s_yes.iter()).map(|(a, b)| (a - b).abs()).sum::<f32>() / 2000.0;
        assert!(diff > 0.001, "shimmer should change the output, avg diff = {diff}");
    }

    #[test]
    fn spectrum_is_hf_dominant() {
        use crate::test_support::{band_energy, magnitude_spectrum};
        let mut h = ModuleHarness::build::<Cymbal>(&[
            ("decay", ParameterValue::Float(2.0)),
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
            "cymbal should be HF-dominant: hf={hf}, lf={lf}, ratio={}",
            hf / lf.max(1e-12),
        );
    }

    #[test]
    fn envelope_peak_is_early() {
        use crate::test_support::windowed_rms;
        let mut h = ModuleHarness::build::<Cymbal>(&[
            ("decay", ParameterValue::Float(1.0)),
        ]);
        h.disconnect_input("velocity");
        h.set_mono("trigger", 1.0);
        h.tick();
        h.set_mono("trigger", 0.0);
        let samples = h.run_mono(8192, "out");
        let rms = windowed_rms(&samples, 256);
        let (peak_idx, _) = rms
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .unwrap();
        assert!(
            peak_idx < rms.len() / 4,
            "cymbal envelope peak should occur in first quarter, peak_idx={peak_idx} of {}",
            rms.len(),
        );
        // And the tail should be quieter than the peak (sustained decay).
        let tail_rms = rms[rms.len() - 4..].iter().copied().sum::<f32>() / 4.0;
        assert!(tail_rms < rms[peak_idx] * 0.8, "tail not decaying past peak");
    }
}
