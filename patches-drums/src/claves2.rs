/// Cascaded struck-resonator clave (ADR 0002).
///
/// Two `BridgedT` stages in series with an unusual trigger relationship:
/// stage 1 is excited by the trigger pulse and rings at the tuned frequency;
/// stage 1's bandpass output, gated by a rising-edge detector against a
/// running-peak threshold, becomes the **excitation pulse** for stage 2.
/// Stage 2 is therefore re-struck on every cycle of stage 1's burst, giving
/// an effective Q far past stage 2's literal Q parameter — a short, very
/// bright, fast-decaying click with extended ringing tail.
///
/// The audible decay is driven mostly by how fast stage 1's bp output stops
/// crossing the threshold, not by stage 2's own ring; stage 2's Q matters
/// less than the cascade rate. `cascade_mix` crossfades between stage 1
/// alone (`0.0`, equivalent to a single-stage clave) and stage 1 + stage 2
/// (`1.0`).
///
/// Sister to [`crate::claves::Claves`]; the two coexist.
///
/// # Inputs
///
/// | Port       | Kind | Description                                          |
/// |------------|------|------------------------------------------------------|
/// | `trigger`  | mono | Rising edge strikes stage 1                          |
/// | `velocity` | mono | Velocity (0.0–1.0); latched on trigger, scales output. Defaults to 1.0 when disconnected |
///
/// # Outputs
///
/// | Port  | Kind | Description    |
/// |-------|------|----------------|
/// | `out` | mono | Cascaded clave |
///
/// # Parameters
///
/// | Name           | Type  | Range       | Default | Description                                |
/// |----------------|-------|-------------|---------|--------------------------------------------|
/// | `tune`         | float | 500–5000 Hz | 2500    | Resonator centre frequency (both stages)   |
/// | `q`            | float | 10.0–80.0   | 40      | Resonator Q (cascade pushes effective Q higher) |
/// | `cascade_mix`  | float | 0.0–1.0     | 0.7     | Crossfade between stage-1 alone and stage-1 + stage-2 |
/// | `pulse_ms`     | float | 0.05–2.0    | 0.5     | Trigger excitation duration                |
/// | `clip`         | float | 0.0–1.0     | 0.2     | Feedback-clipper amount (both stages)      |
/// | `pulse_shape`  | enum | dirac / exp_decay / half_sine / filtered_click | dirac | Structural; excitation shape |
use patches_sdk::cables::TriggerInput;
use patches_sdk::module_params;
use patches_sdk::modules::{CountAxis, ModuleDescriptorTemplate, ParameterTemplate, PortTemplate};
use patches_sdk::param_frame::ParamView;
use patches_sdk::{
    AudioEnvironment, BuildError, CablePool, InputPort, InstanceId, Module, ModuleDescriptor,
    MonoInput, MonoOutput, OutputPort, ParameterKind, StructuralParams,
};

use crate::primitives::{BridgedT, Excitation, PulseShape};

patches_sdk::params_enum! {
    pub enum Claves2PulseShape {
        Dirac => "dirac",
        ExpDecay => "exp_decay",
        HalfSine => "half_sine",
        FilteredClick => "filtered_click",
    }
}

impl Claves2PulseShape {
    fn to_primitive(self) -> PulseShape {
        match self {
            Self::Dirac => PulseShape::Dirac,
            Self::ExpDecay => PulseShape::ExpDecay,
            Self::HalfSine => PulseShape::HalfSine,
            Self::FilteredClick => PulseShape::FilteredClick,
        }
    }
}

fn read_pulse_shape(s: &StructuralParams) -> Claves2PulseShape {
    s.get_int("pulse_shape", 0)
        .and_then(|i| u32::try_from(i).ok())
        .and_then(|i| Claves2PulseShape::try_from(i).ok())
        .unwrap_or(Claves2PulseShape::Dirac)
}

module_params! {
    Claves2 {
        tune: Float,
        q: Float,
        cascade_mix: Float,
        pulse_ms: Float,
        clip: Float,
    }
}

