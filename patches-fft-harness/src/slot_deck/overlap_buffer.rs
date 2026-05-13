//! Audio-thread overlap-add buffer with circulating buffer pool.
//!
//! Buffers cycle through three states:
//! 1. **Filling** — owned by the audio thread, receiving samples one at a time
//! 2. **Processing** — transferred to the processing thread via ring buffer
//! 3. **Draining** — returned from processing thread, being overlap-added
//!
//! After draining, buffers return to the filling pool. A single pool of buffers
//! circulates; no separate input/output pools are needed.
//!
//! `OverlapBuffer` is `!Send` — it must remain on the audio thread.

use super::active_windows::ActiveWindows;
use super::config::SlotDeckConfig;
use super::filled_slot::FilledSlot;
use super::processor_handle::ProcessorHandle;

/// Audio-thread overlap-add buffer with circulating buffer pool.
pub struct OverlapBuffer {
    // !Send: audio thread only.
    _not_send: std::marker::PhantomData<*const ()>,
    hop_size_mask: u64,
    write_head: u64,
    read_head: u64,

    /// Free buffers ready to be claimed for new filling windows.
    filling_pool: Vec<Box<[f32]>>,
    /// Currently filling windows (at most `overlap_factor` simultaneously active).
    filling: ActiveWindows,

    /// Send filled buffers to the processing thread.
    outbound_tx: rtrb::Producer<FilledSlot>,
    /// Receive processed buffers from the processing thread.
    inbound_rx: rtrb::Consumer<FilledSlot>,

    /// Currently draining windows (overlap-add contributors).
    draining: ActiveWindows,

    /// Thread handle for the processor thread. When set, the audio thread
    /// calls `unpark()` after pushing a filled slot so the processor wakes
    /// promptly instead of spinning.
    processor_thread: Option<std::thread::Thread>,
}

impl OverlapBuffer {
    /// Create an `OverlapBuffer` and connect it to a processor thread.
    ///
    /// The `spawn` closure receives a `ProcessorHandle` and must return
    /// a `JoinHandle` for the spawned processor. `OverlapBuffer` extracts
    /// the `Thread` for wakeup; the `JoinHandle` is returned to the caller
    /// for lifecycle management.
    ///
    /// All buffer allocation happens here. `write` and `read` never allocate.
    pub fn new(
        config: SlotDeckConfig,
        spawn: impl FnOnce(ProcessorHandle) -> std::thread::JoinHandle<()>,
    ) -> (OverlapBuffer, std::thread::JoinHandle<()>) {
        let (mut buf, handle) = Self::new_unthreaded(config);
        let join_handle = spawn(handle);
        buf.processor_thread = Some(join_handle.thread().clone());
        (buf, join_handle)
    }

    /// Create an `OverlapBuffer` without a processor thread.
    ///
    /// Returns both the buffer and a `ProcessorHandle` for the caller to
    /// drive synchronously (e.g. in tests). The buffer will not attempt to
    /// wake a processor thread on dispatch.
    pub fn new_unthreaded(config: SlotDeckConfig) -> (OverlapBuffer, ProcessorHandle) {
        let pool_size = config.pool_size();
        let window_size = config.window_size;
        let overlap_factor = config.overlap_factor;
        let total_latency = config.total_latency();
        let hop_size = config.hop_size();

        let filling_pool: Vec<Box<[f32]>> = (0..pool_size)
            .map(|_| vec![0.0f32; window_size].into_boxed_slice())
            .collect();

        let (outbound_tx, outbound_rx) = rtrb::RingBuffer::new(pool_size);
        let (inbound_tx, inbound_rx) = rtrb::RingBuffer::new(pool_size);

        let overlap_buf = OverlapBuffer {
            _not_send: std::marker::PhantomData,
            hop_size_mask: hop_size.wrapping_sub(1) as u64,
            // Pre-advance write_head by total_latency so read_head can start at 0
            // and the first `total_latency` reads return silence naturally.
            write_head: total_latency as u64,
            read_head: 0,
            filling_pool,
            filling: ActiveWindows::new(overlap_factor, window_size),
            outbound_tx,
            inbound_rx,
            draining: ActiveWindows::new(pool_size, window_size),
            processor_thread: None,
        };

        let handle = ProcessorHandle::new(outbound_rx, inbound_tx);
        (overlap_buf, handle)
    }

    /// Write one input sample. Called once per audio callback sample.
    ///
    /// All failure paths are silent degradation — never blocks, never panics,
    /// never allocates.
    pub fn write(&mut self, sample: f32) {
        // At each hop boundary, open a new filling window.
        if self.write_head & self.hop_size_mask == 0 {
            if let Some(buf) = self.filling_pool.pop() {
                if let Some(rejected) = self.filling.insert(self.write_head, buf) {
                    self.filling_pool.push(rejected);
                }
            }
            // If no free buffer is available, the window is silently skipped.
        }

        // Write sample into every active filling window, dispatch if one completes.
        if let Some(slot) = self.filling.write_sample(self.write_head, sample) {
            if let Err(rtrb::PushError::Full(slot)) = self.outbound_tx.push(slot) {
                // Ring buffer full: recycle buffer immediately, skipping
                // the processing/draining circuit.
                self.filling_pool.push(slot.data);
            } else if let Some(ref thread) = self.processor_thread {
                thread.unpark();
            }
        }

        self.write_head = self.write_head.wrapping_add(1);
    }

    /// Read one output sample. Called once per audio callback sample.
    ///
    /// Returns the plain overlap-add sum of all active draining windows.
    /// For WOLA, normalisation is baked into the synthesis window via
    /// [`WindowBuffer::normalised_wola`], so no per-sample correction is needed here.
    ///
    /// Returns 0.0 until result windows arrive from the processing thread.
    pub fn read(&mut self) -> f32 {
        let read_head = self.read_head;
        let window_size = self.draining.window_size();

        // Drain newly-arrived processed buffers into draining windows.
        while let Ok(slot) = self.inbound_rx.pop() {
            // Too late — all samples expired: recycle immediately.
            if read_head >= slot.start + window_size as u64 {
                self.filling_pool.push(slot.data);
                continue;
            }
            if let Some(rejected) = self.draining.insert(slot.start, slot.data) {
                // No room: recycle to filling pool.
                self.filling_pool.push(rejected);
            }
        }

        // Overlap-add contributions and expire finished windows.
        let (sum, expired) = self.draining.read_sum(read_head);
        if let Some(buf) = expired {
            // Recycle to filling pool — completes the circuit.
            self.filling_pool.push(buf);
        }

        self.read_head = read_head.wrapping_add(1);
        sum
    }
}
