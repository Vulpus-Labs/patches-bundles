//! Vintage BBD reverb module — 4-line FDN built on bucket-brigade delays.
//!
//! Eight [`crate::bbd::Bbd`] 1024-stage lines with mutually-coprime
//! delays are cross-mixed by an 8×8 Hadamard matrix and fed back with a
//! decay coefficient. Eight lines (rather than four) gives enough modal
//! density to avoid audible beating between a sparse set of resonances.
//!
//! No compander. The NE570 pair's round-trip gain is only unity at
//! `ref_level`; at other levels it is `(ref/level)^0.25`, which is
//! benign on a single-pass BBD delay but destabilises an FDN feedback
//! loop (quiet tail → loop gain > 1 → runaway → saturate → compressor
//! drags it silent → cycle). The BBDs' own anti-imaging filters and
//! bucket saturation carry the vintage voice without it.
//!
//! Character: the BBD anti-imaging/reconstruction filters provide the
//! dark HF damping real reverbs need, and the compander's program-
//! dependent hiss fills the tail with gentle analog grit. There is no
//! dedicated damping filter or early-reflection stage — the BBD colour
//! is the reverb's voice.
//!
//! Not a Schroeder reverb and not a faithful model of any specific
//! hardware unit (BBD reverbs were rare); more a plausible vintage
//! plate/room built from the same parts a 1980s pedal-builder had.
//!
//! # Inputs
//!
//! | Port | Kind | Description |
//! |------|------|-------------|
//! | `in` | stereo | Stereo audio input (mono-broadcast feeds both halves) |
//! | `drywet_cv` | mono | Additive CV for dry/wet |
//! | `size_cv` | mono | Additive CV for size |
//! | `decay_cv` | mono | Additive CV for decay |
//!
//! # Outputs
//!
//! | Port | Kind | Description |
//! |------|------|-------------|
//! | `out` | stereo | Stereo wet/dry output |
//!
//! # Parameters
//!
//! | Name | Type | Range | Default | Description |
//! |------|------|-------|---------|-------------|
//! | `dry_wet` | float | 0.0--1.0 | `0.3` | Dry/wet mix |
//! | `size` | float | 0.0--1.0 | `0.5` | Room size (scales all four delays) |
//! | `decay` | float | 0.0--0.95 | `0.7` | FDN feedback coefficient |
//! | `damping` | float | 0.0--1.0 | `0.5` | HF damping in feedback (0 bright, 1 dark) |
//! | `jitter` | float | 0.0--1.0 | `0.0` | BBD clock jitter amount |

use patches_sdk::module_params;
use patches_sdk::param_frame::ParamView;
use patches_sdk::{
    AudioEnvironment, CablePool, CountAxis, InputPort, InstanceId, Module, ModuleDescriptor,
    ModuleDescriptorTemplate, MonoInput, OutputPort, ParameterKind, ParameterTemplate,
    PortTemplate, StereoInput, StereoOutput,
};
use patches_sdk::{StructuralParams, BuildError};
use patches_dsp::approximate::fast_tanh;

use crate::bbd::{Bbd, BbdDevice};

module_params! {
    VReverb {
        dry_wet: Float,
        size:    Float,
        decay:   Float,
        damping: Float,
        jitter:  Float,
    }
}

const DECAY_MAX: f32 = 0.95;
const N: usize = 8;
/// Normalisation for the 8-tap orthogonal stereo pickoff: dividing by
/// `sqrt(N)` keeps `wet_l` and `wet_r` energy independent of the tap count.
const WET_NORM: f32 = 0.353_553_4; // 1.0 / sqrt(8)

/// Two-pole reconstruction / damping LPF on each BBD output. `damping = 0`
/// → bright (8 kHz), `damping = 1` → dark (1.2 kHz). Even the brightest
/// setting must suppress the BBD clock images: at the longest delay
/// (~78 ms), clock = 1024/0.078 ≈ 13 kHz, so the cascade's 12 dB/oct
/// rolloff above 8 kHz is what removes audible clock whine. This filter
/// also serves as the recirculation HF damping (since `y_prev = y` is
/// taken after filtering), giving per-pass tail darkening that real
/// plates/rooms have.
const DAMP_FC_MIN_HZ: f32 = 1_200.0;
const DAMP_FC_MAX_HZ: f32 = 8_000.0;

