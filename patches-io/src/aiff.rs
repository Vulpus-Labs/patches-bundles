//! AIFF file reader.
//!
//! Reads uncompressed AIFF files and returns per-channel f32 samples normalised
//! to [-1, 1]. Supports 8, 16, and 24-bit sample depths.

use std::path::Path;

use crate::audio_data::{AudioData, AudioIoError};

/// Read an AIFF file and return per-channel audio data with sample rate.
pub fn read_aiff(path: &Path) -> Result<AudioData, AudioIoError> {
    let data = std::fs::read(path)?;
    parse_aiff(&data)
}

// ---------------------------------------------------------------------------
// Internal parsing
// ---------------------------------------------------------------------------

/// Decode an 80-bit IEEE 754 extended float (big-endian) to f64.
///
/// AIFF stores sample rate as a 10-byte extended precision float.
fn decode_extended_float(bytes: &[u8]) -> f64 {
    let sign = if bytes[0] & 0x80 != 0 { -1.0 } else { 1.0 };
    let exponent = (((bytes[0] as u16) & 0x7F) << 8) | (bytes[1] as u16);
    let mut mantissa: u64 = 0;
    for &b in &bytes[2..10] {
        mantissa = (mantissa << 8) | b as u64;
    }
    if exponent == 0 && mantissa == 0 {
        return 0.0;
    }
    // Bias for 80-bit extended is 16383.
    let f_exp = exponent as i32 - 16383;
    // The mantissa has an explicit integer bit (bit 63).
    let f_mantissa = mantissa as f64 / (1u64 << 63) as f64;
    sign * f_mantissa * (2.0f64).powi(f_exp)
}

/// Parse AIFF data from a byte slice.
fn parse_aiff(data: &[u8]) -> Result<AudioData, AudioIoError> {
    if data.len() < 12 || &data[0..4] != b"FORM" || &data[8..12] != b"AIFF" {
        return Err(AudioIoError::MalformedFile("not a valid AIFF file".into()));
    }

    let mut num_channels: u16 = 0;
    let mut num_frames: u32 = 0;
    let mut sample_size: u16 = 0;
    let mut sample_rate: f64 = 0.0;
    let mut comm_found = false;
    let mut ssnd_data: Option<&[u8]> = None;

    // Walk IFF chunks.
    let mut pos = 12;
    while pos + 8 <= data.len() {
        let chunk_id = &data[pos..pos + 4];
        let chunk_size = u32::from_be_bytes([
            data[pos + 4],
            data[pos + 5],
            data[pos + 6],
            data[pos + 7],
        ]) as usize;
        let chunk_body = pos + 8;

        if chunk_id == b"COMM" && chunk_size >= 18 {
            num_channels = u16::from_be_bytes([data[chunk_body], data[chunk_body + 1]]);
            num_frames = u32::from_be_bytes([
                data[chunk_body + 2],
                data[chunk_body + 3],
                data[chunk_body + 4],
                data[chunk_body + 5],
            ]);
            sample_size = u16::from_be_bytes([data[chunk_body + 6], data[chunk_body + 7]]);
            sample_rate = decode_extended_float(&data[chunk_body + 8..chunk_body + 18]);
            comm_found = true;
        } else if chunk_id == b"SSND" && chunk_size >= 8 {
            let offset = u32::from_be_bytes([
                data[chunk_body],
                data[chunk_body + 1],
                data[chunk_body + 2],
                data[chunk_body + 3],
            ]);
            let audio_start = chunk_body + 8 + offset as usize;
            let audio_end = (chunk_body + chunk_size).min(data.len());
            if audio_start <= audio_end {
                ssnd_data = Some(&data[audio_start..audio_end]);
            }
        }

        // Chunks are padded to even size.
        let padded = if chunk_size % 2 == 1 {
            chunk_size + 1
        } else {
            chunk_size
        };
        pos = chunk_body + padded;
    }

    if !comm_found {
        return Err(AudioIoError::MalformedFile(
            "AIFF file missing COMM chunk".into(),
        ));
    }
    let ssnd = ssnd_data.ok_or_else(|| {
        AudioIoError::MalformedFile("AIFF file missing SSND chunk".into())
    })?;

    let bytes_per_sample = match sample_size {
        8 => 1,
        16 => 2,
        24 => 3,
        _ => {
            return Err(AudioIoError::UnsupportedEncoding(format!(
                "unsupported AIFF bit depth: {sample_size}"
            )))
        }
    };

    let n_ch = num_channels as usize;
    let frame_size = bytes_per_sample * n_ch;
    let total_frames = num_frames as usize;
    let mut channels: Vec<Vec<f32>> =
        (0..n_ch).map(|_| Vec::with_capacity(total_frames)).collect();

    for frame in 0..total_frames {
        let frame_start = frame * frame_size;
        if frame_start + frame_size > ssnd.len() {
            break;
        }
        for (ch, channel) in channels.iter_mut().enumerate() {
            let sample_start = frame_start + ch * bytes_per_sample;
            let sample_bytes = &ssnd[sample_start..sample_start + bytes_per_sample];
            let sample_f = decode_sample_be(sample_bytes, sample_size);
            channel.push(sample_f as f32);
        }
    }

    Ok(AudioData {
        channels,
        sample_rate,
    })
}

