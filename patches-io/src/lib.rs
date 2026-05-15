//! Audio file I/O for Patches bundles.
//!
//! Reads AIFF and WAV files, returning per-channel samples at a caller-specified
//! sample rate, resampling via `patches-dsp`'s windowed-sinc kernel where needed.
//!
//! Originally lived in the main `patches` repo; moved here once the only
//! in-tree consumer was the convolution reverb's impulse-response loader.

use std::path::Path;

mod audio_data;
pub use audio_data::{AudioData, AudioIoError};

pub mod aiff;
pub mod wav_read;

pub use aiff::read_aiff;
pub use wav_read::read_wav;

/// Read any supported audio file, resampling to `target_rate`.
///
/// Format is detected by file extension: `.aiff`/`.aif` for AIFF, `.wav` for WAV.
pub fn read_audio(path: &Path, target_rate: f64) -> Result<AudioData, AudioIoError> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());

    let data = match ext.as_deref() {
        Some("aiff" | "aif") => aiff::read_aiff(path)?,
        Some("wav") => wav_read::read_wav(path)?,
        _ => return Err(AudioIoError::UnrecognisedFormat(path.into())),
    };

    if data.channels.is_empty() || data.channels[0].is_empty() {
        return Err(AudioIoError::NoSamples);
    }

    Ok(data.resample(target_rate))
}

/// Read any supported audio file, mix to mono, resample to `target_rate`.
pub fn read_mono(path: &Path, target_rate: f64) -> Result<Vec<f32>, AudioIoError> {
    let data = read_audio(path, target_rate)?;
    Ok(data.mix_to_mono())
}

/// Read any supported audio file, extract stereo pair, resample to `target_rate`.
///
/// Mono files are duplicated to both channels. Files with more than 2 channels
/// use the first two.
pub fn read_stereo(path: &Path, target_rate: f64) -> Result<(Vec<f32>, Vec<f32>), AudioIoError> {
    let data = read_audio(path, target_rate)?;
    Ok(data.to_stereo())
}
