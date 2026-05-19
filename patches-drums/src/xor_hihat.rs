//! XOR-pair flavour hi-hats (E004). `XorClosedHiHat` and `XorOpenHiHat`
//! are structurally identical to [`crate::hihat::ClosedHiHat`] /
//! [`crate::hihat::OpenHiHat`] with the single substitution
//! `MetallicTone` → [`XorPairTone`](crate::primitives::XorPairTone).
//! Same envelopes, same HP-noise mix, same `choke` input on the open
//! variant — only the generator's texture differs: coarser, denser
//! inharmonic content from the three pair intermod products.

/// Closed hi-hat synthesiser (XOR-pair flavour).
///
/// Sister to [`crate::hihat::ClosedHiHat`]; the two coexist. Generator
/// is [`XorPairTone`](crate::primitives::XorPairTone) for a coarser,
/// denser texture than the original.
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
/// | `pitch`  | float | 100–8000 Hz   | 400     | Base frequency of XOR-pair tone |
/// | `decay`  | float | 0.005–0.2 s   | 0.04    | Amplitude decay time            |
/// | `tone`   | float | 0.0–1.0       | 0.5     | XOR-pair vs noise mix           |
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
use crate::primitives::{DecayEnvelope, XorPairTone};
use patches_dsp::{SvfKernel, svf_f, q_to_damp, xorshift64};

module_params! {
    XorHiHat {
        pitch:  Float,
        decay:  Float,
        tone:   Float,
        filter: Float,
    }
}

pub struct XorClosedHiHat {
    instance_id: InstanceId,
    descriptor: ModuleDescriptor,
    sample_rate: f32,
    pitch: f32,
    decay_time: f32,
    tone: f32,
    filter_freq: f32,
    latched_velocity: f32,
    xor: XorPairTone,
    amp_env: DecayEnvelope,
    hp_filter: SvfKernel,
    prng_state: u64,
    in_trigger: TriggerInput,
    in_velocity: MonoInput,
    out_audio: MonoOutput,
}

impl Module for XorClosedHiHat {
    fn template() -> ModuleDescriptorTemplate {
        const T: ModuleDescriptorTemplate = ModuleDescriptorTemplate {
            name: "XorClosedHiHat",
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
        let mut xor = XorPairTone::new(sr);
        xor.set_frequency(400.0);
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
            xor,
            amp_env,
            hp_filter: SvfKernel::new_static(f, d),
            prng_state: instance_id.as_u64() + 1,
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
        }

        if !trigger_rose && self.amp_env.is_silent() {
            pool.write_mono(&self.out_audio, 0.0);
            return;
        }

        let amp = self.amp_env.tick(trigger_rose);

        let metal = self.xor.tick();
        let white = xorshift64(&mut self.prng_state);
        let (_lp, hp, _bp) = self.hp_filter.tick(white);

        let mix = metal * self.tone + hp * (1.0 - self.tone);
        let output = mix * amp;

        pool.write_mono(&self.out_audio, output * self.latched_velocity);
    }

    fn as_any(&self) -> &dyn std::any::Any { self }
}

/// Open hi-hat synthesiser (XOR-pair flavour).
///
/// Sister to [`crate::hihat::OpenHiHat`]; the two coexist. Same
/// generator substitution as [`XorClosedHiHat`], plus the existing
/// `choke` input behaviour.
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
/// | `pitch`  | float | 100–8000 Hz   | 400     | Base frequency of XOR-pair tone |
/// | `decay`  | float | 0.05–4.0 s    | 0.5     | Amplitude decay time            |
/// | `tone`   | float | 0.0–1.0       | 0.5     | XOR-pair vs noise mix           |
/// | `filter` | float | 2000–16000 Hz | 8000    | Noise highpass cutoff           |
pub struct XorOpenHiHat {
    instance_id: InstanceId,
    descriptor: ModuleDescriptor,
    sample_rate: f32,
    pitch: f32,
    decay_time: f32,
    tone: f32,
    filter_freq: f32,
    latched_velocity: f32,
    xor: XorPairTone,
    amp_env: DecayEnvelope,
    hp_filter: SvfKernel,
    prng_state: u64,
    in_trigger: TriggerInput,
    in_choke: TriggerInput,
    in_velocity: MonoInput,
    out_audio: MonoOutput,
}

