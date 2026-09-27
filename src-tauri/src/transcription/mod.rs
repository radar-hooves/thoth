//! Transcription subsystem with dual backends
//!
//! Primary: whisper.cpp with Metal GPU acceleration (fastest)
//! Fallback: Sherpa-ONNX with Parakeet models (cross-platform)

mod au_spelling_map;
pub mod bias;
pub mod download;
pub mod filter;
#[cfg(all(target_os = "macos", feature = "fluidaudio"))]
pub mod fluidaudio;
pub mod gate;
pub mod ggml_header;
pub mod manifest;
#[cfg(feature = "parakeet")]
pub mod parakeet;
pub mod whisper;

pub use filter::{FilterOptions, OutputFilter};
pub use gate::Priority;
pub use manifest::{ModelInfo, fetch_model_manifest, get_manifest_update_time};

use crate::error::Error;
use parking_lot::Mutex;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Transcription backend type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TranscriptionBackend {
    /// Whisper with Metal GPU acceleration (primary, fastest)
    #[default]
    Whisper,
    /// Sherpa-ONNX with Parakeet models (fallback)
    Parakeet,
    /// FluidAudio with Apple Neural Engine via CoreML (fastest on Apple Silicon)
    FluidAudio,
}

/// Unified transcription service that can use either backend
pub enum TranscriptionService {
    Whisper(whisper::WhisperTranscriptionService),
    #[cfg(feature = "parakeet")]
    Parakeet(parakeet::TranscriptionService),
    #[cfg(all(target_os = "macos", feature = "fluidaudio"))]
    FluidAudio(fluidaudio::TranscriptionService),
}

impl TranscriptionService {
    /// Create a new transcription service with the whisper backend
    pub fn new_whisper(model_path: &std::path::Path) -> anyhow::Result<Self> {
        let service = whisper::WhisperTranscriptionService::new(model_path)?;
        Ok(Self::Whisper(service))
    }

    /// Create a new transcription service with the parakeet backend
    #[cfg(feature = "parakeet")]
    pub fn new_parakeet(model_dir: &std::path::Path) -> anyhow::Result<Self> {
        let service = parakeet::TranscriptionService::new(model_dir)?;
        Ok(Self::Parakeet(service))
    }

    /// Create a new transcription service with the FluidAudio backend (Apple Neural Engine)
    #[cfg(all(target_os = "macos", feature = "fluidaudio"))]
    pub fn new_fluidaudio() -> anyhow::Result<Self> {
        let service = fluidaudio::TranscriptionService::new()?;
        Ok(Self::FluidAudio(service))
    }

    /// Transcribe audio from a WAV file
    pub fn transcribe(&mut self, audio_path: &std::path::Path) -> anyhow::Result<String> {
        match self {
            Self::Whisper(service) => service.transcribe(audio_path),
            #[cfg(feature = "parakeet")]
            Self::Parakeet(service) => service.transcribe(audio_path),
            #[cfg(all(target_os = "macos", feature = "fluidaudio"))]
            Self::FluidAudio(service) => service.transcribe(audio_path),
        }
    }

    /// Get the backend type
    pub fn backend(&self) -> TranscriptionBackend {
        match self {
            Self::Whisper(_) => TranscriptionBackend::Whisper,
            #[cfg(feature = "parakeet")]
            Self::Parakeet(_) => TranscriptionBackend::Parakeet,
            #[cfg(all(target_os = "macos", feature = "fluidaudio"))]
            Self::FluidAudio(_) => TranscriptionBackend::FluidAudio,
        }
    }
}

/// Global transcription service instance
static TRANSCRIPTION_SERVICE: OnceLock<Mutex<Option<TranscriptionService>>> = OnceLock::new();

fn get_service() -> &'static Mutex<Option<TranscriptionService>> {
    TRANSCRIPTION_SERVICE.get_or_init(|| Mutex::new(None))
}

/// Whether the most recent warmup attempt finished without loading any usable
/// model (the selected backend *and* the Whisper fallback both failed).
///
/// This is the difference between "a model is configured / on disk" and "a model
/// can actually transcribe". The record guard and the pipeline's load-wait read
/// it so they can block (or bail fast) instead of recording into a void and then
/// hanging for 60 s on a model that will never load.
static WARMUP_FAILED: AtomicBool = AtomicBool::new(false);

