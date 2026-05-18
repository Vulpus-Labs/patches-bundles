//! Shared parameters and IR generation for convolution reverb.

use std::sync::atomic::AtomicBool;

use patches_dsp::AtomicF32;
use patches_dsp::noise::xorshift64;

// ---------------------------------------------------------------------------
// Shared parameters (audio thread → processing thread via atomics)
// ---------------------------------------------------------------------------

pub(super) struct SharedParams {
    pub(super) mix: AtomicF32,
    pub(super) shutdown: AtomicBool,
}

impl SharedParams {
    pub(super) fn new() -> Self {
        Self {
            mix: AtomicF32::new(1.0),
            shutdown: AtomicBool::new(false),
        }
    }
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Block size for the convolver (N). The FFT operates on 2N = 2048 samples.
pub(super) const BLOCK_SIZE: usize = 1024;

/// Processing budget in audio-clock samples.
pub(super) const PROCESSING_BUDGET: usize = 1024;

/// Maximum tier block size for the non-uniform convolver.
/// Tiers double from BLOCK_SIZE up to this cap.
pub(super) const MAX_TIER_BLOCK_SIZE: usize = 32768;

patches_sdk::params_enum! {
    pub enum IrVariant {
        Room => "room",
        Hall => "hall",
        Plate => "plate",
        File => "file",
    }
}

/// File extensions supported by the convolution reverb's `ir_path` parameter.
pub(super) const IR_FILE_EXTENSIONS: &[&str] = &["wav", "aiff", "aif"];

// ---------------------------------------------------------------------------
// Synthetic IR generation
// ---------------------------------------------------------------------------

/// Normalise a buffer so its peak is at `target`.
fn normalise(buf: &mut [f32], target: f32) {
    let peak = buf.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
    if peak > 0.0 {
        let scale = target / peak;
        for s in buf {
            *s *= scale;
        }
    }
}

/// Generate a synthetic impulse response: exponential-decay noise with optional
/// one-pole lowpass filtering and pre-delay ramp.
///
/// - `lowpass_cutoff`: one-pole LP coefficient (0.0 = bypass)
/// - `ramp_rate`: pre-delay ramp speed in 1/s (0.0 = no ramp)
pub(super) fn generate_ir(
    sample_rate: f32,
    duration_secs: f32,
    seed: u64,
    lowpass_cutoff: f32,
    ramp_rate: f32,
) -> Vec<f32> {
    let len = (sample_rate * duration_secs) as usize;
    let decay_rate = 6.0 / duration_secs;
    let mut rng = seed;
    let mut ir = Vec::with_capacity(len);
    let mut lp_state = 0.0f32;
    for i in 0..len {
        let t = i as f32 / sample_rate;
        let ramp = if ramp_rate > 0.0 { (t * ramp_rate).min(1.0) } else { 1.0 };
        let raw = xorshift64(&mut rng) * ramp * (-decay_rate * t).exp();
        if lowpass_cutoff > 0.0 {
            lp_state += lowpass_cutoff * (raw - lp_state);
            ir.push(lp_state);
        } else {
            ir.push(raw);
        }
    }
    normalise(&mut ir, 0.5);
    ir
}

/// Tunable parameters for a synthetic IR variant.
struct VariantParams {
    duration_secs: f32,
    seed_l: u64,
    seed_r: u64,
    lowpass_l: f32,
    lowpass_r: f32,
    ramp_rate: f32,
}

const ROOM_PARAMS:  VariantParams = VariantParams { duration_secs: 0.4, seed_l: 12345, seed_r: 54321, lowpass_l: 0.0,  lowpass_r: 0.0,  ramp_rate: 0.0   };
const HALL_PARAMS:  VariantParams = VariantParams { duration_secs: 1.5, seed_l: 67890, seed_r: 9876,  lowpass_l: 0.15, lowpass_r: 0.13, ramp_rate: 0.0   };
const PLATE_PARAMS: VariantParams = VariantParams { duration_secs: 2.0, seed_l: 24680, seed_r: 13579, lowpass_l: 0.0,  lowpass_r: 0.0,  ramp_rate: 200.0 };

/// Tunable parameters for each synthetic variant. `File` falls through to
/// the caller (no synthetic generation).
fn variant_params(variant: IrVariant) -> Option<&'static VariantParams> {
    match variant {
        IrVariant::Room  => Some(&ROOM_PARAMS),
        IrVariant::Hall  => Some(&HALL_PARAMS),
        IrVariant::Plate => Some(&PLATE_PARAMS),
        IrVariant::File  => None,
    }
}

/// Generate a synthetic mono IR for the given variant. Returns `None` for
/// `IrVariant::File` (the caller resolves it from `ir_path`).
pub(super) fn generate_variant_ir(variant: IrVariant, sample_rate: f32) -> Option<Vec<f32>> {
    let p = variant_params(variant)?;
    Some(generate_ir(sample_rate, p.duration_secs, p.seed_l, p.lowpass_l, p.ramp_rate))
}

/// Generate a synthetic stereo IR pair for the given variant. Returns `None`
/// for `IrVariant::File`.
pub(super) fn generate_stereo_variant_ir(
    variant: IrVariant,
    sample_rate: f32,
) -> Option<(Vec<f32>, Vec<f32>)> {
    let p = variant_params(variant)?;
    Some((
        generate_ir(sample_rate, p.duration_secs, p.seed_l, p.lowpass_l, p.ramp_rate),
        generate_ir(sample_rate, p.duration_secs, p.seed_r, p.lowpass_r, p.ramp_rate),
    ))
}

