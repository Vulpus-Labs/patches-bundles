//! Processing-thread handle for the slot deck.

use super::filled_slot::FilledSlot;

/// Processing-thread handle corresponding to an `OverlapBuffer`.
///
/// Receives filled buffers from the audio thread for in-place processing,
/// then sends the same buffers back. The buffer must always be returned —
/// it cannot be dropped, because the audio thread needs it back for reuse.
///
/// `ProcessorHandle: Send`.
pub struct ProcessorHandle {
    /// Receive filled buffers from the audio thread.
    inbound_rx: rtrb::Consumer<FilledSlot>,
    /// Send processed buffers back to the audio thread.
    outbound_tx: rtrb::Producer<FilledSlot>,
}

// SAFETY: ProcessorHandle is explicitly Send — caller moves it to the processing thread.
// Both fields are rtrb types which are Send when T: Send, and FilledSlot is Send.
unsafe impl Send for ProcessorHandle {}

impl ProcessorHandle {
    pub(super) fn new(
        inbound_rx: rtrb::Consumer<FilledSlot>,
        outbound_tx: rtrb::Producer<FilledSlot>,
    ) -> Self {
        Self { inbound_rx, outbound_tx }
    }

    /// Pop the next filled buffer from the audio thread, if any.
    pub fn pop(&mut self) -> Option<FilledSlot> {
        self.inbound_rx.pop().ok()
    }

    /// Push a processed buffer back to the audio thread.
    ///
    /// Returns `Err` if the return ring buffer is full. In the circulating
    /// design, the caller should retry rather than dropping the buffer, since
    /// it must return to the audio thread's filling pool.
    pub fn push(&mut self, slot: FilledSlot) -> Result<(), FilledSlot> {
        match self.outbound_tx.push(slot) {
            Ok(()) => Ok(()),
            Err(rtrb::PushError::Full(slot)) => Err(slot),
        }
    }

    /// Run a processing loop until `shutdown` is set.
    ///
    /// Calls `process_fn` for each filled slot. The closure receives a mutable
    /// reference to the slot for in-place processing. After the closure returns,
    /// the buffer is pushed back to the audio thread (spinning if necessary,
    /// checking `shutdown` between attempts to avoid deadlock).
    ///
    /// When no slot is available the thread parks, waiting for the audio
    /// thread to call `unpark()` after pushing a new filled slot. A timeout
    /// of 1 ms guards against missed wakeups without burning CPU.
    pub fn run_until_shutdown(
        &mut self,
        shutdown: &std::sync::atomic::AtomicBool,
        mut process_fn: impl FnMut(&mut FilledSlot),
    ) {
        while !shutdown.load(std::sync::atomic::Ordering::Relaxed) {
            match self.pop() {
                Some(mut slot) => {
                    process_fn(&mut slot);
                    // Must push back — buffer must circulate.
                    loop {
                        match self.outbound_tx.push(slot) {
                            Ok(()) => break,
                            Err(rtrb::PushError::Full(s)) => {
                                slot = s;
                                if shutdown.load(std::sync::atomic::Ordering::Relaxed) {
                                    return;
                                }
                                std::thread::park_timeout(
                                    std::time::Duration::from_millis(1),
                                );
                            }
                        }
                    }
                }
                None => std::thread::park_timeout(std::time::Duration::from_millis(1)),
            }
        }
    }
}
