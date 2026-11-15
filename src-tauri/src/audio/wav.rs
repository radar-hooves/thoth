//! WAV reading on `hound`, so a file with extra chunks parses like any other.

use anyhow::{Result, anyhow};
use std::path::Path;

/// Silence added before the first word so the model can initialise (500 ms at 16 kHz).
const LEADING_SILENCE: usize = 8_000;
/// Silence added after the last word so the model can finalise (1.5 s at 16 kHz).
const TRAILING_SILENCE: usize = 24_000;

/// Read a WAV file as mono f32 samples (channels averaged) and its sample rate.
pub fn read_mono_f32(path: &Path) -> Result<(Vec<f32>, u32)> {
    let reader = hound::WavReader::open(path)?;
    let spec = reader.spec();

    let samples: Vec<f32> = match (spec.sample_format, spec.bits_per_sample) {
        (hound::SampleFormat::Int, bits @ 1..=32) => {
            let scale = (1u64 << (bits - 1)) as f32;
            reader
                .into_samples::<i32>()
                .filter_map(|s| s.ok())
                .map(|s| s as f32 / scale)
                .collect()
        }
        (hound::SampleFormat::Float, 32) => reader
            .into_samples::<f32>()
            .filter_map(|s| s.ok())
            .collect(),
        (format, bits) => {
            return Err(anyhow!("Unsupported audio format: {bits} bits, {format:?}"));
        }
    };

    let channels = spec.channels.max(1) as usize;
    let mono = if channels > 1 {
        samples
            .chunks(channels)
            .map(|frame| frame.iter().sum::<f32>() / channels as f32)
            .collect()
    } else {
        samples
    };

    Ok((mono, spec.sample_rate))
}

/// Duration of a WAV file in seconds, or `None` if it cannot be read.
pub fn duration_secs(path: &Path) -> Option<f64> {
    let reader = hound::WavReader::open(path).ok()?;
    let spec = reader.spec();
    if spec.sample_rate == 0 {
        return None;
    }
    Some(reader.duration() as f64 / spec.sample_rate as f64)
}

/// Trim leading silence from long recordings (the tail is always kept), then
/// pad both ends with silence for the transcription model.
pub fn trim_and_pad(samples: &[f32], sample_rate: u32) -> Vec<f32> {
    let (start, end) = crate::audio::vad::trim_silence(samples, sample_rate);
    let trimmed = &samples[start..end];
    let mut padded = Vec::with_capacity(LEADING_SILENCE + trimmed.len() + TRAILING_SILENCE);
    padded.extend(std::iter::repeat_n(0.0f32, LEADING_SILENCE));
    padded.extend_from_slice(trimmed);
    padded.extend(std::iter::repeat_n(0.0f32, TRAILING_SILENCE));
    padded
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_wav(path: &Path, channels: u16, samples: &[i16]) {
        let spec = hound::WavSpec {
            channels,
            sample_rate: 16_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(path, spec).unwrap();
        for s in samples {
            w.write_sample(*s).unwrap();
        }
        w.finalize().unwrap();
    }

    #[test]
    fn duration_and_samples_read_a_plain_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.wav");
        write_wav(&path, 1, &vec![1000; 16_000]);
        assert!((duration_secs(&path).unwrap() - 1.0).abs() < 1e-9);
        let (s, rate) = read_mono_f32(&path).unwrap();
        assert_eq!((s.len(), rate), (16_000, 16_000));
    }

    #[test]
    fn stereo_is_averaged_to_mono() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.wav");
        write_wav(&path, 2, &[16384, -16384, 16384, 16384]);
        let (s, _) = read_mono_f32(&path).unwrap();
        assert_eq!(s, vec![0.0, 0.5]);
    }

    #[test]
    fn a_file_with_an_extra_chunk_keeps_its_true_duration() {
        let dir = tempfile::tempdir().unwrap();
        let plain = dir.path().join("p.wav");
        write_wav(&plain, 1, &vec![0; 16_000]);
        let bytes = std::fs::read(&plain).unwrap();
        // Insert a LIST chunk between `fmt ` (ends at 36) and `data`.
        let extra: &[u8] = b"LIST\x08\x00\x00\x00abcdefgh";
        let mut out = bytes[..36].to_vec();
        out.extend_from_slice(extra);
        out.extend_from_slice(&bytes[36..]);
        let riff = (out.len() - 8) as u32;
        out[4..8].copy_from_slice(&riff.to_le_bytes());
        let path = dir.path().join("x.wav");
        std::fs::File::create(&path)
            .unwrap()
            .write_all(&out)
            .unwrap();
        assert!((duration_secs(&path).unwrap() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn unreadable_file_is_none() {
        assert!(duration_secs(Path::new("/nonexistent.wav")).is_none());
    }

    #[test]
    fn padding_adds_half_a_second_before_and_one_and_a_half_after() {
        let out = trim_and_pad(&vec![0.1; 1000], 16_000);
        assert_eq!(out.len(), 8_000 + 1000 + 24_000);
        assert_eq!(out[7_999], 0.0);
        assert_eq!(out[8_000], 0.1);
    }
}
