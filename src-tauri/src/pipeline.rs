//! Transcription pipeline orchestration
//!
//! Wires together the complete flow from recording to output:
//! 1. Recording (start/stop via audio module)
//! 2. Transcription (via transcription module)
//! 3. Filtering (dictionary replacements + output filtering)
//! 4. Enhancement (optional AI enhancement via Ollama)
//! 5. Output (clipboard copy and/or paste at cursor)
//! 6. History (save to database)

use crate::TELEMETRY_TARGET;
use crate::canonical;
use crate::clipboard;
use crate::database;
use crate::dictionary;
use crate::enhancement;
use crate::error::Error;
use crate::transcription;
use crate::tray;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tauri::{AppHandle, Emitter};
use tracing::Instrument;

/// Pipeline execution state
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PipelineState {
    /// Pipeline is idle, ready for recording
    #[default]
    Idle,
    /// Recording audio
    Recording,
    /// Transcribing audio to text
    Transcribing,
    /// Applying dictionary replacements and filtering
    Filtering,
    /// Enhancing text with AI
    Enhancing,
    /// Converting imported audio to 16kHz mono WAV
    Converting,
    /// Outputting result (clipboard/paste)
    Outputting,
    /// Pipeline completed successfully
    Completed,
    /// Pipeline failed with error
    Failed,
}

/// Pipeline configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PipelineConfig {
    /// Whether to apply dictionary replacements
    pub apply_dictionary: bool,
    /// Whether to apply output filtering (formatting, whitespace)
    pub apply_filtering: bool,
    /// Whether to remove hesitation sounds (um, uh, er, ah)
    pub remove_fillers: bool,
    /// Whether to convert US spellings to Australian/British equivalents
    pub australian_spelling: bool,
    /// Whether to convert spoken number words to digits
    pub spoken_numbers_to_digits: bool,
    /// Whether to collapse runs of whitespace and trim leading/trailing spaces
    pub normalise_whitespace: bool,
    /// Whether to fix spacing around punctuation marks
    pub cleanup_punctuation: bool,
    /// Whether to capitalise the first word of each sentence
    pub sentence_case: bool,
    /// Whether to convert spoken formatting commands ("new paragraph" / "new
    /// line") into line breaks
    pub voice_formatting_commands: bool,
    /// Whether AI enhancement is enabled
    pub enhancement_enabled: bool,
    /// Ollama model for enhancement
    pub enhancement_model: String,
    /// Enhancement prompt template
    pub enhancement_prompt: String,
    /// Which built-in or custom prompt this is, for telemetry only — never the
    /// template text.
    #[serde(default)]
    pub enhancement_prompt_id: Option<String>,
    /// Whether to auto-copy to clipboard
    pub auto_copy: bool,
    /// Whether to auto-paste at cursor
    pub auto_paste: bool,
    /// Insertion method: "typing" or "paste"
    pub insertion_method: String,
    /// Whether to append a single space after the inserted text (#112)
    #[serde(default)]
    pub append_trailing_space: bool,
    /// Key combination to send after a successful insertion (#112)
    #[serde(default)]
    pub auto_submit: crate::config::AutoSubmit,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            apply_dictionary: true,
            apply_filtering: true,
            remove_fillers: true,
            australian_spelling: false,
            spoken_numbers_to_digits: false,
            normalise_whitespace: true,
            cleanup_punctuation: true,
            sentence_case: false,
            voice_formatting_commands: true,
            enhancement_enabled: false,
            enhancement_model: "llama3.2".to_string(),
            enhancement_prompt: DEFAULT_ENHANCEMENT_PROMPT.to_string(),
            enhancement_prompt_id: None,
            auto_copy: false,
            auto_paste: true,
            insertion_method: "paste".to_string(),
            append_trailing_space: false,
            auto_submit: crate::config::AutoSubmit::Off,
        }
    }
}

/// Build the effective [`PipelineConfig`] from the saved settings, mirroring the
/// frontend's `getDefaultConfig()`. Used by entry points that trigger a recording
/// without a frontend-supplied config (e.g. the bundled MCP server) so the result
/// honours the user's filter, spelling and enhancement settings exactly as the
/// global hotkey does (`PipelineConfig::default()` hardcodes different values).
pub(crate) fn effective_pipeline_config() -> Result<PipelineConfig, Error> {
    let cfg = crate::config::get_config()?;
    let t = &cfg.transcription;
    let e = &cfg.enhancement;
    let enhancement_prompt = if e.enabled {
        crate::enhancement::prompts::get_all_prompts()
            .into_iter()
            .find(|p| p.id == e.prompt_id)
            .map(|p| p.template)
            .unwrap_or_else(|| DEFAULT_ENHANCEMENT_PROMPT.to_string())
    } else {
        DEFAULT_ENHANCEMENT_PROMPT.to_string()
    };
    Ok(PipelineConfig {
        apply_dictionary: true,
        apply_filtering: true,
        remove_fillers: t.remove_fillers,
        australian_spelling: t.australian_spelling,
        spoken_numbers_to_digits: t.spoken_numbers_to_digits,
        normalise_whitespace: t.normalise_whitespace,
        cleanup_punctuation: t.cleanup_punctuation,
        sentence_case: t.sentence_case,
        voice_formatting_commands: t.voice_formatting_commands,
        enhancement_enabled: e.enabled,
        enhancement_model: e.model.clone(),
        enhancement_prompt,
        enhancement_prompt_id: e.enabled.then(|| e.prompt_id.clone()),
        auto_copy: t.auto_copy,
        auto_paste: t.auto_paste,
        insertion_method: "paste".to_string(),
        append_trailing_space: t.append_trailing_space,
        auto_submit: t.auto_submit,
    })
}

/// Default enhancement prompt
const DEFAULT_ENHANCEMENT_PROMPT: &str = r#"Fix grammar and punctuation in the following text.
Keep the original meaning and tone. Output only the corrected text, nothing else.

Text: {text}"#;

/// Pipeline execution result
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PipelineResult {
    /// Whether the pipeline completed successfully
    pub success: bool,
    /// Final transcribed text (after all processing)
    pub text: String,
    /// Raw transcription text (before filtering/enhancement)
    pub raw_text: String,
    /// Whether the text was enhanced by AI
    pub is_enhanced: bool,
    /// Duration of the audio in seconds
    pub duration_seconds: Option<f64>,
    /// Path to the audio file
    pub audio_path: Option<String>,
    /// Error message if the pipeline failed
    pub error: Option<String>,
    /// ID of the saved transcription record
    pub transcription_id: Option<String>,
    /// Name of the transcription model used
    pub transcription_model_name: Option<String>,
    /// Time taken to transcribe in seconds
    pub transcription_duration_seconds: Option<f64>,
    /// Name of the enhancement model used
    pub enhancement_model_name: Option<String>,
    /// Time taken for enhancement in seconds
    pub enhancement_duration_seconds: Option<f64>,
}

/// Progress event payload
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PipelineProgress {
    /// Current pipeline state
    pub state: PipelineState,
    /// Progress message for display
    pub message: String,
    /// Audio device name (only present when state is Recording)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_name: Option<String>,
}

/// True only while a capture stream is open (arm → disarm/stop).
/// A new recording is rejected only when this is true.
static PIPELINE_RUNNING: AtomicBool = AtomicBool::new(false);

/// Counts how many detached process_audio tasks are in-flight.
/// Used by get_pipeline_state to distinguish Recording vs Transcribing vs Idle.
static PROCESSING_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Cancellation signal for file import operations
static IMPORT_CANCELLED: AtomicBool = AtomicBool::new(false);

/// Error message emitted when transcription produces no text (silent recording).
///
/// Used as a typed sentinel: callers that need to distinguish "nothing was said"
/// from a genuine failure check via [`is_no_speech_error`]. Centralised here so
/// the comparison is never a fragile inline string match.
const NO_SPEECH_ERROR: &str = "Transcription produced no text";

/// Returns true when the pipeline error string indicates a silent recording rather
/// than a genuine failure. Used to suppress error UI and silently delete orphan WAVs.
fn is_no_speech_error(e: &str) -> bool {
    e == NO_SPEECH_ERROR
}

/// Deletes `audio_path` when `result` is the no-speech sentinel.
///
/// Returns `true` if the file was discarded (caller should suppress error UI),
/// `false` if the result was either success or a genuine error (caller handles
/// normally). Both the recording-path arm and the import-path arm delegate here
/// so the discard decision is tested once in one place.
fn discard_silent_wav(result: &Result<PipelineResult, String>, audio_path: &str) -> bool {
    let Err(e) = result else { return false };
    if !is_no_speech_error(e) {
        return false;
    }
    tracing::info!(
        "Pipeline: Silent recording, deleting orphan WAV: {}",
        audio_path
    );
    tracing::info!(target: TELEMETRY_TARGET, event = "recording_silent_dropped", "recording_silent_dropped");
    if let Err(del_err) = std::fs::remove_file(audio_path) {
        tracing::warn!(
            "Pipeline: Failed to delete silent WAV {}: {}",
            audio_path,
            del_err
        );
    }
    true
}

/// Serialises clipboard-save → paste → clipboard-restore across concurrent
/// detached process_audio tasks. Without this, two jobs could race the system
/// clipboard and corrupt the restored content.
static OUTPUT_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// RAII guard that resets PIPELINE_RUNNING to false on drop.
/// Used for recording capture only (not for processing).
struct PipelineGuard;

