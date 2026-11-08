//! Audio file decoding and conversion to 16kHz mono WAV
//!
//! Uses symphonia for format-agnostic decoding and the existing AudioConverter
//! (rubato) for high-quality resampling. The format is decided by probing the
//! file's content, never its name: the extension is only a hint.
//!
//! Containers: WAV, AIFF, CAF, MP3, ADTS (raw AAC), MP4/M4A/MOV, Ogg, MKV/WebM, FLAC.
//! Codecs: PCM, ADPCM, MP3, AAC, ALAC, FLAC, Vorbis, and Opus (bundled libopus,
//! which symphonia lacks). AMR is the one common voice-note codec with no decoder.

use crate::audio::format::AudioConverter;
use std::path::Path;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicBool, Ordering};
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::codecs::registry::CodecRegistry;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

/// Maximum file size for import (500 MB)
const MAX_FILE_SIZE: u64 = 500 * 1024 * 1024;

/// Target sample rate for transcription
const TARGET_SAMPLE_RATE: u32 = 16_000;

/// Chunk size for the rubato resampler (frames per call)
const RESAMPLE_CHUNK_SIZE: usize = 1024;

/// Check cancellation every N packets
const CANCEL_CHECK_INTERVAL: u32 = 50;

/// Symphonia's enabled codecs plus Opus, which symphonia has no decoder for.
static CODECS: LazyLock<CodecRegistry> = LazyLock::new(|| {
    let mut registry = CodecRegistry::new();
    symphonia::default::register_enabled_codecs(&mut registry);
    registry.register_audio_decoder::<symphonia_adapter_libopus::OpusDecoder>();
    registry
});

/// Name an AMR file from its magic bytes (`#!AMR`), which no extension can fake.
fn is_amr(path: &Path) -> bool {
    use std::io::Read;
    let mut magic = [0u8; 5];
    std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut magic))
        .is_ok()
        && &magic == b"#!AMR"
}

const AMR_REFUSAL: &str = "AMR audio cannot be decoded (no AMR decoder is bundled). \
Convert it to WAV, M4A or Ogg Opus first.";

