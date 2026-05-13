//! Background IR loader thread and request/response plumbing for convolution reverb.

use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::Arc;

use patches_fft_harness::partitioned_convolution::NonUniformConvolver;
use patches_fft_harness::slot_deck::{OverlapBuffer, SlotDeckConfig};

use super::params::{
    SharedParams, BLOCK_SIZE, IR_VARIANTS, MAX_TIER_BLOCK_SIZE,
    PROCESSING_BUDGET, generate_stereo_variant_ir, generate_variant_ir,
};

// ---------------------------------------------------------------------------
// Processing thread
// ---------------------------------------------------------------------------

pub(super) fn run_processor(
    mut handle: patches_fft_harness::slot_deck::ProcessorHandle,
    shared: Arc<SharedParams>,
    mut convolver: NonUniformConvolver,
    block_size: usize,
) {
    // Scratch buffers (allocated once).
    let mut dry = vec![0.0f32; block_size];
    let mut conv_output = vec![0.0f32; block_size];

    handle.run_until_shutdown(&shared.shutdown, |slot| {
        let mix = shared.mix.load();

        // Save dry signal before in-place overwrite.
        dry.copy_from_slice(&slot.data);

        // Run the convolver.
        convolver.process_block(&dry, &mut conv_output);

        // Dry/wet mix — write result back into the circulating buffer.
        for i in 0..block_size {
            slot.data[i] = dry[i] * (1.0 - mix) + conv_output[i] * mix;
        }
    });
}

// ---------------------------------------------------------------------------
// Processor kit: the result of building a convolution processor
// ---------------------------------------------------------------------------

/// A single-channel convolution processor (OverlapBuffer + thread + shared params).
pub(super) struct ProcessorKit {
    pub(super) overlap_buffer: OverlapBuffer,
    pub(super) shared: Arc<SharedParams>,
    pub(super) thread: std::thread::JoinHandle<()>,
}

/// Build a single-channel processor from a pre-built convolver.
pub(super) fn build_processor(convolver: NonUniformConvolver, base_mix: f32, name: &str) -> ProcessorKit {
    let config = SlotDeckConfig::new(BLOCK_SIZE, 1, PROCESSING_BUDGET)
        .expect("convolution_reverb: invalid SlotDeckConfig");
    let shared = Arc::new(SharedParams::new());
    shared.mix.store(base_mix);
    let shared_clone = Arc::clone(&shared);
    let thread_name = name.to_owned();
    let (overlap_buffer, thread) = OverlapBuffer::new(config, |handle| {
        std::thread::Builder::new()
            .name(thread_name)
            .spawn(move || run_processor(handle, shared_clone, convolver, BLOCK_SIZE))
            .expect("convolution_reverb: failed to spawn processing thread")
    });
    ProcessorKit { overlap_buffer, shared, thread }
}

// ---------------------------------------------------------------------------
// Async IR loading infrastructure
// ---------------------------------------------------------------------------

/// Request to resolve an IR and build a convolution processor.
pub(super) struct IrLoadRequest {
    pub(super) stereo: bool,
    pub(super) variant_idx: u8,
    pub(super) sample_rate: f32,
    pub(super) base_mix: f32,
    /// Pre-computed spectral data decoded from the `ir_path` structural
    /// param at `prepare` time. When `Some`, the loader skips synthesis
    /// and builds the convolver directly from this data via
    /// `NonUniformConvolver::from_pre_fft`. The `Arc` is moved off the
    /// audio thread into the loader, so its deallocation never happens
    /// on the audio thread.
    pub(super) pre_fft_data: Option<Arc<[f32]>>,
}

/// A ready-to-use mono convolution processor.
pub(super) struct MonoProcessorReady {
    pub(super) kit: ProcessorKit,
}

/// A ready-to-use stereo convolution processor.
pub(super) struct StereoProcessorReady {
    pub(super) kit_l: ProcessorKit,
    pub(super) kit_r: ProcessorKit,
    pub(super) shared: Arc<SharedParams>,
}

/// Result of an async IR load.
pub(super) enum ProcessorReady {
    Mono(MonoProcessorReady),
    Stereo(Box<StereoProcessorReady>),
}