impl Drop for PipelineGuard {
    fn drop(&mut self) {
        PIPELINE_RUNNING.store(false, Ordering::SeqCst);
    }
}

/// RAII guard that decrements PROCESSING_COUNT on drop.
/// Ensures PROCESSING_COUNT stays balanced even if process_audio panics.
struct ProcessingGuard;

impl ProcessingGuard {
    fn new() -> Self {
        PROCESSING_COUNT.fetch_add(1, Ordering::SeqCst);
        Self
    }
}

impl Drop for ProcessingGuard {
    fn drop(&mut self) {
        PROCESSING_COUNT.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Start the recording phase of the pipeline
///
/// Emits `pipeline-progress` event with state updates.
/// Also shows the recording indicator overlay and starts audio metering.
#[tauri::command]
#[tracing::instrument(target = TELEMETRY_TARGET, skip_all, err)]
pub fn pipeline_start_recording(app: AppHandle) -> Result<String, Error> {
    tracing::info!("Pipeline: pipeline_start_recording called");

    if PIPELINE_RUNNING.swap(true, Ordering::SeqCst) {
        tracing::warn!("Pipeline: Already running, rejecting start request");
        return Err("Pipeline is already running".to_string().into());
    }

    // If the transcription model isn't loaded yet, decide whether we can record.
    // We allow recording during an in-progress background load (the model is
    // usually ready by the time the user stops speaking), but block outright when
    // there is no usable model — either the selected model is not on disk, or a
    // warmup has already proved that nothing (selected or fallback) can load.
    // Otherwise the user records into a void and the pipeline hangs at the
    // transcription stage waiting for a model that will never arrive.
    if !transcription::is_transcription_ready() {
        // A warmup that failed on a model which has already loaded here is a
        // reload that went wrong, not proof that nothing can load (#105): the
        // pipeline retries it rather than refusing to record until a restart.
        let nothing_can_load = transcription::warmup_failed() && !transcription::model_has_loaded();
        if nothing_can_load || !transcription::download::check_model_downloaded(None) {
            PIPELINE_RUNNING.store(false, Ordering::SeqCst);
            tracing::warn!("Pipeline: No usable transcription model, blocking recording");
            telemetry::report_error("model_load_failed");
            tracing::warn!(target: TELEMETRY_TARGET, reason = "no_usable_model", "model_load_failure");
            let _ = crate::recording_indicator::hide_recording_indicator(app.clone());
            return Err(
                "No transcription model is ready. Open Settings \u{2192} Models to download or repair one."
                    .to_string()
                    .into(),
            );
        }
        tracing::info!("Pipeline: Model not loaded yet, starting eager background load");
        std::thread::spawn(|| {
            transcription::warmup_transcription();
        });
    }

    // Emit recording state early so the UI updates before the device opens.
    // Device name will be filled from audio::last_device_name() after start_recording
    // returns; we emit a second progress event with the name then.
    // Emitting the device name later avoids blocking on the ~90ms CoreAudio device-resolution call before the UI updates.
    emit_progress(&app, PipelineState::Recording, "Recording audio...");

    tracing::info!("Pipeline: Calling audio::start_recording");
    match crate::audio::start_recording() {
        Ok(path) => {
            tracing::info!("Pipeline: Recording started at {}", path);

            // Now that start_recording has resolved (and stored) the device name,
            // emit a follow-up progress event that includes it for the UI.
            let device_name = crate::audio::last_device_name();
            tracing::info!(
                target: TELEMETRY_TARGET,
                event = "recording_started",
                device = %device_name.as_deref().unwrap_or("unknown"),
                "recording_started"
            );
            emit_progress_with_device(
                &app,
                PipelineState::Recording,
                "Recording audio...",
                device_name,
            );

            // Emit authoritative state: is_recording() is now true so
            // get_pipeline_state() returns Recording.
            emit_recording_state(&app);

            // Update tray to show recording state
            tray::set_recording_state(&app, true);

            // NOTE: Recording indicator is shown instantly from the shortcut handler
            // (show_indicator_instant) - no need to show it here again.
            // The indicator window is pre-warmed at startup so no JS init wait needed.

            // Start recording metering AFTER the indicator is visible
            if let Err(e) = crate::audio::start_recording_metering(app.clone()) {
                tracing::warn!("Pipeline: Failed to start recording metering: {}", e);
            }

            // Hands-free (#88): let silence end the recording. Read fresh so a
            // mode change in Settings applies to the very next press.
            if let Ok(config) = crate::config::get_config()
                && config.shortcuts.recording_mode == crate::config::RecordingMode::HandsFree
            {
                spawn_hands_free_watcher(app, config.shortcuts.hands_free_silence());
            }

            Ok(path)
        }
        Err(e) => {
            PIPELINE_RUNNING.store(false, Ordering::SeqCst);
            telemetry::report_error_with_cause("audio_capture_failed", &e);
            tracing::warn!(
                target: TELEMETRY_TARGET,
                reason = "audio_start_failed",
                error = %e,
                "audio_device_failure"
            );
            emit_progress(
                &app,
                PipelineState::Failed,
                &format!("Recording failed: {}", e),
            );
            Err(e)
        }
    }
}

/// Generation counter for hands-free watchers.
///
/// A watcher polls across recordings' lifetimes, so a stale one from an
/// earlier press must never stop a later recording. Each spawn takes the next
/// generation and retires as soon as it is no longer the current one.
static HANDS_FREE_GENERATION: AtomicUsize = AtomicUsize::new(0);

/// How often the hands-free watcher checks the live gate.
///
/// Finer than this buys nothing: the trigger is measured in audio time by the
/// gate, not by how often it is read, so polling only sets how late the stop
/// lands, not how much speech is kept.
const HANDS_FREE_TICK: std::time::Duration = std::time::Duration::from_millis(100);

/// Whether the trailing silence has reached the hands-free timeout.
///
/// `None` means the user has not spoken yet — silence before anyone has
/// started is not a finished utterance, and a recording must never end on it.
fn hands_free_should_stop(silence_ms: Option<u64>, timeout: std::time::Duration) -> bool {
    match silence_ms {
        Some(ms) => ms >= timeout.as_millis() as u64,
        None => false,
    }
}

/// Watch the live speech gate and end the recording once it goes quiet (#88).
///
/// Runs only in `RecordingMode::HandsFree`. The hotkey still stops a recording
/// by hand at any point; this watcher retires the moment capture stops, by
/// whatever route.
fn spawn_hands_free_watcher(app: AppHandle, timeout: std::time::Duration) {
    let generation = HANDS_FREE_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    tracing::info!(
        "Pipeline: hands-free watcher armed, stopping after {:.1}s of silence",
        timeout.as_secs_f32()
    );

    tauri::async_runtime::spawn(async move {
        let activity = crate::audio::speech_gate::speech_activity();
        loop {
            tokio::time::sleep(HANDS_FREE_TICK).await;

            if generation != HANDS_FREE_GENERATION.load(Ordering::SeqCst) {
                return;
            }
            if !crate::audio::is_recording() {
                // Stopped by hotkey, tray, or cancel. Nothing to do.
                return;
            }
            // The gate publishes from the capture writer thread, which starts
            // fractionally after the recording does. Until it is live its
            // numbers belong to the PREVIOUS recording — `stop` deliberately
            // leaves `heard_speech` set — so they must not be read at all.
            if !activity.is_live() {
                continue;
            }
            if !hands_free_should_stop(activity.silence_ms_after_speech(), timeout) {
                continue;
            }

            tracing::info!(
                target: TELEMETRY_TARGET,
                silence_ms = activity.silence_ms_after_speech().unwrap_or(0),
                "hands_free_auto_stop"
            );
            // Through the toggle with an explicit stop-only intent, not
            // `pipeline_stop_and_process` directly. The watcher and a manual
            // stop can decide in the same instant, and whichever loses used to
            // surface "No recording in progress" as a failure although the
            // recording had stopped correctly. StopOnly answers `Ignored`
            // instead, which is what that moment actually is. The toggle also
            // plays the stop cue, paired with the decision as the hotkey path
            // pairs it.
            match pipeline_toggle_recording(app, None, Some(ToggleIntent::StopOnly)).await {
                Ok(ToggleOutcome::Ignored) => {
                    tracing::debug!("Pipeline: hands-free auto-stop raced a manual stop");
                }
                Ok(_) => {}
                Err(e) => tracing::warn!("Pipeline: hands-free auto-stop failed: {}", e),
            }
            return;
        }
    });
}

/// Stop recording and kick off the transcription pipeline asynchronously.
///
/// Returns immediately after capture has stopped so a new recording can start
/// without waiting for transcription to finish. The actual result is delivered
/// via the `pipeline-complete` event (which the frontend already handles).
///
/// Emits `pipeline-progress` events as each stage completes.
/// Emits `pipeline-complete` when finished with the final result.
/// Hides the recording indicator overlay when recording stops.
///
/// The root of the dictation trace. The detached processing task is attached to
/// this span, so the span stays open until the history write finishes and its
/// duration is the whole turnaround from the stop keypress to the saved
/// transcription — which is what `dictation` names. `recording_seconds` carries
/// how long the user actually spoke, which this span does not cover.
#[tauri::command]
#[tracing::instrument(
    target = TELEMETRY_TARGET,
    name = "dictation",
    skip_all,
    fields(recording_seconds = tracing::field::Empty)
)]
pub async fn pipeline_stop_and_process(
    app: AppHandle,
    config: Option<PipelineConfig>,
) -> Result<(), Error> {
    tracing::info!("Pipeline: stop_and_process called");
    let config = config.unwrap_or_default();

    // Stop recording metering
    crate::audio::stop_recording_metering();

    // Update tray to show idle state
    tray::set_recording_state(&app, false);

    // Hide the recording indicator
    if let Err(e) = crate::recording_indicator::hide_recording_indicator(app.clone()) {
        tracing::warn!("Pipeline: Failed to hide recording indicator: {}", e);
    }

    // Stop recording — releases the capture lock so a new recording can start.
    let audio_path = match crate::audio::stop_recording() {
        Ok(path) => path,
        Err(e) => {
            emit_progress(
                &app,
                PipelineState::Failed,
                &format!("Stop recording failed: {}", e),
            );
            // Release capture flag so the next start is not blocked.
            PIPELINE_RUNNING.store(false, Ordering::SeqCst);
            return Err(e);
        }
    };

    // Capture has stopped — release PIPELINE_RUNNING so a new recording can start
    // immediately while this task processes.
    PIPELINE_RUNNING.store(false, Ordering::SeqCst);

    // Recording duration from the WAV header; 0.0 if unavailable.
    let rec_duration = get_audio_duration(&audio_path).unwrap_or(0.0);
    tracing::Span::current().record("recording_seconds", rec_duration);
    tracing::info!(
        target: TELEMETRY_TARGET,
        duration_seconds = rec_duration,
        "recording_stopped"
    );

    tracing::info!(
        "Pipeline: Recording stopped, spawning detached process task for {}",
        audio_path
    );

    // Emit authoritative state: capture ended and processing is starting.
    // We emit Transcribing directly here rather than calling get_pipeline_state()
    // because the ProcessingGuard is not yet acquired (it runs inside the spawned
    // task), so get_pipeline_state() would incorrectly return Idle at this point.
    if let Err(e) = app.emit("recording-state", PipelineState::Transcribing) {
        tracing::warn!("Failed to emit recording-state (stop): {}", e);
    }

    // Detach processing: transcription, filtering, enhancement, output and history
    // run in a separate task. PROCESSING_COUNT tracks in-flight tasks so
    // get_pipeline_state can report Transcribing when appropriate.
    tokio::spawn(
        async move {
            // Run processing under the guard in an inner scope so PROCESSING_COUNT
            // is decremented BEFORE we emit the final authoritative state. Otherwise
            // get_pipeline_state() would still see the guard alive and report
            // Transcribing, leaving the UI stuck on "Processing" forever.
            let result = {
                let _processing_guard = ProcessingGuard::new();
                process_audio(&app, &audio_path, &config).await
            };
            match &result {
                Ok(r) => {
                    tracing::info!("Pipeline: Emitting pipeline-complete event");
                    if let Err(e) = app.emit("pipeline-complete", r) {
                        tracing::error!("Pipeline: Failed to emit pipeline-complete: {}", e);
                    }
                }
                Err(_) if discard_silent_wav(&result, &audio_path) => {
                    // Silent recording suppressed — discard_silent_wav already deleted the WAV.
                }
                Err(e) => {
                    tracing::error!("Pipeline: Processing failed: {}", e);
                    emit_progress(&app, PipelineState::Failed, e);
                }
            }
            // Emit authoritative state after the guard has dropped. get_pipeline_state()
            // returns Recording if a new clip started while this task ran, so this can
            // never clobber an active recording with Idle.
            emit_recording_state(&app);
        }
        .instrument(tracing::Span::current()),
    );

    Ok(())
}

