/// Closed hi-hat synthesiser (E003) — modal-bank variant.
///
/// Sister to [`crate::hihat::ClosedHiHat`]; the two coexist.
///
/// Six parallel high-Q [`BridgedT`](crate::primitives::BridgedT) resonators
/// in a [`ModalBank`] struck by an excitation pulse, mixed with HP-filtered
/// white noise via `tone`. Per-partial Q gives an inhomogeneous decay —
/// high partials fade before low ones — which is audibly different from the
/// summed-square `MetallicTone` voice that powers
/// [`crate::hihat::ClosedHiHat`].
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
/// | Name          | Type  | Range         | Default     | Description                                                  |
/// |---------------|-------|---------------|-------------|--------------------------------------------------------------|
/// | `pitch`       | float | 100–8000 Hz   | 400         | Modal-bank base frequency                                    |
/// | `decay`       | float | 0.005–0.2 s   | 0.04        | Outer envelope decay time                                    |
/// | `decay_slope` | float | 0.0–1.0       | 0.7         | 0 = flat per-partial Q, 1 = strongly decreasing              |
/// | `tone`        | float | 0.0–1.0       | 0.5         | Modal-bank vs HP-noise mix                                   |
/// | `filter`      | float | 2000–16000 Hz | 8000        | Noise highpass cutoff                                        |
/// | `pulse_ms`    | float | 0.1–10.0      | 0.5         | Excitation duration                                          |
/// | `pulse_shape` | enum  | dirac / exp_decay / half_sine / filtered_click | half_sine | Structural; excitation shape |
use patches_dsp::{SvfKernel, q_to_damp, svf_f, xorshift64};
use patches_sdk::cables::TriggerInput;
use patches_sdk::module_params;
use patches_sdk::modules::{CountAxis, ModuleDescriptorTemplate, ParameterTemplate, PortTemplate};
use patches_sdk::param_frame::ParamView;
use patches_sdk::{
    AudioEnvironment, BuildError, CablePool, InputPort, InstanceId, Module, ModuleDescriptor,
    MonoInput, MonoOutput, OutputPort, ParameterKind, StructuralParams,
};

use crate::primitives::{DecayEnvelope, Excitation, ModalBank, PulseShape};

patches_sdk::params_enum! {
    pub enum HiHat2PulseShape {
        Dirac => "dirac",
        ExpDecay => "exp_decay",
        HalfSine => "half_sine",
        FilteredClick => "filtered_click",
    }
}

impl HiHat2PulseShape {
    fn to_primitive(self) -> PulseShape {
        match self {
            Self::Dirac => PulseShape::Dirac,
            Self::ExpDecay => PulseShape::ExpDecay,
            Self::HalfSine => PulseShape::HalfSine,
            Self::FilteredClick => PulseShape::FilteredClick,
        }
    }
}

fn read_pulse_shape(s: &StructuralParams) -> HiHat2PulseShape {
    s.get_int("pulse_shape", 0)
        .and_then(|i| u32::try_from(i).ok())
        .and_then(|i| HiHat2PulseShape::try_from(i).ok())
        .unwrap_or(HiHat2PulseShape::HalfSine)
}

/// Interpolate the per-partial Q profile from flat (slope=0) to the default
/// decreasing profile (slope=1), keeping the mean constant so the outer
/// envelope `decay` time and `decay_slope` shape act independently.
fn q_profile_for_slope(slope: f32) -> [f32; 6] {
    let default = ModalBank::default_q_profile();
    let mean = default.iter().sum::<f32>() / 6.0;
    let s = slope.clamp(0.0, 1.0);
    let mut out = [0.0f32; 6];
    for i in 0..6 {
        out[i] = mean + (default[i] - mean) * s;
    }
    out
}

module_params! {
    HiHat2 {
        pitch: Float,
        decay: Float,
        decay_slope: Float,
        tone: Float,
        filter: Float,
        pulse_ms: Float,
    }
}