// SAFETY: OverlapBuffer is !Send as a lint against casual cross-thread use.
// Single ownership transfer from loader thread to audio thread at
// periodic_update is safe (same reasoning as the module's own Send impl).
unsafe impl Send for ProcessorReady {}

/// Old processor handles to shut down and deallocate off the audio thread.
///
/// The `OverlapBuffer` fields are not read — they exist so their drop runs on
/// the loader thread rather than the audio thread.
#[allow(dead_code)]
pub(super) struct ProcessorTeardown {
    pub(super) shared: Arc<SharedParams>,
    pub(super) threads: Vec<std::thread::JoinHandle<()>>,
    pub(super) overlap_buffers: Vec<OverlapBuffer>,
}

// SAFETY: Same reasoning as ProcessorReady.
unsafe impl Send for ProcessorTeardown {}

impl ProcessorTeardown {
    /// Signal the processor thread(s) to shut down and join them.
    pub(super) fn shutdown_and_join(self) {
        self.shared.shutdown.store(true, Relaxed);
        for thread in self.threads {
            let _ = thread.join();
        }
    }
}

/// Shut down and clean up an unclaimed processor result.
pub(super) fn cleanup_processor_ready(ready: ProcessorReady) {
    match ready {
        ProcessorReady::Mono(MonoProcessorReady { kit }) => {
            kit.shared.shutdown.store(true, Relaxed);
            let _ = kit.thread.join();
        }
        ProcessorReady::Stereo(stereo) => {
            stereo.shared.shutdown.store(true, Relaxed);
            let _ = stereo.kit_l.thread.join();
            let _ = stereo.kit_r.thread.join();
        }
    }
}

/// Build a mono `ProcessorReady` from a convolver.
pub(super) fn build_mono_ready(convolver: NonUniformConvolver, base_mix: f32) -> ProcessorReady {
    let kit = build_processor(convolver, base_mix, "patches-conv-reverb");
    ProcessorReady::Mono(MonoProcessorReady { kit })
}

/// Build a stereo `ProcessorReady` from two convolvers.
pub(super) fn build_stereo_ready(
    conv_l: NonUniformConvolver,
    conv_r: NonUniformConvolver,
    base_mix: f32,
) -> ProcessorReady {
    let shared = Arc::new(SharedParams::new());
    shared.mix.store(base_mix);

    let config_l = SlotDeckConfig::new(BLOCK_SIZE, 1, PROCESSING_BUDGET)
        .expect("stereo_conv_reverb: invalid SlotDeckConfig");
    let shared_l = Arc::clone(&shared);
    let (overlap_l, thread_l) = OverlapBuffer::new(config_l, |handle| {
        std::thread::Builder::new()
            .name("patches-conv-reverb-l".into())
            .spawn(move || run_processor(handle, shared_l, conv_l, BLOCK_SIZE))
            .expect("stereo_conv_reverb: failed to spawn L thread")
    });

    let config_r = SlotDeckConfig::new(BLOCK_SIZE, 1, PROCESSING_BUDGET)
        .expect("stereo_conv_reverb: invalid SlotDeckConfig");
    let shared_r = Arc::clone(&shared);
    let (overlap_r, thread_r) = OverlapBuffer::new(config_r, |handle| {
        std::thread::Builder::new()
            .name("patches-conv-reverb-r".into())
            .spawn(move || run_processor(handle, shared_r, conv_r, BLOCK_SIZE))
            .expect("stereo_conv_reverb: failed to spawn R thread")
    });

    ProcessorReady::Stereo(Box::new(StereoProcessorReady {
        kit_l: ProcessorKit { overlap_buffer: overlap_l, shared: Arc::clone(&shared), thread: thread_l },
        kit_r: ProcessorKit { overlap_buffer: overlap_r, shared: Arc::clone(&shared), thread: thread_r },
        shared,
    }))
}

// ---------------------------------------------------------------------------
// IR loader thread
// ---------------------------------------------------------------------------