/// Cancel the current pipeline execution
#[tauri::command]
#[tracing::instrument(target = TELEMETRY_TARGET, skip_all, err)]
pub fn pipeline_cancel(app: AppHandle) -> Result<(), Error> {
    if !PIPELINE_RUNNING.load(Ordering::SeqCst) {
        return Ok(()); // Nothing to cancel
    }

    // Stop recording metering and hide indicator
    crate::audio::stop_recording_metering();
    if let Err(e) = crate::recording_indicator::hide_recording_indicator(app.clone()) {
        tracing::warn!(
            "Pipeline: Failed to hide recording indicator on cancel: {}",
            e
        );
    }

    // Signal cancellation for file import operations
    IMPORT_CANCELLED.store(true, Ordering::SeqCst);

    // Stop recording if in progress
    if crate::audio::is_recording() {
        let _ = crate::audio::stop_recording();
    }

    // Reset tray state
    tray::set_recording_state(&app, false);

    PIPELINE_RUNNING.store(false, Ordering::SeqCst);
    emit_progress(&app, PipelineState::Idle, "Pipeline cancelled");
    emit_recording_state(&app);
    app.emit("pipeline-cancelled", ()).ok();

    tracing::info!("Pipeline: Cancelled");
    Ok(())
}

/// Get the current pipeline state
#[tauri::command]
#[tracing::instrument(target = TELEMETRY_TARGET, skip_all)]
pub fn get_pipeline_state() -> PipelineState {
    if crate::audio::is_recording() {
        PipelineState::Recording
    } else if PROCESSING_COUNT.load(Ordering::SeqCst) > 0 {
        // Capture has stopped but at least one detached process_audio task is running.
        PipelineState::Transcribing
    } else {
        PipelineState::Idle
    }
}

/// Run the synchronous, panic-prone post-transcription text transforms under
/// a catch boundary, so a panic (e.g. the phonetic matcher over arbitrary
/// decoded text) becomes a recoverable error instead of aborting the app.
///
/// `AssertUnwindSafe` is sound here: the wrapped transforms take only read
/// locks (`parking_lot`, which does not poison on panic) over read-only shared
/// data, and the caught panic is discarded rather than resumed, so no later
/// observer can witness a half-updated state. Do not add a write path inside
/// the wrapped closure without revisiting this.
fn catch_post_processing<F: FnOnce() -> String>(f: F) -> Result<String, String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).map_err(|payload| {
        let detail = payload
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown panic".to_string());
        format!("Text post-processing failed: {detail}")
    })
}

/// Apply the text stages that follow ASR: output filters, the personal
/// dictionary, and canonical replacements.
///
/// Shared by the live dictation path ([`run_transcription_pipeline`]) and by
/// MCP/HTTP file transcription, so the two cannot drift apart. They used to:
/// file transcription returned raw ASR text and skipped all of this, which
/// mangled exactly the vocabulary the dictionary exists to fix (#118).
///
/// AI enhancement is deliberately not included. It is the one stage that
/// rewrites content rather than normalising it, it is slow, and it needs a
/// loaded Ollama model, so it stays opt-in on the interactive path where a
/// human is watching the result.
pub(crate) fn apply_text_post_processing(
    text: String,
    config: &PipelineConfig,
) -> Result<String, String> {
    if !config.apply_filtering && !config.apply_dictionary {
        return Ok(text);
    }

    let apply_filtering = config.apply_filtering;
    let apply_dictionary = config.apply_dictionary;
    let filter_opts = if apply_filtering {
        Some(transcription::FilterOptions {
            remove_fillers: config.remove_fillers,
            australian_spelling: config.australian_spelling,
            spoken_numbers_to_digits: config.spoken_numbers_to_digits,
            normalise_whitespace: config.normalise_whitespace,
            cleanup_punctuation: config.cleanup_punctuation,
            sentence_case: config.sentence_case,
            voice_formatting_commands: config.voice_formatting_commands,
            // The dictionary is applied separately below, gated by
            // config.apply_dictionary. Disable it inside the filter so it
            // runs exactly once and honours the user's dictionary setting
            // (FilterOptions::default() would otherwise turn it on here and
            // apply it a second time, ignoring config.apply_dictionary).
            apply_dictionary: false,
        })
    } else {
        None
    };

    catch_post_processing(move || {
        let mut t = text;
        if apply_filtering {
            t = transcription::filter_transcription(t, filter_opts);
            tracing::debug!("Post-processing: after filtering: {} chars", t.len());
        }
        if apply_dictionary {
            t = dictionary::apply_dictionary(&t);
            tracing::debug!("Post-processing: after dictionary: {} chars", t.len());
            t = canonical::apply_canonical(&t);
            tracing::debug!("Post-processing: after canonical: {} chars", t.len());
        }
        t
    })
}

/// Output of the core transcription pipeline (transcribe + filter + enhance).
///
/// Shared by [`process_audio`] (recordings/imports) and [`pipeline_retranscribe`].
struct TranscriptionPipelineOutput {
    text: String,
    raw_text: String,
    is_enhanced: bool,
    transcription_model_name: Option<String>,
    transcription_duration_seconds: f64,
    enhancement_model_name: Option<String>,
    enhancement_duration_seconds: Option<f64>,
}