impl Module for XorOpenHiHat {
    fn template() -> ModuleDescriptorTemplate {
        const T: ModuleDescriptorTemplate = ModuleDescriptorTemplate {
            name: "XorOpenHiHat",
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
        let mut xor = XorPairTone::new(sr);
        xor.set_frequency(400.0);
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
            xor,
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
            self.xor.trigger();
        }

        if choke_rose {
            self.amp_env.choke();
        }

        if !trigger_rose && self.amp_env.is_silent() {
            pool.write_mono(&self.out_audio, 0.0);
            return;
        }

        let amp = self.amp_env.tick(trigger_rose);

        let metal = self.xor.tick();
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
    fn xor_closed_hihat_trigger_produces_output() {
        let mut h = ModuleHarness::build::<XorClosedHiHat>(&[]);
        h.disconnect_input("velocity");
        h.set_mono("trigger", 1.0);
        h.tick();
        h.set_mono("trigger", 0.0);
        let rms = h.measure_rms(1000, "out");
        assert!(rms > 0.01, "xor closed hihat should produce output, rms = {rms}");
    }

    #[test]
    fn xor_closed_hihat_short_decay() {
        let mut h = ModuleHarness::build::<XorClosedHiHat>(&[
            ("decay", ParameterValue::Float(0.04)),
        ]);
        h.disconnect_input("velocity");
        h.set_mono("trigger", 1.0);
        h.tick();
        h.set_mono("trigger", 0.0);
        for _ in 0..8000 {
            h.tick();
        }
        let rms = h.measure_rms(1000, "out");
        assert!(rms < 0.005, "xor closed hihat should decay quickly, rms = {rms}");
    }

    #[test]
    fn xor_closed_hihat_velocity_scales_output() {
        let mut h_full = ModuleHarness::build::<XorClosedHiHat>(&[]);
        h_full.disconnect_input("velocity");
        h_full.set_mono("trigger", 1.0);
        h_full.tick();
        h_full.set_mono("trigger", 0.0);
        let rms_full = h_full.measure_rms(500, "out");

        let mut h_half = ModuleHarness::build::<XorClosedHiHat>(&[]);
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
    fn xor_open_hihat_long_decay() {
        let mut h = ModuleHarness::build::<XorOpenHiHat>(&[
            ("decay", ParameterValue::Float(0.4)),
        ]);
        h.disconnect_input("velocity");
        h.set_mono("trigger", 1.0);
        h.tick();
        h.set_mono("trigger", 0.0);

        for _ in 0..9000 {
            h.tick();
        }
        let rms_mid = h.measure_rms(500, "out");
        assert!(rms_mid > 0.001, "xor open hihat should still ring at 9000 samples, rms = {rms_mid}");

        for _ in 0..(45000 - 9000 - 500) {
            h.tick();
        }
        let rms_tail = h.measure_rms(500, "out");
        assert!(rms_tail < 0.001, "xor open hihat should be decayed by 45000 samples, rms = {rms_tail}");
    }

    #[test]
    fn xor_open_hihat_choke_silences() {
        let mut h = ModuleHarness::build::<XorOpenHiHat>(&[
            ("decay", ParameterValue::Float(2.0)),
        ]);
        h.disconnect_input("velocity");
        h.set_mono("trigger", 1.0);
        h.tick();
        h.set_mono("trigger", 0.0);

        for _ in 0..500 {
            h.tick();
        }

        let rms_before = h.measure_rms(100, "out");
        assert!(rms_before > 0.001, "should still be ringing before choke");

        h.set_mono("choke", 1.0);
        h.tick();
        h.set_mono("choke", 0.0);

        let rms_after = h.measure_rms(100, "out");
        assert!(rms_after < 0.001, "should be silent after choke, rms = {rms_after}");
    }

    #[test]
    fn xor_open_hihat_velocity_scales_output() {
        let mut h_full = ModuleHarness::build::<XorOpenHiHat>(&[]);
        h_full.disconnect_input("velocity");
        h_full.set_mono("trigger", 1.0);
        h_full.tick();
        h_full.set_mono("trigger", 0.0);
        let rms_full = h_full.measure_rms(2000, "out");

        let mut h_half = ModuleHarness::build::<XorOpenHiHat>(&[]);
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
    fn xor_closed_hihat_spectrum_is_hf_dominant() {
        use crate::test_support::{band_energy, magnitude_spectrum};
        let mut h = ModuleHarness::build::<XorClosedHiHat>(&[
            ("decay", ParameterValue::Float(0.2)),
        ]);
        h.disconnect_input("velocity");
        h.set_mono("trigger", 1.0);
        h.tick();
        h.set_mono("trigger", 0.0);
        let samples = h.run_mono(2048, "out");
        let spec = magnitude_spectrum(&samples, 2048);
        let lf = band_energy(&spec, 44100.0, 2048, 20.0, 500.0);
        let hf = band_energy(&spec, 44100.0, 2048, 2000.0, 20000.0);
        assert!(
            hf > 5.0 * lf,
            "xor closed hihat should be HF-dominant: hf={hf}, lf={lf}"
        );
    }

    #[test]
    fn xor_open_hihat_spectrum_is_hf_dominant() {
        use crate::test_support::{band_energy, magnitude_spectrum};
        let mut h = ModuleHarness::build::<XorOpenHiHat>(&[
            ("decay", ParameterValue::Float(0.5)),
        ]);
        h.disconnect_input("velocity");
        h.set_mono("trigger", 1.0);
        h.tick();
        h.set_mono("trigger", 0.0);
        let samples = h.run_mono(2048, "out");
        let spec = magnitude_spectrum(&samples, 2048);
        let lf = band_energy(&spec, 44100.0, 2048, 20.0, 500.0);
        let hf = band_energy(&spec, 44100.0, 2048, 2000.0, 20000.0);
        assert!(
            hf > 5.0 * lf,
            "xor open hihat should be HF-dominant: hf={hf}, lf={lf}"
        );
    }

    #[test]
    fn xor_closed_hihat_spectrum_differs_from_closed_hihat() {
        use crate::hihat::ClosedHiHat;
        use crate::test_support::magnitude_spectrum;
        let fft_size = 2048;

        let mut h_xor = ModuleHarness::build::<XorClosedHiHat>(&[
            ("tone", ParameterValue::Float(1.0)),
        ]);
        h_xor.disconnect_input("velocity");
        let mut h_metal = ModuleHarness::build::<ClosedHiHat>(&[
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
            "xor closed hihat spectrum should differ from closed hihat: avg diff = {avg_diff}"
        );
    }

    #[test]
    fn xor_open_hihat_spectrum_differs_from_open_hihat() {
        use crate::hihat::OpenHiHat;
        use crate::test_support::magnitude_spectrum;
        let fft_size = 2048;

        let mut h_xor = ModuleHarness::build::<XorOpenHiHat>(&[
            ("tone", ParameterValue::Float(1.0)),
        ]);
        h_xor.disconnect_input("velocity");
        let mut h_metal = ModuleHarness::build::<OpenHiHat>(&[
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
            "xor open hihat spectrum should differ from open hihat: avg diff = {avg_diff}"
        );
    }
}