pub struct ClosedHiHat2 {
    instance_id: InstanceId,
    descriptor: ModuleDescriptor,
    sample_rate: f32,
    pitch: f32,
    decay_time: f32,
    decay_slope: f32,
    tone: f32,
    filter_freq: f32,
    pulse_ms: f32,
    latched_velocity: f32,
    bank: ModalBank,
    excitation: Excitation,
    amp_env: DecayEnvelope,
    hp_filter: SvfKernel,
    prng_state: u64,
    in_trigger: TriggerInput,
    in_velocity: MonoInput,
    out_audio: MonoOutput,
}

impl Module for ClosedHiHat2 {
    fn template() -> ModuleDescriptorTemplate {
        const T: ModuleDescriptorTemplate = ModuleDescriptorTemplate {
            name: "ClosedHiHat2",
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
                    name: params::decay_slope.as_str(),
                    kind: ParameterKind::Float { min: 0.0, max: 1.0, default: 0.7 },
                },
                ParameterTemplate {
                    name: params::tone.as_str(),
                    kind: ParameterKind::Float { min: 0.0, max: 1.0, default: 0.5 },
                },
                ParameterTemplate {
                    name: params::filter.as_str(),
                    kind: ParameterKind::Float { min: 2000.0, max: 16000.0, default: 8000.0 },
                },
                ParameterTemplate {
                    name: params::pulse_ms.as_str(),
                    kind: ParameterKind::Float { min: 0.1, max: 10.0, default: 0.5 },
                },
            ],
            structural_params: &[ParameterTemplate {
                name: "pulse_shape",
                kind: ParameterKind::Enum {
                    variants: HiHat2PulseShape::VARIANTS,
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
        let mut amp_env = DecayEnvelope::new(sr);
        amp_env.set_decay(0.04);
        let mut bank = ModalBank::with_default_metal_profile(sr, 400.0);
        bank.set_q_profile(q_profile_for_slope(0.7));
        let mut excitation = Excitation::new(sr, instance_id.as_u64());
        excitation.set_shape(read_pulse_shape(structural).to_primitive());
        excitation.set_pulse_ms(0.5);
        let f = svf_f(8000.0, sr);
        let d = q_to_damp(0.3);
        Ok(Self {
            instance_id,
            descriptor,
            sample_rate: sr,
            pitch: 400.0,
            decay_time: 0.04,
            decay_slope: 0.7,
            tone: 0.5,
            filter_freq: 8000.0,
            pulse_ms: 0.5,
            latched_velocity: 1.0,
            bank,
            excitation,
            amp_env,
            hp_filter: SvfKernel::new_static(f, d),
            prng_state: instance_id.as_u64() + 1,
            in_trigger: TriggerInput::default(),
            in_velocity: MonoInput::default(),
            out_audio: MonoOutput::default(),
        })
    }

    fn update_validated_parameters(&mut self, p: &ParamView<'_>) {
        let pitch = p.get(params::pitch);
        if pitch != self.pitch {
            self.pitch = pitch;
            self.bank.set_base_freq(pitch);
        }
        let decay = p.get(params::decay);
        if decay != self.decay_time {
            self.decay_time = decay;
            self.amp_env.set_decay(decay);
        }
        let decay_slope = p.get(params::decay_slope);
        if decay_slope != self.decay_slope {
            self.decay_slope = decay_slope;
            self.bank.set_q_profile(q_profile_for_slope(decay_slope));
        }
        self.tone = p.get(params::tone);
        let filter = p.get(params::filter);
        if filter != self.filter_freq {
            self.filter_freq = filter;
            let f = svf_f(filter, self.sample_rate);
            let d = q_to_damp(0.3);
            self.hp_filter.set_static(f, d);
        }
        let pulse_ms = p.get(params::pulse_ms);
        if pulse_ms != self.pulse_ms {
            self.pulse_ms = pulse_ms;
            self.excitation.set_pulse_ms(pulse_ms);
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
            self.bank.reset_state();
            self.excitation.trigger();
        }

        if !trigger_rose && self.amp_env.is_silent() {
            pool.write_mono(&self.out_audio, 0.0);
            return;
        }

        let exc = self.excitation.tick();
        let bank_out = self.bank.tick(exc);
        let white = xorshift64(&mut self.prng_state);
        let (_lp, hp, _bp) = self.hp_filter.tick(white);

        let mix = bank_out * self.tone + hp * (1.0 - self.tone);
        let amp = self.amp_env.tick(trigger_rose);
        pool.write_mono(&self.out_audio, mix * amp * self.latched_velocity);
    }

    fn as_any(&self) -> &dyn std::any::Any { self }
}

/// Open hi-hat synthesiser (E003) — modal-bank variant.
///
/// Sister to [`crate::hihat::OpenHiHat`]; the two coexist.
///
/// Same modal-bank architecture as [`ClosedHiHat2`] but with a longer default
/// decay range and a `choke` input mirroring the existing
/// [`crate::hihat::OpenHiHat`]. Per-partial Q gives an inhomogeneous decay —
/// the long tail darkens over time as high partials fade first.
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
/// | Name          | Type  | Range         | Default     | Description                                                  |
/// |---------------|-------|---------------|-------------|--------------------------------------------------------------|
/// | `pitch`       | float | 100–8000 Hz   | 400         | Modal-bank base frequency                                    |
/// | `decay`       | float | 0.05–4.0 s    | 0.4         | Outer envelope decay time                                    |
/// | `decay_slope` | float | 0.0–1.0       | 0.5         | 0 = flat per-partial Q, 1 = strongly decreasing              |
/// | `tone`        | float | 0.0–1.0       | 0.5         | Modal-bank vs HP-noise mix                                   |
/// | `filter`      | float | 2000–16000 Hz | 7000        | Noise highpass cutoff                                        |
/// | `pulse_ms`    | float | 0.1–10.0      | 0.5         | Excitation duration                                          |
/// | `pulse_shape` | enum  | dirac / exp_decay / half_sine / filtered_click | half_sine | Structural; excitation shape |
pub struct OpenHiHat2 {
    instance_id: InstanceId,
    descriptor: ModuleDescriptor,
    sample_rate: f32,
    pitch: f32,
    decay_time: f32,
    decay_slope: f32,
    tone: f32,
    filter_freq: f32,
    pulse_ms: f32,
    latched_velocity: f32,
    bank: ModalBank,
    excitation: Excitation,
    amp_env: DecayEnvelope,
    hp_filter: SvfKernel,
    prng_state: u64,
    in_trigger: TriggerInput,
    in_choke: TriggerInput,
    in_velocity: MonoInput,
    out_audio: MonoOutput,
}

impl Module for OpenHiHat2 {
    fn template() -> ModuleDescriptorTemplate {
        const T: ModuleDescriptorTemplate = ModuleDescriptorTemplate {
            name: "OpenHiHat2",
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
                    kind: ParameterKind::Float { min: 0.05, max: 4.0, default: 0.4 },
                },
                ParameterTemplate {
                    name: params::decay_slope.as_str(),
                    kind: ParameterKind::Float { min: 0.0, max: 1.0, default: 0.5 },
                },
                ParameterTemplate {
                    name: params::tone.as_str(),
                    kind: ParameterKind::Float { min: 0.0, max: 1.0, default: 0.5 },
                },
                ParameterTemplate {
                    name: params::filter.as_str(),
                    kind: ParameterKind::Float { min: 2000.0, max: 16000.0, default: 7000.0 },
                },
                ParameterTemplate {
                    name: params::pulse_ms.as_str(),
                    kind: ParameterKind::Float { min: 0.1, max: 10.0, default: 0.5 },
                },
            ],
            structural_params: &[ParameterTemplate {
                name: "pulse_shape",
                kind: ParameterKind::Enum {
                    variants: HiHat2PulseShape::VARIANTS,
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
        let mut amp_env = DecayEnvelope::new(sr);
        amp_env.set_decay(0.4);
        let mut bank = ModalBank::with_default_metal_profile(sr, 400.0);
        bank.set_q_profile(q_profile_for_slope(0.5));
        let mut excitation = Excitation::new(sr, instance_id.as_u64());
        excitation.set_shape(read_pulse_shape(structural).to_primitive());
        excitation.set_pulse_ms(0.5);
        let f = svf_f(7000.0, sr);
        let d = q_to_damp(0.3);
        Ok(Self {
            instance_id,
            descriptor,
            sample_rate: sr,
            pitch: 400.0,
            decay_time: 0.4,
            decay_slope: 0.5,
            tone: 0.5,
            filter_freq: 7000.0,
            pulse_ms: 0.5,
            latched_velocity: 1.0,
            bank,
            excitation,
            amp_env,
            hp_filter: SvfKernel::new_static(f, d),
            prng_state: instance_id.as_u64() + 1,
            in_trigger: TriggerInput::default(),
            in_choke: TriggerInput::default(),
            in_velocity: MonoInput::default(),
            out_audio: MonoOutput::default(),
        })
    }

    fn update_validated_parameters(&mut self, p: &ParamView<'_>) {
        let pitch = p.get(params::pitch);
        if pitch != self.pitch {
            self.pitch = pitch;
            self.bank.set_base_freq(pitch);
        }
        let decay = p.get(params::decay);
        if decay != self.decay_time {
            self.decay_time = decay;
            self.amp_env.set_decay(decay);
        }
        let decay_slope = p.get(params::decay_slope);
        if decay_slope != self.decay_slope {
            self.decay_slope = decay_slope;
            self.bank.set_q_profile(q_profile_for_slope(decay_slope));
        }
        self.tone = p.get(params::tone);
        let filter = p.get(params::filter);
        if filter != self.filter_freq {
            self.filter_freq = filter;
            let f = svf_f(filter, self.sample_rate);
            let d = q_to_damp(0.3);
            self.hp_filter.set_static(f, d);
        }
        let pulse_ms = p.get(params::pulse_ms);
        if pulse_ms != self.pulse_ms {
            self.pulse_ms = pulse_ms;
            self.excitation.set_pulse_ms(pulse_ms);
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
            self.bank.reset_state();
            self.excitation.trigger();
        }

        if choke_rose {
            self.amp_env.choke();
        }

        if !trigger_rose && self.amp_env.is_silent() {
            pool.write_mono(&self.out_audio, 0.0);
            return;
        }

        let exc = self.excitation.tick();
        let bank_out = self.bank.tick(exc);
        let white = xorshift64(&mut self.prng_state);
        let (_lp, hp, _bp) = self.hp_filter.tick(white);

        let mix = bank_out * self.tone + hp * (1.0 - self.tone);
        let amp = self.amp_env.tick(trigger_rose);
        pool.write_mono(&self.out_audio, mix * amp * self.latched_velocity);
    }

    fn as_any(&self) -> &dyn std::any::Any { self }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{band_energy, magnitude_spectrum};
    use patches_sdk::test_support::ModuleHarness;
    use patches_sdk::ParameterValue;

    fn fire(h: &mut ModuleHarness) {
        h.set_mono("trigger", 1.0);
        h.tick();
        h.set_mono("trigger", 0.0);
    }

    #[test]
    fn closed_trigger_produces_output() {
        let mut h = ModuleHarness::build::<ClosedHiHat2>(&[]);
        h.disconnect_input("velocity");
        fire(&mut h);
        let rms = h.measure_rms(1000, "out");
        assert!(rms > 0.01, "rms = {rms}");
    }

    #[test]
    fn closed_short_decay() {
        let mut h = ModuleHarness::build::<ClosedHiHat2>(&[]);
        h.disconnect_input("velocity");
        fire(&mut h);
        for _ in 0..8000 {
            h.tick();
        }
        let rms = h.measure_rms(1000, "out");
        assert!(rms < 0.005, "rms = {rms}");
    }

    #[test]
    fn open_long_decay() {
        let mut h = ModuleHarness::build::<OpenHiHat2>(&[
            ("decay", ParameterValue::Float(0.4)),
        ]);
        h.disconnect_input("velocity");
        fire(&mut h);
        for _ in 0..8000 {
            h.tick();
        }
        let rms_mid = h.measure_rms(1000, "out");
        assert!(rms_mid > 0.005, "should still ring at 8000: rms = {rms_mid}");
        for _ in 0..35000 {
            h.tick();
        }
        let rms_end = h.measure_rms(1000, "out");
        assert!(rms_end < 0.005, "should be decayed at 44000: rms = {rms_end}");
    }

    #[test]
    fn open_choke_silences() {
        let mut h = ModuleHarness::build::<OpenHiHat2>(&[
            ("decay", ParameterValue::Float(2.0)),
        ]);
        h.disconnect_input("velocity");
        fire(&mut h);
        for _ in 0..2000 {
            h.tick();
        }
        let rms_before = h.measure_rms(100, "out");
        assert!(rms_before > 0.001, "should still be ringing: {rms_before}");

        h.set_mono("choke", 1.0);
        h.tick();
        h.set_mono("choke", 0.0);
        for _ in 0..4000 {
            h.tick();
        }
        let rms_after = h.measure_rms(100, "out");
        assert!(rms_after < 0.001, "should be silent after choke: {rms_after}");
    }

    #[test]
    fn closed_hf_dominant() {
        let mut h = ModuleHarness::build::<ClosedHiHat2>(&[]);
        h.disconnect_input("velocity");
        fire(&mut h);
        let samples = h.run_mono(2048, "out");
        let spec = magnitude_spectrum(&samples, 2048);
        let lf = band_energy(&spec, 44100.0, 2048, 20.0, 500.0);
        let hf = band_energy(&spec, 44100.0, 2048, 4000.0, 16000.0);
        assert!(hf > 5.0 * lf, "hf={hf}, lf={lf}");
    }

    #[test]
    fn open_hf_dominant() {
        let mut h = ModuleHarness::build::<OpenHiHat2>(&[]);
        h.disconnect_input("velocity");
        fire(&mut h);
        let samples = h.run_mono(2048, "out");
        let spec = magnitude_spectrum(&samples, 2048);
        let lf = band_energy(&spec, 44100.0, 2048, 20.0, 500.0);
        let hf = band_energy(&spec, 44100.0, 2048, 4000.0, 16000.0);
        assert!(hf > 5.0 * lf, "hf={hf}, lf={lf}");
    }

    #[test]
    fn open_tail_darkens() {
        let mut h = ModuleHarness::build::<OpenHiHat2>(&[
            ("decay", ParameterValue::Float(2.0)),
            ("decay_slope", ParameterValue::Float(1.0)),
            ("tone", ParameterValue::Float(1.0)),
            ("pitch", ParameterValue::Float(2000.0)),
        ]);
        h.disconnect_input("velocity");
        fire(&mut h);
        let samples = h.run_mono(8192, "out");
        let hf_share = |buf: &[f32]| -> f32 {
            let spec = magnitude_spectrum(buf, buf.len());
            let total = band_energy(&spec, 44100.0, buf.len(), 20.0, 20000.0);
            let hi = band_energy(&spec, 44100.0, buf.len(), 4000.0, 10000.0);
            hi / total.max(1e-12)
        };
        let early = hf_share(&samples[0..2048]);
        let late = hf_share(&samples[4096..8192]);
        assert!(late < early, "tail should darken: early={early}, late={late}");
    }

    #[test]
    fn closed_velocity_scales_output() {
        let mut h_full = ModuleHarness::build::<ClosedHiHat2>(&[]);
        h_full.disconnect_input("velocity");
        fire(&mut h_full);
        let rms_full = h_full.measure_rms(500, "out");

        let mut h_half = ModuleHarness::build::<ClosedHiHat2>(&[]);
        h_half.set_mono("velocity", 0.5);
        fire(&mut h_half);
        let rms_half = h_half.measure_rms(500, "out");

        let ratio = rms_half / rms_full;
        assert!((ratio - 0.5).abs() < 0.1, "ratio = {ratio}");
    }

    #[test]
    fn open_velocity_scales_output() {
        let mut h_full = ModuleHarness::build::<OpenHiHat2>(&[]);
        h_full.disconnect_input("velocity");
        fire(&mut h_full);
        let rms_full = h_full.measure_rms(2000, "out");

        let mut h_half = ModuleHarness::build::<OpenHiHat2>(&[]);
        h_half.set_mono("velocity", 0.5);
        fire(&mut h_half);
        let rms_half = h_half.measure_rms(2000, "out");

        let ratio = rms_half / rms_full;
        assert!((ratio - 0.5).abs() < 0.1, "ratio = {ratio}");
    }
}