/// Mutually-coprime base delays in milliseconds. Scaled by `size` into
/// the 1024-stage BBD's honest range (≲ 85 ms).
const BASE_DELAYS_MS: [f32; N] = [
    19.3, 23.1, 29.7, 31.3, 37.9, 41.7, 47.3, 53.9,
];
/// Size parameter maps linearly from `SIZE_MIN_SCALE` to `SIZE_MAX_SCALE`.
const SIZE_MIN_SCALE: f32 = 0.35;
const SIZE_MAX_SCALE: f32 = 1.45;

/// 8×8 normalised Hadamard, built as `[[H4, H4], [H4, -H4]] / sqrt(2)`.
/// Rows orthonormal with overall factor `1/sqrt(8)`.
#[inline(always)]
fn hadamard8(v: [f32; N]) -> [f32; N] {
    // 4×4 Hadamard sub-block (un-normalised).
    #[inline(always)]
    fn h4(a: f32, b: f32, c: f32, d: f32) -> [f32; 4] {
        [a + b + c + d, a - b + c - d, a + b - c - d, a - b - c + d]
    }
    let t = h4(v[0], v[1], v[2], v[3]);
    let u = h4(v[4], v[5], v[6], v[7]);
    let s = 1.0 / (8.0_f32).sqrt();
    [
        s * (t[0] + u[0]),
        s * (t[1] + u[1]),
        s * (t[2] + u[2]),
        s * (t[3] + u[3]),
        s * (t[0] - u[0]),
        s * (t[1] - u[1]),
        s * (t[2] - u[2]),
        s * (t[3] - u[3]),
    ]
}

/// Map `damping` ∈ [0, 1] to a one-pole LPF coefficient at sample rate `sr`.
/// Linear in log-cutoff so the control feel is even.
#[inline]
fn damping_coeff(sr: f32, damping: f32) -> f32 {
    let lo = DAMP_FC_MIN_HZ.ln();
    let hi = DAMP_FC_MAX_HZ.ln();
    // damping = 0 → max cutoff (bright), damping = 1 → min cutoff (dark)
    let fc = (hi + (lo - hi) * damping).exp();
    let a = 1.0 - (-std::f32::consts::TAU * fc / sr).exp();
    a.clamp(0.0, 1.0)
}

/// Vintage BBD reverb. See the module-level documentation.
pub struct VReverb {
    instance_id: InstanceId,
    descriptor: ModuleDescriptor,

    bbds: [Bbd; N],
    /// Previous-sample BBD outputs carried through the 1-sample cable
    /// delay that makes the FDN causal.
    y_prev: [f32; N],

    dry_wet: f32,
    size: f32,
    decay: f32,
    /// One-pole LPF coefficient `a = 1 - exp(-2π fc / sr)` shared by
    /// both stages of the per-line cascade.
    damp_a: f32,
    /// First-stage LPF state per line.
    damp_z1: [f32; N],
    /// Second-stage LPF state per line (cascaded for 12 dB/oct).
    damp_z2: [f32; N],
    sr: f32,

    in_stereo: StereoInput,
    drywet_cv: MonoInput,
    size_cv: MonoInput,
    decay_cv: MonoInput,
    out_stereo: StereoOutput,
}

impl Module for VReverb {
    fn template() -> ModuleDescriptorTemplate {
        const T: ModuleDescriptorTemplate = ModuleDescriptorTemplate {
            name: "VReverb",
            axes: &[CountAxis::CHANNELS],
            global_inputs: &[
                PortTemplate::stereo("in"),
                PortTemplate::mono("drywet_cv"),
                PortTemplate::mono("size_cv"),
                PortTemplate::mono("decay_cv"),
            ],
            per_axis_inputs: &[],
            global_outputs: &[PortTemplate::stereo("out")],
            per_axis_outputs: &[],
            realtime_params: &[
                ParameterTemplate { name: params::dry_wet.as_str(), kind: ParameterKind::Float { min: 0.0, max: 1.0,       default: 0.3 } },
                ParameterTemplate { name: params::size.as_str(),    kind: ParameterKind::Float { min: 0.0, max: 1.0,       default: 0.5 } },
                ParameterTemplate { name: params::decay.as_str(),   kind: ParameterKind::Float { min: 0.0, max: DECAY_MAX, default: 0.7 } },
                ParameterTemplate { name: params::damping.as_str(), kind: ParameterKind::Float { min: 0.0, max: 1.0,       default: 0.5 } },
                ParameterTemplate { name: params::jitter.as_str(),  kind: ParameterKind::Float { min: 0.0, max: 1.0,       default: 0.0 } },
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
        let sr = env.sample_rate;
        let interval = env.periodic_update_interval;
        Self {
            instance_id,
            descriptor,
            bbds: {
                let seed_base = (instance_id.as_u64() ^ 0xBBD0_0040) as u32;
                std::array::from_fn(|i| {
                    let mut b = Bbd::new_with_smoothing_interval(
                        &BbdDevice::BBD_1024, sr, interval,
                    );
                    b.set_jitter_seed(seed_base.wrapping_add(i as u32));
                    b
                })
            },
            y_prev: [0.0; N],
            dry_wet: 0.3,
            size: 0.5,
            decay: 0.7,
            damp_a: damping_coeff(sr, 0.5),
            damp_z1: [0.0; N],
            damp_z2: [0.0; N],
            sr,
            in_stereo: StereoInput::default(),
            drywet_cv: MonoInput::default(),
            size_cv: MonoInput::default(),
            decay_cv: MonoInput::default(),
            out_stereo: StereoOutput::default(),
        }
    })}

