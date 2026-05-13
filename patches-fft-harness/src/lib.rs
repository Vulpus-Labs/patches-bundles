//! `patches-fft-harness` — overlap-add / overlap-save FFT scaffolding
//! shared by FFT-based module bundles (currently
//! `patches-fft-bundle`: pitch_shift + convolution_reverb).
//!
//! Holds the buffer-shuffling, slot-deck, partitioned convolver, and
//! spectral pitch shifter that previously lived in `patches-dsp`.
//! `RealPackedFft` itself stays in `patches-dsp`; this crate depends
//! on it.
//!
//! Built as an rlib only: external module authors who want OLA/WOLA
//! infrastructure can git-dep this crate without pulling the stdlib
//! bundle.

pub mod slot_deck;
pub mod spectral_pitch_shift;
pub mod partitioned_convolution;

mod window_buffer;
pub use window_buffer::WindowBuffer;

pub use spectral_pitch_shift::SpectralPitchShifter;
pub use partitioned_convolution::{PartitionedConvolver, IrPartitions, NonUniformConvolver};

#[cfg(test)]
mod test_support;
