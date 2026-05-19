/// Cymbal synthesiser (E003) — modal-bank crash voice.
///
/// Sister to [`crate::cymbal::Cymbal`]; the two coexist.
///
/// The crash content is six parallel high-Q [`BridgedT`](crate::primitives::BridgedT)
/// resonators in a [`ModalBank`] struck by an excitation pulse, with per-partial
/// Q so high partials radiate energy faster than low ones (the tail darkens
/// as the bank rings). A parallel low-frequency `BridgedT` body resonator at
/// ~160 Hz adds gong-weight beneath the high partials — modelled as its own
/// resonator rather than as a partial of the bank because the body is at a
/// different scale from the partial-ratio set and benefits from independent
/// gain control. HP-filtered white noise mixes via `tone` matching the existing
/// `Cymbal`'s shape, and a slow shimmer LFO routes per-partial frequency
/// modulation through [`ModalBank::tick_with_modulation`].
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
/// | Name          | Type  | Range         | Default     | Description                                                            |
/// |---------------|-------|---------------|-------------|------------------------------------------------------------------------|
/// | `pitch`       | float | 200–10000 Hz  | 600         | Modal-bank base frequency                                              |
/// | `decay`       | float | 0.2–8.0 s     | 2.0         | Outer envelope decay (per-partial Q gives inhomogeneous decay on top)  |
/// | `decay_slope` | float | 0.0–1.0       | 0.5         | 0 = flat per-partial Q, 1 = strongly decreasing (fast HF decay)        |
/// | `tone`        | float | 0.0–1.0       | 0.5         | Modal-bank vs HP-noise mix                                             |
/// | `filter`      | float | 2000–16000 Hz | 6000        | Noise highpass cutoff                                                  |
/// | `shimmer`     | float | 0.0–1.0       | 0.2         | Slow LFO depth on partial frequencies                                  |
/// | `body_mix`    | float | 0.0–1.0       | 0.3         | Gain of the parallel low body resonator                                |
/// | `pulse_ms`    | float | 0.1–10.0      | 1.0         | Excitation duration                                                    |
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

use crate::primitives::{BridgedT, DecayEnvelope, Excitation, ModalBank, PulseShape};

const BODY_FREQ_HZ: f32 = 160.0;
const BODY_Q: f32 = 5.0;
const SHIMMER_HZ: f32 = 3.0;
const SHIMMER_MOD_SCALE: f32 = 20.0;

patches_sdk::params_enum! {
    pub enum Cymbal2PulseShape {
        Dirac => "dirac",
        ExpDecay => "exp_decay",
        HalfSine => "half_sine",
        FilteredClick => "filtered_click",
    }
}

impl Cymbal2PulseShape {
    fn to_primitive(self) -> PulseShape {
        match self {
            Self::Dirac => PulseShape::Dirac,
            Self::ExpDecay => PulseShape::ExpDecay,
            Self::HalfSine => PulseShape::HalfSine,
            Self::FilteredClick => PulseShape::FilteredClick,
        }
    }
}

fn read_pulse_shape(s: &StructuralParams) -> Cymbal2PulseShape {
    s.get_int("pulse_shape", 0)
        .and_then(|i| u32::try_from(i).ok())
        .and_then(|i| Cymbal2PulseShape::try_from(i).ok())
        .unwrap_or(Cymbal2PulseShape::HalfSine)
}

module_params! {
    Cymbal2 {
        pitch: Float,
        decay: Float,
        decay_slope: Float,
        tone: Float,
        filter: Float,
        shimmer: Float,
        body_mix: Float,
        pulse_ms: Float,
    }
}

/// Interpolate the per-partial Q profile from flat at slope=0 to the
/// default decreasing profile at slope=1, keeping the mean constant so
/// `decay` (which scales the outer envelope) and `decay_slope` (which
/// shapes the per-partial decay) act independently.
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

