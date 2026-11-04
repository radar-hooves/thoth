//! FluidAudio transcription using Apple Neural Engine via CoreML
//!
//! Runs Parakeet TDT models on the Apple Neural Engine (ANE) for
//! dramatically faster transcription (~210x real-time factor).
//!
//! Requires the `fluidaudio` Cargo feature and macOS with Apple Silicon.
//!
//! # NOTE — empirically load-bearing constants and invariants
//!
//! The segmentation constants (`SINGLE_SHOT_MAX_SECS`, `SEGMENT_TARGET_SECS`,
//! pad lengths) and the no-normalisation invariant in `write_padded_wav` are
//! empirically tuned to work around a non-monotonic failure mode in FluidAudio
//! 0.15's chunked decoder: depending on recording length the decoder either
//! drops the tail or appends a hallucinated filler token. Padding-only tuning
//! merely moves which lengths fail. Any change to this decode path MUST be
//! verified via a LIVE APP recording at 19 s, 52 s, 144 s, and ~5 min.
//! Unit tests and the fork's standalone CLI example do NOT exercise the full
//! decode path (they skip the app's pad + WAV-write step).

#![cfg(all(target_os = "macos", feature = "fluidaudio"))]

use anyhow::{Result, anyhow};
use fluidaudio_rs::FluidAudio;
use std::path::{Path, PathBuf};

use super::tail::{TAIL_MIN_TOTAL_SECS, TAIL_SECS, ends_in_speech, splice_tail};

/// Maximum audio length (seconds, before padding) handed to FluidAudio as a
/// single unit.
///
/// FluidAudio 0.15 decodes audio up to its single-shot limit (15.0 s /
/// 240 000 samples at 16 kHz) in one reliable pass. Longer audio takes its
/// sliding-window chunk processor, whose *final* chunk decodes unreliably:
/// depending on where the last boundary lands relative to the audio length it
/// either drops the tail (a long recording loses its closing words) or appends
/// a hallucinated filler token ("Okay", "Thank you") on the trailing low-energy
/// frames. Tuning the padding only moves which lengths land badly; the only
/// robust fix is to keep every unit at or below the single-shot limit. With the
/// padding below a unit of this length stays under 240 000 samples.
const SINGLE_SHOT_MAX_SECS: f32 = 14.4;

/// Target maximum length (seconds, before padding) of each segment when a
/// recording must be split. Comfortably under [`SINGLE_SHOT_MAX_SECS`] so a
/// padded segment still fits the single-shot decoder.
const SEGMENT_TARGET_SECS: f32 = 13.0;

/// Minimum segment length (seconds) when splitting; avoids tiny fragments and
/// keeps the cut search from landing immediately after the previous boundary.
const SEGMENT_MIN_SECS: f32 = 4.0;

/// Leading silence padding (seconds) wrapped around every unit so the TDT
/// decoder has run-in room before the first word.
const LEAD_PAD_SECS: f32 = 0.25;

/// Trailing silence padding (seconds) so the decoder has run-out room to
/// finalise the last word.
const TRAIL_PAD_SECS: f32 = 0.25;

/// Analysis frame length (seconds) for silence-gap detection.
const GAP_FRAME_SECS: f32 = 0.02;

/// Minimum pause length (seconds) that qualifies as a segment cut point.
const GAP_MIN_SECS: f32 = 0.30;

/// A frame counts as silent below this fraction of the recording's speech
/// level. Relative so the detector adapts to quiet lapel-mic recordings.
const GAP_REL_THRESHOLD: f32 = 0.10;

/// Absolute RMS floor below which a frame is silent regardless of the relative
/// threshold; guards against a near-silent recording producing a speech level
/// of ~0 and flagging everything as silence.
const GAP_ABS_FLOOR: f32 = 0.004;

/// Silence duration above which a segment seam is treated as a genuine sentence
/// boundary (preserving punctuation and the following capital). Below this
/// threshold the seam is mid-sentence — the silence is a breath or short pause
/// — so any trailing terminal punctuation the model added is stripped and the
/// next segment's first token is lowercased (subject to pronoun/acronym guards).
///
/// Empirically a breath pause is ~0.30–0.55 s; a real sentence boundary is
/// typically 0.7 s or more. 0.6 s sits in the gap.
const SENTENCE_PAUSE_SECS: f32 = 0.6;

/// Transcription service using FluidAudio (Apple Neural Engine)
pub struct TranscriptionService {
    audio: FluidAudio,
}

// FluidAudio bridge is Send+Sync internally
unsafe impl Send for TranscriptionService {}

impl TranscriptionService {
    /// Create and initialise the FluidAudio transcription service
    ///
    /// This calls `init_asr()` which downloads and compiles CoreML models
    /// on first run (~500 MB download + 20-30s ANE compilation).
    /// Subsequent calls load from cache in ~1s.
    pub fn new() -> Result<Self> {
        let audio = FluidAudio::new()
            .map_err(|e| anyhow!("Failed to create FluidAudio instance: {}", e))?;

        // Check Apple Silicon before attempting ANE init
        if !audio.is_apple_silicon() {
            return Err(anyhow!(
                "FluidAudio requires Apple Silicon (M1/M2/M3/M4). \
                 Intel Macs are not supported."
            ));
        }

        tracing::info!("Initialising FluidAudio ASR (Neural Engine)...");
        let start = std::time::Instant::now();

        audio
            .init_asr()
            .map_err(|e| anyhow!("Failed to initialise FluidAudio ASR: {}", e))?;

        tracing::info!(
            "FluidAudio ASR initialised in {:.1}s",
            start.elapsed().as_secs_f32()
        );

        Ok(Self { audio })
    }

