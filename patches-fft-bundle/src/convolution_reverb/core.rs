//! [`ConvReverbCore`] — shared state machine for the mono and stereo
//! convolution-reverb modules.

use std::sync::atomic::Ordering::Relaxed;
use std::sync::Arc;

use patches_sdk::build_error::BuildError;
use patches_sdk::module_params;
use patches_sdk::parameter_map::{ParameterMap, ParameterValue};
use patches_sdk::param_frame::ParamView;

use patches_fft_harness::partitioned_convolution::NonUniformConvolver;
use patches_fft_harness::slot_deck::OverlapBuffer;

use super::ir_loader::{
    build_mono_ready, build_stereo_ready, cleanup_processor_ready, IrLoadRequest, IrLoader,
    MonoProcessorReady, ProcessorReady, ProcessorTeardown, StereoProcessorReady,
};
use super::params::{
    generate_stereo_variant_ir, generate_variant_ir, SharedParams, BLOCK_SIZE,
    FILE_VARIANT_IDX, IR_VARIANTS, IrVariant, MAX_TIER_BLOCK_SIZE,
};

module_params! {
    ConvReverbCoreParams {
        mix: Float,
        ir:  Enum<IrVariant>,
    }
}

/// Core state machine shared by [`super::ConvolutionReverb`] and
/// [`super::StereoConvReverb`].
///
/// Manages the IR loader, processor thread lifecycle, parameter caching, and
/// the install/teardown protocol. Parameterised by channel count: 1 for mono,
/// 2 for stereo. Each channel has its own `OverlapBuffer` and processor thread.
pub(super) struct ConvReverbCore {
    stereo: bool,
    sample_rate: f32,

    // Per-channel overlap buffers (1 for mono, 2 for stereo)
    pub(super) overlap_buffers: Vec<Option<OverlapBuffer>>,

    // Shared parameters (one set — all channels share the same mix)
    shared: Arc<SharedParams>,

    // Cached parameter values
    base_mix: f32,
    ir_variant_idx: u8,

    // Processing thread handles (one per channel)
    pub(super) threads: Vec<Option<std::thread::JoinHandle<()>>>,

    // Async IR loading
    ir_loader: IrLoader,
}

// SAFETY: ConvReverbCore is constructed on the control thread and sent once
// to the audio thread (via Module: Send), where it remains for its lifetime.
// OverlapBuffer is !Send as a lint against casual cross-thread use, but single
// ownership transfer at plan activation is safe.
unsafe impl Send for ConvReverbCore {}

impl Drop for ConvReverbCore {
    fn drop(&mut self) {
        self.shared.shutdown.store(true, Relaxed);
        for thread in &mut self.threads {
            if let Some(h) = thread.take() {
                let _ = h.join();
            }
        }
        // IrLoader's Drop handles the loader thread and any unclaimed results.
    }
}

impl ConvReverbCore {
    pub(super) fn new(stereo: bool, sample_rate: f32) -> Self {
        let channels = if stereo { 2 } else { 1 };
        Self {
            stereo,
            sample_rate,
            overlap_buffers: (0..channels).map(|_| None).collect(),
            shared: Arc::new(SharedParams::new()),
            base_mix: 1.0,
            ir_variant_idx: 0,
            threads: (0..channels).map(|_| None).collect(),
            ir_loader: IrLoader::new(),
        }
    }

    /// Install fields from a `ProcessorReady` into self.
    fn adopt_ready(&mut self, ready: ProcessorReady) {
        match ready {
            ProcessorReady::Mono(MonoProcessorReady { kit }) => {
                self.overlap_buffers[0] = Some(kit.overlap_buffer);
                self.shared = kit.shared;
                self.threads[0] = Some(kit.thread);
            }
            ProcessorReady::Stereo(stereo) => {
                let StereoProcessorReady { kit_l, kit_r, shared } = *stereo;
                self.overlap_buffers[0] = Some(kit_l.overlap_buffer);
                self.overlap_buffers[1] = Some(kit_r.overlap_buffer);
                self.shared = shared;
                self.threads[0] = Some(kit_l.thread);
                self.threads[1] = Some(kit_r.thread);
            }
        }
    }

    /// Start processor(s) from a `ProcessorReady` result (control thread).
    ///
    /// Shuts down any existing processor threads synchronously — safe because
    /// this only runs on the control thread during build.
    fn start_from_ready(&mut self, ready: ProcessorReady) {
        // Shut down existing processors.
        self.shared.shutdown.store(true, Relaxed);
        for thread in &mut self.threads {
            if let Some(h) = thread.take() {
                let _ = h.join();
            }
        }
        self.adopt_ready(ready);
    }

    /// Install a processor received from the IR loader (audio thread).
    ///
    /// Sends the old processor to the loader thread for off-audio-thread teardown.
    fn install_from_ready(&mut self, ready: ProcessorReady) {
        // Collect old threads and overlap buffers for teardown.
        let old_shared = std::mem::replace(
            &mut self.shared,
            Arc::new(SharedParams::new()),
        );
        let old_threads: Vec<_> = self.threads.iter_mut()
            .filter_map(|t| t.take())
            .collect();
        let old_overlaps: Vec<_> = self.overlap_buffers.iter_mut()
            .filter_map(|o| o.take())
            .collect();

        if !old_threads.is_empty() {
            old_shared.shutdown.store(true, Relaxed);
            let teardown = ProcessorTeardown {
                shared: old_shared,
                threads: old_threads,
                overlap_buffers: old_overlaps,
            };
            match self.ir_loader.teardown_tx.push(teardown) {
                Ok(()) => self.ir_loader.wake(),
                Err(rtrb::PushError::Full(td)) => {
                    eprintln!(
                        "patches: IR teardown buffer full — detaching old processor"
                    );
                    td.shared.shutdown.store(true, Relaxed);
                    drop(td);
                }
            }
        }

        self.adopt_ready(ready);
    }