/// Per-module IR loader service.
///
/// Runs a background thread that generates synthetic IRs, builds convolvers,
/// and spawns processing threads — all off the audio thread. Results are
/// delivered via a lock-free ring buffer polled in
/// [`patches_sdk::modules::module::PeriodicUpdate::periodic_update`].
pub(super) struct IrLoader {
    pub(super) request_tx: rtrb::Producer<IrLoadRequest>,
    pub(super) teardown_tx: rtrb::Producer<ProcessorTeardown>,
    pub(super) result_rx: rtrb::Consumer<ProcessorReady>,
    pub(super) thread: std::thread::Thread,
    pub(super) handle: Option<std::thread::JoinHandle<()>>,
    pub(super) shutdown: Arc<AtomicBool>,
}

impl IrLoader {
    pub(super) fn new() -> Self {
        let (request_tx, request_rx) = rtrb::RingBuffer::new(2);
        let (teardown_tx, teardown_rx) = rtrb::RingBuffer::new(4);
        let (result_tx, result_rx) = rtrb::RingBuffer::new(2);
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_clone = Arc::clone(&shutdown);

        let handle = std::thread::Builder::new()
            .name("patches-ir-loader".into())
            .spawn(move || ir_loader_main(shutdown_clone, request_rx, teardown_rx, result_tx))
            .expect("convolution_reverb: failed to spawn IR loader thread");

        let thread = handle.thread().clone();

        Self {
            request_tx,
            teardown_tx,
            result_rx,
            thread,
            handle: Some(handle),
            shutdown,
        }
    }

    /// Wake the loader thread (e.g. after pushing a request or teardown).
    pub(super) fn wake(&self) {
        self.thread.unpark();
    }
}

impl Drop for IrLoader {
    fn drop(&mut self) {
        self.shutdown.store(true, Relaxed);
        self.thread.unpark();
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
        // Clean up any unclaimed processor results.
        while let Ok(ready) = self.result_rx.pop() {
            cleanup_processor_ready(ready);
        }
    }
}

fn ir_loader_main(
    shutdown: Arc<AtomicBool>,
    mut request_rx: rtrb::Consumer<IrLoadRequest>,
    mut teardown_rx: rtrb::Consumer<ProcessorTeardown>,
    mut result_tx: rtrb::Producer<ProcessorReady>,
) {
    loop {
        // Drain teardown requests first.
        while let Ok(td) = teardown_rx.pop() {
            td.shutdown_and_join();
        }

        match request_rx.pop() {
            Ok(req) => {
                let result = if let Some(pre_fft) = req.pre_fft_data {
                    // Pre-computed spectral data from a FloatBuffer parameter.
                    if req.stereo {
                        let left_len = pre_fft[0] as usize;
                        let conv_l = NonUniformConvolver::from_pre_fft(&pre_fft[1..1 + left_len]);
                        let conv_r = NonUniformConvolver::from_pre_fft(&pre_fft[1 + left_len..]);
                        build_stereo_ready(conv_l, conv_r, req.base_mix)
                    } else {
                        let convolver = NonUniformConvolver::from_pre_fft(&pre_fft);
                        build_mono_ready(convolver, req.base_mix)
                    }
                } else {
                    // Synthetic IR variant — generate noise IR.
                    let variant = IR_VARIANTS[req.variant_idx as usize];
                    if req.stereo {
                        let (ir_l, ir_r) = generate_stereo_variant_ir(variant, req.sample_rate);
                        let conv_l = NonUniformConvolver::new(&ir_l, BLOCK_SIZE, MAX_TIER_BLOCK_SIZE);
                        let conv_r = NonUniformConvolver::new(&ir_r, BLOCK_SIZE, MAX_TIER_BLOCK_SIZE);
                        build_stereo_ready(conv_l, conv_r, req.base_mix)
                    } else {
                        let ir = generate_variant_ir(variant, req.sample_rate);
                        let convolver = NonUniformConvolver::new(&ir, BLOCK_SIZE, MAX_TIER_BLOCK_SIZE);
                        build_mono_ready(convolver, req.base_mix)
                    }
                };
                let _ = result_tx.push(result);
            }
            Err(_) => {
                if shutdown.load(Relaxed) {
                    // Final teardown drain before exiting.
                    while let Ok(td) = teardown_rx.pop() {
                        td.shutdown_and_join();
                    }
                    break;
                }
                std::thread::park();
            }
        }
    }
}
