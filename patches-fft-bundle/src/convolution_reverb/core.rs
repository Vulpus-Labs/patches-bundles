//! [`ConvReverbCore`] — shared state machine for the mono and stereo
//! convolution-reverb modules.
//!
//! Post-ADR-0060, IR selection (`ir` enum + `ir_path` file) is structural:
//! any change rebuilds the module via the planner. The audio thread never
//! constructs convolvers, spawns threads, or touches the heap; it only
//! reads/writes overlap buffers and the shared mix atomic.

use std::sync::atomic::Ordering::Relaxed;
use std::sync::Arc;

use patches_sdk::module_params;
use patches_sdk::param_frame::ParamView;

use super::params::{IrVariant, SharedParams};
use super::processor::ProcessorKit;

module_params! {
    ConvReverbCoreParams {
        mix: Float,
        ir:  Enum<IrVariant>,
    }
}

/// Core state machine shared by [`super::ConvolutionReverb`] and
/// [`super::StereoConvReverb`].
///
/// Owns the per-channel processor kits (overlap buffers + worker threads),
/// the shared parameter block, and the cached base mix. Constructed in
/// `Module::prepare` (control thread); moved to the audio thread for its
/// lifetime.
pub(super) struct ConvReverbCore {
    /// Per-channel processor kits. `None` when the module is in passthrough
    /// mode (e.g. `ir = file` with no `ir_path`).
    pub(super) kits: Vec<Option<ProcessorKit>>,

    /// Shared parameter block. Cloned into each kit's worker; the canonical
    /// `Arc` lives here so the audio thread can update `mix` without
    /// touching the kits.
    pub(super) shared: Arc<SharedParams>,

    base_mix: f32,
}

impl Drop for ConvReverbCore {
    fn drop(&mut self) {
        self.shared.shutdown.store(true, Relaxed);
        for kit in self.kits.iter_mut().filter_map(|k| k.take()) {
            kit.shared.shutdown.store(true, Relaxed);
            let _ = kit.thread.join();
        }
    }
}

impl ConvReverbCore {
    /// Build with pre-constructed kits (one per channel). Pass an empty
    /// `Vec` slot (`None`) per channel for passthrough mode.
    pub(super) fn from_kits(kits: Vec<Option<ProcessorKit>>, shared: Arc<SharedParams>, base_mix: f32) -> Self {
        shared.mix.store(base_mix);
        Self { kits, shared, base_mix }
    }

    /// Set the cached base mix (control thread, `apply_unpacked_params`).
    pub(super) fn set_base_mix(&mut self, mix: f32) {
        self.base_mix = mix;
        self.shared.mix.store(mix);
    }

    /// Update the worker-visible mix from the cached base mix and the
    /// current CV value. Called from `periodic_update` on the audio thread.
    pub(super) fn update_shared_mix(&self, mix_cv: f32) {
        let mix = (self.base_mix + mix_cv).clamp(0.0, 1.0);
        self.shared.mix.store(mix);
    }

    /// Realtime parameter update on the audio thread. Only `mix` is
    /// realtime now; `ir` is structural so any change rebuilds the module.
    pub(super) fn update_validated_parameters(&mut self, p: &ParamView<'_>) {
        self.base_mix = p.get(params::mix);
        self.update_shared_mix(0.0);
    }
}
