//! Whisper transcription using whisper.cpp with GPU acceleration
//!
//! This is the primary transcription backend for maximum performance.
//! Uses Metal GPU on macOS and Vulkan on Linux for "snappy as fuck" transcription.

use anyhow::{Result, anyhow};
use std::path::{Path, PathBuf};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

/// Transcription service using Whisper with GPU acceleration
pub struct WhisperTranscriptionService {
    ctx: WhisperContext,
}

impl WhisperTranscriptionService {
    /// Create a new whisper transcription service
    ///
    /// Expects a path to a ggml whisper model file (e.g., ggml-large-v3-turbo.bin)
    ///
    /// GPU acceleration is enabled by default:
    /// - macOS: Uses Metal GPU (always available)
    /// - Linux: Uses CUDA, HIP/ROCm, or Vulkan if compiled with the respective feature
    /// - If GPU initialization fails on Linux, falls back to CPU automatically
    pub fn new(model_path: &Path) -> Result<Self> {
        if !model_path.exists() {
            return Err(anyhow!("Whisper model not found: {}", model_path.display()));
        }

        tracing::info!("Loading Whisper model from {}", model_path.display());

        let model_str = model_path.to_str().ok_or_else(|| {
            anyhow!(
                "Model path contains invalid UTF-8: {}",
                model_path.display()
            )
        })?;

        // Try GPU first, fall back to CPU if it fails
        let ctx = Self::try_load_with_gpu(model_str).or_else(|e| {
            tracing::warn!("GPU initialization failed: {:?}, trying CPU fallback", e);
            Self::load_with_cpu(model_str)
        })?;

        Ok(Self { ctx })
    }

    /// Try to load the model with GPU acceleration
    #[cfg(target_os = "macos")]
    fn try_load_with_gpu(model_str: &str) -> Result<WhisperContext> {
        let mut params = WhisperContextParameters::default();
        params.use_gpu(true);

        let ctx = WhisperContext::new_with_params(model_str, params)
            .map_err(|e| anyhow!("Failed to load Whisper model with Metal: {:?}", e))?;

        tracing::info!("Whisper model loaded with Metal GPU acceleration");
        Ok(ctx)
    }

    /// Try to load the model with GPU acceleration (Linux)
    #[cfg(target_os = "linux")]
    fn try_load_with_gpu(model_str: &str) -> Result<WhisperContext> {
        let mut params = WhisperContextParameters::default();
        params.use_gpu(true);

        let ctx = WhisperContext::new_with_params(model_str, params)
            .map_err(|e| anyhow!("Failed to load Whisper model with GPU: {:?}", e))?;

        // Log the compiled backend (single source of truth: GpuBackendType::compiled).
        let backend = crate::platform::GpuBackendType::compiled();
        if backend == crate::platform::GpuBackendType::Cpu {
            tracing::info!(
                "Whisper model loaded with CPU backend (no GPU feature enabled). For GPU \
                 acceleration, build with --features vulkan (vendor-neutral), cuda (NVIDIA), or \
                 hipblas (AMD). Release Linux builds ship with Vulkan."
            );
        } else {
            tracing::info!("Whisper model loaded with {backend} GPU acceleration");
        }

        Ok(ctx)
    }

    /// Try to load the model with GPU acceleration (Windows)
    #[cfg(target_os = "windows")]
    fn try_load_with_gpu(model_str: &str) -> Result<WhisperContext> {
        let mut params = WhisperContextParameters::default();
        params.use_gpu(true);

        let ctx = WhisperContext::new_with_params(model_str, params)
            .map_err(|e| anyhow!("Failed to load Whisper model with GPU: {:?}", e))?;

        #[cfg(feature = "cuda")]
        tracing::info!("Whisper model loaded with CUDA GPU acceleration");
        #[cfg(not(feature = "cuda"))]
        tracing::info!("Whisper model loaded (GPU acceleration requires --features cuda)");

        Ok(ctx)
    }

    /// Load the model with CPU only (fallback)
    fn load_with_cpu(model_str: &str) -> Result<WhisperContext> {
        let mut params = WhisperContextParameters::default();
        params.use_gpu(false);

        let ctx = WhisperContext::new_with_params(model_str, params)
            .map_err(|e| anyhow!("Failed to load Whisper model with CPU: {:?}", e))?;

        tracing::info!("Whisper model loaded with CPU backend (GPU fallback)");
        Ok(ctx)
    }

