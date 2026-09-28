//! Thoth - Privacy-first voice transcription
//!
//! Desktop application for macOS and Linux.

use std::sync::Mutex;

use tauri::{Manager, RunEvent};

use crate::error::Error;

pub mod app_handle;
pub mod audio;
pub mod canonical;
pub mod changelog;
pub mod clipboard;
pub mod commands;
pub mod config;
pub mod control_api;
pub mod database;
pub mod dictionary;
pub mod enhancement;
pub mod error;
pub mod export;
pub mod keyboard_service;
pub mod mcp_server;
pub mod mouse_tracker;
pub mod pipeline;
pub mod platform;
pub mod recording_indicator;
pub mod shortcuts;
pub mod sound;
pub mod storage;
pub mod telemetry_metrics;
pub mod telemetry_settings;
pub mod text_insert;
#[cfg(target_os = "macos")]
mod traffic_lights;
pub mod transcription;
pub mod tray;

/// Header height in pixels (must match CSS --header-height)
#[cfg(target_os = "macos")]
const HEADER_HEIGHT: f64 = 52.0;

/// Traffic light X position (left margin)
#[cfg(target_os = "macos")]
const TRAFFIC_LIGHT_X: f64 = 13.0;

/// Register a single shortcut, using keyboard_service for standalone modifiers
fn register_single_shortcut(
    app: &tauri::AppHandle,
    id: &str,
    accelerator: &str,
    description: &str,
) -> Result<(), Error> {
    // Check if this is a standalone modifier shortcut (e.g., ShiftRight)
    if keyboard_service::is_modifier_shortcut(accelerator) {
        // Register with keyboard service
        if keyboard_service::register_modifier_shortcut(
            id.to_string(),
            accelerator.to_string(),
            description.to_string(),
        ) {
            Ok(())
        } else {
            Err(format!("Failed to register modifier shortcut: {}", accelerator).into())
        }
    } else {
        // Use regular global shortcut system
        shortcuts::register_shortcut(
            app.clone(),
            id.to_string(),
            accelerator.to_string(),
            description.to_string(),
        )
    }
}

/// Re-register all shortcuts from saved config.
///
/// Unregisters everything first, then registers from the current config.
/// Called by the frontend after clearing or resetting a shortcut.
#[tauri::command]
#[tracing::instrument(target = TELEMETRY_TARGET, skip_all, err)]
fn reregister_shortcuts(app: tauri::AppHandle) -> Result<(), Error> {
    // Unregister everything
    shortcuts::unregister_all_shortcuts(app.clone())?;

    // Re-register from config
    let cfg = config::get_config().map_err(|e| format!("Failed to load config: {}", e))?;
    register_shortcuts_from_config(&app, &cfg);

    tracing::info!("Re-registered all shortcuts from config");
    Ok(())
}

/// Register shortcuts from saved configuration
fn register_shortcuts_from_config(app: &tauri::AppHandle, cfg: &config::Config) {
    use shortcuts::manager::shortcut_ids;

    // On Wayland, the actual global-shortcut binding is owned by the XDG portal,
    // set up once here. On X11 this is a no-op and the per-shortcut registration
    // below binds via the Tauri plugin as on macOS.
    #[cfg(target_os = "linux")]
    shortcuts::linux::init_global_shortcuts(app);

    // Collect (id, accelerator, description) tuples for all configured shortcuts
    let shortcuts: Vec<(&str, &str, &str)> = [
        Some((
            shortcut_ids::TOGGLE_RECORDING,
            cfg.shortcuts.toggle_recording.as_str(),
            "Toggle recording",
        )),
        cfg.shortcuts.toggle_recording_alt.as_deref().map(|accel| {
            (
                shortcut_ids::TOGGLE_RECORDING_ALT,
                accel,
                "Toggle recording (alternative)",
            )
        }),
        cfg.shortcuts.copy_last.as_deref().map(|accel| {
            (
                shortcut_ids::COPY_LAST_TRANSCRIPTION,
                accel,
                "Copy last transcription",
            )
        }),
        cfg.shortcuts.toggle_enhancement.as_deref().map(|accel| {
            (
                shortcut_ids::TOGGLE_ENHANCEMENT,
                accel,
                "Toggle AI enhancement",
            )
        }),
    ]
    .into_iter()
    .flatten()
    .filter(|(_, accel, _)| !accel.is_empty())
    .collect();

    for (id, accelerator, description) in shortcuts {
        match register_single_shortcut(app, id, accelerator, description) {
            Ok(()) => tracing::info!("Registered {} shortcut: {}", id, accelerator),
            Err(e) => tracing::warn!("Failed to register {} shortcut: {}", id, e),
        }
    }

    // Start the keyboard service if any modifier shortcuts were registered
    keyboard_service::start_monitoring(app.clone());
}