    fn update_validated_parameters(&mut self, p: &ParamView<'_>) {
        self.dry_wet = p.get(params::dry_wet).clamp(0.0, 1.0);
        self.size = p.get(params::size).clamp(0.0, 1.0);
        self.decay = p.get(params::decay).clamp(0.0, DECAY_MAX);
        let damping = p.get(params::damping).clamp(0.0, 1.0);
        self.damp_a = damping_coeff(self.sr, damping);
        let jitter = p.get(params::jitter).clamp(0.0, 1.0);
        for b in self.bbds.iter_mut() {
            b.set_jitter_amount(jitter);
        }
    }

    fn descriptor(&self) -> &ModuleDescriptor {
        &self.descriptor
    }

    fn instance_id(&self) -> InstanceId {
        self.instance_id
    }

    fn set_ports(&mut self, inputs: &[InputPort], outputs: &[OutputPort]) {
        self.in_stereo = StereoInput::from_ports(inputs, 0);
        self.drywet_cv = MonoInput::from_ports(inputs, 1);
        self.size_cv = MonoInput::from_ports(inputs, 2);
        self.decay_cv = MonoInput::from_ports(inputs, 3);
        self.out_stereo = StereoOutput::from_ports(outputs, 0);
    }

    fn process(&mut self, pool: &mut CablePool<'_>) {
        // Mono-broadcast already replicates the source on both halves,
        // so the dedicated mono-fallback branch is no longer needed.
        let (l_in, r_in) = pool.read_stereo(&self.in_stereo);

        let decay = (self.decay + pool.read_mono(&self.decay_cv)).clamp(0.0, DECAY_MAX);

        // Drive the first four BBD lines from the left channel and the
        // second four from the right. The Hadamard mix cross-pollinates
        // them in the tail, so energy migrates between sides for a
        // natural stereo spread while early reflections stay sided.
        let x_l = fast_tanh(l_in);
        let x_r = fast_tanh(r_in);
        let mixed = hadamard8(self.y_prev);

        let mut y = [0.0_f32; N];
        let a = self.damp_a;
        for k in 0..N {
            let drive_src = if k < N / 2 { x_l } else { x_r };
            // Soft-clip the recirculating path: Hadamard + tanh is
            // strictly passive, so this bounds the loop at `decay < 1`.
            let drive = drive_src + fast_tanh(decay * mixed[k]);
            // Two-pole LPF (cascaded one-poles) on the BBD output.
            // Doubles as reconstruction filter (kills BBD clock images
            // at audible long-delay clock rates) and recirculation HF
            // damping (since y feeds y_prev next sample).
            let raw = self.bbds[k].process(drive);
            let s1 = patches_dsp::flush_denormal(self.damp_z1[k] + a * (raw - self.damp_z1[k]));
            self.damp_z1[k] = s1;
            let s2 = patches_dsp::flush_denormal(self.damp_z2[k] + a * (s1 - self.damp_z2[k]));
            self.damp_z2[k] = s2;
            y[k] = s2;
        }
        self.y_prev = y;

        // Decorrelated stereo pickoff: alternating signs across the
        // eight taps so L and R draw on orthogonal state combinations.
        let wet_l = WET_NORM * (y[0] - y[1] + y[2] - y[3] + y[4] - y[5] + y[6] - y[7]);
        let wet_r = WET_NORM * (y[0] + y[1] - y[2] - y[3] + y[4] + y[5] - y[6] - y[7]);

        let eff_dw = (self.dry_wet + pool.read_mono(&self.drywet_cv)).clamp(0.0, 1.0);
        pool.write_stereo(
            &self.out_stereo,
            l_in + eff_dw * (wet_l - l_in),
            r_in + eff_dw * (wet_r - r_in),
        );
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn wants_periodic(&self) -> bool { true }

    fn periodic_update(&mut self, pool: &CablePool<'_>) {
        // Delay times are driven by the `size` parameter + `size_cv`
        // input; both change at Periodic cadence, so one `set_delay`
        // per line per tick is enough. The BBD smooths internally
        // across its own (finer) smoothing interval.
        let size = (self.size + pool.read_mono(&self.size_cv)).clamp(0.0, 1.0);
        let scale = SIZE_MIN_SCALE + (SIZE_MAX_SCALE - SIZE_MIN_SCALE) * size;
        for (k, base) in BASE_DELAYS_MS.iter().enumerate() {
            self.bbds[k].set_delay_seconds(base * scale * 0.001);
        }
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use patches_sdk::parameter_map::{ParameterMap, ParameterValue};
    use patches_sdk::test_support::{params, ModuleHarness};
    use patches_sdk::{AudioEnvironment, ModuleShape};

    const SR: f32 = 48_000.0;
    const ENV: AudioEnvironment = AudioEnvironment {
        sample_rate: SR,
        poly_voices: 16,
        periodic_update_interval: 32,
        hosted: false,
    };

    fn shape() -> ModuleShape {
        ModuleShape { channels: 0 }
    }

    fn disconnect_cvs(h: &mut ModuleHarness) {
        h.disconnect_input("drywet_cv");
        h.disconnect_input("size_cv");
        h.disconnect_input("decay_cv");
    }

    #[test]
    fn dry_wet_zero_passes_only_dry() {
        let mut h =
            ModuleHarness::build_full::<VReverb>(params!["dry_wet" => 0.0_f32], ENV, shape());
        disconnect_cvs(&mut h);
        h.set_stereo("in", 0.7, 0.7);
        h.tick();
        let (l, r) = h.read_stereo("out");
        assert_eq!(l, 0.7);
        assert_eq!(r, 0.7);
    }

    #[test]
    fn output_is_bounded_under_sustained_input() {
        let mut h = ModuleHarness::build_full::<VReverb>(params![], ENV, shape());
        let mut pm = ParameterMap::new();
        pm.insert_param("dry_wet", 0, ParameterValue::Float(1.0));
        pm.insert_param("size", 0, ParameterValue::Float(0.7));
        pm.insert_param("decay", 0, ParameterValue::Float(0.9));
        h.update_params_map(&pm);
        disconnect_cvs(&mut h);

        for i in 0..40_000 {
            let t = i as f32 / SR;
            let x = 0.5 * (std::f32::consts::TAU * 440.0 * t).sin();
            h.set_stereo("in", x, x);
            h.tick();
            let (l, r) = h.read_stereo("out");
            assert!(
                l.is_finite() && r.is_finite() && l.abs() < 5.0 && r.abs() < 5.0,
                "diverged at i={i}: l={l} r={r}"
            );
        }
    }

    #[test]
    fn impulse_tail_decays() {
        let mut h = ModuleHarness::build_full::<VReverb>(params![], ENV, shape());
        let mut pm = ParameterMap::new();
        pm.insert_param("dry_wet", 0, ParameterValue::Float(1.0));
        pm.insert_param("size", 0, ParameterValue::Float(0.5));
        pm.insert_param("decay", 0, ParameterValue::Float(0.6));
        h.update_params_map(&pm);
        disconnect_cvs(&mut h);

        h.set_stereo("in", 1.0, 1.0);
        h.tick();
        h.set_stereo("in", 0.0, 0.0);

        let mut early_peak = 0.0_f32;
        for _ in 0..((0.2 * SR) as usize) {
            h.tick();
            let (l, r) = h.read_stereo("out");
            let m = l.abs().max(r.abs());
            if m > early_peak {
                early_peak = m;
            }
        }
        let mut late_peak = 0.0_f32;
        for _ in 0..((0.5 * SR) as usize) {
            h.tick();
            let (l, r) = h.read_stereo("out");
            let m = l.abs().max(r.abs());
            if m > late_peak {
                late_peak = m;
            }
        }
        assert!(
            late_peak < early_peak,
            "tail should decay: early={early_peak} late={late_peak}"
        );
    }
}