    /// Transcribe audio from a WAV file
    ///
    /// The file should be 16kHz mono WAV. Returns the transcribed text.
    pub fn transcribe(&self, audio_path: &Path) -> Result<String> {
        let (samples, sample_rate) = crate::audio::wav::read_mono_f32(audio_path)?;

        tracing::info!(
            "Loaded audio: {} samples at {}Hz ({:.2}s)",
            samples.len(),
            sample_rate,
            samples.len() as f32 / sample_rate as f32
        );

        // Check audio levels for debugging
        let max_level = samples.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        let rms = (samples.iter().map(|s| s * s).sum::<f32>() / samples.len().max(1) as f32).sqrt();
        tracing::info!(
            "Audio levels - max: {:.4}, RMS: {:.4} (dB: {:.1})",
            max_level,
            rms,
            20.0 * (rms + 1e-10).log10()
        );

        if max_level < 0.001 {
            tracing::warn!(
                "Audio appears to be silent or extremely quiet (max level: {})",
                max_level
            );
        }

        if sample_rate != 16000 {
            tracing::warn!(
                "Audio sample rate is {}Hz, expected 16000Hz. Results may be affected.",
                sample_rate
            );
        }

        // Trim leading silence and pad with silence so the model can initialise
        // before the first word and finalise after the last.
        let samples = crate::audio::wav::trim_and_pad(&samples, sample_rate);

        self.transcribe_samples(&samples)
    }

    /// Transcribe audio samples directly
    ///
    /// Samples should be 16kHz f32 mono audio.
    pub fn transcribe_samples(&self, samples: &[f32]) -> Result<String> {
        self.transcribe_samples_biased(samples, super::bias::vocabulary_bias().as_deref())
    }

    /// Transcribe with an explicit decode bias, rather than the user's own.
    ///
    /// The bias is a parameter and not a global read so that the effect of one
    /// can be measured against the same audio without it (#106).
    pub fn transcribe_samples_biased(&self, samples: &[f32], bias: Option<&str>) -> Result<String> {
        let start = std::time::Instant::now();

        // Create a state for this transcription
        let mut state = self
            .ctx
            .create_state()
            .map_err(|e| anyhow!("Failed to create whisper state: {:?}", e))?;

        // Configure transcription parameters
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });

        // Bias decoding toward the user's own vocabulary (#106). Post-hoc
        // dictionary and canonical correction still runs afterwards: it repairs
        // what the model got close to, this helps it choose correctly first.
        //
        // whisper-rs turns the prompt into a raw pointer and `FullParams` has no
        // `Drop`, so each call leaks its own copy — a few hundred bytes per
        // dictation, capped by `bias::MAX_PROMPT_CHARS`, and only when the user
        // has terms configured.
        if let Some(prompt) = bias {
            tracing::debug!("Biasing decoding with {} chars of vocabulary", prompt.len());
            params.set_initial_prompt(prompt);
        }

        // English-only is a deliberate current scope limit, not an oversight
        // (#102). Whisper's language auto-detection costs an extra inference
        // pass before decoding, which is a latency hit on the hot path, so the
        // language is pinned rather than detected.
        //
        // A `transcription.language` config field used to sit alongside this
        // and was never read; it was removed rather than left looking
        // functional. Making this configurable means resolving an effective
        // language per request and adding UI for it: reopen #102 if multilingual
        // transcription is wanted.
        params.set_language(Some("en"));

        // Disable translation, we want transcription. Same scope note as above:
        // the bundled large-v3-turbo model is translate-capable, but exposing it
        // needs a user-facing toggle, not a silent default.
        params.set_translate(false);

        // No timestamps needed for basic transcription
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);

        // Single segment mode for faster processing
        params.set_single_segment(false);

        // Run transcription
        state
            .full(params, samples)
            .map_err(|e| anyhow!("Transcription failed: {:?}", e))?;

        // Collect results using the iterator API
        let mut text = String::new();
        for segment in state.as_iter() {
            if let Ok(segment_text) = segment.to_str() {
                if !text.is_empty() {
                    text.push(' ');
                }
                text.push_str(segment_text);
            }
        }

        let duration = start.elapsed();
        let audio_duration = samples.len() as f32 / 16000.0;
        let rtf = duration.as_secs_f32() / audio_duration;

        tracing::info!(
            "Transcribed {:.2}s audio in {:.2}s (RTF: {:.3})",
            audio_duration,
            duration.as_secs_f32(),
            rtf
        );

        tracing::info!(
            "Transcription result: '{}' ({} chars)",
            text.trim(),
            text.len()
        );

        Ok(text.trim().to_string())
    }
}

/// Get the default whisper model directory path
pub fn get_whisper_model_directory() -> PathBuf {
    crate::config::data_dir().join("models").join("whisper")
}

/// Get the path to a specific whisper model
pub fn get_whisper_model_path(model_id: &str) -> PathBuf {
    get_whisper_model_directory().join(format!("{}.bin", model_id))
}

/// Check if a whisper model is downloaded
pub fn is_whisper_model_downloaded(model_id: &str) -> bool {
    get_whisper_model_path(model_id).exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_whisper_model_directory() {
        let dir = get_whisper_model_directory();
        assert!(dir.ends_with("whisper"));
        assert!(dir.to_string_lossy().contains(".thoth"));
    }

    #[test]
    fn test_get_whisper_model_path() {
        let path = get_whisper_model_path("ggml-large-v3-turbo");
        assert!(path.to_string_lossy().contains("ggml-large-v3-turbo.bin"));
    }
}