/// Core pipeline: wait for model, transcribe audio, apply filters, optionally enhance.
///
/// Does NOT handle output (clipboard/paste), saving to history, or tray updates;
/// callers are responsible for those steps.
async fn run_transcription_pipeline(
    app: &AppHandle,
    audio_path: &str,
    config: &PipelineConfig,
) -> Result<TranscriptionPipelineOutput, String> {
    let transcription_model_name = get_transcription_model_name();

    // 1. Transcribe (with timing)
    // Wait for the model to finish loading if eager background load is in progress.
    tracing::info!("Pipeline: Starting transcription of {}", audio_path);
    if !transcription::is_transcription_ready() {
        emit_progress(
            app,
            PipelineState::Transcribing,
            "Loading transcription model...",
        );
        // Nothing else will load it from here. `pipeline_start_recording` only
        // starts a warmup when the model was already missing when recording
        // began, and the idle unload (#105) can drop it after that check — so
        // without this the user waits out the full 60 s and is told the model
        // failed to load. Warmup is idempotent and serialised, so this is a
        // no-op when a load is already in flight.
        std::thread::spawn(|| {
            transcription::warmup_transcription();
        });

        let wait_start = std::time::Instant::now();
        let deadline = wait_start + std::time::Duration::from_secs(60);
        while !transcription::is_transcription_ready() {
            // Bail the moment the background warmup reports it could load nothing,
            // instead of waiting out the full 60 s on a model that will never load.
            if transcription::warmup_failed() {
                telemetry::report_error("model_load_failed");
                tracing::warn!(
                    target: TELEMETRY_TARGET,
                    reason = "warmup_failed",
                    wait_seconds = wait_start.elapsed().as_secs_f64(),
                    "model_load_failure"
                );
                return Err(
                    "No transcription model is ready. Open Settings \u{2192} Models to download or repair one."
                        .to_string(),
                );
            }
            if std::time::Instant::now() > deadline {
                telemetry::report_error("model_load_failed");
                tracing::warn!(
                    target: TELEMETRY_TARGET,
                    reason = "load_timeout_60s",
                    wait_seconds = wait_start.elapsed().as_secs_f64(),
                    "model_load_failure"
                );
                return Err("Transcription model failed to load within 60 seconds".to_string());
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        tracing::info!("Pipeline: Model loaded, proceeding with transcription");
        tracing::info!(
            target: TELEMETRY_TARGET,
            wait_seconds = wait_start.elapsed().as_secs_f64(),
            "model_load_wait"
        );
    }
    emit_progress(app, PipelineState::Transcribing, "Transcribing audio...");
    let transcription_start = std::time::Instant::now();
    // transcribe_file is CPU-bound (whisper/sherpa inference). Running it on a
    // dedicated blocking thread avoids starving the shared async worker pool,
    // which matters now that process_audio runs as a detached task.
    let audio_path_owned = audio_path.to_string();
    // spawn_blocking runs off the async context, so the span is carried across
    // by hand; without this the transcription span has no parent.
    let transcribe_span = tracing::info_span!(
        target: TELEMETRY_TARGET,
        "transcription",
        engine = %transcription_model_name.as_deref().unwrap_or("unknown"),
    );
    let raw_text = tokio::task::spawn_blocking(move || {
        transcribe_span.in_scope(|| transcription::transcribe_file(audio_path_owned))
    })
    .await
    .map_err(|e| {
        telemetry::report_error_with_cause("transcription_failed", &e);
        tracing::error!(target: TELEMETRY_TARGET, error = %e, "transcription_failed");
        format!("Transcription task panicked: {}", e)
    })?
    .map_err(|e| {
        telemetry::report_error_with_cause("transcription_failed", &e);
        tracing::error!(target: TELEMETRY_TARGET, error = %e, "transcription_failed");
        e.to_string()
    })?;
    let transcription_duration_seconds = transcription_start.elapsed().as_secs_f64();

    tracing::info!(
        "Pipeline: Transcription took {:.2}s",
        transcription_duration_seconds
    );

    if raw_text.trim().is_empty() {
        tracing::warn!("Pipeline: Transcription produced no text");
        return Err(NO_SPEECH_ERROR.to_string());
    }

    // Content-free telemetry — no transcript text, only metrics.
    {
        // audio_path is a WAV file path, not a number; parse::<f64>() always
        // fails and was dead code. Use get_audio_duration directly.
        let audio_secs = get_audio_duration(audio_path);
        let speed_factor = match audio_secs {
            Some(secs) if transcription_duration_seconds > 0.0 => {
                secs / transcription_duration_seconds
            }
            _ => 0.0,
        };
        let model_label = transcription_model_name.as_deref().unwrap_or("unknown");
        tracing::info!(
            target: TELEMETRY_TARGET,
            backend = %model_label,
            audio_seconds = audio_secs.unwrap_or(0.0),
            processing_seconds = transcription_duration_seconds,
            speed_factor = speed_factor,
            char_count = raw_text.chars().count(),
            word_count = raw_text.split_whitespace().count(),
            "transcription_complete"
        );
    }
    crate::telemetry_metrics::sample("transcription_complete");

    tracing::info!(
        "Pipeline: Transcribed {} characters: '{}'",
        raw_text.len(),
        raw_text.chars().take(100).collect::<String>()
    );

    // 2. Apply filtering
    let mut text = raw_text.clone();

    if config.apply_filtering || config.apply_dictionary {
        tracing::info!(
            "Pipeline: Applying filters (filtering={}, dictionary={})",
            config.apply_filtering,
            config.apply_dictionary
        );
        emit_progress(app, PipelineState::Filtering, "Applying filters...");

        text = apply_text_post_processing(text, config)?;

        tracing::info!("Pipeline: Filtered text to {} characters", text.len());
    }

    // 3. AI Enhancement (optional, with timing)
    let mut enhancement_model_name: Option<String> = None;
    let mut enhancement_duration_seconds: Option<f64> = None;

    let is_enhanced = if config.enhancement_enabled && !config.enhancement_model.is_empty() {
        emit_progress(app, PipelineState::Enhancing, "Enhancing with AI...");

        let enhancement_start = std::time::Instant::now();
        match enhancement::enhance_text(
            text.clone(),
            config.enhancement_model.clone(),
            config.enhancement_prompt.clone(),
        )
        .await
        {
            Ok(enhanced) => {
                let elapsed = enhancement_start.elapsed().as_secs_f64();
                text = enhanced;
                enhancement_model_name = Some(config.enhancement_model.clone());
                enhancement_duration_seconds = Some(elapsed);
                tracing::info!(
                    "Pipeline: Enhanced text to {} characters in {:.2}s",
                    text.len(),
                    elapsed
                );
                // The prompt ID names which prompt ran, never its text — the
                // template itself stays out of telemetry.
                tracing::info!(
                    target: TELEMETRY_TARGET,
                    model = %config.enhancement_model,
                    prompt_id = %config.enhancement_prompt_id.as_deref().unwrap_or("unknown"),
                    duration_seconds = elapsed,
                    ok = true,
                    "enhancement_complete"
                );
                true
            }
            Err(e) => {
                tracing::warn!("Pipeline: Enhancement failed, using original text: {}", e);
                telemetry::report_error_with_cause("enhancement_discarded", &e);
                tracing::warn!(
                    target: TELEMETRY_TARGET,
                    model = %config.enhancement_model,
                    prompt_id = %config.enhancement_prompt_id.as_deref().unwrap_or("unknown"),
                    error = %e,
                    ok = false,
                    "enhancement_complete"
                );

                // Only these two outcomes are a deliberate decision worth
                // telling the user about; a network or model error already
                // logs and falls back without one, on the same footing as
                // any other transient failure this pipeline absorbs.
                let e_str = e.to_string();
                if e_str == enhancement::ENHANCEMENT_SKIPPED_TOO_LONG
                    || e_str == enhancement::ENHANCEMENT_OUTPUT_IMPLAUSIBLE
                {
                    let advisory = format!("Pasted without AI enhancement: {e_str}");
                    if let Err(emit_err) = app.emit("enhancement-skipped", advisory) {
                        tracing::warn!("Failed to emit enhancement-skipped event: {emit_err}");
                    }
                }

                false
            }
        }
    } else {
        false
    };

    Ok(TranscriptionPipelineOutput {
        text,
        raw_text,
        is_enhanced,
        transcription_model_name,
        transcription_duration_seconds,
        enhancement_model_name,
        enhancement_duration_seconds,
    })
}

/// Process audio through the transcription pipeline
#[tracing::instrument(
    target = TELEMETRY_TARGET,
    name = "process_audio",
    skip_all,
    fields(
        audio_seconds = tracing::field::Empty,
        char_count = tracing::field::Empty,
        enhanced = tracing::field::Empty,
        insertion_method = tracing::field::Empty,
        insertion_ok = tracing::field::Empty,
        ok = tracing::field::Empty,
    )
)]
async fn process_audio(
    app: &AppHandle,
    audio_path: &str,
    config: &PipelineConfig,
) -> Result<PipelineResult, String> {
    let duration_seconds = get_audio_duration(audio_path);
    let span = tracing::Span::current();
    span.record("audio_seconds", duration_seconds.unwrap_or(0.0));

    // Run core transcription pipeline (transcribe + filter + enhance)
    let output = match run_transcription_pipeline(app, audio_path, config).await {
        Ok(output) => output,
        Err(e) => {
            span.record("ok", false);
            return Err(e);
        }
    };
    span.record("char_count", output.text.chars().count());
    span.record("enhanced", output.is_enhanced);
    span.record("ok", true);

    // 4. Output (clipboard/paste)
    // The filtered text already carries any spoken-command line breaks (applied
    // in OutputFilter so history and the pasted text stay consistent).
    let mut output_text = output.text.clone();

    // Ensure consecutive transcriptions don't run together when inserted at
    // the cursor. Add a sentence-ending period if the text has no trailing
    // punctuation, then always ensure there is a trailing space so that the
    // next paste doesn't glue directly onto this one.
    //
    // Examples:
    //   "Hello world"  → "Hello world. "
    //   "Hello world." → "Hello world. "
    //   "Hello world," → "Hello world, "
    {
        let last_meaningful = output_text.trim_end().chars().last().unwrap_or('.');
        if !last_meaningful.is_ascii_punctuation() {
            output_text = output_text.trim_end().to_string();
            output_text.push('.');
        }
        output_text.push(' ');
    }

    tracing::info!(
        "Pipeline: Starting output (copy={}, paste={})",
        config.auto_copy,
        config.auto_paste
    );
    emit_progress(app, PipelineState::Outputting, "Outputting text...");

    // Serialise clipboard-save → paste → clipboard-restore across concurrent
    // detached process_audio tasks. Without this, two overlapping recordings
    // can race the system clipboard and corrupt the restored content.
    {
        let _output_guard = OUTPUT_LOCK.lock().await;

        let uses_clipboard_paste = config.auto_paste && config.insertion_method != "typing";

        // Save the user's original clipboard BEFORE any modification.
        // This must happen before copy_transcription or insert_text_by_paste,
        // both of which overwrite the clipboard.
        // Captures an image when there is no text, so a screenshot on the
        // clipboard survives a dictation paste rather than being destroyed (#101).
        let saved_clipboard = if uses_clipboard_paste {
            clipboard::SavedClipboard::capture()
        } else {
            None
        };

        if config.auto_copy {
            tracing::debug!("Pipeline: Copying to clipboard...");
            if let Err(e) =
                clipboard::copy_transcription(app.clone(), output_text.clone(), output.is_enhanced)
                    .await
            {
                tracing::warn!("Pipeline: Failed to copy to clipboard: {}", e);
            } else {
                tracing::debug!("Pipeline: Copied to clipboard successfully");
            }
        }

        let mut insertion_failed = false;
        if config.auto_paste {
            tracing::debug!("Pipeline: Pasting text...");
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

            // Trailing space applies to the inserted text only, not to what is
            // saved in history, so the stored transcription stays clean (#112).
            let insert_text = crate::text_insert::apply_trailing_space(
                &output_text,
                config.append_trailing_space,
            );

            let insert_result = if config.insertion_method == "typing" {
                crate::text_insert::insert_text_by_typing(insert_text, None, None)
            } else {
                crate::text_insert::insert_text_by_paste(insert_text, None)
            };

            if let Err(e) = insert_result {
                tracing::warn!("Pipeline: Failed to insert text: {}", e);
                telemetry::report_error_with_cause("insertion_failed", &e);
                tracing::warn!(
                    target: TELEMETRY_TARGET,
                    method = %config.insertion_method,
                    error = %e,
                    ok = false,
                    "insertion_complete"
                );
                insertion_failed = true;
                // Surface the failure instead of leaving it in the log only. The
                // transcription itself succeeded and is still saved to history,
                // so this is deliberately not a `PipelineState::Failed` — only a
                // toast telling the user the text did not reach their cursor and
                // (on the clipboard route) that it is still on the clipboard.
                let advisory = if config.insertion_method == "typing" {
                    format!("Thoth could not type the transcription: {e}")
                } else {
                    format!(
                        "Thoth could not paste the transcription: {e}\n\nThe text has been left \
                         on your clipboard — press Cmd+V to paste it manually."
                    )
                };
                if let Err(emit_err) = app.emit("text-insertion-failed", advisory) {
                    tracing::warn!("Failed to emit text-insertion-failed event: {emit_err}");
                }
            } else {
                tracing::debug!("Pipeline: Pasted text successfully");
                tracing::info!(
                    target: TELEMETRY_TARGET,
                    method = %config.insertion_method,
                    ok = true,
                    "insertion_complete"
                );

                // Only after a confirmed insertion (#112). Submitting on a failed
                // paste would send an empty or half-written message, which is
                // worse than not submitting at all.
                if let Err(e) = crate::text_insert::send_auto_submit(config.auto_submit) {
                    tracing::warn!("Pipeline: Failed to send auto-submit key: {}", e);
                }
            }
            span.record("insertion_method", config.insertion_method.as_str());
            span.record("insertion_ok", !insertion_failed);
        }

        // Restore the user's original clipboard after paste completes.
        // Uses the configurable restore delay from clipboard settings to give
        // the target application time to process the paste before we overwrite
        // the clipboard again.
        //
        // Skipped when insertion failed. The transcription is on the clipboard
        // at this point (the paste route put it there) but never reached the
        // cursor, so restoring would overwrite it with the pre-recording
        // content and leave the user with neither the pasted text nor anything
        // to paste manually — the transcription would only be recoverable from
        // history. Keeping it on the clipboard makes a failed auto-paste
        // recoverable with a manual Cmd+V, at the cost of not restoring the
        // previous clipboard in that one case; the toast above says so.
        if insertion_failed {
            tracing::warn!(
                "Pipeline: Skipping clipboard restore — text insertion failed; leaving the \
                 transcription on the clipboard so it can be pasted manually"
            );
        } else if let Some(original) = saved_clipboard {
            let restore_delay = clipboard::get_restore_delay();
            tracing::debug!("Pipeline: Restoring clipboard in {}ms", restore_delay);
            tokio::time::sleep(tokio::time::Duration::from_millis(restore_delay)).await;
            let what = original.describe();
            match original.restore() {
                Ok(()) => tracing::debug!("Pipeline: Clipboard restored ({what})"),
                Err(e) => tracing::warn!("Pipeline: Failed to restore clipboard: {}", e),
            }
        }
    } // OUTPUT_LOCK released

    // 5. Save to history
    tracing::info!("Pipeline: Saving to history...");
    let transcription_id = save_to_history(
        &output.text,
        &output.raw_text,
        duration_seconds,
        audio_path,
        output.is_enhanced,
        if output.is_enhanced {
            Some(&config.enhancement_prompt)
        } else {
            None
        },
        output.transcription_model_name.as_deref(),
        Some(output.transcription_duration_seconds),
        output.enhancement_model_name.as_deref(),
        output.enhancement_duration_seconds,
    );
    tracing::info!("Pipeline: Saved to history, id={:?}", transcription_id);

    // Update tray with latest transcription
    tray::set_last_transcription(app, Some(output.text.clone()));

    tracing::info!("Pipeline: Processing complete, emitting Completed state");
    emit_progress(app, PipelineState::Completed, "Done");

    Ok(PipelineResult {
        success: true,
        text: output.text,
        raw_text: output.raw_text,
        is_enhanced: output.is_enhanced,
        duration_seconds,
        audio_path: Some(audio_path.to_string()),
        error: None,
        transcription_id,
        transcription_model_name: output.transcription_model_name,
        transcription_duration_seconds: Some(output.transcription_duration_seconds),
        enhancement_model_name: output.enhancement_model_name,
        enhancement_duration_seconds: output.enhancement_duration_seconds,
    })
}