/// Returns `true` when the last warmup attempt loaded no usable transcription model.
pub fn warmup_failed() -> bool {
    WARMUP_FAILED.load(Ordering::SeqCst)
}

/// Whether a model has ever loaded in this process.
///
/// [`WARMUP_FAILED`] used to be equivalent to "nothing can load on this
/// machine", because a model was loaded at most once per process. Since the
/// idle unload (#105) a warmup can also fail while RE-loading a model that was
/// working minutes ago, and refusing to record from then until a restart turns a
/// transient fault into a permanent one. Recording is blocked outright only when
/// nothing has ever loaded; a failed reload gets another attempt.
static MODEL_HAS_LOADED: AtomicBool = AtomicBool::new(false);

/// Returns `true` once a transcription model has loaded at least once here.
pub fn model_has_loaded() -> bool {
    MODEL_HAS_LOADED.load(Ordering::SeqCst)
}

fn init_whisper_transcription_unlocked(model_path: String) -> Result<(), Error> {
    let service =
        TranscriptionService::new_whisper(&PathBuf::from(model_path)).map_err(|e| e.to_string())?;

    let mut guard = get_service().lock();
    *guard = Some(service);
    touch_model_used();

    tracing::info!(
        "Whisper transcription service initialised ({} backend)",
        crate::platform::GpuBackendType::compiled()
    );
    Ok(())
}

/// Initialise the transcription service with whisper backend (primary).
#[tauri::command]
pub fn init_whisper_transcription(model_path: String) -> Result<(), Error> {
    let _serialised = WARMUP_LOCK.lock();
    init_whisper_transcription_unlocked(model_path)
}

fn init_parakeet_transcription_unlocked(_model_dir: String) -> Result<(), Error> {
    #[cfg(feature = "parakeet")]
    {
        let service = TranscriptionService::new_parakeet(&PathBuf::from(_model_dir))
            .map_err(|e| e.to_string())?;

        let mut guard = get_service().lock();
        *guard = Some(service);
        touch_model_used();

        tracing::info!("Parakeet transcription service initialised");
        Ok(())
    }

    #[cfg(not(feature = "parakeet"))]
    Err("Parakeet backend not available in this build"
        .to_string()
        .into())
}

/// Initialise the transcription service with parakeet backend (fallback).
#[tauri::command]
pub fn init_parakeet_transcription(model_dir: String) -> Result<(), Error> {
    let _serialised = WARMUP_LOCK.lock();
    init_parakeet_transcription_unlocked(model_dir)
}

fn init_fluidaudio_transcription_unlocked() -> Result<(), Error> {
    #[cfg(all(target_os = "macos", feature = "fluidaudio"))]
    {
        let service = TranscriptionService::new_fluidaudio().map_err(|e| e.to_string())?;

        let mut guard = get_service().lock();
        *guard = Some(service);
        touch_model_used();

        // Write sentinel marker so check_model_downloaded() returns true
        if let Err(e) = fluidaudio::write_ready_marker() {
            tracing::warn!("Failed to write FluidAudio ready marker: {}", e);
        }

        // Persist the manifest version for update-available comparisons
        let fa_id = "fluidaudio-parakeet-tdt-coreml";
        let manifest = manifest::get_fallback_manifest();
        if let Some(fa_model) = manifest.models.iter().find(|m| m.id == fa_id) {
            let version_path = manifest::get_model_directory(fa_id).join(".version");
            if let Err(e) = std::fs::write(&version_path, fa_model.version.trim()) {
                tracing::warn!("Failed to write .version sidecar for FluidAudio: {}", e);
            } else {
                tracing::debug!(
                    "Wrote .version sidecar for FluidAudio ({})",
                    fa_model.version
                );
            }
        }

        tracing::info!("FluidAudio transcription service initialised (Neural Engine)");
        Ok(())
    }

    #[cfg(not(all(target_os = "macos", feature = "fluidaudio")))]
    Err("FluidAudio backend not available in this build"
        .to_string()
        .into())
}

