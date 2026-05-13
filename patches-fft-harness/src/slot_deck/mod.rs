//! Windowed cross-thread transfer buffer (`SlotDeck`).
//!
//! See ADR 0023 for design rationale. This module provides the transfer
//! primitive for algorithms that need to process audio in windows on a
//! separate thread (e.g. convolution reverb, FFT-based effects).

mod config;
pub use config::SlotDeckConfig;

mod filled_slot;
pub use filled_slot::FilledSlot;

pub(crate) mod active_windows;

mod overlap_buffer;
pub use overlap_buffer::OverlapBuffer;

mod processor_handle;
pub use processor_handle::ProcessorHandle;