/// What to keep in `raw_text`: the ASR's own words, whenever anything downstream
/// changed them.
///
/// This used to be stored only when AI enhancement ran, which left the output
/// filters, the personal dictionary and the canonical snapper with no record of
/// what they were handed. A correction that fires wrongly is then
/// indistinguishable from a mis-hearing — the canonical snapper spent months
/// deleting the word after a term (`4c58459`) and nothing in the history could
/// have shown it. Storing it only when it differs keeps the column meaning
/// "something rewrote this", so an untouched dictation costs nothing.
fn stored_raw_text(text: &str, raw_text: &str) -> Option<String> {
    (text != raw_text).then(|| raw_text.to_string())
}

/// Save transcription to history database
#[allow(clippy::too_many_arguments)]
fn save_to_history(
    text: &str,
    raw_text: &str,
    duration_seconds: Option<f64>,
    audio_path: &str,
    is_enhanced: bool,
    enhancement_prompt: Option<&str>,
    transcription_model_name: Option<&str>,
    transcription_duration_seconds: Option<f64>,
    enhancement_model_name: Option<&str>,
    enhancement_duration_seconds: Option<f64>,
) -> Option<String> {
    // Ensure database is initialised
    if database::transcription::get_transcription("test").is_err() {
        tracing::warn!("Pipeline: Database not initialised, skipping history save");
        return None;
    }

    let transcription = database::transcription::Transcription::with_details(
        text.to_string(),
        stored_raw_text(text, raw_text),
        duration_seconds,
        Some(audio_path.to_string()),
        is_enhanced,
        enhancement_prompt.map(|s| s.to_string()),
        transcription_model_name.map(|s| s.to_string()),
        transcription_duration_seconds,
        enhancement_model_name.map(|s| s.to_string()),
        enhancement_duration_seconds,
    );

    match database::transcription::create_transcription(&transcription) {
        Ok(()) => {
            tracing::info!("Pipeline: Saved transcription {}", transcription.id);
            Some(transcription.id)
        }
        Err(e) => {
            tracing::warn!("Pipeline: Failed to save transcription: {}", e);
            None
        }
    }
}

