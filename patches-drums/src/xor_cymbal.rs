/// Cymbal synthesiser (E004) — XOR-pair flavour.
///
/// Sister to [`crate::cymbal::Cymbal`]; the two coexist. Same envelope,
/// same HP-noise mix, same shimmer LFO — the single substitution is the
/// generator: `MetallicTone` → [`XorPairTone`](crate::primitives::XorPairTone),
/// giving a coarser, denser inharmonic texture from the three pair
/// intermod products.
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
/// | `pitch`   | float | 200–10000 Hz  | 600     | Base frequency of XOR-pair tone    |
/// | `decay`   | float | 0.2–8.0 s     | 2.0     | Amplitude decay time               |
/// | `tone`    | float | 0.0–1.0       | 0.5     | XOR-pair vs noise mix              |
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
use crate::primitives::{DecayEnvelope, XorPairTone};
use patches_dsp::{SvfKernel, svf_f, q_to_damp, xorshift64};

module_params! {
    XorCymbal {
        pitch:   Float,
        decay:   Float,
        tone:    Float,
        filter:  Float,
        shimmer: Float,
    }
}

pub struct XorCymbal {
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
    xor: XorPairTone,
    amp_env: DecayEnvelope,
    hp_filter: SvfKernel,
    prng_state: u64,
    lfo_phase: f32,
    lfo_increment: f32,
    in_trigger: TriggerInput,
    in_velocity: MonoInput,
    out_audio: MonoOutput,
}

impl Module for XorCymbal {
    fn template() -> ModuleDescriptorTemplate {
        const T: ModuleDescriptorTemplate = ModuleDescriptorTemplate {
            name: "XorCymbal",
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
        let mut xor = XorPairTone::new(sr);
        xor.set_frequency(600.0);
        let f = svf_f(6000.0, sr);
        let d = q_to_damp(0.3);
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
            xor,
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
        let pitch = p.get(params::pitch);
        if pitch != self.pitch {
            self.pitch = pitch;
            self.xor.set_frequency(pitch);
        }
        let decay = p.get(params::decay);
        if decay != self.decay_time {
            self.decay_time = decay;
            self.amp_env.set_decay(decay);
        }
        self.tone = p.get(params::tone);
        let filter = p.get(params::filter);
        if filter != self.filter_freq {
            self.filter_freq = filter;
            let f = svf_f(filter, self.sample_rate);
            let d = q_to_damp(0.3);
            self.hp_filter.set_static(f, d);
        }
        let shimmer = p.get(params::shimmer);
        if shimmer != self.shimmer {
            self.shimmer = shimmer;
            self.mod_depth = shimmer * 20.0;
        }
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
            self.xor.trigger();
            self.lfo_phase = 0.0;
        }

        if !trigger_rose && self.amp_env.is_silent() {
            pool.write_mono(&self.out_audio, 0.0);
            return;
        }

        let amp = self.amp_env.tick(trigger_rose);

        let metal = self.xor.tick_with_modulation(self.mod_depth, self.lfo_phase);

        self.lfo_phase += self.lfo_increment;
        if self.lfo_phase >= 1.0 {
            self.lfo_phase -= 1.0;
        }

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
        let mut h = ModuleHarness::build::<XorCymbal>(&[]);
        h.disconnect_input("velocity");
        h.set_mono("trigger", 1.0);
        h.tick();
        h.set_mono("trigger", 0.0);
        let rms = h.measure_rms(5000, "out");
        assert!(rms > 0.001, "xor cymbal should produce output, rms = {rms}");
    }

    #[test]
    fn long_decay() {
        let mut h = ModuleHarness::build::<XorCymbal>(&[
            ("decay", ParameterValue::Float(4.0)),
        ]);
        h.disconnect_input("velocity");
        h.set_mono("trigger", 1.0);
        h.tick();
        h.set_mono("trigger", 0.0);

        for _ in 0..44100 {
            h.tick();
        }
        let rms = h.measure_rms(1000, "out");
        assert!(rms > 0.001, "xor cymbal with 4s decay should still ring at 1s, rms = {rms}");
    }

    #[test]
    fn shimmer_produces_modulation() {
        let mut h_no = ModuleHarness::build::<XorCymbal>(&[
            ("shimmer", ParameterValue::Float(0.0)),
            ("tone", ParameterValue::Float(1.0)),
        ]);
        h_no.disconnect_input("velocity");
        let mut h_yes = ModuleHarness::build::<XorCymbal>(&[
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

        let diff: f32 = s_no.iter().zip(s_yes.iter()).map(|(a, b)| (a - b).abs()).sum::<f32>() / 2000.0;
        assert!(diff > 0.001, "shimmer should change the output, avg diff = {diff}");
    }

    #[test]
    fn spectrum_is_hf_dominant() {
        use crate::test_support::{band_energy, magnitude_spectrum};
        let mut h = ModuleHarness::build::<XorCymbal>(&[
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
            "xor cymbal should be HF-dominant: hf={hf}, lf={lf}, ratio={}",
            hf / lf.max(1e-12),
        );
    }

    #[test]
    fn envelope_peak_is_early() {
        use crate::test_support::windowed_rms;
        let mut h = ModuleHarness::build::<XorCymbal>(&[
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
            "xor cymbal envelope peak should occur in first quarter, peak_idx={peak_idx} of {}",
            rms.len(),
        );
        let tail_rms = rms[rms.len() - 4..].iter().copied().sum::<f32>() / 4.0;
        assert!(tail_rms < rms[peak_idx] * 0.8, "tail not decaying past peak");
    }

    #[test]
    fn spectrum_differs_from_cymbal() {
        use crate::cymbal::Cymbal;
        use crate::test_support::magnitude_spectrum;
        let fft_size = 4096;

        let mut h_xor = ModuleHarness::build::<XorCymbal>(&[
            ("shimmer", ParameterValue::Float(0.0)),
            ("tone", ParameterValue::Float(1.0)),
        ]);
        h_xor.disconnect_input("velocity");
        let mut h_metal = ModuleHarness::build::<Cymbal>(&[
            ("shimmer", ParameterValue::Float(0.0)),
            ("tone", ParameterValue::Float(1.0)),
        ]);
        h_metal.disconnect_input("velocity");

        h_xor.set_mono("trigger", 1.0);
        h_xor.tick();
        h_xor.set_mono("trigger", 0.0);
        h_metal.set_mono("trigger", 1.0);
        h_metal.tick();
        h_metal.set_mono("trigger", 0.0);

        let xor_samples = h_xor.run_mono(fft_size, "out");
        let metal_samples = h_metal.run_mono(fft_size, "out");

        let xor_spec = magnitude_spectrum(&xor_samples, fft_size);
        let metal_spec = magnitude_spectrum(&metal_samples, fft_size);

        let avg_diff: f32 = xor_spec
            .iter()
            .zip(metal_spec.iter())
            .map(|(x, m)| (x - m).abs())
            .sum::<f32>() / xor_spec.len() as f32;
        assert!(
            avg_diff > 0.005,
            "xor cymbal spectrum should differ from cymbal: avg diff = {avg_diff}"
        );
    }

    #[test]
    fn velocity_scales_output() {
        let mut h_full = ModuleHarness::build::<XorCymbal>(&[]);
        h_full.disconnect_input("velocity");
        h_full.set_mono("trigger", 1.0);
        h_full.tick();
        h_full.set_mono("trigger", 0.0);
        let rms_full = h_full.measure_rms(2000, "out");

        let mut h_half = ModuleHarness::build::<XorCymbal>(&[]);
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
}