/// Initialise the transcription service with FluidAudio backend (Apple Neural Engine).
#[tauri::command]
pub fn init_fluidaudio_transcription() -> Result<(), Error> {
    let _serialised = WARMUP_LOCK.lock();
    init_fluidaudio_transcription_unlocked()
}

/// Initialise the transcription service (auto-detect best backend).
///
/// Tries whisper first, falls back to parakeet if whisper model not found.
fn init_transcription_unlocked(model_path: String) -> Result<(), Error> {
    let path = PathBuf::from(&model_path);

    // If it's a direct .bin file path, use whisper
    if path.extension().map(|e| e == "bin").unwrap_or(false) {
        return init_whisper_transcription_unlocked(model_path);
    }

    // If it's a directory, check what's inside
    if path.is_dir() {
        // First, check for whisper .bin files (priority for Metal GPU)
        if let Ok(entries) = std::fs::read_dir(&path) {
            for entry in entries.filter_map(|e| e.ok()) {
                let entry_path = entry.path();
                if entry_path
                    .extension()
                    .map(|ext| ext == "bin")
                    .unwrap_or(false)
                {
                    tracing::info!("Found whisper model in directory, using Metal GPU backend");
                    return init_whisper_transcription_unlocked(
                        entry_path.to_string_lossy().to_string(),
                    );
                }
            }
        }

        // No whisper model found, check for ONNX files (parakeet)
        #[cfg(feature = "parakeet")]
        {
            let encoder = path.join("encoder.int8.onnx");
            if encoder.exists() {
                tracing::info!("Found ONNX model in directory, using Parakeet backend");
                return init_parakeet_transcription_unlocked(model_path);
            }
        }
        #[cfg(not(feature = "parakeet"))]
        {
            let encoder = path.join("encoder.int8.onnx");
            if encoder.exists() {
                tracing::warn!(
                    "ONNX models found but Parakeet backend not available in this build"
                );
            }
        }

        return Err(format!(
            "No valid transcription model found in directory: {}",
            path.display()
        )
        .into());
    }

    Err(format!(
        "Model path does not exist or is not valid: {}",
        path.display()
    )
    .into())
}

/// Initialise the transcription service (auto-detect best backend).
///
/// Tries whisper first, falls back to parakeet if whisper model not found.
#[tauri::command]
pub fn init_transcription(model_path: String) -> Result<(), Error> {
    let _serialised = WARMUP_LOCK.lock();
    init_transcription_unlocked(model_path)
}

/// Minimum RMS level to consider audio as containing speech.
/// Audio below this threshold is considered silence and won't be transcribed.
/// This prevents Whisper hallucinations on silent recordings.
/// -54 dB ≈ 0.002 linear amplitude. Low enough for quiet/low-gain mics
/// while still filtering out true digital silence.
const MIN_SPEECH_RMS: f32 = 0.002;

/// Transcribe audio from a file path
///
/// Accepts WAV, MP3, M4A, OGG (Vorbis), and FLAC. Non-WAV files and WAV files
/// that are not already 16 kHz mono i16 are transcoded to a temporary WAV before
/// being passed to the ASR backend. The temp file is deleted when this function
/// returns, whether successfully or not.
///
/// Returns empty string if the audio is essentially silent (no speech detected),
/// which prevents Whisper from hallucinating phrases like "Thank you" on silent input.
///
/// Runs at [`Priority::Interactive`]: this is the entry point the live dictation
/// pipeline uses, so it takes the model ahead of queued background file jobs.
/// Background callers must use [`transcribe_file_with_priority`] instead.
#[tauri::command]
pub fn transcribe_file(audio_path: String) -> Result<String, Error> {
    transcribe_file_with_priority(audio_path, Priority::Interactive)
}

