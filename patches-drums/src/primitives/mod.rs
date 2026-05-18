//! Drum-voice primitives: envelopes, sweeps, saturation, metallic tone,
//! and burst generation. Each primitive lives in its own submodule and is
//! independently usable by drum voices in `patches-modules`.

mod bridged_t;
mod burst;
mod envelope;
mod excitation;
mod metallic;
mod saturate;
mod struck_resonator_voice;
mod sweep;
mod tpt_svf;

pub use bridged_t::BridgedT;
pub use burst::BurstGenerator;
pub use envelope::DecayEnvelope;
pub use excitation::{Excitation, PulseShape};
pub use metallic::MetallicTone;
pub use saturate::saturate;
pub use struck_resonator_voice::StruckResonatorVoice;
pub use sweep::PitchSweep;
