//! WAV file reader.
//!
//! Reads uncompressed PCM WAV files (16-bit and 24-bit) and returns per-channel
//! f32 samples normalised to [-1, 1].

use std::path::Path;

use crate::audio_data::{AudioData, AudioIoError};

/// Read a WAV file and return per-channel audio data with sample rate.
pub fn read_wav(path: &Path) -> Result<AudioData, AudioIoError> {
    let data = std::fs::read(path)?;
    parse_wav(&data)
}

// ---------------------------------------------------------------------------
// Internal parsing
// ---------------------------------------------------------------------------

/// Parse WAV data from a byte slice.
pub(crate) fn parse_wav(data: &[u8]) -> Result<AudioData, AudioIoError> {
    if data.len() < 12 || &data[0..4] != b"RIFF" || &data[8..12] != b"WAVE" {
        return Err(AudioIoError::MalformedFile("not a valid WAV file".into()));
    }

    let mut num_channels: u16 = 0;
    let mut sample_rate: u32 = 0;
    let mut bits_per_sample: u16 = 0;
    let mut format_tag: u16 = 0;
    let mut fmt_found = false;
    let mut audio_data: Option<&[u8]> = None;

    // Walk RIFF chunks.
    let mut pos = 12;
    while pos + 8 <= data.len() {
        let chunk_id = &data[pos..pos + 4];
        let chunk_size = u32::from_le_bytes([
            data[pos + 4],
            data[pos + 5],
            data[pos + 6],
            data[pos + 7],
        ]) as usize;
        let chunk_body = pos + 8;

        // Guard against truncated chunks whose declared size exceeds the
        // remaining bytes — otherwise indexing below panics on crafted files.
        if chunk_body + chunk_size > data.len() {
            return Err(AudioIoError::MalformedFile(format!(
                "WAV chunk '{}' declares size {} but only {} bytes remain",
                String::from_utf8_lossy(chunk_id),
                chunk_size,
                data.len().saturating_sub(chunk_body),
            )));
        }

        if chunk_id == b"fmt " && chunk_size >= 16 {
            format_tag = u16::from_le_bytes([data[chunk_body], data[chunk_body + 1]]);
            num_channels = u16::from_le_bytes([data[chunk_body + 2], data[chunk_body + 3]]);
            sample_rate = u32::from_le_bytes([
                data[chunk_body + 4],
                data[chunk_body + 5],
                data[chunk_body + 6],
                data[chunk_body + 7],
            ]);
            // bytes 8..11: byte rate (skip)
            // bytes 12..13: block align (skip)
            bits_per_sample =
                u16::from_le_bytes([data[chunk_body + 14], data[chunk_body + 15]]);
            fmt_found = true;
        } else if chunk_id == b"data" {
            let end = (chunk_body + chunk_size).min(data.len());
            audio_data = Some(&data[chunk_body..end]);
        }

        // Chunks are padded to even size.
        let padded = if chunk_size % 2 == 1 {
            chunk_size + 1
        } else {
            chunk_size
        };
        pos = chunk_body + padded;
    }

    if !fmt_found {
        return Err(AudioIoError::MalformedFile(
            "WAV file missing fmt chunk".into(),
        ));
    }

    // Only support PCM (1).
    if format_tag != 1 {
        return Err(AudioIoError::UnsupportedEncoding(format!(
            "WAV format tag {format_tag} (only PCM/1 supported)"
        )));
    }

    let raw = audio_data.ok_or_else(|| {
        AudioIoError::MalformedFile("WAV file missing data chunk".into())
    })?;

    let bytes_per_sample: usize = match bits_per_sample {
        8 => 1,
        16 => 2,
        24 => 3,
        _ => {
            return Err(AudioIoError::UnsupportedEncoding(format!(
                "unsupported WAV bit depth: {bits_per_sample}"
            )))
        }
    };

    let n_ch = num_channels as usize;
    let frame_size = bytes_per_sample * n_ch;
    let total_frames = raw.len().checked_div(frame_size).unwrap_or(0);

    let mut channels: Vec<Vec<f32>> =
        (0..n_ch).map(|_| Vec::with_capacity(total_frames)).collect();

    for frame in 0..total_frames {
        let frame_start = frame * frame_size;
        for (ch, channel) in channels.iter_mut().enumerate() {
            let sample_start = frame_start + ch * bytes_per_sample;
            let sample_bytes = &raw[sample_start..sample_start + bytes_per_sample];
            let sample_f = decode_sample_le(sample_bytes, bits_per_sample);
            channel.push(sample_f as f32);
        }
    }

    Ok(AudioData {
        channels,
        sample_rate: sample_rate as f64,
    })
}