/// Transcribe a file at an explicit [`Priority`].
///
/// See [`transcription::gate`](gate) for why the tier matters: the model is a
/// single process-wide instance, and without a priority the user's own
/// dictation queues behind whatever batch work is outstanding (#118).
pub fn transcribe_file_with_priority(
    audio_path: String,
    priority: Priority,
) -> Result<String, Error> {
    let input = PathBuf::from(&audio_path);

    // Decide whether a transcode is needed.
    // is_target_format_wav returns true only for 16kHz mono i16 WAV files.
    // For any other input we run decode_audio_to_wav first.
    let needs_transcode = !crate::audio::decode::is_target_format_wav(&input);

    let (wav_path, _temp): (PathBuf, Option<tempfile::NamedTempFile>) = if needs_transcode {
        let temp = tempfile::Builder::new()
            .prefix("thoth_transcode_")
            .suffix(".wav")
            .tempfile()
            .map_err(|e| format!("Failed to create temporary file: {}", e))?;
        let temp_path = temp.path().to_path_buf();
        let cancel = std::sync::atomic::AtomicBool::new(false);
        crate::audio::decode::decode_audio_to_wav(&input, &temp_path, &cancel)?;
        tracing::info!("Transcoded {} to temporary 16kHz mono WAV", input.display());
        (temp_path, Some(temp))
    } else {
        (input.clone(), None)
    };

    // Check if audio contains speech before transcribing
    if !audio_has_speech(&wav_path)? {
        tracing::info!(
            "Audio file appears to be silent, skipping transcription: {}",
            audio_path
        );
        return Ok(String::new());
    }

    // Take the gate only now: transcoding and the silence check do not touch
    // the model, so background jobs can do that work without holding the
    // resource the interactive path is waiting for.
    let _permit = gate::gate().acquire(priority);

    let mut guard = get_service().lock();
    if guard.is_none() {
        // The idle watcher may have unloaded between the caller's readiness
        // check and here, and the control API's background jobs arrive without
        // one at all. An unload the user opted into costs latency; it must
        // never cost a dictation.
        drop(guard);
        tracing::info!("No transcription model loaded at transcribe time, reloading");
        warmup_transcription();
        guard = get_service().lock();
    }
    let service = guard
        .as_mut()
        .ok_or_else(|| "Transcription service not initialised".to_string())?;

    let transcript = service.transcribe(&wav_path).map_err(|e| e.to_string())?;
    touch_model_used();
    Ok(transcript)
    // _permit drops here, releasing the model to the next waiter; _temp drops
    // too, deleting the temp file (if any) on both Ok and Err paths.
}

/// Check if a WAV file contains speech (has sufficient audio energy)
///
/// Reads the audio samples and calculates RMS. If the RMS is below
/// the silence threshold, returns false (no speech detected).
fn audio_has_speech(path: &std::path::Path) -> Result<bool, String> {
    use std::io::Read;

    let file =
        std::fs::File::open(path).map_err(|e| format!("Failed to open audio file: {}", e))?;
    let mut reader = std::io::BufReader::new(file);

    // Read WAV header (44 bytes minimum for standard WAV)
    let mut header = [0u8; 44];
    reader
        .read_exact(&mut header)
        .map_err(|e| format!("Failed to read WAV header: {}", e))?;

    // Verify RIFF/WAVE header
    if &header[0..4] != b"RIFF" || &header[8..12] != b"WAVE" {
        return Err("Not a valid WAV file".to_string());
    }

    // Get format info
    let channels = u16::from_le_bytes([header[22], header[23]]) as usize;
    let bits_per_sample = u16::from_le_bytes([header[34], header[35]]);

    if bits_per_sample != 16 {
        // For non-16-bit audio, assume it has speech (can't easily check)
        tracing::debug!(
            "Non-16-bit audio ({}), assuming speech present",
            bits_per_sample
        );
        return Ok(true);
    }

    // Read audio data and calculate RMS
    let mut audio_data = Vec::new();
    reader
        .read_to_end(&mut audio_data)
        .map_err(|e| format!("Failed to read audio data: {}", e))?;

    // Convert i16 samples to f32
    let samples: Vec<f32> = audio_data
        .chunks_exact(2)
        .map(|chunk| {
            let sample = i16::from_le_bytes([chunk[0], chunk[1]]);
            sample as f32 / 32768.0
        })
        .collect();

    // If stereo, average to mono for RMS calculation
    let mono_samples: Vec<f32> = if channels > 1 {
        samples
            .chunks(channels)
            .map(|frame| frame.iter().sum::<f32>() / channels as f32)
            .collect()
    } else {
        samples
    };

    // Check for speech using windowed RMS rather than overall RMS.
    // Short recordings often contain startup silence from the audio stream
    // initialising, which dilutes the overall RMS below the threshold even
    // when speech is clearly present in part of the recording.
    let overall_rms = crate::audio::metering::calculate_rms(&mono_samples);

    // Also find the peak RMS in 500ms windows
    let window_size = 8000; // 500 ms at 16 kHz
    let peak_window_rms = mono_samples
        .chunks(window_size)
        .map(crate::audio::metering::calculate_rms)
        .fold(0.0f32, f32::max);

    tracing::debug!(
        "Audio RMS: overall={:.6}, peak_window={:.6} (threshold: {}), samples: {}",
        overall_rms,
        peak_window_rms,
        MIN_SPEECH_RMS,
        mono_samples.len()
    );

    Ok(peak_window_rms >= MIN_SPEECH_RMS)
}