    /// Send an IR load request to the loader thread. O(1), non-blocking,
    /// no allocation — safe to call on the audio thread.
    fn send_load_request(&mut self, request: IrLoadRequest) {
        if self.ir_loader.request_tx.push(request).is_ok() {
            self.ir_loader.wake();
        }
    }

    pub(super) fn update_shared_mix(&self, mix_cv: f32) {
        let mix = (self.base_mix + mix_cv).clamp(0.0, 1.0);
        self.shared.mix.store(mix);
    }

    /// Handle parameter updates on the control thread (initial build).
    ///
    /// Resolves the IR synchronously — file I/O and convolver construction
    /// are safe here (not the audio thread).
    pub(super) fn update_parameters(
        &mut self,
        params: &ParameterMap,
        module_name: &'static str,
        pre_fft_ir: Option<Vec<f32>>,
    ) -> Result<(), BuildError> {
        if let Some(ParameterValue::Float(v)) = params.get("mix", 0) {
            self.base_mix = *v;
        }
        if let Some(&ParameterValue::Enum(v)) = params.get("ir", 0) {
            let idx = v as u8;
            if (idx as usize) < IR_VARIANTS.len() {
                self.ir_variant_idx = idx;
            }
        }

        // Pre-decoded IR from the `ir_path` structural param (ADR 0060).
        if let Some(data) = pre_fft_ir {
            let ready = if self.stereo {
                let left_len = data[0] as usize;
                let conv_l = NonUniformConvolver::from_pre_fft(&data[1..1 + left_len]);
                let conv_r = NonUniformConvolver::from_pre_fft(&data[1 + left_len..]);
                build_stereo_ready(conv_l, conv_r, self.base_mix)
            } else {
                let convolver = NonUniformConvolver::from_pre_fft(&data);
                build_mono_ready(convolver, self.base_mix)
            };
            self.start_from_ready(ready);
            self.update_shared_mix(0.0);
            return Ok(());
        }

        // Synthetic IR variants.
        let variant = IR_VARIANTS[self.ir_variant_idx as usize];
        if variant == "file" {
            // `ir: file` selected but no `ir_path` provided — passthrough.
            self.update_shared_mix(0.0);
            return Ok(());
        }

        let ready = if self.stereo {
            let (ir_l, ir_r) = generate_stereo_variant_ir(variant, self.sample_rate);
            let conv_l = NonUniformConvolver::new(&ir_l, BLOCK_SIZE, MAX_TIER_BLOCK_SIZE);
            let conv_r = NonUniformConvolver::new(&ir_r, BLOCK_SIZE, MAX_TIER_BLOCK_SIZE);
            build_stereo_ready(conv_l, conv_r, self.base_mix)
        } else {
            let ir = generate_variant_ir(variant, self.sample_rate);
            let convolver = NonUniformConvolver::new(&ir, BLOCK_SIZE, MAX_TIER_BLOCK_SIZE);
            build_mono_ready(convolver, self.base_mix)
        };
        self.start_from_ready(ready);
        self.update_shared_mix(0.0);

        let _ = module_name; // used for error context if needed in future
        Ok(())
    }

    /// Handle parameter updates on the audio thread (hot reload).
    ///
    /// Must be real-time safe: no file I/O, no thread spawn/join, no blocking.
    pub(super) fn update_validated_parameters(&mut self, p: &ParamView<'_>) {
        self.base_mix = p.get(params::mix);

        // ADR 0060: `ir_path` is structural; hot-reloading the file IR
        // triggers an instance rebuild (planner-side, ticket 0740) rather
        // than an audio-thread param update.
        let mut ir_changed = false;

        let ir: IrVariant = p.get(params::ir);
        let idx = ir as u8;
        if (idx as usize) < IR_VARIANTS.len() && idx != self.ir_variant_idx {
            self.ir_variant_idx = idx;
            ir_changed = true;
        }

        if ir_changed && self.ir_variant_idx != FILE_VARIANT_IDX {
            self.send_load_request(IrLoadRequest {
                stereo: self.stereo,
                variant_idx: self.ir_variant_idx,
                sample_rate: self.sample_rate,
                base_mix: self.base_mix,
                pre_fft_data: None,
            });
        }

        self.update_shared_mix(0.0);
    }

    /// Poll for completed async IR load and install if ready.
    pub(super) fn poll_loader(&mut self) {
        let expected_mono = !self.stereo;
        if let Ok(ready) = self.ir_loader.result_rx.pop() {
            let matches = matches!(
                (&ready, expected_mono),
                (ProcessorReady::Mono(_), true) | (ProcessorReady::Stereo(_), false)
            );
            if matches {
                self.install_from_ready(ready);
            } else {
                cleanup_processor_ready(ready);
            }
        }
    }
}