pub struct Cymbal2 {
    instance_id: InstanceId,
    descriptor: ModuleDescriptor,
    sample_rate: f32,
    pitch: f32,
    decay_time: f32,
    decay_slope: f32,
    tone: f32,
    filter_freq: f32,
    shimmer: f32,
    body_mix: f32,
    pulse_ms: f32,
    mod_depth: f32,
    latched_velocity: f32,
    bank: ModalBank,
    body: BridgedT,
    excitation: Excitation,
    amp_env: DecayEnvelope,
    hp_filter: SvfKernel,
    prng_state: u64,
    lfo_phase: f32,
    lfo_increment: f32,
    in_trigger: TriggerInput,
    in_velocity: MonoInput,
    out_audio: MonoOutput,
}

impl Module for Cymbal2 {
    fn template() -> ModuleDescriptorTemplate {
        const T: ModuleDescriptorTemplate = ModuleDescriptorTemplate {
            name: "Cymbal2",
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
                    name: params::decay_slope.as_str(),
                    kind: ParameterKind::Float { min: 0.0, max: 1.0, default: 0.5 },
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
                ParameterTemplate {
                    name: params::body_mix.as_str(),
                    kind: ParameterKind::Float { min: 0.0, max: 1.0, default: 0.3 },
                },
                ParameterTemplate {
                    name: params::pulse_ms.as_str(),
                    kind: ParameterKind::Float { min: 0.1, max: 10.0, default: 1.0 },
                },
            ],
            structural_params: &[ParameterTemplate {
                name: "pulse_shape",
                kind: ParameterKind::Enum {
                    variants: Cymbal2PulseShape::VARIANTS,
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
        amp_env.set_decay(2.0);
        let mut bank = ModalBank::with_default_metal_profile(sr, 600.0);
        bank.set_q_profile(q_profile_for_slope(0.5));
        let body = BridgedT::new(sr, BODY_FREQ_HZ, BODY_Q);
        let mut excitation = Excitation::new(sr, instance_id.as_u64());
        excitation.set_shape(read_pulse_shape(structural).to_primitive());
        excitation.set_pulse_ms(1.0);
        let f = svf_f(6000.0, sr);
        let d = q_to_damp(0.3);
        Ok(Self {
            instance_id,
            descriptor,
            sample_rate: sr,
            pitch: 600.0,
            decay_time: 2.0,
            decay_slope: 0.5,
            tone: 0.5,
            filter_freq: 6000.0,
            shimmer: 0.2,
            body_mix: 0.3,
            pulse_ms: 1.0,
            mod_depth: 0.2 * SHIMMER_MOD_SCALE,
            latched_velocity: 1.0,
            bank,
            body,
            excitation,
            amp_env,
            hp_filter: SvfKernel::new_static(f, d),
            prng_state: instance_id.as_u64() + 1,
            lfo_phase: 0.0,
            lfo_increment: SHIMMER_HZ / sr,
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
        let shimmer = p.get(params::shimmer);
        if shimmer != self.shimmer {
            self.shimmer = shimmer;
            self.mod_depth = shimmer * SHIMMER_MOD_SCALE;
        }
        self.body_mix = p.get(params::body_mix);
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
            self.body.reset_state();
            self.excitation.trigger();
            self.lfo_phase = 0.0;
        }

        if !trigger_rose && self.amp_env.is_silent() {
            pool.write_mono(&self.out_audio, 0.0);
            return;
        }

        let exc = self.excitation.tick();
        let bank_out = self.bank.tick_with_modulation(exc, self.mod_depth, self.lfo_phase);
        let body_out = self.body.tick(exc, 0.0);
        let white = xorshift64(&mut self.prng_state);
        let (_lp, hp, _bp) = self.hp_filter.tick(white);

        let mix = bank_out * self.tone + hp * (1.0 - self.tone) + body_out * self.body_mix;
        let amp = self.amp_env.tick(trigger_rose);
        let output = mix * amp * self.latched_velocity;

        self.lfo_phase += self.lfo_increment;
        if self.lfo_phase >= 1.0 {
            self.lfo_phase -= 1.0;
        }

        pool.write_mono(&self.out_audio, output);
    }

    fn as_any(&self) -> &dyn std::any::Any { self }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{band_energy, magnitude_spectrum, windowed_rms};
    use patches_sdk::test_support::ModuleHarness;
    use patches_sdk::ParameterValue;

    fn make(args: &[(&str, ParameterValue)]) -> ModuleHarness {
        let mut h = ModuleHarness::build::<Cymbal2>(args);
        h.disconnect_input("velocity");
        h
    }

    fn fire(h: &mut ModuleHarness) {
        h.set_mono("trigger", 1.0);
        h.tick();
        h.set_mono("trigger", 0.0);
    }

    #[test]
    fn trigger_produces_output() {
        let mut h = make(&[]);
        fire(&mut h);
        let rms = h.measure_rms(5000, "out");
        assert!(rms > 0.001, "rms = {rms}");
    }

    #[test]
    fn long_decay() {
        let mut h = make(&[("decay", ParameterValue::Float(4.0))]);
        fire(&mut h);
        for _ in 0..44000 {
            h.tick();
        }
        let rms = h.measure_rms(1000, "out");
        assert!(rms > 0.001, "rms = {rms}");
    }

    #[test]
    fn spectrum_is_hf_dominant() {
        let mut h = make(&[("decay", ParameterValue::Float(2.0))]);
        fire(&mut h);
        let samples = h.run_mono(4096, "out");
        let spec = magnitude_spectrum(&samples, 4096);
        let lf = band_energy(&spec, 44100.0, 4096, 20.0, 500.0);
        let hf = band_energy(&spec, 44100.0, 4096, 2000.0, 20000.0);
        assert!(hf > 4.0 * lf, "hf={hf}, lf={lf}, ratio={}", hf / lf.max(1e-12));
    }

    #[test]
    fn tail_darkens() {
        let mut h = make(&[
            ("decay", ParameterValue::Float(4.0)),
            ("decay_slope", ParameterValue::Float(1.0)),
            ("tone", ParameterValue::Float(1.0)),
            ("shimmer", ParameterValue::Float(0.0)),
            ("body_mix", ParameterValue::Float(0.0)),
            ("pitch", ParameterValue::Float(2000.0)),
        ]);
        fire(&mut h);
        let samples = h.run_mono(8192, "out");
        let early = &samples[0..2048];
        let late = &samples[4096..8192];
        let hf_share = |buf: &[f32]| -> f32 {
            let spec = magnitude_spectrum(buf, buf.len());
            let total = band_energy(&spec, 44100.0, buf.len(), 20.0, 20000.0);
            let hi = band_energy(&spec, 44100.0, buf.len(), 4000.0, 10000.0);
            hi / total.max(1e-12)
        };
        let early_share = hf_share(early);
        let late_share = hf_share(late);
        assert!(
            late_share < early_share,
            "tail should darken: early={early_share}, late={late_share}"
        );
    }

    #[test]
    fn envelope_peak_is_early() {
        let mut h = make(&[("decay", ParameterValue::Float(1.0))]);
        fire(&mut h);
        let samples = h.run_mono(8192, "out");
        let rms = windowed_rms(&samples, 256);
        let peak_idx = rms
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .map(|(i, _)| i)
            .unwrap_or(0);
        assert!(
            peak_idx < rms.len() / 4,
            "peak idx {peak_idx} of {}",
            rms.len()
        );
        let tail = rms[rms.len() - 4..].iter().copied().sum::<f32>() / 4.0;
        assert!(tail < rms[peak_idx] * 0.8, "tail not decaying past peak");
    }

    #[test]
    fn shimmer_changes_output() {
        let mut h_no = make(&[
            ("shimmer", ParameterValue::Float(0.0)),
            ("tone", ParameterValue::Float(1.0)),
            ("body_mix", ParameterValue::Float(0.0)),
        ]);
        let mut h_yes = make(&[
            ("shimmer", ParameterValue::Float(1.0)),
            ("tone", ParameterValue::Float(1.0)),
            ("body_mix", ParameterValue::Float(0.0)),
        ]);
        fire(&mut h_no);
        fire(&mut h_yes);
        let s_no = h_no.run_mono(2000, "out");
        let s_yes = h_yes.run_mono(2000, "out");
        let diff: f32 =
            s_no.iter().zip(&s_yes).map(|(a, b)| (a - b).abs()).sum::<f32>() / 2000.0;
        assert!(diff > 0.001, "avg diff {diff}");
    }

    #[test]
    fn body_resonator_adds_lf() {
        let mut h_no = make(&[
            ("body_mix", ParameterValue::Float(0.0)),
            ("tone", ParameterValue::Float(1.0)),
        ]);
        let mut h_yes = make(&[
            ("body_mix", ParameterValue::Float(1.0)),
            ("tone", ParameterValue::Float(1.0)),
        ]);
        fire(&mut h_no);
        fire(&mut h_yes);
        let s_no = h_no.run_mono(4096, "out");
        let s_yes = h_yes.run_mono(4096, "out");
        let lf = |buf: &[f32]| -> f32 {
            let spec = magnitude_spectrum(buf, buf.len());
            band_energy(&spec, 44100.0, buf.len(), 60.0, 250.0)
        };
        let lf_no = lf(&s_no);
        let lf_yes = lf(&s_yes);
        assert!(
            lf_yes > lf_no * 3.0,
            "body should add LF: no={lf_no}, yes={lf_yes}"
        );
    }

    #[test]
    fn repeated_identical_param_updates_are_idempotent() {
        // Repeated `update_validated_parameters` with the same values
        // must not perturb the audio — confirms the value-changed guards
        // skip the downstream coefficient writes / bank profile pushes.
        let mut a = make(&[]);
        let mut b = make(&[]);
        let same = [
            ("pitch", ParameterValue::Float(600.0)),
            ("decay", ParameterValue::Float(2.0)),
            ("decay_slope", ParameterValue::Float(0.5)),
            ("tone", ParameterValue::Float(0.5)),
            ("filter", ParameterValue::Float(6000.0)),
            ("shimmer", ParameterValue::Float(0.2)),
            ("body_mix", ParameterValue::Float(0.3)),
            ("pulse_ms", ParameterValue::Float(1.0)),
        ];
        for _ in 0..10 {
            a.update_validated_parameters(&same);
        }
        b.update_validated_parameters(&same);
        fire(&mut a);
        fire(&mut b);
        let sa = a.run_mono(256, "out");
        let sb = b.run_mono(256, "out");
        // PRNG seeds differ between harness instances (InstanceId is global),
        // so RMS-level equivalence is the right check, not bit-identity.
        let rms = |buf: &[f32]| (buf.iter().map(|x| x * x).sum::<f32>() / buf.len() as f32).sqrt();
        let ra = rms(&sa);
        let rb = rms(&sb);
        let ratio = ra / rb.max(1e-12);
        assert!(
            (ratio - 1.0).abs() < 0.1,
            "RMS should match within 10% — repeated identical param updates altered audio: a={ra}, b={rb}"
        );
    }

    #[test]
    fn voice_silent_after_envelope_snap() {
        // Short decay; far past the −140 dBFS snap threshold the output
        // must be *exactly* zero, not just small. Confirms the
        // amp_env.is_silent() early-out in process is reached.
        let mut h = make(&[("decay", ParameterValue::Float(0.2))]);
        fire(&mut h);
        for _ in 0..200_000 {
            h.tick();
        }
        let samples = h.run_mono(1024, "out");
        for (i, s) in samples.iter().enumerate() {
            assert_eq!(*s, 0.0, "sample {i} should be exactly zero after envelope silence");
        }
    }

    #[test]
    fn velocity_scales_output() {
        let mut h_full = make(&[]);
        fire(&mut h_full);
        let rms_full = h_full.measure_rms(2000, "out");

        let mut h_half = ModuleHarness::build::<Cymbal2>(&[]);
        h_half.set_mono("velocity", 0.5);
        fire(&mut h_half);
        let rms_half = h_half.measure_rms(2000, "out");

        let ratio = rms_half / rms_full;
        assert!((ratio - 0.5).abs() < 0.1, "ratio = {ratio}");
    }
}