/// Serialises every model construction, so startup, model selection and wake
/// warmup cannot build models concurrently.
///
/// Construction happens before the new service replaces the old one, so even one
/// replacement peaks at two models. A second constructor would add another model:
/// 500 MB for FluidAudio or up to 3.1 GB for large-v3-turbo plus Metal buffers.
static WARMUP_LOCK: Mutex<()> = Mutex::new(());

/// When the loaded model was last used, or loaded. `None` when none is loaded.
static LAST_USED: Mutex<Option<Instant>> = Mutex::new(None);

/// How often the idle watcher looks at the clock and re-reads the config.
///
/// The timeout a user sets is minutes; a ten-second tick is finer than they can
/// perceive, and a tick that unloads nothing is one elapsed-time comparison.
const IDLE_UNLOAD_TICK: Duration = Duration::from_secs(10);

/// Record that the loaded model has just been loaded or used.
fn touch_model_used() {
    *LAST_USED.lock() = Some(Instant::now());
    MODEL_HAS_LOADED.store(true, Ordering::SeqCst);
}

/// Whether an idle model should be unloaded now.
///
/// Separate from the unload itself so the decision is testable without a
/// multi-gigabyte model behind it. Each way of answering "no" is a real case:
/// nothing is loaded, the model is still inside its timeout, or the model is
/// busy — the user is recording, or a transcription is running or queued behind
/// the gate and would only have to load it again.
fn should_unload(last_used: Option<Instant>, timeout: Duration, busy: bool) -> bool {
    if busy {
        return false;
    }
    match last_used {
        Some(used) => used.elapsed() >= timeout,
        None => false,
    }
}

/// What an unload attempt did.
///
/// Three outcomes rather than a bool because "did not unload" has two very
/// different meanings, and only one of them is the safety property worth
/// asserting: `Busy` says the model was being used and was deliberately left
/// alone, `NotLoaded` says there was nothing there in the first place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnloadOutcome {
    /// The model was dropped and its memory released.
    Unloaded,
    /// Nothing was loaded, so there was nothing to reclaim.
    NotLoaded,
    /// A warmup or a transcription holds the model; it was left loaded.
    Busy,
}

/// Drop the loaded transcription model, freeing its weights.
///
/// Never waits on a lock: [`WARMUP_LOCK`] held means a model is being built and
/// the service lock held means one is transcribing, and both are reasons to
/// leave it alone until the next tick rather than to queue behind it. That is
/// what stops an unload landing in the middle of a dictation.
pub fn unload_transcription_model() -> UnloadOutcome {
    let Some(_serialised) = WARMUP_LOCK.try_lock() else {
        tracing::debug!("Idle unload skipped: a warmup is in flight");
        return UnloadOutcome::Busy;
    };
    let Some(mut guard) = get_service().try_lock() else {
        tracing::debug!("Idle unload skipped: the model is in use");
        return UnloadOutcome::Busy;
    };
    if guard.take().is_none() {
        return UnloadOutcome::NotLoaded;
    }
    // Lock order is service -> LAST_USED wherever both are held;
    // `maybe_unload_idle_model` reads LAST_USED and releases it before calling in here.
    *LAST_USED.lock() = None;
    tracing::info!("Transcription model unloaded after idle timeout");
    UnloadOutcome::Unloaded
}

