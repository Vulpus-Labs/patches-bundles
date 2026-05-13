//! Polyphonic Juno-style DCO. One [`VDcoVoice`] per voice; ports (`voct`,
//! `pwm`, `out`) are poly. Shares the DSP core with the mono [`super::VDco`].
//!
//! # Inputs
//!
//! | Port | Kind | Description |
//! |------|------|-------------|
//! | `voct` | poly | Pitch CV per voice (1 V/oct, added to `frequency`) |
//! | `fm` | poly | FM CV per voice (linear Hz or exponential V/oct, per `fm_type`) |
//! | `pwm` | poly | Pulse width per voice (0..1; clamped to `[0.02, 0.98]`) |
//! | `sync` | poly_trigger | Per-voice sub-sample hard-sync (ADR 0047) |
//!
//! # Outputs
//!
//! | Port | Kind | Description |
//! |------|------|-------------|
//! | `out` | poly | Pre-mixed signal (saw + pulse + triangle + sub + noise) per voice |
//! | `reset_out` | poly_trigger | Per-voice sub-sample fractional position of each phase wrap (ADR 0047) |
//!
//! # Parameters
//!
//! | Name | Type | Range | Default | Description |
//! |------|------|-------|---------|-------------|
//! | `frequency` | float | -4.0--12.0 | `0.0` | Baseline pitch (V/oct offset from C0 ≈ 16.35 Hz) |
//! | `fm_type` | enum | `linear` / `logarithmic` | `linear` | FM input interpretation |
//! | `saw_gain` | float | 0.0--1.0 | `1.0` | Saw level in the mix |
//! | `pulse_gain` | float | 0.0--1.0 | `0.0` | Pulse level in the mix |
//! | `triangle_gain` | float | 0.0--1.0 | `0.0` | Wavefolded triangle level |
//! | `sub_gain` | float | 0.0--1.0 | `0.0` | Sub (÷2 square) level |
//! | `noise_gain` | float | 0.0--1.0 | `0.0` | Noise level (internally scaled ≈ 0.5) |
//! | `curve` | float | 0.0--1.0 | `0.1` | Analog cap-charge curvature applied to the phase read (always-on vintage colour) |
//! | `sync_softness` | float | 0.0--1.0 | `0.0` | 0 = instant hard sync (PolyBLEP path). >0 slews the phase toward the reset target (RC-discharge) with τ = softness²·3 samples; BLEP residual is skipped. |

use patches_sdk::cables::PolyTriggerInput;
use patches_sdk::module_params;
use patches_sdk::param_frame::ParamView;
use patches_sdk::{
    AudioEnvironment, CablePool, CountAxis, InputPort, InstanceId, Module, ModuleDescriptor,
    ModuleDescriptorTemplate, OutputPort, ParameterKind, ParameterTemplate, PolyInput,
    PolyOutput, PortTemplate,
};
use patches_sdk::{StructuralParams, BuildError};

use super::core::{
    advance, compute_increment, render_and_advance, render_and_advance_soft,
    render_sync_and_advance, voct_to_increment, VDcoFmType, VDcoMix, VDcoVoice,
};

module_params! {
    VPolyDco {
        frequency:        Float,
        fm_type:          Enum<VDcoFmType>,
        saw_gain:         Float,
        pulse_gain:       Float,
        triangle_gain:    Float,
        sub_gain:        Float,
        noise_gain:      Float,
        curve: Float,
        sync_softness: Float,
    }
}

pub struct VPolyDco {
    instance_id: InstanceId,
    descriptor: ModuleDescriptor,
    voices: [VDcoVoice; 16],
    sample_rate: f32,
    mix: VDcoMix,
    frequency: f32,
    fm_type: VDcoFmType,
    in_voct: PolyInput,
    in_fm: PolyInput,
    in_pwm: PolyInput,
    in_sync: PolyTriggerInput,
    out: PolyOutput,
    reset_out: PolyOutput,
}

impl Module for VPolyDco {
    fn template() -> ModuleDescriptorTemplate {
        const T: ModuleDescriptorTemplate = ModuleDescriptorTemplate {
            name: "VPolyDco",
            axes: &[CountAxis::CHANNELS],
            global_inputs: &[
                PortTemplate::poly("voct"),
                PortTemplate::poly("fm"),
                PortTemplate::poly("pwm"),
                PortTemplate::poly_trigger("sync"),
            ],
            per_axis_inputs: &[],
            global_outputs: &[PortTemplate::poly("out"), PortTemplate::poly_trigger("reset_out")],
            per_axis_outputs: &[],
            realtime_params: &[
                ParameterTemplate { name: params::frequency.as_str(),     kind: ParameterKind::Float { min: -4.0, max: 12.0, default: 0.0 } },
                ParameterTemplate { name: params::fm_type.as_str(),       kind: ParameterKind::Enum { variants: VDcoFmType::VARIANTS, default: "linear" } },
                ParameterTemplate { name: params::saw_gain.as_str(),      kind: ParameterKind::Float { min: 0.0, max: 1.0, default: 1.0 } },
                ParameterTemplate { name: params::pulse_gain.as_str(),    kind: ParameterKind::Float { min: 0.0, max: 1.0, default: 0.0 } },
                ParameterTemplate { name: params::triangle_gain.as_str(), kind: ParameterKind::Float { min: 0.0, max: 1.0, default: 0.0 } },
                ParameterTemplate { name: params::sub_gain.as_str(),      kind: ParameterKind::Float { min: 0.0, max: 1.0, default: 0.0 } },
                ParameterTemplate { name: params::noise_gain.as_str(),    kind: ParameterKind::Float { min: 0.0, max: 1.0, default: 0.0 } },
                ParameterTemplate { name: params::curve.as_str(),         kind: ParameterKind::Float { min: 0.0, max: 1.0, default: 0.1 } },
                ParameterTemplate { name: params::sync_softness.as_str(), kind: ParameterKind::Float { min: 0.0, max: 1.0, default: 0.0 } },
            ],
            structural_params: &[],
            per_axis_realtime_params: &[],
            per_axis_structural_params: &[],
        };
        T
    }

