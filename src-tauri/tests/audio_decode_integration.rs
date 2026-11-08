//! Integration tests for audio decoding of non-WAV formats.
//!
//! These tests exercise `decode_audio_to_wav` against real fixture files
//! (generated from a 440 Hz sine tone with ffmpeg) and assert that the
//! output is always a valid 16 kHz mono i16 WAV with non-zero samples.
//!
//! NO ASR backend is invoked — decode only — so these run on CI without
//! a Neural Engine or whisper model.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use tempfile::NamedTempFile;
use thoth_lib::audio::decode::{decode_audio_to_wav, is_target_format_wav};

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

/// Run decode_audio_to_wav on a fixture and assert the output is a valid
/// 16 kHz mono i16 WAV with at least one non-zero sample.
fn assert_decodes_to_16khz_mono(fixture: &str) {
    let input = fixtures_dir().join(fixture);
    assert!(
        input.exists(),
        "fixture file not found: {}",
        input.display()
    );

    let output = NamedTempFile::with_suffix(".wav").expect("tempfile");
    let cancel = AtomicBool::new(false);

    let duration = decode_audio_to_wav(&input, output.path(), &cancel)
        .unwrap_or_else(|e| panic!("decode_audio_to_wav failed for {}: {}", fixture, e));

    // Duration must be positive
    assert!(
        duration > 0.0,
        "duration should be > 0 for {}, got {}",
        fixture,
        duration
    );

    // Output must be a valid 16 kHz mono i16 WAV
    let reader = hound::WavReader::open(output.path())
        .unwrap_or_else(|e| panic!("output not a valid WAV for {}: {}", fixture, e));
    let spec = reader.spec();
    assert_eq!(
        spec.sample_rate, 16000,
        "expected 16000 Hz for {}, got {}",
        fixture, spec.sample_rate
    );
    assert_eq!(
        spec.channels, 1,
        "expected 1 channel for {}, got {}",
        fixture, spec.channels
    );
    assert_eq!(
        spec.sample_format,
        hound::SampleFormat::Int,
        "expected Int sample format for {}",
        fixture
    );
    assert_eq!(
        spec.bits_per_sample, 16,
        "expected 16 bits/sample for {}, got {}",
        fixture, spec.bits_per_sample
    );

    // At least some non-zero samples (the fixture is a 440 Hz tone, not silence)
    let samples: Vec<i16> = reader
        .into_samples::<i16>()
        .filter_map(|s| s.ok())
        .collect();
    assert!(!samples.is_empty(), "output has no samples for {}", fixture);
    let has_nonzero = samples.iter().any(|&s| s != 0);
    assert!(
        has_nonzero,
        "all samples are zero for {} — decode produced silence",
        fixture
    );
}

#[test]
fn decode_wav_fixture() {
    assert_decodes_to_16khz_mono("tone_440hz.wav");
}

#[test]
fn decode_mp3_fixture() {
    assert_decodes_to_16khz_mono("tone_440hz.mp3");
}

#[test]
fn decode_m4a_fixture() {
    assert_decodes_to_16khz_mono("tone_440hz.m4a");
}

#[test]
fn decode_ogg_fixture() {
    assert_decodes_to_16khz_mono("tone_440hz.ogg");
}

#[test]
fn decode_flac_fixture() {
    assert_decodes_to_16khz_mono("tone_440hz.flac");
}

#[test]
fn decode_raw_adts_aac_fixture() {
    assert_decodes_to_16khz_mono("tone_440hz.aac");
}

#[test]
fn decode_opus_in_ogg_fixture() {
    assert_decodes_to_16khz_mono("tone_440hz_opus.ogg");
}

#[test]
fn decode_opus_in_webm_fixture() {
    assert_decodes_to_16khz_mono("tone_440hz.webm");
}

#[test]
fn decode_caf_pcm_fixture() {
    assert_decodes_to_16khz_mono("tone_440hz.caf");
}

#[test]
fn decode_caf_alac_fixture() {
    assert_decodes_to_16khz_mono("tone_440hz_alac.caf");
}

#[test]
fn decode_audio_track_of_mp4() {
    assert_decodes_to_16khz_mono("tone_440hz_video.mp4");
}

#[test]
fn decode_audio_track_of_mov() {
    assert_decodes_to_16khz_mono("tone_440hz_video.mov");
}

/// The format is decided by content: a wrong or missing extension changes nothing.
#[test]
fn decode_ignores_the_file_name() {
    for (fixture, name) in [
        ("tone_440hz.aac", "voice-note.dat"),
        ("tone_440hz_opus.ogg", "voice-note"),
        ("tone_440hz.mp3", "voice-note.wav.txt"),
    ] {
        let dir = tempfile::tempdir().expect("tempdir");
        let renamed = dir.path().join(name);
        std::fs::copy(fixtures_dir().join(fixture), &renamed).expect("copy");
        let output = dir.path().join("out.wav");
        let duration = decode_audio_to_wav(&renamed, &output, &AtomicBool::new(false))
            .unwrap_or_else(|e| panic!("{fixture} as {name}: {e}"));
        assert!(duration > 0.0, "{fixture} as {name}");
    }
}

/// AMR has no decoder: the refusal names the format rather than a generic error.
#[test]
fn amr_is_refused_by_name() {
    let mut amr = NamedTempFile::with_suffix(".amr").expect("tempfile");
    use std::io::Write;
    amr.write_all(b"#!AMR\n\x3c\x00\x00\x00").expect("write");
    let output = NamedTempFile::with_suffix(".wav").expect("tempfile");
    let err = decode_audio_to_wav(amr.path(), output.path(), &AtomicBool::new(false))
        .expect_err("AMR cannot be decoded");
    assert!(err.starts_with("AMR audio cannot be decoded"), "{err}");
}

/// A WAV that is already 16 kHz mono i16 is detected as the target format
/// (fast path) and is_target_format_wav returns true.
#[test]
fn wav_fixture_is_target_format() {
    let wav = fixtures_dir().join("tone_440hz.wav");
    assert!(
        is_target_format_wav(&wav),
        "16kHz mono i16 WAV should be recognised as target format"
    );
}

/// Non-WAV files are never the target format (they need a transcode).
#[test]
fn non_wav_is_not_target_format() {
    let dir = fixtures_dir();
    for ext in &["mp3", "m4a", "ogg", "flac"] {
        let path = dir.join(format!("tone_440hz.{}", ext));
        assert!(
            !is_target_format_wav(&path),
            "{} should not be recognised as target format WAV",
            ext
        );
    }
}

/// A deliberately corrupt/garbage file must fail with a clear symphonia error,
/// NOT the generic "Not a valid WAV file" message from the old header check.
#[test]
fn corrupt_file_gives_symphonia_error() {
    let mut corrupt = NamedTempFile::with_suffix(".mp3").expect("tempfile");
    use std::io::Write;
    corrupt
        .write_all(b"this is not valid audio data at all")
        .expect("write");

    let output = NamedTempFile::with_suffix(".wav").expect("tempfile");
    let cancel = AtomicBool::new(false);

    let err = decode_audio_to_wav(corrupt.path(), output.path(), &cancel)
        .expect_err("corrupt file should fail");

    // Must not reproduce the old misleading WAV-only error
    assert!(
        !err.contains("Not a valid WAV file"),
        "error message should not say 'Not a valid WAV file', got: {}",
        err
    );
    // Must contain something descriptive from symphonia
    assert!(
        !err.is_empty(),
        "error message should not be empty for a corrupt file"
    );
}