/// Unload the model when it has been idle for at least `timeout`.
pub fn maybe_unload_idle_model(timeout: Duration) -> UnloadOutcome {
    let last_used = *LAST_USED.lock();
    let busy = crate::audio::is_recording() || gate::gate().in_use();
    if !should_unload(last_used, timeout, busy) {
        return UnloadOutcome::NotLoaded;
    }
    unload_transcription_model()
}

/// The idle timeout a stored setting means.
///
/// `None` and `0` both mean never: the config file is hand-editable and `0` is
/// what someone types for "off", which would otherwise read as "unload
/// immediately, on every tick".
fn idle_unload_timeout(setting: Option<u64>) -> Option<Duration> {
    setting.filter(|secs| *secs > 0).map(Duration::from_secs)
}

/// The configured idle timeout, or `None` when the user has not asked for one.
fn configured_idle_unload() -> Option<Duration> {
    idle_unload_timeout(
        crate::config::get_config()
            .ok()?
            .transcription
            .model_idle_unload_secs,
    )
}

/// Start the background watcher that unloads an idle model (#105).
///
/// Re-reads the config every tick, so switching the timeout on or off takes
/// effect without restarting the app.
pub fn spawn_idle_unload_watcher() {
    std::thread::spawn(|| {
        loop {
            std::thread::sleep(IDLE_UNLOAD_TICK);
            if let Some(timeout) = configured_idle_unload() {
                maybe_unload_idle_model(timeout);
            }
        }
    });
}

/// Eagerly initialise the transcription model in the background.
/// Triggers Metal shader compilation so the first recording is instant.
///
/// A no-op when a model is already loaded. Warmup means "make sure a model is
/// loaded", never "load one again": every caller either guards on
/// [`is_transcription_ready`] already (the three pipeline entry points) or runs
/// when nothing can be loaded yet (startup). The one that did not was the macOS
/// wake observer, which subscribes to `NSWorkspaceScreensDidWakeNotification` as
/// well as `NSWorkspaceDidWakeNotification` — so a lid open, a display waking or
/// a monitor hotplug each rebuilt the whole model. Measured on this machine's own
/// logs before the guard: 6 wake events on 2026-09-01, 6 full model loads (#171).
///
/// Records the outcome in [`WARMUP_FAILED`] so the record guard and pipeline can
/// react immediately when no usable model could be loaded.
pub fn warmup_transcription() {
    // Held for the whole attempt: a caller that blocks here finds the model
    // already loaded when it gets in, and returns without loading a second one.
    let _serialised = WARMUP_LOCK.lock();

    if is_transcription_ready() {
        tracing::debug!("Warmup skipped: a transcription model is already loaded");
        WARMUP_FAILED.store(false, Ordering::SeqCst);
        return;
    }

    // Clear the flag so an in-progress retry is treated optimistically; if this
    // attempt also fails to produce a service, it is set again below.
    WARMUP_FAILED.store(false, Ordering::SeqCst);
    warmup_transcription_inner();
    let loaded = is_transcription_ready();
    WARMUP_FAILED.store(!loaded, Ordering::SeqCst);
    if loaded {
        // Start the idle clock at the load, so a model warmed and never used is
        // still reclaimed.
        touch_model_used();
    }
    if !loaded {
        tracing::warn!("Warmup finished without loading a usable transcription model");
    }
}

fn warmup_transcription_inner() {
    let selected_id = crate::config::get_config()
        .ok()
        .and_then(|c| c.transcription.model_id.clone());

    let manifest = manifest::get_fallback_manifest();

    // Resolve model type for the selected model
    let selected_model_type = selected_id.as_ref().and_then(|id| {
        manifest
            .models
            .iter()
            .find(|m| m.id == *id)
            .map(|m| m.model_type.as_str())
    });

    // ── FluidAudio path ────────────────────────────────────────────────
    // Try FluidAudio when explicitly selected OR when nothing is selected
    // (it's the recommended default on Apple Silicon).
    let should_try_fluidaudio =
        selected_model_type == Some("fluidaudio_coreml") || selected_id.is_none();

    if should_try_fluidaudio && try_warmup_fluidaudio() {
        return;
    }
    // FluidAudio unavailable/not cached — fall through to Whisper

    // ── Whisper/Parakeet path ──────────────────────────────────────────
    if selected_id.is_some() && selected_model_type != Some("fluidaudio_coreml") {
        // A specific non-FluidAudio model is selected — try to init it
        let model_dir = get_model_directory();
        if !download::check_model_downloaded(None) {
            tracing::info!("Selected model not downloaded yet, skipping warmup");
            return;
        }
        match init_transcription_unlocked(model_dir) {
            Ok(()) => {
                tracing::info!("Transcription model warmed up");
                return;
            }
            Err(e) => {
                tracing::warn!("Transcription warmup failed: {}", e);
                // Backend might be unavailable, fall through to Whisper fallback
            }
        }
    }

    // ── Whisper fallback ───────────────────────────────────────────────
    warmup_whisper_fallback(&manifest);
}

