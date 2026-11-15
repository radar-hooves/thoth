//! Parakeet TDT transcription using Sherpa-ONNX
//!
//! Uses the Parakeet V3 NeMo Transducer model for high-quality
//! offline speech recognition.
//!
//! Requires the `parakeet` Cargo feature to be enabled.

use anyhow::{Result, anyhow};
use sherpa_onnx::{OfflineRecognizer, OfflineRecognizerConfig, OfflineTransducerModelConfig};
use std::path::{Path, PathBuf};

/// Transcription service using Parakeet TDT model
pub struct TranscriptionService {
    recognizer: OfflineRecognizer,
}

impl TranscriptionService {
    /// Create a new transcription service with models from the given directory
    ///
    /// Expects the following files in the model directory:
    /// - `encoder.int8.onnx`
    /// - `decoder.int8.onnx`
    /// - `joiner.int8.onnx`
    /// - `tokens.txt`
    pub fn new(model_dir: &Path) -> Result<Self> {
        if !model_dir.exists() {
            return Err(anyhow!(
                "Model directory does not exist: {}",
                model_dir.display()
            ));
        }

        // Check for required files
        let encoder = model_dir.join("encoder.int8.onnx");
        let decoder = model_dir.join("decoder.int8.onnx");
        let joiner = model_dir.join("joiner.int8.onnx");
        let tokens = model_dir.join("tokens.txt");

        for path in [&encoder, &decoder, &joiner, &tokens] {
            if !path.exists() {
                return Err(anyhow!("Required model file not found: {}", path.display()));
            }
        }

        tracing::info!("Loading Parakeet model from {}", model_dir.display());

        let encoder_str = encoder.to_string_lossy().to_string();
        let decoder_str = decoder.to_string_lossy().to_string();
        let joiner_str = joiner.to_string_lossy().to_string();
        let tokens_str = tokens.to_string_lossy().to_string();
        let num_threads = num_cpus::get().min(8) as i32;

        let build_config = |provider: Option<String>| {
            let mut config = OfflineRecognizerConfig::default();
            config.model_config.transducer = OfflineTransducerModelConfig {
                encoder: Some(encoder_str.clone()),
                decoder: Some(decoder_str.clone()),
                joiner: Some(joiner_str.clone()),
            };
            config.model_config.tokens = Some(tokens_str.clone());
            config.model_config.model_type = Some("nemo_transducer".into());
            config.model_config.num_threads = num_threads;
            config.model_config.debug = false;
            config.model_config.provider = provider;
            config.decoding_method = Some("greedy_search".into());
            config
        };

        // Try CoreML on macOS for GPU acceleration, fall back to CPU if it fails.
        // OfflineRecognizer::create returns Option — None means init failed.
        #[cfg(target_os = "macos")]
        let recognizer = {
            tracing::info!("Attempting CoreML provider for GPU acceleration");
            match OfflineRecognizer::create(&build_config(Some("coreml".into()))) {
                Some(r) => {
                    tracing::info!("CoreML provider initialised successfully");
                    r
                }
                None => {
                    tracing::warn!("CoreML provider failed, falling back to CPU");
                    OfflineRecognizer::create(&build_config(Some("cpu".into()))).ok_or_else(
                        || anyhow!("Failed to create Parakeet recognizer with CPU provider"),
                    )?
                }
            }
        };

        // Linux/Windows: with the `parakeet-cuda` feature, try the CUDA execution
        // provider (NVIDIA GPU) first and fall back to CPU. Without it, CPU only.
        // Note: onnxruntime silently falls back to CPU if the CUDA EP/libs aren't
        // present, so confirm real GPU use via `nvidia-smi` during a transcription.
        #[cfg(all(not(target_os = "macos"), feature = "parakeet-cuda"))]
        let recognizer = {
            tracing::info!("Attempting CUDA provider for GPU acceleration");
            match OfflineRecognizer::create(&build_config(Some("cuda".into()))) {
                Some(r) => {
                    tracing::info!("CUDA provider initialised (verify GPU use with nvidia-smi)");
                    r
                }
                None => {
                    tracing::warn!("CUDA provider failed, falling back to CPU");
                    OfflineRecognizer::create(&build_config(Some("cpu".into())))
                        .ok_or_else(|| anyhow!("Failed to create Parakeet recognizer with CPU"))?
                }
            }
        };

        #[cfg(all(not(target_os = "macos"), not(feature = "parakeet-cuda")))]
        let recognizer = {
            tracing::info!("Using CPU provider");
            OfflineRecognizer::create(&build_config(Some("cpu".into())))
                .ok_or_else(|| anyhow!("Failed to create Parakeet recognizer"))?
        };

        tracing::info!("Parakeet model loaded successfully");
        Ok(Self { recognizer })
    }

    /// Transcribe audio from a WAV file
    ///
    /// The file should be 16kHz mono WAV. Returns the transcribed text.
    pub fn transcribe(&mut self, audio_path: &Path) -> Result<String> {
        let (samples, sample_rate) = crate::audio::wav::read_mono_f32(audio_path)?;

        tracing::info!(
            "Loaded audio: {} samples at {}Hz ({:.2}s), path: {}",
            samples.len(),
            sample_rate,
            samples.len() as f32 / sample_rate as f32,
            audio_path.display()
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

        let start = std::time::Instant::now();
        let text = self.transcribe_samples(&samples);
        let duration = start.elapsed();

        let audio_duration = samples.len() as f32 / sample_rate as f32;
        let rtf = duration.as_secs_f32() / audio_duration;

        tracing::info!(
            "Transcribed {:.2}s audio in {:.2}s (RTF: {:.3})",
            audio_duration,
            duration.as_secs_f32(),
            rtf
        );

        tracing::info!("Transcription result: '{}' ({} chars)", text, text.len());

        Ok(text)
    }

    /// Transcribe audio samples directly
    ///
    /// Samples should be 16kHz f32 mono audio.
    pub fn transcribe_samples(&mut self, samples: &[f32]) -> String {
        let stream = self.recognizer.create_stream();
        stream.accept_waveform(16000, samples);
        self.recognizer.decode(&stream);
        stream.get_result().map(|r| r.text).unwrap_or_default()
    }
}

/// Get the default model directory path
pub fn get_model_directory() -> PathBuf {
    crate::config::data_dir().join("models").join("parakeet")
}

/// Check if the model is downloaded
pub fn is_model_downloaded() -> bool {
    let model_dir = get_model_directory();
    model_dir.join("encoder.int8.onnx").exists()
        && model_dir.join("decoder.int8.onnx").exists()
        && model_dir.join("joiner.int8.onnx").exists()
        && model_dir.join("tokens.txt").exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_model_directory() {
        let dir = get_model_directory();
        assert!(dir.ends_with("parakeet"));
        assert!(dir.to_string_lossy().contains(".thoth"));
    }

    #[test]
    fn test_is_model_downloaded() {
        // May or may not be downloaded depending on environment
        let _result = is_model_downloaded();
    }

    #[test]
    fn test_service_creation_missing_model() {
        let result = TranscriptionService::new(Path::new("/nonexistent/path"));
        assert!(result.is_err());
    }

    // Integration test - only runs if model is downloaded
    #[test]
    #[ignore] // Run with: cargo test -- --ignored
    fn test_transcription() {
        if !is_model_downloaded() {
            println!("Model not downloaded, skipping test");
            return;
        }

        let model_dir = get_model_directory();
        let _service = TranscriptionService::new(&model_dir).expect("Failed to create service");

        // Would need a test audio file to actually test transcription
        println!("Service created successfully");
    }
}