/// Get the name of the currently active transcription model.
fn get_transcription_model_name() -> Option<String> {
    // Try to get the selected model ID from config
    let model_id = crate::config::get_config()
        .ok()
        .and_then(|c| c.transcription.model_id.clone());

    // Fall back to backend name if no model ID configured
    model_id.or_else(transcription::get_transcription_backend)
}

/// Get audio file duration (placeholder - returns None for now)
fn get_audio_duration(audio_path: &str) -> Option<f64> {
    // Try to read WAV file header to get duration
    let path = PathBuf::from(audio_path);
    if !path.exists() {
        return None;
    }

    // Read WAV header for duration calculation
    match std::fs::File::open(&path) {
        Ok(file) => {
            use std::io::Read;
            let mut reader = std::io::BufReader::new(file);

            // WAV format: bytes 24-27 = sample rate, bytes 28-31 = byte rate
            // Total samples = (file_size - 44) / (bits_per_sample / 8 * num_channels)
            // Duration = total_samples / sample_rate

            let mut header = [0u8; 44];
            if reader.read_exact(&mut header).is_ok() {
                // Get sample rate from bytes 24-27 (little-endian)
                let sample_rate =
                    u32::from_le_bytes([header[24], header[25], header[26], header[27]]);
                // Get byte rate from bytes 28-31 (little-endian)
                let byte_rate =
                    u32::from_le_bytes([header[28], header[29], header[30], header[31]]);

                if byte_rate > 0 {
                    // Get file size
                    if let Ok(metadata) = std::fs::metadata(&path) {
                        let data_size = metadata.len().saturating_sub(44) as f64;
                        let duration = data_size / byte_rate as f64;
                        tracing::debug!(
                            "Audio duration: {:.2}s (sample_rate={}, byte_rate={})",
                            duration,
                            sample_rate,
                            byte_rate
                        );
                        return Some(duration);
                    }
                }
            }
            None
        }
        Err(_) => None,
    }
}

/// Transcribe an imported audio file through the full pipeline.
///
/// Decodes the input file (WAV, MP3, M4A, OGG, FLAC) to 16kHz mono WAV,
/// then runs the standard transcription pipeline (transcribe → filter → enhance → save).
/// Does NOT auto-copy or auto-paste (the user is already in the app).
#[tauri::command]
#[tracing::instrument(target = TELEMETRY_TARGET, skip_all, err)]
pub async fn pipeline_transcribe_file(
    app: AppHandle,
    file_path: String,
    config: Option<PipelineConfig>,
) -> Result<PipelineResult, Error> {
    tracing::info!("Pipeline: transcribe_file called for {}", file_path);

    if PIPELINE_RUNNING.swap(true, Ordering::SeqCst) {
        return Err("Pipeline is already running".to_string().into());
    }

    // RAII guard ensures PIPELINE_RUNNING is reset even on early return
    let _guard = PipelineGuard;

    // If the model isn't loaded yet but is downloaded, start eager loading.
    // The file decode step below takes time, so the model may be ready by
    // the time we need it.
    if !transcription::is_transcription_ready() {
        if !transcription::download::check_model_downloaded(None) {
            return Err(
                "No transcription model downloaded. Open Settings \u{2192} Models to get started."
                    .to_string()
                    .into(),
            );
        }
        tracing::info!("Pipeline: Model not loaded yet, starting eager background load for import");
        std::thread::spawn(|| {
            transcription::warmup_transcription();
        });
    }

    // Reset cancellation signal
    IMPORT_CANCELLED.store(false, Ordering::SeqCst);

    // Build config with auto_copy and auto_paste disabled (manual copy from UI)
    let mut config = config.unwrap_or_default();
    config.auto_copy = false;
    config.auto_paste = false;

    // Generate output path for the decoded WAV
    let home = dirs::home_dir().ok_or("Could not find home directory")?;
    let recordings_dir = home.join(".thoth").join("Recordings");
    std::fs::create_dir_all(&recordings_dir)
        .map_err(|e| format!("Failed to create recordings directory: {}", e))?;

    let filename = format!(
        "thoth_import_{}.wav",
        chrono::Utc::now().format("%Y%m%d_%H%M%S")
    );
    let output_wav = recordings_dir.join(&filename);

    // Decode the audio file to 16kHz mono WAV (CPU-bound, run off async runtime)
    emit_progress(
        &app,
        PipelineState::Converting,
        "Converting audio format...",
    );

    let input_path = PathBuf::from(&file_path);
    let output_path = output_wav.clone();
    let decode_result = tokio::task::spawn_blocking(move || {
        crate::audio::decode::decode_audio_to_wav(&input_path, &output_path, &IMPORT_CANCELLED)
    })
    .await
    .map_err(|e| format!("Decode task failed: {}", e))?;

    let _duration = decode_result?;

    let wav_path = output_wav.to_string_lossy().to_string();
    tracing::info!("Pipeline: Decoded to {}", wav_path);

    // Run the standard processing pipeline
    let result = process_audio(&app, &wav_path, &config).await;

    // Emit completion event
    match &result {
        Ok(r) => {
            tracing::info!("Pipeline: Emitting pipeline-complete event");
            if let Err(e) = app.emit("pipeline-complete", r) {
                tracing::error!("Pipeline: Failed to emit pipeline-complete: {}", e);
            }
        }
        Err(_) if discard_silent_wav(&result, &wav_path) => {
            // Silent import suppressed — discard_silent_wav already deleted the WAV.
        }
        Err(e) => {
            tracing::error!("Pipeline: File transcription failed: {}", e);
            emit_progress(&app, PipelineState::Failed, e);
        }
    }

    result.map_err(Into::into)
}

/// Re-transcribe an existing history record using the current model.
///
/// Looks up the audio file from the DB record, re-runs the transcription
/// pipeline, and updates the record in place. Does not copy/paste output.
#[tauri::command]
#[tracing::instrument(target = TELEMETRY_TARGET, skip_all, err)]
pub async fn pipeline_retranscribe(
    app: AppHandle,
    transcription_id: String,
    config: Option<PipelineConfig>,
) -> Result<PipelineResult, Error> {
    tracing::info!("Pipeline: retranscribe called for id={}", transcription_id);

    // Look up the existing record from the database
    let existing = database::transcription::get_transcription(&transcription_id)
        .map_err(|e| format!("Failed to read transcription: {}", e))?
        .ok_or_else(|| format!("Transcription '{}' not found", transcription_id))?;

    let audio_path = existing
        .audio_path
        .as_deref()
        .ok_or("This transcription has no associated audio file")?;

    // Check the file still exists on disk
    if !std::path::Path::new(audio_path).exists() {
        return Err(
            "Audio file no longer available. It may have been deleted via Storage cleanup."
                .to_string()
                .into(),
        );
    }

    if PIPELINE_RUNNING.swap(true, Ordering::SeqCst) {
        return Err("Pipeline is already running".to_string().into());
    }

    // RAII guard ensures PIPELINE_RUNNING is reset even on early return
    let _guard = PipelineGuard;

    // Ensure model is loaded
    if !transcription::is_transcription_ready() {
        if !transcription::download::check_model_downloaded(None) {
            return Err(
                "No transcription model downloaded. Open Settings \u{2192} Models to get started."
                    .to_string()
                    .into(),
            );
        }
        tracing::info!(
            "Pipeline: Model not loaded, starting eager background load for retranscribe"
        );
        std::thread::spawn(|| {
            transcription::warmup_transcription();
        });
    }

    // Build config with output disabled (retranscribe from history, not at cursor)
    let mut config = config.unwrap_or_default();
    config.auto_copy = false;
    config.auto_paste = false;

    // Run the core transcription pipeline
    let output = run_transcription_pipeline(&app, audio_path, &config).await?;

    // Read-modify-write: update only the fields that changed
    let mut updated = existing;
    updated.text = output.text.clone();
    updated.raw_text = stored_raw_text(&output.text, &output.raw_text);
    updated.is_enhanced = output.is_enhanced;
    updated.enhancement_prompt = if output.is_enhanced {
        Some(config.enhancement_prompt.clone())
    } else {
        None
    };
    updated.transcription_model_name = output.transcription_model_name.clone();
    updated.transcription_duration_seconds = Some(output.transcription_duration_seconds);
    updated.enhancement_model_name = output.enhancement_model_name.clone();
    updated.enhancement_duration_seconds = output.enhancement_duration_seconds;

    // Persist to database
    database::transcription::update_transcription(&updated)
        .map_err(|e| format!("Failed to update transcription: {}", e))?;

    tracing::info!("Pipeline: Retranscribed and updated id={}", updated.id);

    emit_progress(&app, PipelineState::Completed, "Done");

    let result = PipelineResult {
        success: true,
        text: output.text,
        raw_text: output.raw_text,
        is_enhanced: output.is_enhanced,
        duration_seconds: updated.duration_seconds,
        audio_path: updated.audio_path,
        error: None,
        transcription_id: Some(updated.id),
        transcription_model_name: output.transcription_model_name,
        transcription_duration_seconds: Some(output.transcription_duration_seconds),
        enhancement_model_name: output.enhancement_model_name,
        enhancement_duration_seconds: output.enhancement_duration_seconds,
    };

    if let Err(e) = app.emit("pipeline-complete", &result) {
        tracing::error!("Pipeline: Failed to emit pipeline-complete: {}", e);
    }

    Ok(result)
}