/// Attempt to warm up FluidAudio. Returns `true` if successful.
fn try_warmup_fluidaudio() -> bool {
    #[cfg(all(target_os = "macos", feature = "fluidaudio"))]
    {
        if fluidaudio::is_cached() {
            match init_fluidaudio_transcription_unlocked() {
                Ok(()) => {
                    tracing::info!("FluidAudio transcription model warmed up (Neural Engine)");
                    return true;
                }
                Err(e) => {
                    tracing::warn!("FluidAudio warmup failed: {}, falling back", e);
                }
            }
        } else {
            tracing::info!("FluidAudio models not yet cached, falling back to Whisper");
        }
    }

    #[cfg(not(all(target_os = "macos", feature = "fluidaudio")))]
    {
        tracing::debug!("FluidAudio backend not available in this build");
    }

    false
}

/// Fall back to the best available downloaded Whisper model during warmup.
fn warmup_whisper_fallback(manifest: &manifest::ModelManifest) {
    // Try the largest/best downloaded Whisper model (manifest order = quality order)
    if let Some(whisper_model) = manifest
        .models
        .iter()
        .find(|m| m.model_type == "whisper_ggml" && manifest::is_model_downloaded(m))
    {
        let whisper_dir = manifest::get_model_directory(&whisper_model.id);
        match init_transcription_unlocked(whisper_dir.to_string_lossy().to_string()) {
            Ok(()) => {
                tracing::info!("Fell back to Whisper model '{}'", whisper_model.id);
            }
            Err(e) => {
                tracing::warn!("Whisper fallback also failed: {}", e);
            }
        }
    } else {
        tracing::info!("No downloaded Whisper model available for fallback");
    }
}

/// Check if transcription service is ready
#[tauri::command]
pub fn is_transcription_ready() -> bool {
    get_service().lock().is_some()
}

/// Get the current transcription backend
#[tauri::command]
pub fn get_transcription_backend() -> Option<String> {
    get_service().lock().as_ref().map(|s| match s.backend() {
        TranscriptionBackend::Whisper => "whisper".to_string(),
        TranscriptionBackend::Parakeet => "parakeet".to_string(),
        TranscriptionBackend::FluidAudio => "fluidaudio".to_string(),
    })
}

/// Get the default model directory path for the currently selected/recommended model
#[tauri::command]
pub fn get_model_directory() -> String {
    let config_model_id = crate::config::get_config()
        .ok()
        .and_then(|c| c.transcription.model_id);

    model_directory_for_selection(config_model_id.as_deref())
        .to_string_lossy()
        .to_string()
}

/// Resolves a configured model id to the directory this build will actually
/// load from. Uses the same `resolve_selected_id` the model list uses for the
/// Active badge, so a model this build cannot run — a config carried from
/// macOS, or a stale id — is never handed to `init_transcription` while the
/// UI shows a different model as active (#128, #100).
fn model_directory_for_selection(configured: Option<&str>) -> PathBuf {
    let fallback = manifest::get_fallback_manifest();
    let resolved_id = manifest::resolve_selected_id(&fallback.models, configured)
        .unwrap_or("ggml-large-v3-turbo");

    manifest::get_model_directory(resolved_id)
}

/// Get the whisper model directory path
#[tauri::command]
pub fn get_whisper_model_directory() -> String {
    whisper::get_whisper_model_directory()
        .to_string_lossy()
        .to_string()
}

/// Check if a whisper model is downloaded
#[tauri::command]
pub fn is_whisper_model_downloaded(model_id: String) -> bool {
    whisper::is_whisper_model_downloaded(&model_id)
}