    /// Transcribe audio from a WAV file.
    ///
    /// Keeps every unit handed to FluidAudio at or below the single-shot decode
    /// limit (see [`SINGLE_SHOT_MAX_SECS`]). Short recordings transcribe in one
    /// pass; longer ones are split at natural pauses into single-shot segments
    /// whose transcripts are joined with a space. This avoids FluidAudio 0.15's
    /// unreliable final-chunk decode, which otherwise drops the tail of long
    /// recordings or appends a hallucinated trailing filler token.
    ///
    /// The engine sometimes drops the last words of a take, so the last
    /// [`TAIL_SECS`] are decoded again on their own and any words the main
    /// transcript lacks are spliced on (see [`splice_tail`]).
    pub fn transcribe(&self, audio_path: &Path) -> Result<String> {
        let (samples, spec) = match read_wav_mono(audio_path) {
            Ok(loaded) => loaded,
            Err(e) => {
                // We couldn't parse the WAV ourselves; hand the original file to
                // FluidAudio rather than failing the transcription outright.
                tracing::warn!(
                    "Could not read WAV for segmentation ({e}); sending original to FluidAudio"
                );
                return self.transcribe_one(audio_path);
            }
        };

        let main = self.decode_segments(audio_path, &samples, &spec)?;
        match self.decode_tail(audio_path, &samples, &spec) {
            Ok(Some(tail)) => {
                let spliced = splice_tail(&main, &tail);
                if spliced != main {
                    tracing::info!("FluidAudio: recovered a dropped ending from the tail clip");
                }
                Ok(spliced)
            }
            Ok(None) => Ok(main),
            // The main transcript is complete bar a possible dropped ending;
            // a failed tail decode must not lose it.
            Err(e) => {
                tracing::warn!("FluidAudio tail decode failed ({e}); keeping the main transcript");
                Ok(main)
            }
        }
    }

    /// Decode the last [`TAIL_SECS`] of a take on their own, or `None` when
    /// the take is too short for that to differ from the main decode or does not end in speech.
    fn decode_tail(
        &self,
        audio_path: &Path,
        samples: &[f32],
        spec: &hound::WavSpec,
    ) -> Result<Option<String>> {
        let rate = spec.sample_rate as f32;
        if samples.len() as f32 / rate <= TAIL_MIN_TOTAL_SECS
            || !ends_in_speech(samples, spec.sample_rate)
        {
            return Ok(None);
        }
        let begin = samples.len() - (TAIL_SECS * rate) as usize;
        let tmp = audio_path.with_extension("tail.wav");
        write_padded_wav(&samples[begin..], spec, &tmp)?;
        let text = self.transcribe_one(&tmp);
        let _ = std::fs::remove_file(&tmp);
        Ok(Some(text?.trim().to_string()))
    }

    /// Decode a take in single-shot segments and join them.
    fn decode_segments(
        &self,
        audio_path: &Path,
        samples: &[f32],
        spec: &hound::WavSpec,
    ) -> Result<String> {
        let (segments, seam_gaps) = plan_segments(samples, spec.sample_rate);
        let total_secs = samples.len() as f32 / spec.sample_rate as f32;
        tracing::info!(
            "FluidAudio: {:.1}s recording → {} segment(s) (single-shot limit {:.1}s)",
            total_secs,
            segments.len(),
            SINGLE_SHOT_MAX_SECS,
        );

        let start = std::time::Instant::now();
        let mut parts: Vec<String> = Vec::with_capacity(segments.len());
        let mut kept: Vec<bool> = Vec::with_capacity(segments.len());
        for (i, &(begin, end)) in segments.iter().enumerate() {
            let tmp = audio_path.with_extension(format!("seg{i}.wav"));
            write_padded_wav(&samples[begin..end], spec, &tmp)?;
            let text = self.transcribe_one(&tmp);
            let _ = std::fs::remove_file(&tmp);

            let text = text?;
            let trimmed = text.trim();
            if trimmed.is_empty() {
                kept.push(false);
            } else {
                kept.push(true);
                parts.push(trimmed.to_string());
            }
        }

        let aligned_gaps = align_kept_gaps(&kept, &seam_gaps);
        let joined = join_segments(&parts, &aligned_gaps);
        tracing::info!(
            "FluidAudio transcript: {} chars, {} words from {} segment(s) in {:.3}s",
            joined.len(),
            joined.split_whitespace().count(),
            segments.len(),
            start.elapsed().as_secs_f32(),
        );
        Ok(joined)
    }