/// Decode a little-endian signed integer sample to f64 in [-1, 1].
fn decode_sample_le(bytes: &[u8], bit_depth: u16) -> f64 {
    match bit_depth {
        8 => {
            // 8-bit WAV samples are unsigned (0–255), centre at 128.
            let u = bytes[0] as i16 - 128;
            u as f64 / 128.0
        }
        16 => {
            let s = i16::from_le_bytes([bytes[0], bytes[1]]);
            s as f64 / 32768.0
        }
        24 => {
            let raw = (bytes[0] as i32) | ((bytes[1] as i32) << 8) | ((bytes[2] as i32) << 16);
            let signed = if raw & 0x80_0000 != 0 {
                raw | !0xFF_FFFF
            } else {
                raw
            };
            signed as f64 / 8_388_608.0
        }
        _ => 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal valid WAV in memory (PCM).
    fn make_wav(channels: u16, sample_rate: u32, bit_depth: u16, samples: &[u8]) -> Vec<u8> {
        let mut buf = Vec::new();
        let bytes_per_sample = bit_depth / 8;
        let block_align = channels * bytes_per_sample;
        let byte_rate = sample_rate * u32::from(block_align);

        buf.extend_from_slice(b"RIFF");
        buf.extend_from_slice(&[0; 4]); // size placeholder
        buf.extend_from_slice(b"WAVE");

        // fmt chunk
        buf.extend_from_slice(b"fmt ");
        buf.extend_from_slice(&16u32.to_le_bytes());
        buf.extend_from_slice(&1u16.to_le_bytes()); // PCM
        buf.extend_from_slice(&channels.to_le_bytes());
        buf.extend_from_slice(&sample_rate.to_le_bytes());
        buf.extend_from_slice(&byte_rate.to_le_bytes());
        buf.extend_from_slice(&block_align.to_le_bytes());
        buf.extend_from_slice(&bit_depth.to_le_bytes());

        // data chunk
        buf.extend_from_slice(b"data");
        buf.extend_from_slice(&(samples.len() as u32).to_le_bytes());
        buf.extend_from_slice(samples);
        if samples.len() % 2 == 1 {
            buf.push(0);
        }

        // Fill in RIFF size
        let riff_size = (buf.len() - 8) as u32;
        buf[4..8].copy_from_slice(&riff_size.to_le_bytes());

        buf
    }

    #[test]
    fn mono_16bit() {
        let half_pos = 16384i16;
        let half_neg = -16384i16;
        let mut samples = Vec::new();
        samples.extend_from_slice(&half_pos.to_le_bytes());
        samples.extend_from_slice(&half_neg.to_le_bytes());

        let wav = make_wav(1, 44100, 16, &samples);
        let parsed = parse_wav(&wav).unwrap();
        assert_eq!(parsed.channels.len(), 1);
        assert_eq!(parsed.channels[0].len(), 2);
        assert!((parsed.channels[0][0] - 0.5).abs() < 0.001);
        assert!((parsed.channels[0][1] + 0.5).abs() < 0.001);
        assert!((parsed.sample_rate - 44100.0).abs() < 0.01);
    }

    #[test]
    fn stereo_16bit() {
        let mut samples = Vec::new();
        // Frame 0: L = +0.5, R = -0.5
        samples.extend_from_slice(&16384i16.to_le_bytes());
        samples.extend_from_slice(&(-16384i16).to_le_bytes());

        let wav = make_wav(2, 48000, 16, &samples);
        let parsed = parse_wav(&wav).unwrap();
        assert_eq!(parsed.channels.len(), 2);
        assert_eq!(parsed.channels[0].len(), 1);
        assert!((parsed.channels[0][0] - 0.5).abs() < 0.001);
        assert!((parsed.channels[1][0] + 0.5).abs() < 0.001);
        assert!((parsed.sample_rate - 48000.0).abs() < 0.01);
    }

    #[test]
    fn mono_24bit() {
        // 24-bit LE: 0x004000 = 4194304 → 0.5
        let samples = vec![0x00, 0x00, 0x40];
        let wav = make_wav(1, 44100, 24, &samples);
        let parsed = parse_wav(&wav).unwrap();
        assert!((parsed.channels[0][0] - 0.5).abs() < 0.001);
    }

    #[test]
    fn negative_24bit() {
        // 24-bit LE: 0xC00000 sign-extended → -0.5
        let samples = vec![0x00, 0x00, 0xC0];
        let wav = make_wav(1, 44100, 24, &samples);
        let parsed = parse_wav(&wav).unwrap();
        assert!((parsed.channels[0][0] + 0.5).abs() < 0.001);
    }

    #[test]
    fn sample_rate_preserved() {
        let samples = vec![0x00, 0x00]; // one frame of silence
        let wav = make_wav(1, 96000, 16, &samples);
        let parsed = parse_wav(&wav).unwrap();
        assert!((parsed.sample_rate - 96000.0).abs() < 0.01);
    }

    #[test]
    fn not_wav() {
        assert!(parse_wav(b"NOT_A_WAV_FILE!!").is_err());
    }

    #[test]
    fn missing_data_chunk() {
        let mut buf = Vec::new();
        buf.extend_from_slice(b"RIFF");
        buf.extend_from_slice(&100u32.to_le_bytes());
        buf.extend_from_slice(b"WAVE");
        buf.extend_from_slice(b"fmt ");
        buf.extend_from_slice(&16u32.to_le_bytes());
        buf.extend_from_slice(&1u16.to_le_bytes()); // PCM
        buf.extend_from_slice(&1u16.to_le_bytes()); // mono
        buf.extend_from_slice(&44100u32.to_le_bytes());
        buf.extend_from_slice(&88200u32.to_le_bytes()); // byte rate
        buf.extend_from_slice(&2u16.to_le_bytes()); // block align
        buf.extend_from_slice(&16u16.to_le_bytes()); // bits

        assert!(parse_wav(&buf).is_err());
    }

    #[test]
    fn truncated_mid_fmt_chunk_rejected() {
        // A RIFF/WAVE header with a fmt chunk whose declared size (16)
        // exceeds the remaining bytes: declaring 16 but only writing 4.
        let mut buf = Vec::new();
        buf.extend_from_slice(b"RIFF");
        buf.extend_from_slice(&100u32.to_le_bytes());
        buf.extend_from_slice(b"WAVE");
        buf.extend_from_slice(b"fmt ");
        buf.extend_from_slice(&16u32.to_le_bytes()); // declares 16 bytes
        buf.extend_from_slice(&1u16.to_le_bytes()); // PCM (2 of the 16)
        buf.extend_from_slice(&1u16.to_le_bytes()); // mono (4 of the 16)
        // Truncated here — parser must reject, not read past end.
        assert!(parse_wav(&buf).is_err());
    }

    #[test]
    fn non_pcm_format_code_rejected() {
        // Format code 3 (IEEE float) in a file this parser only handles PCM.
        // If the parser silently accepts this and misinterprets the bytes,
        // we'd read garbage samples.
        let mut samples = Vec::new();
        samples.extend_from_slice(&0i16.to_le_bytes());
        // Build a WAV with format = 99 (unsupported/invalid).
        let mut buf = Vec::new();
        buf.extend_from_slice(b"RIFF");
        let total_size = 4 + (8 + 16) + (8 + samples.len());
        buf.extend_from_slice(&(total_size as u32).to_le_bytes());
        buf.extend_from_slice(b"WAVE");
        buf.extend_from_slice(b"fmt ");
        buf.extend_from_slice(&16u32.to_le_bytes());
        buf.extend_from_slice(&99u16.to_le_bytes()); // unsupported format code
        buf.extend_from_slice(&1u16.to_le_bytes());
        buf.extend_from_slice(&44100u32.to_le_bytes());
        buf.extend_from_slice(&88200u32.to_le_bytes());
        buf.extend_from_slice(&2u16.to_le_bytes());
        buf.extend_from_slice(&16u16.to_le_bytes());
        buf.extend_from_slice(b"data");
        buf.extend_from_slice(&(samples.len() as u32).to_le_bytes());
        buf.extend_from_slice(&samples);

        assert!(
            parse_wav(&buf).is_err(),
            "unsupported PCM format code 99 must be rejected, not silently accepted"
        );
    }

    #[test]
    fn stereo_to_mono_via_audio_data() {
        let mut samples = Vec::new();
        // L = +1.0, R = -1.0 → mono ≈ 0.0
        samples.extend_from_slice(&32767i16.to_le_bytes());
        samples.extend_from_slice(&(-32768i16).to_le_bytes());

        let wav = make_wav(2, 44100, 16, &samples);
        let parsed = parse_wav(&wav).unwrap();
        let mono = parsed.mix_to_mono();
        assert!(mono[0].abs() < 0.01);
    }
}