/// Decode a big-endian signed integer sample to f64 in [-1, 1].
fn decode_sample_be(bytes: &[u8], bit_depth: u16) -> f64 {
    match bit_depth {
        8 => {
            let s = bytes[0] as i8;
            s as f64 / 128.0
        }
        16 => {
            let s = i16::from_be_bytes([bytes[0], bytes[1]]);
            s as f64 / 32768.0
        }
        24 => {
            let hi = bytes[0] as i32;
            let raw = (hi << 16) | ((bytes[1] as i32) << 8) | (bytes[2] as i32);
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

    /// Build a minimal valid AIFF in memory.
    fn make_aiff(channels: u16, frames: u32, bit_depth: u16, samples: &[u8]) -> Vec<u8> {
        let mut buf = Vec::new();

        buf.extend_from_slice(b"FORM");
        buf.extend_from_slice(&[0; 4]); // size placeholder
        buf.extend_from_slice(b"AIFF");

        // COMM chunk
        buf.extend_from_slice(b"COMM");
        buf.extend_from_slice(&18u32.to_be_bytes());
        buf.extend_from_slice(&channels.to_be_bytes());
        buf.extend_from_slice(&frames.to_be_bytes());
        buf.extend_from_slice(&bit_depth.to_be_bytes());
        // 80-bit extended float for 44100.0
        buf.extend_from_slice(&[0x40, 0x0e, 0xac, 0x44, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);

        // SSND chunk
        let ssnd_size = 8 + samples.len();
        buf.extend_from_slice(b"SSND");
        buf.extend_from_slice(&(ssnd_size as u32).to_be_bytes());
        buf.extend_from_slice(&0u32.to_be_bytes()); // offset
        buf.extend_from_slice(&0u32.to_be_bytes()); // blockSize
        buf.extend_from_slice(samples);
        if samples.len() % 2 == 1 {
            buf.push(0);
        }

        // Fill in FORM size
        let form_size = (buf.len() - 8) as u32;
        buf[4..8].copy_from_slice(&form_size.to_be_bytes());

        buf
    }

    #[test]
    fn mono_16bit() {
        let half_pos = 16384i16;
        let half_neg = -16384i16;
        let mut samples = Vec::new();
        samples.extend_from_slice(&half_pos.to_be_bytes());
        samples.extend_from_slice(&half_neg.to_be_bytes());

        let aiff = make_aiff(1, 2, 16, &samples);
        let parsed = parse_aiff(&aiff).unwrap();
        let mono = parsed.mix_to_mono();
        assert_eq!(mono.len(), 2);
        assert!((mono[0] - 0.5).abs() < 0.001);
        assert!((mono[1] + 0.5).abs() < 0.001);
    }

    #[test]
    fn stereo_to_mono_16bit() {
        let mut samples = Vec::new();
        samples.extend_from_slice(&32767i16.to_be_bytes());
        samples.extend_from_slice(&(-32768i16).to_be_bytes());

        let aiff = make_aiff(2, 1, 16, &samples);
        let parsed = parse_aiff(&aiff).unwrap();
        let mono = parsed.mix_to_mono();
        assert_eq!(mono.len(), 1);
        assert!(mono[0].abs() < 0.01);
    }

    #[test]
    fn stereo_channels_preserved() {
        let mut samples = Vec::new();
        samples.extend_from_slice(&16384i16.to_be_bytes());
        samples.extend_from_slice(&(-16384i16).to_be_bytes());

        let aiff = make_aiff(2, 1, 16, &samples);
        let parsed = parse_aiff(&aiff).unwrap();
        assert_eq!(parsed.channels.len(), 2);
        assert!((parsed.channels[0][0] - 0.5).abs() < 0.001);
        assert!((parsed.channels[1][0] + 0.5).abs() < 0.001);
    }

    #[test]
    fn sample_rate_parsed() {
        let samples = vec![0x00, 0x00];
        let aiff = make_aiff(1, 1, 16, &samples);
        let parsed = parse_aiff(&aiff).unwrap();
        assert!((parsed.sample_rate - 44100.0).abs() < 1.0);
    }

    #[test]
    fn mono_24bit() {
        let samples = vec![0x40, 0x00, 0x00];
        let aiff = make_aiff(1, 1, 24, &samples);
        let parsed = parse_aiff(&aiff).unwrap();
        let mono = parsed.mix_to_mono();
        assert!((mono[0] - 0.5).abs() < 0.001);
    }

    #[test]
    fn negative_24bit() {
        let samples = vec![0xC0, 0x00, 0x00];
        let aiff = make_aiff(1, 1, 24, &samples);
        let parsed = parse_aiff(&aiff).unwrap();
        let mono = parsed.mix_to_mono();
        assert!((mono[0] + 0.5).abs() < 0.001);
    }

    #[test]
    fn not_aiff() {
        assert!(parse_aiff(b"NOT_AN_AIFF_FILE").is_err());
    }

    #[test]
    fn missing_ssnd() {
        let mut buf = Vec::new();
        buf.extend_from_slice(b"FORM");
        buf.extend_from_slice(&100u32.to_be_bytes());
        buf.extend_from_slice(b"AIFF");
        buf.extend_from_slice(b"COMM");
        buf.extend_from_slice(&18u32.to_be_bytes());
        buf.extend_from_slice(&1u16.to_be_bytes());
        buf.extend_from_slice(&0u32.to_be_bytes());
        buf.extend_from_slice(&16u16.to_be_bytes());
        buf.extend_from_slice(&[0x40, 0x0e, 0xac, 0x44, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);

        assert!(parse_aiff(&buf).is_err());
    }
}