    /// Transcribe a single WAV file (one unit, no further segmentation).
    fn transcribe_one(&self, path: &Path) -> Result<String> {
        let result = self
            .audio
            .transcribe_file(path)
            .map_err(|e| anyhow!("FluidAudio transcription failed: {}", e))?;
        Ok(result.text)
    }
}

/// Read a WAV file as mono `f32` samples in `[-1.0, 1.0]`.
///
/// If the file has more than one channel, only the first is kept (recordings
/// are mono, but this stays correct if that ever changes). Returns the samples
/// alongside the file's [`hound::WavSpec`] so segments can be written back in
/// the same format.
fn read_wav_mono(path: &Path) -> Result<(Vec<f32>, hound::WavSpec)> {
    let mut reader = hound::WavReader::open(path)?;
    let spec = reader.spec();
    let channels = (spec.channels as usize).max(1);

    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Int => {
            let scale = (1i64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .samples::<i32>()
                .step_by(channels)
                .map(|s| s.map(|v| v as f32 / scale))
                .collect::<std::result::Result<_, _>>()?
        }
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .step_by(channels)
            .collect::<std::result::Result<_, _>>()?,
    };

    Ok((samples, spec))
}

/// Plan the segments to transcribe for a recording.
///
/// Recordings within the single-shot limit are returned as a single
/// `(0, len)` range. Longer recordings are split greedily at silence: each
/// segment runs to the last detected pause within
/// `[pos + SEGMENT_MIN, pos + SEGMENT_TARGET]`, falling back to a hard cut at
/// `SEGMENT_TARGET` if that window holds no pause. Cutting at pauses keeps
/// segment boundaries off mid-word positions, so joining the transcripts loses
/// no audio.
///
/// Returns `(segments, seam_gaps)` where `seam_gaps[i]` is the silence duration
/// (seconds) of the pause between `segments[i]` and `segments[i+1]`. The gaps
/// slice is always `segments.len() - 1` entries. A hard-cut fallback (no detected
/// silence) produces a gap of `0.0` at that seam.
fn plan_segments(samples: &[f32], rate: u32) -> (Vec<(usize, usize)>, Vec<f32>) {
    let total = samples.len();
    if total == 0 {
        return (vec![(0, 0)], vec![]);
    }
    if total as f32 / rate as f32 <= SINGLE_SHOT_MAX_SECS {
        return (vec![(0, total)], vec![]);
    }

    let cuts = find_silence_cuts(samples, rate);
    let target = (SEGMENT_TARGET_SECS * rate as f32) as usize;
    let min_len = (SEGMENT_MIN_SECS * rate as f32) as usize;

    let mut segments = Vec::new();
    let mut seam_gaps = Vec::new();
    let mut pos = 0usize;
    while total - pos > target {
        let window_lo = pos + min_len;
        let window_hi = pos + target;
        let (cut, gap) = cuts
            .iter()
            .rfind(|&&(c, _)| c > window_lo && c <= window_hi)
            .copied()
            .unwrap_or((window_hi, 0.0));
        segments.push((pos, cut));
        seam_gaps.push(gap);
        pos = cut;
    }
    segments.push((pos, total));
    (segments, seam_gaps)
}

/// Find candidate cut points (sample offsets) at silent gaps in the audio.
///
/// Computes per-frame RMS, derives an adaptive silence threshold from the
/// recording's speech level (so it works on quiet lapel-mic audio), and returns
/// each silent run as `(midpoint_sample, silence_duration_secs)` for runs lasting
/// at least [`GAP_MIN_SECS`].
fn find_silence_cuts(samples: &[f32], rate: u32) -> Vec<(usize, f32)> {
    let frame = ((GAP_FRAME_SECS * rate as f32) as usize).max(1);

    let mut rms: Vec<f32> = Vec::with_capacity(samples.len() / frame + 1);
    let mut i = 0;
    while i + frame <= samples.len() {
        let energy: f32 = samples[i..i + frame].iter().map(|s| s * s).sum();
        rms.push((energy / frame as f32).sqrt());
        i += frame;
    }
    if rms.is_empty() {
        return Vec::new();
    }

    let mut sorted = rms.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let speech_level = sorted[((sorted.len() as f32 * 0.90) as usize).min(sorted.len() - 1)];
    let threshold = (speech_level * GAP_REL_THRESHOLD).max(GAP_ABS_FLOOR);
    let min_gap_frames = ((GAP_MIN_SECS / GAP_FRAME_SECS) as usize).max(1);

    let mut cuts = Vec::new();
    let mut run_start: Option<usize> = None;
    for (fi, &v) in rms.iter().enumerate() {
        if v < threshold {
            run_start.get_or_insert(fi);
        } else if let Some(s) = run_start.take() {
            if fi - s >= min_gap_frames {
                let mid = (s + fi) / 2 * frame;
                let dur = (fi - s) as f32 * GAP_FRAME_SECS;
                cuts.push((mid, dur));
            }
        }
    }
    if let Some(s) = run_start {
        if rms.len() - s >= min_gap_frames {
            let mid = (s + rms.len()) / 2 * frame;
            let dur = (rms.len() - s) as f32 * GAP_FRAME_SECS;
            cuts.push((mid, dur));
        }
    }
    cuts
}

