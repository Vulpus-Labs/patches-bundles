//! Core types for audio file I/O.

use std::fmt;
use std::path::Path;

/// Format-agnostic decoded audio data with per-channel samples and sample rate.
#[derive(Debug, Clone)]
pub struct AudioData {
    /// Per-channel sample data, each channel normalised to [-1, 1].
    pub channels: Vec<Vec<f32>>,
    /// Sample rate in Hz.
    pub sample_rate: f64,
}

impl AudioData {
    /// Number of sample frames (length of the longest channel).
    pub fn num_frames(&self) -> usize {
        self.channels.first().map_or(0, |ch| ch.len())
    }

    /// Number of channels.
    pub fn num_channels(&self) -> usize {
        self.channels.len()
    }

    /// Mix all channels down to mono by averaging.
    pub fn mix_to_mono(&self) -> Vec<f32> {
        if self.channels.is_empty() {
            return Vec::new();
        }
        if self.channels.len() == 1 {
            return self.channels[0].clone();
        }
        let len = self.num_frames();
        let n_ch = self.channels.len() as f32;
        (0..len)
            .map(|i| self.channels.iter().map(|ch| ch[i]).sum::<f32>() / n_ch)
            .collect()
    }

    /// Extract left and right channels. Mono input is duplicated to both.
    /// Files with more than 2 channels use the first two.
    pub fn to_stereo(&self) -> (Vec<f32>, Vec<f32>) {
        match self.channels.len() {
            0 => (Vec::new(), Vec::new()),
            1 => (self.channels[0].clone(), self.channels[0].clone()),
            _ => (self.channels[0].clone(), self.channels[1].clone()),
        }
    }

    /// Resample all channels to `target_rate` using windowed sinc interpolation.
    /// Returns `self` unchanged if the rates already match (within 1 Hz).
    pub fn resample(self, target_rate: f64) -> AudioData {
        if (self.sample_rate - target_rate).abs() <= 1.0 {
            return self;
        }
        let from = self.sample_rate;
        let channels = self
            .channels
            .into_iter()
            .map(|ch| patches_dsp::resample(&ch, from, target_rate))
            .collect();
        AudioData {
            channels,
            sample_rate: target_rate,
        }
    }
}

/// Errors that can occur during audio file I/O.
#[derive(Debug)]
pub enum AudioIoError {
    /// Underlying I/O error.
    Io(std::io::Error),
    /// File extension not recognised as a supported format.
    UnrecognisedFormat(Box<Path>),
    /// File uses an encoding we don't support (e.g. compressed AIFF, non-PCM WAV).
    UnsupportedEncoding(String),
    /// File is structurally invalid (missing required chunks, truncated, etc.).
    MalformedFile(String),
    /// File was parsed successfully but contains no sample data.
    NoSamples,
}

impl fmt::Display for AudioIoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AudioIoError::Io(e) => write!(f, "I/O error: {e}"),
            AudioIoError::UnrecognisedFormat(p) => {
                write!(f, "unrecognised audio format: {}", p.display())
            }
            AudioIoError::UnsupportedEncoding(msg) => write!(f, "unsupported encoding: {msg}"),
            AudioIoError::MalformedFile(msg) => write!(f, "malformed file: {msg}"),
            AudioIoError::NoSamples => write!(f, "file contains no samples"),
        }
    }
}

impl std::error::Error for AudioIoError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            AudioIoError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for AudioIoError {
    fn from(e: std::io::Error) -> Self {
        AudioIoError::Io(e)
    }
}