/// Filter transcription text to clean up filler words and formatting
#[tauri::command]
pub fn filter_transcription(text: String, options: Option<FilterOptions>) -> String {
    let filter_options = options.unwrap_or_default();
    let output_filter = OutputFilter::new(filter_options);
    output_filter.filter(&text)
}

/// Set the selected model ID in config
#[tauri::command]
pub fn set_selected_model_id(model_id: Option<String>) -> Result<(), Error> {
    let mut config = crate::config::get_config().map_err(|e| e.to_string())?;
    config.transcription.model_id = model_id.clone();
    crate::config::set_config(config).map_err(|e| e.to_string())?;

    tracing::info!("Selected model ID set to: {:?}", model_id);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The state the user is in for most of the day: nothing loaded, nothing to
    /// reclaim. A `true` here would make the watcher log an unload every tick.
    #[test]
    fn nothing_loaded_is_not_unloadable() {
        assert!(!should_unload(None, Duration::from_secs(1), false));
    }

    #[test]
    fn a_model_inside_its_timeout_stays_loaded() {
        let just_used = Instant::now();
        assert!(!should_unload(
            Some(just_used),
            Duration::from_secs(600),
            false
        ));
    }

    #[test]
    fn a_model_past_its_timeout_is_unloaded() {
        let long_ago = Instant::now() - Duration::from_secs(601);
        assert!(should_unload(
            Some(long_ago),
            Duration::from_secs(600),
            false
        ));
    }

    /// The dangerous case. The user holds the hotkey down for a five-minute
    /// dictation, or a queued job is waiting on the gate, so the model is idle
    /// by the clock and about to be needed.
    #[test]
    fn an_idle_model_is_not_unloaded_while_recording() {
        let long_ago = Instant::now() - Duration::from_secs(3600);
        assert!(!should_unload(
            Some(long_ago),
            Duration::from_secs(600),
            true
        ));
    }

    /// A timeout of zero would otherwise mean "unload immediately, every tick".
    #[test]
    fn unset_and_zero_timeouts_both_mean_never() {
        assert_eq!(idle_unload_timeout(None), None);
        assert_eq!(idle_unload_timeout(Some(0)), None);
        assert_eq!(
            idle_unload_timeout(Some(600)),
            Some(Duration::from_secs(600))
        );
    }

    /// The safety property, asserted where it can be proved deterministically.
    /// `Busy` is only reachable through the `try_lock` branches, so a held
    /// service lock returning it means the unload really did refuse rather than
    /// finding nothing to do.
    #[test]
    fn an_in_flight_transcription_refuses_the_unload() {
        let _held = get_service().lock();
        assert_eq!(unload_transcription_model(), UnloadOutcome::Busy);
    }

    /// The other half of the same property: a warmup is mid-construction, so
    /// the model about to be installed must not be unloaded out from under it.
    #[test]
    fn a_warmup_in_flight_refuses_the_unload() {
        let _held = WARMUP_LOCK.lock();
        assert_eq!(unload_transcription_model(), UnloadOutcome::Busy);
    }

    /// The shipped default must stay "never": unloading trades the next
    /// dictation's latency for memory, so it is opt-in.
    #[test]
    fn the_default_config_never_unloads() {
        let config = crate::config::TranscriptionConfig::default();
        assert_eq!(config.model_idle_unload_secs, None);
        assert_eq!(idle_unload_timeout(config.model_idle_unload_secs), None);
    }

    /// A config copied from macOS must not point Linux at FluidAudio's CoreML
    /// cache: no Linux build can initialise that backend.
    #[cfg(not(all(target_os = "macos", feature = "fluidaudio")))]
    #[test]
    fn model_directory_ignores_an_unrunnable_configured_model() {
        let manifest = manifest::get_fallback_manifest();
        let configured = "fluidaudio-parakeet-tdt-coreml";
        let resolved = manifest::resolve_selected_id(&manifest.models, Some(configured))
            .expect("the bundled manifest has a runnable model");

        assert_ne!(resolved, configured);
        assert_eq!(
            model_directory_for_selection(Some(configured)),
            manifest::get_model_directory(resolved)
        );
    }
}