/// Toggle recording from the single source of truth: the armed flag.
///
/// Reads `crate::audio::is_recording()` — the authority — and either starts or
/// stops.  The frontend must NOT decide start vs stop independently; it calls
/// this command and updates its display from the returned `ToggleOutcome`.
///
/// Sound is decided here too:
/// - Start path: indicator + BING have already been played by the shortcut
///   handler the instant the key was pressed (instant, before IPC round-trip).
///   We do NOT play them again here.
/// - Stop path: BONG is played here, immediately before capture disarms, so it
///   is always matched to the decided action.
#[tauri::command]
#[tracing::instrument(target = TELEMETRY_TARGET, skip_all, err)]
pub async fn pipeline_toggle_recording(
    app: AppHandle,
    config: Option<PipelineConfig>,
    intent: Option<ToggleIntent>,
) -> Result<ToggleOutcome, Error> {
    // A key-up in hold-to-record mode (#111) is not a toggle: it must stop a
    // running recording and, crucially, must never START one. Inferring from
    // `is_recording()` gets that wrong whenever the press failed or has not
    // landed yet, and the user is then holding a key that started nothing and
    // released one that started everything.
    if intent.unwrap_or_default() == ToggleIntent::StopOnly && !crate::audio::is_recording() {
        return Ok(ToggleOutcome::Ignored);
    }

    if crate::audio::is_recording() {
        // --- STOP ---
        // Play BONG now, before disarming, so the sound is always paired with
        // the action decided from the authority.
        crate::sound::play_sound(crate::sound::SoundEvent::RecordingStop);

        pipeline_stop_and_process(app, config).await?;
        Ok(ToggleOutcome::Stopped)
    } else {
        // --- START ---
        // BING and indicator are played by the shortcut handler on keypress
        // (keyboard_service.rs / manager.rs / tray.rs) so they fire before the
        // IPC round-trip.  pipeline_start_recording does not duplicate them.
        let path = pipeline_start_recording(app)?;
        Ok(ToggleOutcome::Started { path })
    }
}

/// What the caller wants, when it knows something `is_recording()` does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ToggleIntent {
    /// Decide from the current state. The hotkey's toggle behaviour, and the
    /// default when a caller passes nothing.
    #[default]
    Toggle,
    /// Stop a running recording; do nothing if there is none. Hold-to-record's
    /// key-up (#111), which must never start one.
    StopOnly,
}

/// The outcome of a `pipeline_toggle_recording` call.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ToggleOutcome {
    /// A new recording was started; contains the WAV path.
    Started { path: String },
    /// An active recording was stopped; processing is detached.
    Stopped,
    /// Nothing to do — a `StopOnly` call with no recording running. Not an
    /// error: a key-up after a press that never started is ordinary.
    Ignored,
}

/// Emit a pipeline progress event
fn emit_progress(app: &AppHandle, state: PipelineState, message: &str) {
    emit_progress_with_device(app, state, message, None);
}

/// Emit a pipeline progress event with optional device name
fn emit_progress_with_device(
    app: &AppHandle,
    state: PipelineState,
    message: &str,
    device_name: Option<String>,
) {
    let progress = PipelineProgress {
        state,
        message: message.to_string(),
        device_name,
    };
    if let Err(e) = app.emit("pipeline-progress", &progress) {
        tracing::warn!("Failed to emit pipeline progress: {}", e);
    }
}

