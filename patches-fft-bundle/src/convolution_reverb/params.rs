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

/// IR variant names (snake_case) in descriptor/declaration order.
pub(super) const IR_VARIANTS: &[&str] = IrVariant::VARIANTS;

/// Index of the "file" variant in [`IR_VARIANTS`].
pub(super) const FILE_VARIANT_IDX: u8 = IrVariant::File as u8;

/// File extensions supported by the convolution reverb's `ir_data` parameter.
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

// Variant parameters: (duration_secs, seed_l, seed_r, lowpass_l, lowpass_r, ramp_rate)
const ROOM_PARAMS:  (f32, u64, u64, f32, f32, f32) = (0.4, 12345, 54321, 0.0,  0.0,  0.0);
const HALL_PARAMS:  (f32, u64, u64, f32, f32, f32) = (1.5, 67890, 9876,  0.15, 0.13, 0.0);
const PLATE_PARAMS: (f32, u64, u64, f32, f32, f32) = (2.0, 24680, 13579, 0.0,  0.0,  200.0);

fn variant_params(name: &str) -> (f32, u64, u64, f32, f32, f32) {
    match name {
        "room"  => ROOM_PARAMS,
        "hall"  => HALL_PARAMS,
        "plate" => PLATE_PARAMS,
        _       => ROOM_PARAMS,
    }
}

/// Generate a synthetic mono IR for the given variant name.
pub(super) fn generate_variant_ir(variant: &str, sample_rate: f32) -> Vec<f32> {
    let (dur, seed_l, _, lp_l, _, ramp) = variant_params(variant);
    generate_ir(sample_rate, dur, seed_l, lp_l, ramp)
}

/// Generate a synthetic stereo IR pair for the given variant name.
pub(super) fn generate_stereo_variant_ir(variant: &str, sample_rate: f32) -> (Vec<f32>, Vec<f32>) {
    let (dur, seed_l, seed_r, lp_l, lp_r, ramp) = variant_params(variant);
    (
        generate_ir(sample_rate, dur, seed_l, lp_l, ramp),
        generate_ir(sample_rate, dur, seed_r, lp_r, ramp),
    )
}

