//! SlotDeck integration tests.
//!
//! These tests exercise the full pipeline: OverlapBuffer ↔ ProcessorHandle,
//! including threaded round-trips, OLA/WOLA reconstruction, and FFT-based
//! spectral processing. Categories live under `slot_deck/`; unit tests for
//! individual pieces (config validation, ActiveWindows, WindowBuffer) live
//! alongside their implementations.

#[path = "slot_deck/mod.rs"]
mod cases;