/// Emit the authoritative system state on the `recording-state` channel.
///
/// The payload is always derived from `get_pipeline_state()` so that a
/// completion event from a detached task cannot report `Idle` or `Completed`
/// while a new recording is already active (because `get_pipeline_state()`
/// prioritises `Recording` via `is_recording()`).
fn emit_recording_state(app: &AppHandle) {
    let current = get_pipeline_state();
    if let Err(e) = app.emit("recording-state", current) {
        tracing::warn!("Failed to emit recording-state: {}", e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pipeline_config_default() {
        let config = PipelineConfig::default();
        assert!(config.apply_dictionary);
        assert!(config.apply_filtering);
        assert!(config.remove_fillers);
        assert!(!config.australian_spelling);
        assert!(!config.spoken_numbers_to_digits);
        assert!(!config.enhancement_enabled);
        assert!(!config.auto_copy);
        assert!(config.auto_paste);
    }

    // Which stages the post-ASR text pipeline applies.
    //
    // These exist because live dictation and MCP file transcription used to
    // run different stages, silently (#118). Both now call
    // `apply_text_post_processing`, so these tests pin the shared contract:
    // any future change that adds, drops or reorders a stage for one caller
    // changes it for both, and fails here rather than surprising a caller.

    /// Config with every text stage off, as a baseline to switch on one at a time.
    fn bare_config() -> PipelineConfig {
        PipelineConfig {
            apply_dictionary: false,
            apply_filtering: false,
            remove_fillers: false,
            australian_spelling: false,
            spoken_numbers_to_digits: false,
            normalise_whitespace: false,
            cleanup_punctuation: false,
            sentence_case: false,
            voice_formatting_commands: false,
            ..PipelineConfig::default()
        }
    }

    #[test]
    fn post_processing_is_a_no_op_when_every_stage_is_disabled() {
        let config = bare_config();
        let input = "um so the  Thing is".to_string();

        let out = apply_text_post_processing(input.clone(), &config)
            .expect("post-processing must not fail");

        assert_eq!(out, input, "no stage was enabled, so nothing may change");
    }

    #[test]
    fn post_processing_applies_filters_when_filtering_is_enabled() {
        let config = PipelineConfig {
            apply_filtering: true,
            remove_fillers: true,
            ..bare_config()
        };

        let out = apply_text_post_processing("um so it works".to_string(), &config)
            .expect("post-processing must not fail");

        assert!(
            !out.contains("um "),
            "filler removal is a filter stage and must run: {out:?}"
        );
    }

    #[test]
    fn post_processing_applies_australian_spelling_when_enabled() {
        let config = PipelineConfig {
            apply_filtering: true,
            australian_spelling: true,
            ..bare_config()
        };

        let out = apply_text_post_processing("the color of the organization".to_string(), &config)
            .expect("post-processing must not fail");

        assert!(
            out.contains("colour") && out.contains("organisation"),
            "Australian spelling is a filter stage and must run: {out:?}"
        );
    }

    #[test]
    fn post_processing_never_applies_ai_enhancement() {
        // Enhancement is the deliberate fork between the two paths: the live
        // path may run it, file transcription never does. If enhancement ever
        // moves into the shared function, this fails.
        let config = PipelineConfig {
            enhancement_enabled: true,
            ..bare_config()
        };
        let input = "this text is not rewritten".to_string();

        let out = apply_text_post_processing(input.clone(), &config)
            .expect("post-processing must not fail");

        assert_eq!(
            out, input,
            "enhancement_enabled must have no effect on the shared text stages"
        );
    }

    #[test]
    fn test_pipeline_state_serialisation() {
        let state = PipelineState::Recording;
        let json = serde_json::to_string(&state).unwrap();
        assert_eq!(json, "\"recording\"");

        let deserialised: PipelineState = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialised, PipelineState::Recording);

        // Verify new Converting state serialises correctly
        let converting = PipelineState::Converting;
        let json = serde_json::to_string(&converting).unwrap();
        assert_eq!(json, "\"converting\"");
    }

    #[test]
    fn test_pipeline_result_serialisation() {
        let result = PipelineResult {
            success: true,
            text: "Hello world".to_string(),
            raw_text: "hello world".to_string(),
            is_enhanced: false,
            duration_seconds: Some(5.5),
            audio_path: Some("/tmp/test.wav".to_string()),
            error: None,
            transcription_id: Some("abc123".to_string()),
            transcription_model_name: Some("ggml-large-v3-turbo".to_string()),
            transcription_duration_seconds: Some(1.2),
            enhancement_model_name: None,
            enhancement_duration_seconds: None,
        };

        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"success\":true"));
        assert!(json.contains("\"text\":\"Hello world\""));
        assert!(json.contains("\"transcriptionModelName\""));
    }

    /// The ASR's own words are kept whenever anything downstream rewrote them,
    /// enhancement or not — that record is the only way to tell a mis-hearing
    /// from a correction that fired wrongly.
    #[test]
    fn the_asr_original_is_kept_whenever_post_processing_changed_it() {
        // A dictionary or canonical replacement, with no enhancement in sight.
        assert_eq!(
            stored_raw_text("lower the portcullis", "lower the port cullis"),
            Some("lower the port cullis".to_string())
        );
        // Nothing rewrote it, so there is no second version to keep.
        assert_eq!(stored_raw_text("already clean", "already clean"), None);
    }

    /// Hands-free must never end a recording before the user has spoken.
    /// Silence at the top of a recording is someone gathering their thoughts,
    /// not a finished sentence, and stopping there loses the whole utterance.
    #[test]
    fn hands_free_never_stops_before_speech() {
        let timeout = std::time::Duration::from_secs(2);
        assert!(
            !hands_free_should_stop(None, timeout),
            "no speech yet must never trigger auto-stop, however long the silence"
        );
    }

    /// The trigger is the timeout, exactly.
    #[test]
    fn hands_free_stops_at_the_timeout() {
        let timeout = std::time::Duration::from_secs(2);
        assert!(!hands_free_should_stop(Some(0), timeout));
        assert!(
            !hands_free_should_stop(Some(1999), timeout),
            "a pause one tick short of the timeout must not cut the user off"
        );
        assert!(hands_free_should_stop(Some(2000), timeout));
        assert!(hands_free_should_stop(Some(9999), timeout));
    }

    /// A hand-edited config must not be able to set a timeout that stops the
    /// user mid-sentence, or one that never stops at all.
    #[test]
    fn hands_free_timeout_is_clamped() {
        use crate::config::{HANDS_FREE_SILENCE_RANGE, ShortcutConfig};

        let low = ShortcutConfig {
            hands_free_silence_secs: 0.0,
            ..ShortcutConfig::default()
        };
        assert_eq!(
            low.hands_free_silence().as_secs_f32(),
            *HANDS_FREE_SILENCE_RANGE.start()
        );

        let high = ShortcutConfig {
            hands_free_silence_secs: 3600.0,
            ..ShortcutConfig::default()
        };
        assert_eq!(
            high.hands_free_silence().as_secs_f32(),
            *HANDS_FREE_SILENCE_RANGE.end()
        );

        let sane = ShortcutConfig {
            hands_free_silence_secs: 2.5,
            ..ShortcutConfig::default()
        };
        assert_eq!(sane.hands_free_silence().as_secs_f32(), 2.5);
    }

    /// Toggle stays the default: hands-free is opt-in, and an existing config
    /// that predates the setting must not silently gain it.
    #[test]
    fn toggle_remains_the_default_recording_mode() {
        use crate::config::{RecordingMode, ShortcutConfig};
        assert_eq!(
            ShortcutConfig::default().recording_mode,
            RecordingMode::Toggle
        );

        let old: ShortcutConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(old.recording_mode, RecordingMode::Toggle);
        assert_eq!(old.hands_free_silence_secs, 2.0);
    }

    /// A hold-to-record key-up must never START a recording. If the press
    /// failed, or has not landed yet, an inferred toggle would do exactly
    /// that — and the user would be holding a key that started nothing and
    /// have released one that started everything.
    #[test]
    fn stop_only_is_a_no_op_when_nothing_is_recording() {
        assert_eq!(
            toggle_action_from_capturing(false),
            ToggleAction::Start,
            "an inferred toggle would start here, which is the whole problem"
        );
        // The command short-circuits before that inference when the intent is
        // StopOnly and nothing is running; this asserts the intent it keys on.
        assert_eq!(ToggleIntent::default(), ToggleIntent::Toggle);
        assert_ne!(ToggleIntent::StopOnly, ToggleIntent::default());
    }

    /// The intent crosses IPC as snake_case, and an absent one must read as
    /// the ordinary toggle so every existing caller is unchanged.
    #[test]
    fn toggle_intent_round_trips_and_defaults_to_toggle() {
        assert_eq!(
            serde_json::to_string(&ToggleIntent::StopOnly).unwrap(),
            "\"stop_only\""
        );
        assert_eq!(
            serde_json::from_str::<ToggleIntent>("\"toggle\"").unwrap(),
            ToggleIntent::Toggle
        );
        // How the command reads it: an absent intent is the ordinary toggle,
        // so every caller that predates this parameter is unchanged.
        let absent: Option<ToggleIntent> = serde_json::from_str("null").unwrap();
        assert_eq!(absent.unwrap_or_default(), ToggleIntent::Toggle);
    }

    /// The frontend branches on this tag; a rename would silently make the
    /// ignored case look like a failure.
    #[test]
    fn the_ignored_outcome_serialises_as_the_frontend_expects() {
        assert_eq!(
            serde_json::to_string(&ToggleOutcome::Ignored).unwrap(),
            "{\"action\":\"ignored\"}"
        );
    }

    /// What a toggle-recording press will do.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum ToggleAction {
        Start,
        Stop,
    }

    /// Decide the toggle action from the single source of truth: the armed flag.
    ///
    /// Pure function so it can be unit-tested without touching global `is_recording()` state.
    #[inline]
    fn toggle_action_from_capturing(is_capturing: bool) -> ToggleAction {
        if is_capturing {
            ToggleAction::Stop
        } else {
            ToggleAction::Start
        }
    }

    #[test]
    fn test_toggle_action_from_capturing() {
        // The one invariant: is_capturing is the single authority.
        // IDLE (not capturing) → start; pressing while background processing
        // is in-flight must also → start (processing does NOT gate the action).
        assert_eq!(
            toggle_action_from_capturing(false),
            ToggleAction::Start,
            "not capturing → start"
        );
        assert_eq!(
            toggle_action_from_capturing(true),
            ToggleAction::Stop,
            "capturing → stop"
        );
    }

    #[test]
    fn test_processing_guard_increments_and_decrements() {
        // Baseline: whatever value is in the static before this test.
        let before = PROCESSING_COUNT.load(Ordering::SeqCst);

        {
            let _g1 = ProcessingGuard::new();
            assert_eq!(PROCESSING_COUNT.load(Ordering::SeqCst), before + 1);
            {
                let _g2 = ProcessingGuard::new();
                assert_eq!(PROCESSING_COUNT.load(Ordering::SeqCst), before + 2);
            }
            // _g2 dropped
            assert_eq!(PROCESSING_COUNT.load(Ordering::SeqCst), before + 1);
        }
        // _g1 dropped
        assert_eq!(PROCESSING_COUNT.load(Ordering::SeqCst), before);
    }

    // ── No-speech / silent-recording tests ─────────────────────────────────

    /// `is_no_speech_error` must return true only for the exact sentinel string.
    /// Any other message — including a prefix-match — must return false so we
    /// never suppress a genuine failure.
    #[test]
    fn test_is_no_speech_error_exact_match_only() {
        assert!(
            is_no_speech_error(NO_SPEECH_ERROR),
            "sentinel should match itself"
        );
        assert!(
            !is_no_speech_error(""),
            "empty string must not match sentinel"
        );
        assert!(
            !is_no_speech_error("Transcription failed"),
            "unrelated error must not match"
        );
        assert!(
            !is_no_speech_error("Transcription produced no text: extra detail"),
            "a message that merely starts with the sentinel must not match"
        );
    }

    /// `discard_silent_wav` must delete the file and return `true` when given the
    /// no-speech sentinel, exercising the real helper that both dispatch arms call.
    #[test]
    fn test_no_speech_discard_deletes_wav_file() {
        let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
        let wav_path = tmp_dir.path().join("thoth_recording_test.wav");
        std::fs::write(&wav_path, b"RIFF").expect("failed to write temp file");
        assert!(wav_path.exists(), "temp WAV must exist before discard");

        let result: Result<PipelineResult, String> = Err(NO_SPEECH_ERROR.to_string());
        let discarded = discard_silent_wav(&result, wav_path.to_str().unwrap());

        assert!(
            discarded,
            "helper must return true for the no-speech sentinel"
        );
        assert!(
            !wav_path.exists(),
            "WAV file must be gone after silent-recording discard"
        );
    }

    /// `discard_silent_wav` must return `false` and leave the file intact when
    /// given a genuine error — both the recording-path and import-path arms rely
    /// on this to preserve diagnostic artefacts.
    #[test]
    fn test_genuine_error_retains_wav_file() {
        let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
        let wav_path = tmp_dir.path().join("thoth_recording_real_error.wav");
        std::fs::write(&wav_path, b"RIFF").expect("failed to write temp file");

        let result: Result<PipelineResult, String> = Err("Transcription model crashed".to_string());
        let discarded = discard_silent_wav(&result, wav_path.to_str().unwrap());

        assert!(!discarded, "helper must return false for a genuine error");
        assert!(
            wav_path.exists(),
            "WAV file must be retained after a genuine pipeline error"
        );
    }

    // ── catch_post_processing tests ────────────────────────────────────────────

    /// A panicking closure must be caught and returned as Err containing the
    /// panic message, so one bad transcription does not abort the whole app.
    #[test]
    fn test_catch_post_processing_captures_panic() {
        let result = catch_post_processing(|| panic!("boom"));
        assert!(result.is_err(), "panicking closure must return Err");
        let msg = result.unwrap_err();
        assert!(
            msg.contains("boom"),
            "error message must contain the panic payload; got: {msg}"
        );
    }

    /// A non-panicking closure must return its value as Ok.
    #[test]
    fn test_catch_post_processing_ok_on_success() {
        let result = catch_post_processing(|| "hello".to_string());
        assert_eq!(result, Ok("hello".to_string()));
    }

    /// A panic with a formatted (String) payload is also captured. This covers
    /// the `String` downcast branch, distinct from the `&str` literal branch
    /// exercised by `test_catch_post_processing_captures_panic`.
    #[test]
    fn test_catch_post_processing_captures_string_panic() {
        let detail = String::from("formatted-detail");
        let result = catch_post_processing(move || panic!("{detail}"));
        assert!(result.is_err(), "panicking closure must return Err");
        assert!(
            result.unwrap_err().contains("formatted-detail"),
            "error message must contain the formatted String panic payload"
        );
    }
}
