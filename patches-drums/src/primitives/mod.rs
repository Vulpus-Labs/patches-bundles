//! Drum-voice primitives: envelopes, sweeps, saturation, metallic tone,
//! and burst generation. Each primitive lives in its own submodule and is
//! independently usable by drum voices in `patches-modules`.

mod burst;
mod envelope;
mod metallic;
mod saturate;
mod sweep;

pub use burst::BurstGenerator;
pub use envelope::DecayEnvelope;
pub use metallic::MetallicTone;
pub use saturate::saturate;
pub use sweep::PitchSweep;
