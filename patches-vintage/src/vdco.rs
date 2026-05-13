//! Juno-style vintage DCO — mono ([`VDco`]) and poly ([`poly::VPolyDco`]).
//!
//! One phase accumulator (per voice) drives saw, variable-width pulse, a ÷2
//! sub square, and a wavefolded triangle, all phase-locked. An internal
//! white-noise source and mixer are folded in; the output is a single
//! pre-mixed signal intended to feed a downstream HPF → VCF chain. Gains are
//! biased (not equal-loudness): worst-case sum is sent hot on purpose —
//! character belongs to the downstream filter, not here.
//!
//! Triangle is the Jupiter trick: `tri = 1 - 2*|2*phase - 1|`
//! (absolute-value triangle at the fundamental). No separate phasor — a
//! single phase drives all four waveforms.
//!
//! # Inputs
//!
//! | Port | Kind | Description |
//! |------|------|-------------|
//! | `voct` | mono | Pitch CV (1 V/oct, added to `frequency`) |
//! | `fm` | mono | Frequency modulation (linear Hz or exponential V/oct, per `fm_type`) |
//! | `pwm` | mono | Pulse width (0..1; clamped to `[0.02, 0.98]`) |
//! | `sync` | trigger | Sub-sample hard-sync (ADR 0047): on event at `frac`, phase resets and each waveform applies PolyBLEP scaled by its pre→post jump |
//!
//! # Outputs
//!
//! | Port | Kind | Description |
//! |------|------|-------------|
//! | `out` | mono | Pre-mixed signal (saw + pulse + triangle + sub + noise) |
//! | `reset_out` | trigger | Sub-sample fractional position of each phase wrap (ADR 0047) |
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
//! | `sync_softness` | float | 0.0--1.0 | `0.0` | 0 = instant hard sync (PolyBLEP path). >0 slews the phase toward the reset target with time constant τ = softness²·3 samples (Jupiter-8 RC-discharge model); BLEP residual is skipped since the slew is already C⁰-continuous. |

use patches_sdk::module_params;
use patches_sdk::param_frame::ParamView;
use patches_sdk::cables::TriggerInput;
use patches_sdk::{
    AudioEnvironment, CablePool, CountAxis, InputPort, InstanceId, Module, ModuleDescriptor,
    ModuleDescriptorTemplate, MonoInput, MonoOutput, OutputPort, ParameterKind,
    ParameterTemplate, PortTemplate,
};
use patches_sdk::{StructuralParams, BuildError};

mod core;
pub mod poly;
#[cfg(test)]
mod tests;

pub use self::core::{VDcoFmType, VDcoMix, VDcoVoice};
pub use self::poly::VPolyDco;

module_params! {
    VDco {
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

pub struct VDco {
    instance_id: InstanceId,
    descriptor: ModuleDescriptor,
    voice: VDcoVoice,
    sample_rate: f32,
    mix: VDcoMix,
    frequency: f32,
    fm_type: VDcoFmType,
    in_voct: MonoInput,
    in_fm: MonoInput,
    in_pwm: MonoInput,
    in_sync: TriggerInput,
    out: MonoOutput,
    reset_out: MonoOutput,
}

impl Module for VDco {
    fn template() -> ModuleDescriptorTemplate {
        const T: ModuleDescriptorTemplate = ModuleDescriptorTemplate {
            name: "VDco",
            axes: &[CountAxis::CHANNELS],
            global_inputs: &[
                PortTemplate::mono("voct"),
                PortTemplate::mono("fm"),
                PortTemplate::mono("pwm"),
                PortTemplate::trigger("sync"),
            ],
            per_axis_inputs: &[],
            global_outputs: &[PortTemplate::mono("out"), PortTemplate::trigger("reset_out")],
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
        let mut voice = VDcoVoice::new(instance_id.as_u64());
        voice.phase_increment = self::core::voct_to_increment(0.0, env.sample_rate);
        Self {
            instance_id,
            descriptor,
            voice,
            sample_rate: env.sample_rate,
            mix: VDcoMix::DEFAULT,
            frequency: 0.0,
            fm_type: VDcoFmType::Linear,
            in_voct: MonoInput::default(),
            in_fm: MonoInput::default(),
            in_pwm: MonoInput::default(),
            in_sync: TriggerInput::default(),
            out: MonoOutput::default(),
            reset_out: MonoOutput::default(),
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
        self.in_voct = inputs[0].expect_mono();
        self.in_fm = inputs[1].expect_mono();
        self.in_pwm = inputs[2].expect_mono();
        self.in_sync = TriggerInput::from_ports(inputs, 3);
        self.out = outputs[0].expect_mono();
        self.reset_out = outputs[1].expect_trigger();
    }

    fn process(&mut self, pool: &mut CablePool<'_>) {
        let voct_cv = if self.in_voct.is_connected() {
            pool.read_mono(&self.in_voct)
        } else {
            0.0
        };
        let fm_connected = self.in_fm.is_connected();
        let fm_cv = if fm_connected { pool.read_mono(&self.in_fm) } else { 0.0 };
        self.voice.phase_increment = self::core::compute_increment(
            self.frequency + voct_cv,
            fm_cv,
            self.fm_type,
            fm_connected,
            self.sample_rate,
        );

        let sync = if self.in_sync.is_connected() {
            self.in_sync.tick(pool)
        } else {
            None
        };

        let pwm = if self.in_pwm.is_connected() {
            pool.read_mono(&self.in_pwm)
        } else {
            0.5
        };

        let (y, wrap_frac) = if self.mix.sync_softness > 0.0 {
            self::core::render_and_advance_soft(&mut self.voice, sync, pwm, &self.mix)
        } else {
            match sync {
                Some(frac) => {
                    let y = self::core::render_sync_and_advance(&mut self.voice, frac, pwm, &self.mix);
                    (y, 0.0)
                }
                None => {
                    if !self.out.is_connected() && !self.reset_out.is_connected() {
                        self::core::advance(&mut self.voice);
                        return;
                    }
                    let (y, frac) = self::core::render_and_advance(&mut self.voice, pwm, &self.mix);
                    (y, frac)
                }
            }
        };

        if self.out.is_connected() {
            pool.write_mono(&self.out, y);
        }
        if self.reset_out.is_connected() {
            pool.write_mono(&self.reset_out, wrap_frac);
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