/// Write `samples` to `path`, wrapped in leading and trailing silence.
///
/// The padding gives the TDT decoder run-in and run-out room. The audio is
/// copied through unchanged: an earlier version RMS-normalised it (to lift quiet
/// lapel-mic recordings), but with FluidAudio 0.15 that gain shifted the
/// silence-aligned chunk boundaries and dropped the tail of long recordings;
/// the level is left alone.
fn write_padded_wav(samples: &[f32], spec: &hound::WavSpec, path: &Path) -> Result<()> {
    let rate = spec.sample_rate;
    let lead = (LEAD_PAD_SECS * rate as f32) as usize;
    let trail = (TRAIL_PAD_SECS * rate as f32) as usize;

    let mut writer = hound::WavWriter::create(path, *spec)?;
    match spec.sample_format {
        hound::SampleFormat::Int => {
            let scale = ((1i64 << (spec.bits_per_sample - 1)) - 1) as f32;
            for _ in 0..lead {
                writer.write_sample(0i32)?;
            }
            for &s in samples {
                writer.write_sample((s * scale).round().clamp(-scale, scale) as i32)?;
            }
            for _ in 0..trail {
                writer.write_sample(0i32)?;
            }
        }
        hound::SampleFormat::Float => {
            for _ in 0..lead {
                writer.write_sample(0.0f32)?;
            }
            for &s in samples {
                writer.write_sample(s)?;
            }
            for _ in 0..trail {
                writer.write_sample(0.0f32)?;
            }
        }
    }
    writer.finalize()?;
    Ok(())
}

/// Align `seam_gaps` to the segments that actually produced a transcript.
///
/// `kept[i]` is `true` when segment `i` produced a non-empty transcript.
/// `seam_gaps[i]` is the silence duration (seconds) between segment `i` and
/// segment `i+1` (length `kept.len() - 1`).
///
/// Returns the per-seam gaps aligned to the *kept* parts: `result[j]` is the
/// total silence between kept part `j` and kept part `j+1`, folding any
/// dropped (empty) segments between them. `result.len() == (#kept parts) - 1`.
fn align_kept_gaps(kept: &[bool], seam_gaps: &[f32]) -> Vec<f32> {
    let mut out = Vec::new();
    let mut accum = 0.0f32;
    let mut seen_kept = false;
    for i in 0..kept.len() {
        let gap_after = if i + 1 < kept.len() {
            seam_gaps.get(i).copied().unwrap_or(0.0)
        } else {
            0.0
        };
        if kept[i] {
            if seen_kept {
                out.push(accum);
            }
            seen_kept = true;
            accum = gap_after;
        } else {
            accum += gap_after;
        }
    }
    out
}

/// Join segment transcripts into a single string, correcting seam artefacts.
///
/// FluidAudio transcribes each segment independently and truecases the first
/// word as if it were a new sentence. At each seam the joining strategy depends
/// on the silence duration that caused the split:
///
/// - **Short pause** (`gap < SENTENCE_PAUSE_SECS`): mid-sentence split (a breath
///   or brief hesitation). The model may have appended a spurious terminal `.`,
///   `!`, or `?` to the prior segment; that character is stripped. The next
///   segment's first word is then lowercased unless a guard applies (pronoun,
///   acronym, CamelCase proper noun).
///
/// - **Long pause** (`gap >= SENTENCE_PAUSE_SECS`): genuine sentence boundary.
///   Prior punctuation is preserved and the next segment's capitalisation is kept
///   as the model produced it (existing PR #96 behaviour).
///
/// `seam_gaps` must have length `parts.len() - 1`; each entry is the silence
/// duration (seconds) at the corresponding seam. A hard-cut fallback with no
/// detected silence uses `0.0`, which is always treated as a short pause.
pub(crate) fn join_segments(parts: &[String], seam_gaps: &[f32]) -> String {
    if parts.is_empty() {
        return String::new();
    }
    if parts.len() == 1 {
        return parts[0].clone();
    }

    let mut out = String::with_capacity(parts.iter().map(|p| p.len() + 1).sum());
    out.push_str(&parts[0]);

    for (idx, part) in parts[1..].iter().enumerate() {
        let gap = seam_gaps.get(idx).copied().unwrap_or(0.0);
        let is_sentence_boundary = gap >= SENTENCE_PAUSE_SECS;

        if is_sentence_boundary {
            // Long pause — genuine sentence boundary. Determine whether the
            // previous emitted text already ended with terminal punctuation
            // (strip trailing closing brackets/quotes first).
            let prev_trimmed = out.trim_end_matches(|c| {
                matches!(
                    c,
                    '"' | '\''
                        | ')'
                        | ']'
                        | '}'
                        | '\u{201C}'
                        | '\u{201D}'
                        | '\u{2018}'
                        | '\u{2019}'
                )
            });
            let prev_ends_terminal = prev_trimmed
                .chars()
                .next_back()
                .map(|c| matches!(c, '.' | '!' | '?'))
                .unwrap_or(false);

            out.push(' ');
            if prev_ends_terminal {
                // Prior punctuation already present — keep the model's capital.
                out.push_str(part);
            } else {
                // Genuine boundary but no prior punctuation (model omitted it).
                // Keep the model's capital; don't force lowercase.
                out.push_str(part);
            }
        } else {
            // Short pause — mid-sentence split. Strip a trailing terminal
            // punctuation character from the accumulated output (the model
            // appended it because each segment looks like a sentence to it),
            // then lowercase the next segment's first word.
            strip_trailing_terminal(&mut out);
            out.push(' ');

            let first_token = part.split_whitespace().next().unwrap_or("");
            if first_token_needs_lowercase(first_token) {
                let mut chars = part.chars();
                if let Some(first) = chars.next() {
                    out.push(first.to_lowercase().next().unwrap_or(first));
                    out.push_str(chars.as_str());
                }
            } else {
                out.push_str(part);
            }
        }
    }

    out
}