/// Installs the `ring` crypto provider as the process-wide rustls default,
/// exactly once. reqwest is configured with `rustls-no-provider` (to avoid
/// aws-lc-sys, whose C build fails under the -march=armv8-a baseline the macOS
/// build sets for whisper.cpp), so a provider must be installed before any
/// reqwest `Client` is built or reqwest panics. Idempotent and safe to call
/// from every client-construction site, including unit tests that never run
/// `run()`.
pub(crate) fn ensure_crypto_provider() {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// The `tracing` target Thoth's own events and spans use to leave this device.
///
/// `telemetry::init` takes it as the allow-list. Exactly two things reach the
/// exporter:
///
/// - anything Thoth emits with `target: TELEMETRY_TARGET`;
/// - the shared crate's own outbound-HTTP client spans, which it force-allows
///   past this list and which carry the request method, the server host, the
///   server port and the response status — never the URL, path, query or error
///   text — so a call to the user's Ollama or OpenAI-compatible endpoint is
///   visible as a host and a status and nothing more.
///
/// Everything else, including every line that carries transcript text, stays on
/// stderr. The boundary is structural, not a redaction pass, so content cannot
/// leak by mistake.
pub(crate) const TELEMETRY_TARGET: &str = "telemetry";

/// The process's one outbound HTTP client: `reqwest` under the middleware that
/// opens a client span and carries W3C `traceparent`, so a call to Ollama or an
/// OpenAI-compatible endpoint joins the trace it was made from.
///
/// Built once; a clone shares the underlying connection pool. Per-request
/// timeouts, because the middleware client is built without any.
pub(crate) fn http_client() -> reqwest_middleware::ClientWithMiddleware {
    static CLIENT: std::sync::OnceLock<reqwest_middleware::ClientWithMiddleware> =
        std::sync::OnceLock::new();
    CLIENT.get_or_init(telemetry::http_client).clone()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    ensure_crypto_provider();

    let mut builder = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            // Another instance tried to launch — focus the existing main window
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--autostarted"]),
        ))
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_process::init());

    // Updater plugin only on desktop platforms (not mobile)
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        builder = builder.plugin(tauri_plugin_updater::Builder::new().build());
    }

    builder
        .setup(|app| {
            // The whole subscriber, installed before anything logs. On the main
            // thread, never inside a Tokio task: `init` builds the exporters'
            // blocking HTTP client. Where the fleet has set no
            // OTEL_EXPORTER_OTLP_ENDPOINT — a stranger's Mac — this is stderr and
            // nothing else.
            //
            // Managing the Guard is only half of it: Tauri drops NO managed state
            // at exit, so the RunEvent::Exit arm below is what actually drops it
            // and flushes the batch processors. Mutex<Option<_>> because
            // Manager::unmanage is deprecated and documented as unsafe.
            app.manage(Mutex::new(Some(telemetry::init(
                "thoth",
                env!("CARGO_PKG_VERSION"),
                &[TELEMETRY_TARGET],
            ))));

            // On a machine the fleet does not configure there is no
            // environment to read — a Dock-launched app inherits none — so the
            // saved endpoint from Settings is what points the exporter. Where
            // the environment set one it wins and this is a no-op.
            telemetry_settings::apply_saved(app.handle());

            tracing::info!("Thoth starting");

            let gpu_label = platform::get_gpu_info()
                .map(|g| g.gpu_name.unwrap_or_else(|| g.compiled_backend.clone()))
                .unwrap_or_else(|_| "unknown".to_string());
            tracing::info!(
                target: TELEMETRY_TARGET,
                version = env!("CARGO_PKG_VERSION"),
                os = std::env::consts::OS,
                gpu = %gpu_label,
                "app_start"
            );

            // Own-process RSS/CPU on a slow clock, so a memory or CPU trend is
            // visible without the operator having to reproduce it live.
            telemetry_metrics::spawn_periodic_sampler();

            // Store the app handle for the few deep paths that emit user-facing
            // events without a handle of their own (e.g. audio device fallback).
            app_handle::set(app.handle().clone());

            // Request microphone permission BEFORE any audio enumeration.
            // cpal's device enumeration touches CoreAudio which triggers the
            // system mic prompt implicitly — but with no completion handler,
            // so we can't detect the result. By calling requestAccess first,
            // we own the dialog and get the result via our completion handler
            // which emits a permission-changed event to the frontend.
            #[cfg(target_os = "macos")]
            {
                use platform::macos::MicrophoneStatus;
                if platform::macos::check_microphone_permission() == MicrophoneStatus::NotDetermined
                {
                    platform::macos::request_microphone_permission(app.handle().clone());
                }
            }

            // Initialise database early so tray menu queries work immediately
            database::initialise_database().map_err(|e| {
                tracing::error!("Failed to initialise database: {}", e);
                Box::new(e) as Box<dyn std::error::Error>
            })?;

            // Set up the system tray, best-effort. On Linux, tray-icon's
            // libappindicator-sys panics rather than returning an Err when
            // libayatana-appindicator3/libappindicator3 is not on the system
            // — a missing optional library the .deb declares as a dependency
            // but the AppImage and raw binary do not bundle. A caught panic
            // or an ordinary Err here logs and moves on rather than aborting
            // the rest of setup: a missing tray icon is cosmetic, and nothing
            // below this — shortcuts, transcription warmup — should depend
            // on it existing.
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| tray::setup_tray(app))) {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    tracing::error!("Tray icon setup failed, continuing without one: {e}")
                }
                Err(payload) => {
                    let detail = payload
                        .downcast_ref::<&str>()
                        .map(|s| (*s).to_string())
                        .or_else(|| payload.downcast_ref::<String>().cloned())
                        .unwrap_or_else(|| "unknown panic".to_string());
                    tracing::error!("Tray icon setup panicked, continuing without one: {detail}");
                }
            }

            // Load config and register shortcuts
            if let Ok(cfg) = config::get_config() {
                // The behaviour-shaping settings only — never a dictionary
                // entry, a custom prompt's text, or anything that names a
                // person or place.
                tracing::info!(
                    target: TELEMETRY_TARGET,
                    recording_mode = ?cfg.shortcuts.recording_mode,
                    auto_paste = cfg.transcription.auto_paste,
                    auto_copy = cfg.transcription.auto_copy,
                    enhancement_enabled = cfg.enhancement.enabled,
                    enhancement_backend = %cfg.enhancement.backend,
                    remove_fillers = cfg.transcription.remove_fillers,
                    australian_spelling = cfg.transcription.australian_spelling,
                    api_enabled = cfg.integrations.api_enabled,
                    mcp_enabled = cfg.integrations.mcp_enabled,
                    "config_loaded"
                );

                // Wire up the enhancement backend before the first pipeline run
                config::apply_enhancement_backend(&cfg.enhancement);

                // Register shortcuts from config
                let app_handle = app.handle().clone();
                register_shortcuts_from_config(&app_handle, &cfg);

                // On Linux/Wayland with no usable typing tool installed, advise
                // the user once so text insertion does not silently fall back
                // to a permission prompt. Which tools count depends on the
                // desktop, so the advisory works that out itself (#110).
                #[cfg(target_os = "linux")]
                text_insert::emit_linux_typing_advisory(&app_handle);

                // Linux: apply the saved window-decoration preference at startup.
                // With decorations off, the custom in-app close button (the
                // WindowControls component) takes over.
                #[cfg(target_os = "linux")]
                if !cfg.general.window_decorations {
                    if let Some(window) = app.get_webview_window("main") {
                        let _ = window.set_decorations(false);
                        tracing::info!("Window decorations disabled per config");
                    }
                }

                // Start the Local Control API if enabled in config. The API and
                // MCP server default on. The bearer token lives in its own
                // reset-proof store (control_api::token_store), generated once
                // and migrated from any legacy config.json token, so it never
                // rotates across reinstalls or config resets.
                if cfg.integrations.api_enabled {
                    let token = control_api::token_store::get_or_create_token();
                    let port = cfg.integrations.api_port;
                    let mcp = cfg.integrations.mcp_enabled;
                    tauri::async_runtime::spawn(async move {
                        if let Err(e) = control_api::start(port, token, mcp).await {
                            tracing::error!("Control API failed to start on launch: {}", e);
                        }
                    });
                }
            }

            // Version bookkeeping, on EVERY platform.
            //
            // Two things read this and only one of them is macOS-specific, so
            // the recording cannot be: macOS resets stale TCC grants after an
            // update, and every platform suppresses the release-notes modal on
            // a first launch (#113). Keeping the write inside the macOS block
            // left Linux with `last_run_version` permanently unset, so a fresh
            // Linux install opened with notes for a release it had never run.
            //
            // Underscore-prefixed because only the macOS branch below consumes
            // it; the work above happens on every platform regardless.
            let _update_from = {
                let current = env!("CARGO_PKG_VERSION").to_string();

                // A genuinely fresh install has no prior version recorded, and
                // this is the only moment that is knowable — the call below
                // overwrites it. Marking the notes seen here is what stops a
                // first launch opening with a changelog for a release the user
                // was never on.
                let fresh_install = config::get_config()
                    .map(|c| c.general.last_run_version.is_none())
                    .unwrap_or(false);
                if fresh_install && let Err(e) = config::record_whats_new_seen(&current) {
                    tracing::warn!("Failed to record initial what's-new version: {}", e);
                }

                // Record the running version by mutating only this field on the
                // live config and writing it — NOT via a get_config()/set_config()
                // round-trip, which re-saves every setting through that path's
                // preservation guards on a launch that changes nothing else.
                // record_last_run_version writes only when the version changed.
                match config::record_last_run_version(&current) {
                    // Some(prev): a different version was recorded before, i.e.
                    // a genuine update.
                    Ok(prev) => prev.map(|prev| (prev, current)),
                    Err(e) => {
                        tracing::error!("Failed to record last-run version: {}", e);
                        None
                    }
                }
            };

            // macOS-specific setup
            #[cfg(target_os = "macos")]
            {
                // Reset stale macOS permissions after an applied update.
                //
                // macOS keys TCC grants to the code-signing identity, which
                // changes on each build, so after an update the previously
                // granted microphone / accessibility / input-monitoring
                // permissions silently stop working; reset them once so the
                // user re-grants from a clean slate. A genuinely fresh install
                // does NOT trigger a reset — there is nothing stale yet, which
                // is why this reads the UPDATE rather than merely the version.
                if let Some((prev, current)) = &_update_from {
                    tracing::info!(
                        "Update detected ({} → {}); resetting macOS permissions",
                        prev,
                        current
                    );
                    // Spawn so the admin-prompt does not block window setup.
                    tauri::async_runtime::spawn_blocking(|| {
                        match platform::reset_permissions_after_update() {
                            Ok(msg) => tracing::info!("Post-update permission reset: {}", msg),
                            Err(e) => {
                                tracing::warn!("Post-update permission reset skipped: {}", e)
                            }
                        }
                    });
                }

                // Set dock visibility based on user config
                let show_dock = config::get_config()
                    .map(|c| c.general.show_in_dock)
                    .unwrap_or(false);
                if show_dock {
                    app.set_activation_policy(tauri::ActivationPolicy::Regular);
                } else {
                    app.set_activation_policy(tauri::ActivationPolicy::Accessory);
                }

                // Position traffic lights and prevent main window destruction on close.
                // The main window hosts the pipeline event listeners — if it's destroyed,
                // global shortcuts stop working. Hide instead of close.
                if let Some(window) = app.get_webview_window("main") {
                    traffic_lights::setup_traffic_lights(&window, TRAFFIC_LIGHT_X, HEADER_HEIGHT);

                    let win = window.clone();
                    window.on_window_event(move |event| {
                        if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                            api.prevent_close();
                            let _ = win.hide();
                        }
                    });
                }

                // Log accessibility permission status on startup for diagnostics.
                // The frontend pulls the authoritative state via check_accessibility +
                // verify_accessibility_functional on mount — no event emission needed.
                let has_accessibility = platform::check_accessibility();
                if has_accessibility {
                    if platform::verify_accessibility_functional() {
                        tracing::info!("Accessibility permission granted and functional");
                    } else {
                        tracing::warn!(
                            "Accessibility permission appears granted but is stale — \
                             TCC entry may need resetting"
                        );
                    }
                } else {
                    tracing::warn!(
                        "Accessibility permission not granted - global shortcuts may not work"
                    );
                }
            }

            // Pre-warm the recording indicator window to eliminate first-show delay.
            // This loads the webview content in the background so it's ready instantly.
            recording_indicator::prewarm_indicator_window(app.handle());

            // Initialise mouse tracker for cursor-following recording indicator
            mouse_tracker::init(app.handle());

            // Pre-warm the transcription model to trigger Metal shader compilation.
            // This runs on a background thread so it doesn't block app startup.
            std::thread::spawn(|| {
                transcription::warmup_transcription();
            });

            // Reclaim the model's memory once the user has stopped dictating.
            // Off unless they set a timeout; the watcher re-reads the config
            // each tick, so it does not need restarting when they do.
            transcription::spawn_idle_unload_watcher();

            // Re-warm the model after wake-from-sleep (CoreML cache eviction)
            #[cfg(target_os = "macos")]
            platform::macos::register_wake_observer();

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // Commands
            commands::greet,
            commands::show_window,
            commands::open_url,
            commands::remove_quarantine,
            commands::open_privacy_pane,
            commands::relaunch_app,
            commands::set_show_in_dock,
            commands::get_show_in_dock,
            commands::set_audio_device,
            commands::report_update_check,
            // Platform
            platform::check_accessibility,
            platform::request_accessibility,
            platform::verify_accessibility_functional,
            platform::reset_tcc_permissions,
            platform::check_microphone_permission,
            platform::request_microphone_permission,
            platform::get_gpu_info,
            // Audio
            audio::device::list_audio_devices,
            audio::preview::start_audio_preview,
            audio::preview::stop_audio_preview,
            audio::start_recording,
            audio::stop_recording,
            audio::is_recording,
            // Transcription
            transcription::init_transcription,
            transcription::init_whisper_transcription,
            transcription::init_parakeet_transcription,
            transcription::transcribe_file,
            transcription::is_transcription_ready,
            transcription::get_transcription_backend,
            transcription::get_model_directory,
            transcription::get_whisper_model_directory,
            transcription::is_whisper_model_downloaded,
            transcription::filter_transcription,
            transcription::set_selected_model_id,
            transcription::init_fluidaudio_transcription,
            transcription::download::check_model_downloaded,
            transcription::download::get_download_progress,
            transcription::download::download_model,
            transcription::download::delete_model,
            transcription::download::reset_download_state,
            transcription::manifest::fetch_model_manifest,
            transcription::manifest::get_manifest_update_time,
            // Enhancement
            enhancement::check_ollama_available,
            enhancement::list_ollama_models,
            enhancement::check_openai_compat_available,
            enhancement::list_openai_compat_models,
            enhancement::enhance_text,
            enhancement::context::get_clipboard_context,
            enhancement::context::build_enhancement_context,
            // Prompt Templates
            enhancement::prompts::get_all_prompts,
            enhancement::prompts::get_builtin_prompts_cmd,
            enhancement::prompts::get_custom_prompts_cmd,
            enhancement::prompts::save_custom_prompt_cmd,
            enhancement::prompts::delete_custom_prompt_cmd,
            enhancement::prompts::get_prompt_by_id,
            // Database
            database::init_database,
            database::transcription::save_transcription,
            database::transcription::get_transcription_by_id,
            database::transcription::list_all_transcriptions,
            database::transcription::delete_transcription_by_id,
            database::transcription::delete_all_transcriptions_cmd,
            database::transcription::reconcile_orphaned_recordings_cmd,
            database::transcription::search_transcriptions_text,
            database::transcription::count_transcriptions_filtered,
            database::transcription::get_transcription_stats_cmd,
            database::insights::get_insights,
            database::insights::get_cruft_candidates,
            // Trash / quarantine
            database::trash::quarantine_recordings,
            database::trash::restore_recordings,
            database::trash::purge_trash,
            database::trash::list_trash,
            // Export
            export::search_history,
            export::export_to_json,
            export::export_to_csv,
            export::export_to_txt,
            // Config
            changelog::whats_new,
            changelog::mark_whats_new_seen,
            changelog::changelog_releases,
            config::get_config,
            config::get_default_config,
            config::set_config,
            config::set_shortcut_config,
            config::set_enhancement_api_key,
            config::reset_config,
            config::get_config_path_cmd,
            // Shortcuts
            shortcuts::register_shortcut,
            shortcuts::unregister_shortcut,
            shortcuts::list_registered_shortcuts,
            shortcuts::get_default_shortcuts,
            shortcuts::register_default_shortcuts,
            shortcuts::unregister_all_shortcuts,
            shortcuts::try_register_shortcut,
            shortcuts::check_shortcut_available,
            shortcuts::get_shortcut_suggestions,
            reregister_shortcuts,
            // Dictionary
            dictionary::get_dictionary_entries,
            dictionary::add_dictionary_entry,
            dictionary::update_dictionary_entry,
            dictionary::remove_dictionary_entry,
            dictionary::import_dictionary,
            dictionary::export_dictionary,
            dictionary::apply_dictionary_to_text,
            // Canonical terms
            canonical::get_canonical_terms,
            canonical::add_canonical_term,
            canonical::update_canonical_term,
            canonical::remove_canonical_term,
            // Text insertion
            text_insert::insert_text,
            text_insert::insert_text_by_typing,
            text_insert::insert_text_by_paste,
            // Clipboard
            clipboard::copy_to_clipboard,
            clipboard::copy_transcription,
            clipboard::get_clipboard_settings,
            clipboard::set_clipboard_settings,
            clipboard::get_clipboard_history,
            clipboard::clear_clipboard_history,
            clipboard::remove_clipboard_history_entry,
            clipboard::copy_from_history,
            clipboard::restore_clipboard,
            clipboard::get_restore_delay,
            clipboard::paste_transcription,
            // Sound feedback
            sound::play_recording_start_sound,
            sound::play_recording_stop_sound,
            sound::play_transcription_complete_sound,
            sound::play_error_sound,
            sound::are_sounds_enabled,
            sound::set_sounds_enabled,
            sound::get_sound_volume,
            sound::set_sound_volume,
            // Pipeline (full recording -> transcription -> output flow)
            pipeline::pipeline_start_recording,
            pipeline::pipeline_stop_and_process,
            pipeline::pipeline_toggle_recording,
            pipeline::pipeline_transcribe_file,
            pipeline::pipeline_retranscribe,
            pipeline::pipeline_cancel,
            pipeline::get_pipeline_state,
            // Recording indicator
            recording_indicator::show_recording_indicator,
            recording_indicator::hide_recording_indicator,
            // Tray
            tray::get_tray_state_cmd,
            tray::refresh_tray_menu,
            // Storage management
            storage::get_storage_usage,
            storage::delete_all_recordings,
            storage::delete_fluidaudio_cache,
            storage::delete_all_data,
            // Keyboard service (shortcut capture + modifier monitoring)
            keyboard_service::enter_capture_mode,
            keyboard_service::exit_capture_mode,
            keyboard_service::check_input_monitoring,
            keyboard_service::request_input_monitoring,
            keyboard_service::try_start_keyboard_service,
            keyboard_service::report_key_event,
            // Control API
            control_api::get_integrations_status,
            control_api::set_api_enabled,
            control_api::set_mcp_enabled,
            control_api::get_api_token,
            control_api::rotate_api_token,
            control_api::set_api_port,
            // Telemetry
            telemetry_settings::telemetry_get,
            telemetry_settings::telemetry_set,
            telemetry_settings::telemetry_probe,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            if let RunEvent::Exit = event {
                // The only thing that flushes whatever the batch processors are
                // still holding. On the main thread, which is where the Guard
                // must drop: after its bounded flush it joins the exporters'
                // blocking client's own runtime thread.
                let guard = app
                    .state::<Mutex<Option<telemetry::Guard>>>()
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .take();
                drop(guard);
            }
        });
}