    fn prepare(
        env: &AudioEnvironment,
        descriptor: ModuleDescriptor,
        instance_id: InstanceId, _structural: &StructuralParams,
    ) -> Result<Self, BuildError> { Ok({
        // Derive per-voice seeds from instance_id so voices' noise streams are
        // independent both across voices and across instances.
        let base = instance_id.as_u64();
        let mut voices: [VDcoVoice; 16] = std::array::from_fn(|i| {
            VDcoVoice::new(base.wrapping_add((i as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15)))
        });
        let inc = voct_to_increment(0.0, env.sample_rate);
        for v in &mut voices {
            v.phase_increment = inc;
        }
        Self {
            instance_id,
            descriptor,
            voices,
            sample_rate: env.sample_rate,
            mix: VDcoMix::DEFAULT,
            frequency: 0.0,
            fm_type: VDcoFmType::Linear,
            in_voct: PolyInput::default(),
            in_fm: PolyInput::default(),
            in_pwm: PolyInput::default(),
            in_sync: PolyTriggerInput::default(),
            out: PolyOutput::default(),
            reset_out: PolyOutput::default(),
        }
    })}

    fn update_validated_parameters(&mut self, p: &ParamView<'_>) {
        self.frequency = p.get(params::frequency);
        self.fm_type = p.get(params::fm_type);
        self.mix.saw_gain = p.get(params::saw_gain);
        self.mix.pulse_gain = p.get(params::pulse_gain);
        self.mix.triangle_gain = p.get(params::triangle_gain);
        self.mix.sub_gain = p.get(params::sub_gain);
        self.mix.noise_gain = p.get(params::noise_gain);
        self.mix.curve = p.get(params::curve);
        self.mix.sync_softness = p.get(params::sync_softness);
    }

    fn descriptor(&self) -> &ModuleDescriptor {
        &self.descriptor
    }

    fn instance_id(&self) -> InstanceId {
        self.instance_id
    }

    fn set_ports(&mut self, inputs: &[InputPort], outputs: &[OutputPort]) {
        self.in_voct = PolyInput::from_ports(inputs, 0);
        self.in_fm = PolyInput::from_ports(inputs, 1);
        self.in_pwm = PolyInput::from_ports(inputs, 2);
        self.in_sync = PolyTriggerInput::from_ports(inputs, 3);
        self.out = PolyOutput::from_ports(outputs, 0);
        self.reset_out = outputs[1].expect_poly_trigger();
    }

    fn process(&mut self, pool: &mut CablePool<'_>) {
        let voct = if self.in_voct.is_connected() {
            pool.read_poly(&self.in_voct)
        } else {
            [0.0; 16]
        };
        let fm_connected = self.in_fm.is_connected();
        let fm = if fm_connected { pool.read_poly(&self.in_fm) } else { [0.0; 16] };
        for (i, v) in self.voices.iter_mut().enumerate() {
            v.phase_increment = compute_increment(
                self.frequency + voct[i],
                fm[i],
                self.fm_type,
                fm_connected,
                self.sample_rate,
            );
        }

        let sync = if self.in_sync.is_connected() {
            self.in_sync.tick(pool)
        } else {
            [None; 16]
        };

        let out_connected = self.out.is_connected();
        let reset_connected = self.reset_out.is_connected();

        let pw_connected = self.in_pwm.is_connected();
        let pwm = if pw_connected {
            pool.read_poly(&self.in_pwm)
        } else {
            [0.5; 16]
        };

        let mut out = [0.0f32; 16];
        let mut reset = [0.0f32; 16];
        let soft = self.mix.sync_softness > 0.0;
        for (i, v) in self.voices.iter_mut().enumerate() {
            if soft {
                let (y, frac) = render_and_advance_soft(v, sync[i], pwm[i], &self.mix);
                out[i] = y;
                reset[i] = frac;
                continue;
            }
            match sync[i] {
                Some(frac) => {
                    out[i] = render_sync_and_advance(v, frac, pwm[i], &self.mix);
                }
                None => {
                    if !out_connected && !reset_connected {
                        advance(v);
                    } else {
                        let (y, frac) = render_and_advance(v, pwm[i], &self.mix);
                        out[i] = y;
                        reset[i] = frac;
                    }
                }
            }
        }
        if out_connected {
            pool.write_poly(&self.out, out);
        }
        if reset_connected {
            pool.write_poly(&self.reset_out, reset);
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