/// Strip a single trailing terminal punctuation character (`.`, `!`, `?`) from
/// `s` in-place, after any trailing closing brackets/quotes.
///
/// Only the innermost terminal character is removed; closing brackets/quotes
/// that follow it remain. This undoes the model's sentence-final punctuation on
/// a segment that was actually split mid-sentence.
fn strip_trailing_terminal(s: &mut String) {
    // Find the index of the last non-closing character.
    let close_chars: &[char] = &[
        '"', '\'', ')', ']', '}', '\u{201C}', '\u{201D}', '\u{2018}', '\u{2019}',
    ];
    let inner = s.trim_end_matches(|c| close_chars.contains(&c));
    if let Some(last) = inner.chars().next_back() {
        if matches!(last, '.' | '!' | '?') {
            // Byte position of `last` in `inner`, then in `s`.
            let inner_len = inner.len();
            let char_len = last.len_utf8();
            let remove_at = inner_len - char_len;
            s.remove(remove_at);
        }
    }
}

/// Return true if the first character of a seam word should be lowercased.
///
/// Guards that suppress lowercasing (any one is sufficient):
/// - Not an ASCII uppercase letter at position 0 (nothing to do).
/// - Token is `I` or starts with `I'` / `I\u{2019}` (English first-person pronoun,
///   with either the ASCII apostrophe U+0027 or the typographic right single quote U+2019).
/// - Token is entirely uppercase letters, length ≥ 2 (acronym).
/// - Token contains an uppercase letter after position 0 (CamelCase / proper noun).
fn first_token_needs_lowercase(token: &str) -> bool {
    let mut chars = token.chars();
    let Some(first) = chars.next() else {
        return false;
    };

    // Guard: first character must be ASCII uppercase.
    if !first.is_ascii_uppercase() {
        return false;
    }

    // Guard: English first-person pronoun `I` or contractions like `I'm` / `I\u{2019}m`.
    // Covers both the ASCII apostrophe (U+0027) and the typographic right single quote (U+2019).
    if token == "I" || token.starts_with("I'") || token.starts_with("I\u{2019}") {
        return false;
    }

    // Guard: all-uppercase token of length ≥ 2 (acronym).
    if token.len() >= 2 && token.chars().all(|c| c.is_ascii_uppercase()) {
        return false;
    }

    // Guard: contains an uppercase letter after position 0 (CamelCase / proper noun).
    if chars.any(|c| c.is_uppercase()) {
        return false;
    }

    true
}

/// Check if FluidAudio model cache has content (models already compiled)
///
/// When cached, `init_asr()` takes ~1s instead of 20-30s.
pub fn is_cached() -> bool {
    let cache_dir = model_cache_directory();
    if !cache_dir.exists() {
        return false;
    }

    // Check if the directory has any files (FluidAudio populates it after first init)
    std::fs::read_dir(&cache_dir)
        .map(|entries| entries.count() > 0)
        .unwrap_or(false)
}

/// Get FluidAudio's model cache directory
///
/// FluidAudio stores compiled CoreML models in:
/// `~/Library/Application Support/FluidAudio/Models/`
pub fn model_cache_directory() -> PathBuf {
    dirs::home_dir()
        .expect("Could not find home directory")
        .join("Library")
        .join("Application Support")
        .join("FluidAudio")
        .join("Models")
}

/// Write a sentinel marker file to the Thoth model directory
///
/// This integrates with Thoth's `check_model_downloaded()` infrastructure.
/// The marker file `.fluidaudio_ready` signals that FluidAudio has been
/// successfully initialised and models are cached.
pub fn write_ready_marker() -> Result<()> {
    let marker_dir = super::manifest::get_model_directory("fluidaudio-parakeet-tdt-coreml");
    std::fs::create_dir_all(&marker_dir)?;

    let marker_path = marker_dir.join(".fluidaudio_ready");
    std::fs::write(
        &marker_path,
        "FluidAudio models cached and ready.\n\
         CoreML cache: ~/Library/Application Support/FluidAudio/Models/\n",
    )?;

    tracing::info!("Wrote FluidAudio ready marker: {}", marker_path.display());
    Ok(())
}