/// Running-peak rising-edge detector. Triggers stage 2's excitation when
/// stage 1's bp output crosses upward through `0.05 × peak-since-trigger`.
struct EdgeDetector {
    prev: f32,
    peak: f32,
}

impl EdgeDetector {
    fn new() -> Self {
        Self { prev: 0.0, peak: 0.0 }
    }

    fn reset(&mut self) {
        self.prev = 0.0;
        self.peak = 0.0;
    }

    /// Returns `true` when the rising edge crosses the running threshold.
    fn tick(&mut self, x: f32) -> bool {
        let mag = x.abs();
        if mag > self.peak {
            self.peak = mag;
        }
        let threshold = 0.05 * self.peak;
        let fired = self.prev <= threshold && x > threshold;
        self.prev = x;
        fired
    }
}

pub struct Claves2 {
    instance_id: InstanceId,
    descriptor: ModuleDescriptor,
    latched_velocity: f32,
    cascade_mix: f32,
    stage1: BridgedT,
    stage2: BridgedT,
    excitation: Excitation,
    edge: EdgeDetector,
    in_trigger: TriggerInput,
    in_velocity: MonoInput,
    out_audio: MonoOutput,
}

impl Module for Claves2 {
    fn template() -> ModuleDescriptorTemplate {
        const T: ModuleDescriptorTemplate = ModuleDescriptorTemplate {
            name: "Claves2",
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
                    name: params::tune.as_str(),
                    kind: ParameterKind::Float { min: 500.0, max: 5000.0, default: 2500.0 },
                },
                ParameterTemplate {
                    name: params::q.as_str(),
                    kind: ParameterKind::Float { min: 10.0, max: 80.0, default: 40.0 },
                },
                ParameterTemplate {
                    name: params::cascade_mix.as_str(),
                    kind: ParameterKind::Float { min: 0.0, max: 1.0, default: 0.7 },
                },
                ParameterTemplate {
                    name: params::pulse_ms.as_str(),
                    kind: ParameterKind::Float { min: 0.05, max: 2.0, default: 0.5 },
                },
                ParameterTemplate {
                    name: params::clip.as_str(),
                    kind: ParameterKind::Float { min: 0.0, max: 1.0, default: 0.2 },
                },
            ],
            structural_params: &[ParameterTemplate {
                name: "pulse_shape",
                kind: ParameterKind::Enum {
                    variants: Claves2PulseShape::VARIANTS,
                    default: "dirac",
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
        let stage1 = BridgedT::new(sr, 2500.0, 40.0);
        let stage2 = BridgedT::new(sr, 2500.0, 40.0);
        let mut excitation = Excitation::new(sr, instance_id.as_u64());
        excitation.set_shape(read_pulse_shape(structural).to_primitive());
        excitation.set_pulse_ms(0.5);
        Ok(Self {
            instance_id,
            descriptor,
            latched_velocity: 1.0,
            cascade_mix: 0.7,
            stage1,
            stage2,
            excitation,
            edge: EdgeDetector::new(),
            in_trigger: TriggerInput::default(),
            in_velocity: MonoInput::default(),
            out_audio: MonoOutput::default(),
        })
    }

    fn update_validated_parameters(&mut self, p: &ParamView<'_>) {
        let tune = p.get(params::tune);
        let q = p.get(params::q);
        let clip = p.get(params::clip);
        self.stage1.set_tune(tune);
        self.stage1.set_q(q);
        self.stage1.set_clip(clip);
        self.stage2.set_tune(tune);
        self.stage2.set_q(q);
        self.stage2.set_clip(clip);
        self.cascade_mix = p.get(params::cascade_mix);
        self.excitation.set_pulse_ms(p.get(params::pulse_ms));
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
            self.stage1.reset_state();
            self.stage2.reset_state();
            self.excitation.trigger();
            self.edge.reset();
        }
        let s1_in = self.excitation.tick();
        let s1_out = self.stage1.tick(s1_in, 0.0);
        let s2_in = if self.edge.tick(s1_out) { s1_out } else { 0.0 };
        let s2_out = self.stage2.tick(s2_in, 0.0);
        let mixed = s1_out + self.cascade_mix * s2_out;
        pool.write_mono(&self.out_audio, mixed * self.latched_velocity);
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
        let mut h = ModuleHarness::build::<Claves2>(args);
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
        let rms = h.measure_rms(500, "out");
        assert!(rms > 0.001, "rms = {rms}");
    }

    #[test]
    fn pitch_tracking() {
        fn peak(tune: f32) -> usize {
            let mut h = make(&[
                ("tune", ParameterValue::Float(tune)),
                ("cascade_mix", ParameterValue::Float(0.0)),
                ("clip", ParameterValue::Float(0.0)),
            ]);
            fire(&mut h);
            let s = h.run_mono(2048, "out");
            dominant_bin(&magnitude_spectrum(&s, 2048))
        }
        let lo = peak(1000.0);
        let hi = peak(4000.0);
        assert!(hi > lo, "lo={lo}, hi={hi}");
    }

    #[test]
    fn single_stage_collapse_matches_inline_reference() {
        // With cascade_mix = 0, output should match a single-stage BridgedT
        // driven by the same Excitation, sample by sample.
        let mut h = make(&[
            ("tune", ParameterValue::Float(2500.0)),
            ("q", ParameterValue::Float(40.0)),
            ("cascade_mix", ParameterValue::Float(0.0)),
            ("pulse_ms", ParameterValue::Float(0.5)),
            ("clip", ParameterValue::Float(0.0)),
        ]);
        fire(&mut h);
        let module_samples = h.run_mono(500, "out");

        let sr = 44100.0;
        let mut bt = BridgedT::new(sr, 2500.0, 40.0);
        let mut ex = Excitation::new(sr, 1);
        ex.set_shape(PulseShape::Dirac);
        ex.set_pulse_ms(0.5);
        ex.trigger();
        // `fire()` consumes one sample (the trigger tick) which `run_mono`
        // does not return; advance the reference by one before capturing.
        let _ = bt.tick(ex.tick(), 0.0);
        let ref_samples: Vec<f32> = (0..500).map(|_| bt.tick(ex.tick(), 0.0)).collect();

        for (i, (a, b)) in module_samples.iter().zip(ref_samples.iter()).enumerate() {
            assert!(
                (a - b).abs() < 1e-5,
                "sample {i}: module={a}, ref={b}"
            );
        }
    }

    #[test]
    fn cascade_extends_ring() {
        fn rms_at_sample(cascade: f32, start: usize, count: usize) -> f32 {
            let mut h = make(&[
                ("tune", ParameterValue::Float(2500.0)),
                ("q", ParameterValue::Float(30.0)),
                ("cascade_mix", ParameterValue::Float(cascade)),
                ("pulse_ms", ParameterValue::Float(0.5)),
                ("clip", ParameterValue::Float(0.0)),
            ]);
            fire(&mut h);
            for _ in 0..start {
                h.tick();
            }
            h.measure_rms(count, "out")
        }
        let none = rms_at_sample(0.0, 600, 200);
        let full = rms_at_sample(1.0, 600, 200);
        assert!(full > none, "cascade should extend ring: none={none}, full={full}");
    }

    #[test]
    fn output_decays() {
        let mut h = make(&[]);
        fire(&mut h);
        for _ in 0..4000 {
            h.tick();
        }
        let rms = h.measure_rms(500, "out");
        assert!(rms < 0.01, "rms = {rms}");
    }

    #[test]
    fn velocity_scales_output() {
        let mut h_full = make(&[]);
        fire(&mut h_full);
        let rms_full = h_full.measure_rms(500, "out");

        let mut h_half = ModuleHarness::build::<Claves2>(&[]);
        h_half.set_mono("velocity", 0.5);
        fire(&mut h_half);
        let rms_half = h_half.measure_rms(500, "out");

        let ratio = rms_half / rms_full;
        assert!(
            (ratio - 0.5).abs() < 0.1,
            "ratio = {ratio}"
        );
    }
}
