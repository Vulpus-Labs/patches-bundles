//! Per-channel convolution processor: an `OverlapBuffer` paired with a
//! dedicated processing thread that runs the convolver in WOLA blocks.
//!
//! `ProcessorKit` is constructed on the control thread in `Module::prepare`
//! (allocation + thread spawn allowed) and then moved into the audio thread
//! along with the module. There is no audio-thread construction or
//! teardown path — IR changes are structural (ADR 0060) and rebuild the
//! whole module via the planner.

use std::sync::Arc;

use patches_fft_harness::partitioned_convolution::NonUniformConvolver;
use patches_fft_harness::slot_deck::{OverlapBuffer, SlotDeckConfig};

use super::params::{SharedParams, BLOCK_SIZE, PROCESSING_BUDGET};

/// Processing-thread body: runs the convolver in WOLA blocks until
/// `shared.shutdown` is set.
fn run_processor(
    mut handle: patches_fft_harness::slot_deck::ProcessorHandle,
    shared: Arc<SharedParams>,
    mut convolver: NonUniformConvolver,
    block_size: usize,
) {
    let mut dry = vec![0.0f32; block_size];
    let mut conv_output = vec![0.0f32; block_size];

    handle.run_until_shutdown(&shared.shutdown, |slot| {
        let mix = shared.mix.load();

        dry.copy_from_slice(&slot.data);
        convolver.process_block(&dry, &mut conv_output);

        for i in 0..block_size {
            slot.data[i] = dry[i] * (1.0 - mix) + conv_output[i] * mix;
        }
    });
}

/// A single-channel convolution processor: overlap buffer (audio-thread
/// I/O), the shared parameter block, and the worker thread handle.
pub(super) struct ProcessorKit {
    pub(super) overlap_buffer: OverlapBuffer,
    pub(super) shared: Arc<SharedParams>,
    pub(super) thread: std::thread::JoinHandle<()>,
}

/// Build a single-channel processor from a pre-built convolver and a
/// pre-allocated shared parameter block. Spawns the worker thread.
pub(super) fn build_processor(
    convolver: NonUniformConvolver,
    shared: Arc<SharedParams>,
    name: &str,
) -> ProcessorKit {
    let config = SlotDeckConfig::new(BLOCK_SIZE, 1, PROCESSING_BUDGET)
        .expect("convolution_reverb: invalid SlotDeckConfig");
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