/// Remove the sentinel marker file
///
/// Called when the user "deletes" the FluidAudio model from Model Manager.
pub fn remove_ready_marker() -> Result<()> {
    let marker_dir = super::manifest::get_model_directory("fluidaudio-parakeet-tdt-coreml");
    let marker_path = marker_dir.join(".fluidaudio_ready");

    if marker_path.exists() {
        std::fs::remove_file(&marker_path)?;
        tracing::info!("Removed FluidAudio ready marker: {}", marker_path.display());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_cache_directory() {
        let dir = model_cache_directory();
        assert!(dir.to_string_lossy().contains("FluidAudio"));
        assert!(dir.to_string_lossy().contains("Models"));
    }

    #[test]
    fn test_is_cached() {
        // May or may not be cached depending on environment
        let _result = is_cached();
    }

    /// A recording within the single-shot limit is transcribed as one unit.
    #[test]
    fn test_plan_segments_short_is_single() {
        let rate = 16_000u32;
        let samples = vec![0.1f32; (10.0 * rate as f32) as usize]; // 10 s
        let (segs, gaps) = plan_segments(&samples, rate);
        assert_eq!(segs, vec![(0, samples.len())]);
        assert!(gaps.is_empty());
    }

    /// A long recording is split into several units, each within the target
    /// length, and the segments tile the whole recording with no gaps.
    #[test]
    fn test_plan_segments_long_splits_and_covers() {
        let rate = 16_000u32;
        let frame = (GAP_FRAME_SECS * rate as f32) as usize;
        // 60 s alternating 2 s speech / 0.5 s silence so there are real pauses.
        let mut samples = Vec::new();
        while samples.len() < (60.0 * rate as f32) as usize {
            let speech_frames = (2.0 * rate as f32) as usize;
            let silence_frames = (0.5 * rate as f32) as usize;
            samples.extend(std::iter::repeat_n(0.3f32, speech_frames));
            samples.extend(std::iter::repeat_n(0.0f32, silence_frames));
        }
        let (segs, gaps) = plan_segments(&samples, rate);
        assert!(segs.len() > 1, "long recording should split");
        assert_eq!(gaps.len(), segs.len() - 1, "one gap per seam");
        let target = (SEGMENT_TARGET_SECS * rate as f32) as usize;
        assert_eq!(segs.first().unwrap().0, 0);
        assert_eq!(segs.last().unwrap().1, samples.len());
        for (i, &(a, b)) in segs.iter().enumerate() {
            assert!(b > a, "segment must be non-empty");
            if i + 1 < segs.len() {
                assert_eq!(b, segs[i + 1].0, "segments must be contiguous");
                assert!(b - a <= target + frame, "non-final segment within target");
            }
        }
    }

    /// Silence-gap detection finds the pause between two speech bursts and
    /// returns both the midpoint sample and the duration.
    #[test]
    fn test_find_silence_cuts_detects_pause() {
        let rate = 16_000u32;
        let mut samples = vec![0.3f32; (2.0 * rate as f32) as usize]; // 2 s speech
        samples.extend(vec![0.0f32; (0.5 * rate as f32) as usize]); // 0.5 s silence
        samples.extend(vec![0.3f32; (2.0 * rate as f32) as usize]); // 2 s speech
        let cuts = find_silence_cuts(&samples, rate);
        assert_eq!(cuts.len(), 1, "exactly one gap");
        let (midpoint, dur) = cuts[0];
        let cut_secs = midpoint as f32 / rate as f32;
        assert!(
            (2.0..2.5).contains(&cut_secs),
            "cut should fall inside the pause, got {cut_secs}s"
        );
        assert!(
            (0.4..0.6).contains(&dur),
            "silence duration should be ~0.5 s, got {dur}s"
        );
    }

    /// Padding writes a readable WAV whose length is the input plus the pads.
    #[test]
    fn test_write_padded_wav_roundtrip() {
        let rate = 16_000u32;
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let samples = vec![0.2f32; rate as usize]; // 1 s
        let tmp = std::env::temp_dir().join("thoth_pad_test.wav");
        write_padded_wav(&samples, &spec, &tmp).unwrap();

        let (read_back, _) = read_wav_mono(&tmp).unwrap();
        let expected = samples.len()
            + (LEAD_PAD_SECS * rate as f32) as usize
            + (TRAIL_PAD_SECS * rate as f32) as usize;
        assert_eq!(read_back.len(), expected);
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    #[ignore] // Run with: cargo test --features fluidaudio -- --ignored
    fn test_service_creation() {
        let result = TranscriptionService::new();
        match result {
            Ok(_service) => println!("FluidAudio service created successfully"),
            Err(e) => println!("FluidAudio service creation failed: {}", e),
        }
    }

    // --- join_segments tests ---

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|&x| x.to_string()).collect()
    }

    // Short-gap seams: below SENTENCE_PAUSE_SECS (use 0.3 s < 0.6 s).
    fn short_gap() -> Vec<f32> {
        vec![0.3]
    }

    // Long-gap seams: at or above SENTENCE_PAUSE_SECS (use 0.8 s >= 0.6 s).
    fn long_gap() -> Vec<f32> {
        vec![0.8]
    }

    /// Empty input returns empty string.
    #[test]
    fn test_join_segments_empty() {
        assert_eq!(join_segments(&[], &[]), "");
    }

    /// Single part is returned unchanged (no seam to correct).
    #[test]
    fn test_join_segments_single_part_unchanged() {
        assert_eq!(
            join_segments(&s(&["Going through the repo"]), &[]),
            "Going through the repo"
        );
    }

    // --- Short-gap seam tests (breath / mid-sentence split) ---

    /// Short gap + spurious segment-final period stripped, next word lowercased.
    /// This is the core bug the fix addresses: "…this. Recording and see…"
    /// should become "…this recording and see…".
    #[test]
    fn test_join_segments_short_gap_strips_period_and_lowercases() {
        let parts = s(&["take a look at this.", "Recording and see what happens"]);
        let result = join_segments(&parts, &short_gap());
        assert_eq!(
            result, "take a look at this recording and see what happens",
            "got: {result}"
        );
    }

    /// Short gap without a spurious period still lowercases the next word.
    #[test]
    fn test_join_segments_short_gap_no_period_lowercases() {
        let parts = s(&["I'm talking about", "Going through the repo"]);
        let result = join_segments(&parts, &short_gap());
        assert!(
            result.contains("about going through"),
            "expected 'about going through', got: {result}"
        );
    }

    /// Short gap: pronoun `I` is never lowercased even at a short-gap seam.
    #[test]
    fn test_join_segments_short_gap_pronoun_i_preserved() {
        let parts = s(&["and then.", "I went home"]);
        let result = join_segments(&parts, &short_gap());
        assert!(
            result.contains("then I went"),
            "expected 'then I went', got: {result}"
        );
    }

    /// Short gap: acronym guard survives even when the prior segment had a period.
    #[test]
    fn test_join_segments_short_gap_acronym_preserved() {
        let parts = s(&["it hits the.", "API and fails"]);
        let result = join_segments(&parts, &short_gap());
        assert!(
            result.contains("the API and"),
            "expected 'the API and', got: {result}"
        );
    }

    // --- Long-gap seam tests (genuine sentence boundary) ---

    /// Long pause — genuine boundary — preserves the capital even when prior
    /// segment ended with `.`.
    #[test]
    fn test_join_segments_long_gap_preserves_cap_after_period() {
        let parts = s(&["That's the plan.", "Going forward we build."]);
        let result = join_segments(&parts, &long_gap());
        assert!(
            result.contains("plan. Going forward"),
            "expected 'plan. Going forward', got: {result}"
        );
    }

    /// Long pause with NO prior terminal punctuation: model's capital is kept
    /// (we trust the model's sentence detection at genuine boundaries).
    #[test]
    fn test_join_segments_long_gap_no_prior_punct_keeps_cap() {
        let parts = s(&["end of thought", "Beginning of next"]);
        let result = join_segments(&parts, &long_gap());
        assert!(
            result.contains("thought Beginning"),
            "expected 'thought Beginning', got: {result}"
        );
    }

    // --- Tests retained from PR #96 (updated for new signature) ---

    /// Pronoun guard: `I` at a seam is never lowercased.
    #[test]
    fn test_join_segments_pronoun_i_preserved() {
        let parts = s(&["and then", "I went home"]);
        let result = join_segments(&parts, &short_gap());
        assert!(
            result.contains("then I went"),
            "expected 'then I went', got: {result}"
        );
    }

    /// Pronoun contraction guard: `I'm`, `I'll`, etc. are never lowercased.
    #[test]
    fn test_join_segments_pronoun_contraction_preserved() {
        let parts = s(&["and then", "I'm not sure"]);
        let result = join_segments(&parts, &short_gap());
        assert!(
            result.contains("then I'm not"),
            "expected 'then I'm not', got: {result}"
        );
    }

    /// Acronym guard: all-uppercase token of length ≥ 2 is kept uppercase.
    #[test]
    fn test_join_segments_acronym_preserved() {
        let parts = s(&["it hits the", "API and fails"]);
        let result = join_segments(&parts, &short_gap());
        assert!(
            result.contains("the API and"),
            "expected 'the API and', got: {result}"
        );
    }

    /// CamelCase guard: token with internal uppercase is left unchanged.
    #[test]
    fn test_join_segments_camel_case_preserved() {
        let parts = s(&["it broke", "FluidAudio crashed"]);
        let result = join_segments(&parts, &short_gap());
        assert!(
            result.contains("broke FluidAudio"),
            "expected 'broke FluidAudio', got: {result}"
        );
    }

    /// Closing quote after terminal punctuation still counts as terminal (long gap).
    #[test]
    fn test_join_segments_closing_quote_terminal() {
        let parts = s(&["\"done.\"", "Next thing we do"]);
        let result = join_segments(&parts, &long_gap());
        assert!(
            result.contains("\"done.\" Next thing"),
            "expected '\"done.\" Next thing', got: {result}"
        );
    }

    /// `!` counts as terminal punctuation at a long gap.
    #[test]
    fn test_join_segments_exclamation_terminal() {
        let parts = s(&["Great!", "Now let's proceed"]);
        let result = join_segments(&parts, &long_gap());
        assert!(
            result.contains("Great! Now"),
            "expected 'Great! Now', got: {result}"
        );
    }

    /// `?` counts as terminal punctuation at a long gap.
    #[test]
    fn test_join_segments_question_mark_terminal() {
        let parts = s(&["Is that right?", "Seems so"]);
        let result = join_segments(&parts, &long_gap());
        assert!(
            result.contains("right? Seems"),
            "expected 'right? Seems', got: {result}"
        );
    }

    /// Comma is NOT terminal; capital after comma is lowercased (short gap, no period to strip).
    #[test]
    fn test_join_segments_comma_not_terminal() {
        let parts = s(&["well, anyway,", "That's the situation"]);
        let result = join_segments(&parts, &short_gap());
        assert!(
            result.contains("anyway, that's"),
            "expected 'anyway, that's', got: {result}"
        );
    }

    /// Three or more parts are all joined correctly (all short gaps).
    #[test]
    fn test_join_segments_multiple_parts() {
        let parts = s(&["first part", "Second part", "Third part"]);
        let gaps = vec![0.3, 0.3];
        let result = join_segments(&parts, &gaps);
        assert_eq!(result, "first part second part third part");
    }

    /// 2-char all-uppercase acronym `OK` at a seam is preserved.
    #[test]
    fn test_join_segments_ok_acronym_preserved() {
        let parts = s(&["that sounds", "OK for now"]);
        let result = join_segments(&parts, &short_gap());
        assert!(
            result.contains("sounds OK for"),
            "expected 'sounds OK for', got: {result}"
        );
    }

    /// Mixed-case acronym `TVs` (CamelCase guard) at a seam is preserved.
    #[test]
    fn test_join_segments_tvs_camel_preserved() {
        let parts = s(&["the old", "TVs were huge"]);
        let result = join_segments(&parts, &short_gap());
        assert!(
            result.contains("old TVs were"),
            "expected 'old TVs were', got: {result}"
        );
    }

    /// Typographic apostrophe contraction `I\u{2019}m` at a seam is preserved.
    #[test]
    fn test_join_segments_typographic_apostrophe_contraction_preserved() {
        let parts = s(&["and then", "I\u{2019}m not sure"]);
        let result = join_segments(&parts, &short_gap());
        assert!(
            result.contains("then I\u{2019}m not"),
            "expected 'then I\u{2019}m not', got: {result}"
        );
    }

    // --- strip_trailing_terminal unit tests ---

    #[test]
    fn test_strip_trailing_terminal_removes_period() {
        let mut s = "hello.".to_string();
        strip_trailing_terminal(&mut s);
        assert_eq!(s, "hello");
    }

    #[test]
    fn test_strip_trailing_terminal_removes_exclamation() {
        let mut s = "wow!".to_string();
        strip_trailing_terminal(&mut s);
        assert_eq!(s, "wow");
    }

    #[test]
    fn test_strip_trailing_terminal_removes_question() {
        let mut s = "really?".to_string();
        strip_trailing_terminal(&mut s);
        assert_eq!(s, "really");
    }

    #[test]
    fn test_strip_trailing_terminal_no_terminal_unchanged() {
        let mut s = "no punct".to_string();
        strip_trailing_terminal(&mut s);
        assert_eq!(s, "no punct");
    }

    #[test]
    fn test_strip_trailing_terminal_comma_unchanged() {
        let mut s = "anyway,".to_string();
        strip_trailing_terminal(&mut s);
        assert_eq!(s, "anyway,");
    }

    #[test]
    fn test_strip_trailing_terminal_period_before_quote() {
        // "done." → removes the period that precedes the closing quote
        let mut s = "\"done.\"".to_string();
        strip_trailing_terminal(&mut s);
        assert_eq!(s, "\"done\"");
    }

    #[test]
    fn test_strip_trailing_terminal_empty_unchanged() {
        let mut s = String::new();
        strip_trailing_terminal(&mut s);
        assert_eq!(s, "");
    }

    // --- align_kept_gaps tests ---

    /// All segments kept: gaps pass through unchanged (the bug case — must NOT accumulate).
    #[test]
    fn test_align_kept_gaps_all_kept() {
        assert_eq!(
            align_kept_gaps(&[true, true, true], &[0.3, 0.4]),
            vec![0.3, 0.4]
        );
    }

    /// Middle segment empty: its gaps on both sides fold into one.
    #[test]
    fn test_align_kept_gaps_middle_empty() {
        let result = align_kept_gaps(&[true, false, true], &[0.3, 0.4]);
        assert_eq!(result.len(), 1);
        assert!(
            (result[0] - 0.7).abs() < 1e-5,
            "expected ~0.7, got {}",
            result[0]
        );
    }

    /// Leading empty segment: no seam before the first kept part.
    #[test]
    fn test_align_kept_gaps_leading_empty() {
        assert_eq!(
            align_kept_gaps(&[false, true, true], &[0.3, 0.4]),
            vec![0.4]
        );
    }

    /// Single kept segment: no seams.
    #[test]
    fn test_align_kept_gaps_single_kept() {
        assert_eq!(align_kept_gaps(&[true], &[]), Vec::<f32>::new());
    }

    /// Two kept segments: one gap passed through.
    #[test]
    fn test_align_kept_gaps_two_kept() {
        assert_eq!(align_kept_gaps(&[true, true], &[0.5]), vec![0.5]);
    }
}