/// Decode an audio file to 16kHz mono WAV suitable for transcription.
///
/// Decides by content, not name: see the module docs for what is decoded.
/// Returns the audio duration in seconds on success.
pub fn decode_audio_to_wav(
    input_path: &Path,
    output_path: &Path,
    cancel: &AtomicBool,
) -> Result<f64, String> {
    // Validate file exists and check size
    let metadata = std::fs::metadata(input_path).map_err(|e| format!("Cannot read file: {}", e))?;

    if metadata.len() > MAX_FILE_SIZE {
        return Err(format!(
            "File is too large ({:.0} MB). Maximum supported size is {} MB.",
            metadata.len() as f64 / (1024.0 * 1024.0),
            MAX_FILE_SIZE / (1024 * 1024)
        ));
    }

    // Fast path: if already 16kHz mono WAV, copy directly
    if is_target_format_wav(input_path) {
        tracing::info!("Audio file is already 16kHz mono WAV, copying directly");
        std::fs::copy(input_path, output_path)
            .map_err(|e| format!("Failed to copy WAV file: {}", e))?;
        return get_wav_duration(output_path);
    }

    // Open the file and create a media source stream
    let file =
        std::fs::File::open(input_path).map_err(|e| format!("Failed to open audio file: {}", e))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    // Hint the format from file extension
    let mut hint = Hint::new();
    if let Some(ext) = input_path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    // Probe the format (symphonia 0.6: probe() replaces format())
    let mut format = symphonia::default::get_probe()
        .probe(
            &hint,
            mss,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(|e| {
            if is_amr(input_path) {
                AMR_REFUSAL.to_string()
            } else {
                format!("Not a recognised audio or video file ({})", e)
            }
        })?;

    // Find the first audio track (symphonia 0.6: default_track(TrackType::Audio))
    let track = format
        .default_track(TrackType::Audio)
        .ok_or_else(|| "The file has no audio track".to_string())?;

    let track_id = track.id;

    // codec_params is now Option<CodecParameters (enum)>; .audio() extracts audio params
    let audio_params = track
        .codec_params
        .as_ref()
        .and_then(|p| p.audio())
        .ok_or_else(|| "No audio codec parameters found".to_string())?
        .clone();

    let source_rate = audio_params
        .sample_rate
        .ok_or_else(|| "Cannot determine sample rate from audio file".to_string())?;
    let source_channels = audio_params
        .channels
        .as_ref()
        .map(|c| c.count())
        .unwrap_or(1);

    tracing::info!(
        "Decoding: {}Hz, {} channels -> {}Hz mono",
        source_rate,
        source_channels,
        TARGET_SAMPLE_RATE
    );

    // Create the decoder (symphonia 0.6: make_audio_decoder() replaces make())
    let mut decoder = CODECS
        .make_audio_decoder(&audio_params, &AudioDecoderOptions::default())
        .map_err(|e| {
            format!(
                "Cannot decode this audio codec ({:?}): {}",
                audio_params.codec, e
            )
        })?;

    // Create the resampler (handles arbitrary sample rate ratios)
    let mut converter = AudioConverter::new(
        source_rate,
        TARGET_SAMPLE_RATE,
        source_channels,
        RESAMPLE_CHUNK_SIZE,
    )
    .map_err(|e| format!("Failed to create resampler: {}", e))?;

    // Open the output WAV file
    let wav_spec = hound::WavSpec {
        channels: 1,
        sample_rate: TARGET_SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut wav_writer = hound::WavWriter::create(output_path, wav_spec)
        .map_err(|e| format!("Failed to create output WAV file: {}", e))?;

    // Decode loop with chunk buffering for rubato
    let mut resample_buffer: Vec<f32> = Vec::new();
    let mut total_source_frames: u64 = 0;
    let mut packet_count: u32 = 0;
    let frames_per_chunk = RESAMPLE_CHUNK_SIZE * source_channels;

    loop {
        // Periodic cancellation check
        if packet_count.is_multiple_of(CANCEL_CHECK_INTERVAL) && cancel.load(Ordering::Relaxed) {
            // Clean up partial output
            drop(wav_writer);
            let _ = std::fs::remove_file(output_path);
            return Err("Import cancelled".to_string());
        }
        packet_count += 1;

        // Get next packet (symphonia 0.6: Ok(None) signals EOF instead of UnexpectedEof)
        let packet = match format.next_packet() {
            Ok(Some(packet)) => packet,
            Ok(None) => break, // End of stream
            Err(SymphoniaError::ResetRequired) => break,
            Err(e) => return Err(format!("Error reading audio: {}", e)),
        };

        // Skip packets from other tracks
        if packet.track_id != track_id {
            continue;
        }

        // Decode the packet into audio samples
        let decoded = match decoder.decode(&packet) {
            Ok(decoded) => decoded,
            Err(SymphoniaError::IoError(_)) | Err(SymphoniaError::DecodeError(_)) => continue,
            Err(e) => return Err(format!("Decode error: {}", e)),
        };

        // Collect decoded samples as interleaved f32 (SampleBuffer removed in 0.6;
        // copy_to_vec_interleaved is the replacement on GenericAudioBufferRef)
        let mut samples: Vec<f32> = Vec::new();
        decoded.copy_to_vec_interleaved(&mut samples);

        let frame_count = samples.len() / source_channels.max(1);
        total_source_frames += frame_count as u64;

        // Accumulate in resample buffer
        resample_buffer.extend_from_slice(&samples);

        // Drain in exact chunk sizes for rubato
        while resample_buffer.len() >= frames_per_chunk {
            let chunk: Vec<f32> = resample_buffer.drain(..frames_per_chunk).collect();
            let resampled = converter
                .process_to_i16(&chunk)
                .map_err(|e| format!("Resampling error: {}", e))?;

            for &sample in &resampled {
                wav_writer
                    .write_sample(sample)
                    .map_err(|e| format!("Failed to write WAV sample: {}", e))?;
            }
        }
    }

    // Finalise: resample the trailing partial chunk AND drain the resampler's
    // internal delay line, so the last few milliseconds of the file are not left
    // buffered inside the resampler and lost.
    let tail = converter
        .finish_to_i16(&resample_buffer)
        .map_err(|e| format!("Resampling error during finalise: {}", e))?;
    for &sample in &tail {
        wav_writer
            .write_sample(sample)
            .map_err(|e| format!("Failed to write WAV sample: {}", e))?;
    }

    wav_writer
        .finalize()
        .map_err(|e| format!("Failed to finalise WAV file: {}", e))?;

    let duration = if source_rate > 0 {
        total_source_frames as f64 / source_rate as f64
    } else {
        0.0
    };

    tracing::info!(
        "Decoded {:.2}s of audio to {}",
        duration,
        output_path.display()
    );
    Ok(duration)
}

/// Check if a WAV file is already in 16kHz mono i16 format (fast path).
pub fn is_target_format_wav(path: &Path) -> bool {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    if !ext.eq_ignore_ascii_case("wav") {
        return false;
    }

    match hound::WavReader::open(path) {
        Ok(reader) => {
            let spec = reader.spec();
            spec.sample_rate == TARGET_SAMPLE_RATE
                && spec.channels == 1
                && spec.sample_format == hound::SampleFormat::Int
                && spec.bits_per_sample == 16
        }
        Err(_) => false,
    }
}

/// Get the duration of a WAV file from its header.
fn get_wav_duration(path: &Path) -> Result<f64, String> {
    let reader =
        hound::WavReader::open(path).map_err(|e| format!("Failed to read WAV file: {}", e))?;
    let spec = reader.spec();
    let num_samples = reader.len() as f64;
    let duration = num_samples / (spec.sample_rate as f64 * spec.channels as f64);
    Ok(duration)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    #[test]
    fn test_file_size_rejection() {
        // Create a temp file path that doesn't exist but has known metadata issues
        let cancel = AtomicBool::new(false);
        let result = decode_audio_to_wav(
            Path::new("/nonexistent/file.mp3"),
            Path::new("/tmp/out.wav"),
            &cancel,
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Cannot read file"));
    }

    #[test]
    fn test_cancellation() {
        let cancel = AtomicBool::new(true); // Pre-cancelled
        // Use a file that exists but will be cancelled immediately
        let input = tempfile::NamedTempFile::new().unwrap();
        let output = tempfile::NamedTempFile::new().unwrap();

        // Write a minimal valid WAV header so it gets past the size check
        // but the decode loop checks cancellation
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 44100,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        {
            let mut writer = hound::WavWriter::create(input.path(), spec).unwrap();
            for _ in 0..44100 {
                writer.write_sample(0i16).unwrap();
                writer.write_sample(0i16).unwrap();
            }
            writer.finalize().unwrap();
        }

        let result = decode_audio_to_wav(input.path(), output.path(), &cancel);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("cancelled"));
    }

    #[test]
    fn test_wav_fast_path() {
        let cancel = AtomicBool::new(false);

        // Create a 16kHz mono WAV (target format)
        let input = tempfile::NamedTempFile::with_suffix(".wav").unwrap();
        let output = tempfile::NamedTempFile::with_suffix(".wav").unwrap();

        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        {
            let mut writer = hound::WavWriter::create(input.path(), spec).unwrap();
            // Write 1 second of silence
            for _ in 0..16000 {
                writer.write_sample(0i16).unwrap();
            }
            writer.finalize().unwrap();
        }

        let result = decode_audio_to_wav(input.path(), output.path(), &cancel);
        assert!(result.is_ok());
        let duration = result.unwrap();
        assert!(
            (duration - 1.0).abs() < 0.01,
            "Expected ~1.0s, got {}",
            duration
        );

        // Verify output is valid WAV
        let reader = hound::WavReader::open(output.path()).unwrap();
        assert_eq!(reader.spec().sample_rate, 16000);
        assert_eq!(reader.spec().channels, 1);
    }

    #[test]
    fn test_wav_resample() {
        let cancel = AtomicBool::new(false);

        // Create a 44.1kHz stereo WAV (needs conversion)
        let input = tempfile::NamedTempFile::with_suffix(".wav").unwrap();
        let output = tempfile::NamedTempFile::with_suffix(".wav").unwrap();

        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 44100,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        {
            let mut writer = hound::WavWriter::create(input.path(), spec).unwrap();
            // Write ~1 second of a simple tone (stereo)
            for i in 0..44100 {
                let sample = ((i as f64 * 440.0 * 2.0 * std::f64::consts::PI / 44100.0).sin()
                    * 16000.0) as i16;
                writer.write_sample(sample).unwrap();
                writer.write_sample(sample).unwrap();
            }
            writer.finalize().unwrap();
        }

        let result = decode_audio_to_wav(input.path(), output.path(), &cancel);
        assert!(result.is_ok());
        let duration = result.unwrap();
        assert!(
            (duration - 1.0).abs() < 0.05,
            "Expected ~1.0s, got {}",
            duration
        );

        // Verify output is 16kHz mono
        let reader = hound::WavReader::open(output.path()).unwrap();
        assert_eq!(reader.spec().sample_rate, 16000);
        assert_eq!(reader.spec().channels, 1);
    }
}
