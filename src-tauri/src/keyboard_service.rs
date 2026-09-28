//! Unified keyboard service for Thoth
//!
//! Manages a SINGLE device_query polling thread that operates in two modes:
//! - **Monitoring**: Detects registered modifier-only shortcuts (ShiftRight, etc.)
//!   and fires shortcut events. Standard key combos (F13, Cmd+Shift+Space) are
//!   handled separately by Tauri's GlobalShortcut plugin.
//! - **Capturing**: Reports all key presses as capture events for the Settings UI
//!   shortcut editor. Does NOT fire shortcut events.
//!
//! The mode is stored as an AtomicU8, so transitions are instant and atomic.
//! The polling loop checks the mode AFTER reading keys, eliminating any
//! race window between mode switch and key processing.
//!
//! This module replaces the previous `modifier_monitor.rs` and `keyboard_capture.rs`
//! which had two independent polling threads that raced against each other.

use crate::error::Error;
use device_query::{DeviceQuery, DeviceState, Keycode};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::thread;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Poll interval for keyboard state (ms)
const POLL_INTERVAL_MS: u64 = 20;

/// Cooldown between shortcut triggers to prevent double-firing (ms)
const TRIGGER_COOLDOWN_MS: u64 = 500;

/// Threshold for "brief press" vs "hold" in toggle mode (ms)
const BRIEF_PRESS_THRESHOLD_MS: u64 = 500;

// ---------------------------------------------------------------------------
// Mode state machine
// ---------------------------------------------------------------------------

/// Operating mode for the keyboard service
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeyboardMode {
    /// No polling. Thread exits when it sees this.
    Idle = 0,
    /// Polling for registered modifier shortcuts.
    Monitoring = 1,
    /// Polling to capture key input for the settings UI.
    Capturing = 2,
}

impl KeyboardMode {
    fn from_u8(val: u8) -> Self {
        match val {
            1 => Self::Monitoring,
            2 => Self::Capturing,
            _ => Self::Idle,
        }
    }
}

/// Current mode (atomic, lock-free)
static MODE: AtomicU8 = AtomicU8::new(0); // Idle

/// Whether the polling thread is currently alive
static THREAD_RUNNING: AtomicBool = AtomicBool::new(false);

// ---------------------------------------------------------------------------
// Modifier key types (ported from modifier_monitor.rs)
// ---------------------------------------------------------------------------

/// Modifier keys that can be used as standalone shortcuts
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ModifierKey {
    ShiftRight,
    ShiftLeft,
    ControlRight,
    ControlLeft,
    AltRight,
    AltLeft,
    MetaRight,
    MetaLeft,
}

impl ModifierKey {
    /// Convert from accelerator string
    pub fn from_accelerator(s: &str) -> Option<Self> {
        match s {
            "ShiftRight" => Some(Self::ShiftRight),
            "ShiftLeft" => Some(Self::ShiftLeft),
            "ControlRight" => Some(Self::ControlRight),
            "ControlLeft" => Some(Self::ControlLeft),
            "AltRight" => Some(Self::AltRight),
            "AltLeft" => Some(Self::AltLeft),
            "MetaRight" => Some(Self::MetaRight),
            "MetaLeft" => Some(Self::MetaLeft),
            _ => None,
        }
    }

    /// Convert to accelerator string
    pub fn to_accelerator(self) -> &'static str {
        match self {
            Self::ShiftRight => "ShiftRight",
            Self::ShiftLeft => "ShiftLeft",
            Self::ControlRight => "ControlRight",
            Self::ControlLeft => "ControlLeft",
            Self::AltRight => "AltRight",
            Self::AltLeft => "AltLeft",
            Self::MetaRight => "MetaRight",
            Self::MetaLeft => "MetaLeft",
        }
    }

    /// Convert to device_query Keycode
    fn to_keycode(self) -> Keycode {
        match self {
            Self::ShiftRight => Keycode::RShift,
            Self::ShiftLeft => Keycode::LShift,
            Self::ControlRight => Keycode::RControl,
            Self::ControlLeft => Keycode::LControl,
            Self::AltRight => Keycode::RAlt,
            Self::AltLeft => Keycode::LAlt,
            Self::MetaRight => Keycode::RMeta,
            Self::MetaLeft => Keycode::LMeta,
        }
    }
}

// ---------------------------------------------------------------------------
// Modifier shortcut registry
// ---------------------------------------------------------------------------

/// Registered modifier shortcut
#[derive(Debug, Clone)]
struct ModifierShortcut {
    id: String,
    modifier: ModifierKey,
    description: String,
}

/// State for tracking key press timing (monitoring mode)
#[derive(Debug, Default)]
struct KeyState {
    is_pressed: bool,
    press_time: Option<Instant>,
    last_trigger: Option<Instant>,
    hands_free_mode: bool,
}

/// Registry of modifier shortcuts
#[derive(Default)]
struct ModifierRegistry {
    shortcuts: HashMap<String, ModifierShortcut>,
    key_states: HashMap<ModifierKey, KeyState>,
}

static REGISTRY: OnceLock<RwLock<ModifierRegistry>> = OnceLock::new();

fn get_registry() -> &'static RwLock<ModifierRegistry> {
    REGISTRY.get_or_init(|| RwLock::new(ModifierRegistry::default()))
}

// ---------------------------------------------------------------------------
// Capture event types (ported from keyboard_capture.rs)
// ---------------------------------------------------------------------------

/// Captured key combination
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapturedShortcut {
    /// The formatted accelerator string (e.g., "CommandOrControl+Shift+Y")
    pub accelerator: String,
    /// Individual keys pressed (for display)
    pub keys: Vec<String>,
    /// Whether this is a valid shortcut
    pub is_valid: bool,
}

/// Event payload for real-time key updates during capture
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyCaptureEvent {
    /// Currently pressed keys
    pub keys: Vec<String>,
    /// Current accelerator string (may be incomplete)
    pub accelerator: String,
    /// Whether the current combination is valid
    pub is_valid: bool,
}

// ---------------------------------------------------------------------------
// Public API: modifier shortcut management
// ---------------------------------------------------------------------------

/// Check if an accelerator is a standalone modifier
pub fn is_modifier_shortcut(accelerator: &str) -> bool {
    ModifierKey::from_accelerator(accelerator).is_some()
}

/// Register a modifier shortcut for monitoring
pub fn register_modifier_shortcut(id: String, accelerator: String, description: String) -> bool {
    let Some(modifier) = ModifierKey::from_accelerator(&accelerator) else {
        return false;
    };

    let mut registry = get_registry().write();
    registry.shortcuts.insert(
        id.clone(),
        ModifierShortcut {
            id,
            modifier,
            description,
        },
    );
    registry.key_states.entry(modifier).or_default();

    tracing::info!(
        "Registered modifier shortcut: {} -> {:?}",
        accelerator,
        modifier
    );
    true
}

/// Unregister a modifier shortcut
pub fn unregister_modifier_shortcut(id: &str) -> bool {
    let mut registry = get_registry().write();
    if registry.shortcuts.remove(id).is_some() {
        tracing::info!("Unregistered modifier shortcut: {}", id);
        true
    } else {
        false
    }
}

/// Unregister all modifier shortcuts
pub fn unregister_all_modifier_shortcuts() {
    let mut registry = get_registry().write();
    registry.shortcuts.clear();
    registry.key_states.clear();
    tracing::info!("Unregistered all modifier shortcuts");
}

/// Check if a modifier shortcut is registered
pub fn is_modifier_shortcut_registered(id: &str) -> bool {
    get_registry().read().shortcuts.contains_key(id)
}

/// Get all registered modifier shortcuts
pub fn list_modifier_shortcuts() -> Vec<(String, String, String)> {
    get_registry()
        .read()
        .shortcuts
        .values()
        .map(|s| {
            (
                s.id.clone(),
                s.modifier.to_accelerator().to_string(),
                s.description.clone(),
            )
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Public API: mode management
// ---------------------------------------------------------------------------

/// Check if capture mode is currently active.
/// Used by GlobalShortcut callbacks to discard events during capture.
pub fn is_capture_active() -> bool {
    KeyboardMode::from_u8(MODE.load(Ordering::Acquire)) == KeyboardMode::Capturing
}

/// Start the monitoring thread (called on app startup).
/// Whether the modifier-key polling thread can run on this platform.
///
/// `device_query` reads global key state, which is unavailable on Wayland
/// (the compositor does not expose it). Returns false on Wayland so neither the
/// startup nor the runtime-restart path spins up a thread that can never see a
/// key. Always true off Linux and on X11.
fn modifier_monitoring_supported() -> bool {
    #[cfg(target_os = "linux")]
    {
        if crate::shortcuts::get_display_server() == crate::shortcuts::DisplayServer::Wayland {
            tracing::warn!(
                "Modifier key shortcuts are not supported on Wayland (device_query cannot read \
                 global key state). Use a function-key shortcut (F13, F14) instead."
            );
            return false;
        }
    }
    true
}

/// Only enters Monitoring if there are registered modifier shortcuts.
pub fn start_monitoring(app: AppHandle) {
    let has_shortcuts = !get_registry().read().shortcuts.is_empty();
    if !has_shortcuts {
        tracing::debug!("No modifier shortcuts registered, staying Idle");
        return;
    }

    if !modifier_monitoring_supported() {
        return;
    }

    MODE.store(KeyboardMode::Monitoring as u8, Ordering::Release);
    ensure_thread_running(app);
}

/// Stop the service entirely (for app shutdown)
pub fn stop_service() {
    MODE.store(KeyboardMode::Idle as u8, Ordering::Release);
    // Thread will exit on next iteration when it sees Idle
    tracing::info!("Keyboard service stopping");
}

/// Restart monitoring after shortcut registration changes.
/// Sets mode to Monitoring and ensures thread is running.
pub fn restart_monitoring(app: AppHandle) {
    let has_shortcuts = !get_registry().read().shortcuts.is_empty();
    if has_shortcuts && modifier_monitoring_supported() {
        // Clear stale key states
        get_registry().write().key_states.clear();
        MODE.store(KeyboardMode::Monitoring as u8, Ordering::Release);
        ensure_thread_running(app);
    } else {
        MODE.store(KeyboardMode::Idle as u8, Ordering::Release);
    }
}

// ---------------------------------------------------------------------------
// IPC Commands
// ---------------------------------------------------------------------------

/// Check if Input Monitoring permission is available (macOS)
#[tauri::command]
pub fn check_input_monitoring() -> bool {
    tauri_plugin_telemetry::traced_sync("check_input_monitoring", || {
        Ok::<_, std::convert::Infallible>({
            #[cfg(target_os = "macos")]
            {
                crate::platform::check_input_monitoring_permission()
            }
            #[cfg(not(target_os = "macos"))]
            {
                true
            }
        })
    })
    .unwrap()
}

/// Request Input Monitoring permission (opens System Preferences on macOS)
#[tauri::command]
pub fn request_input_monitoring() {
    tauri_plugin_telemetry::traced_sync("request_input_monitoring", || {
        #[cfg(target_os = "macos")]
        {
            crate::platform::open_input_monitoring_settings();
        }

        Ok::<_, std::convert::Infallible>(())
    })
    .unwrap()
}

/// Try to start the keyboard monitoring service.
///
/// Called by the frontend after Input Monitoring permission is newly granted,
/// so the keyboard service can start without requiring an app restart.
/// Idempotent: no-ops if already running or no modifier shortcuts are registered.
#[tauri::command]
pub fn try_start_keyboard_service(app: AppHandle) {
    tauri_plugin_telemetry::traced_sync("try_start_keyboard_service", || {
        start_monitoring(app);

        Ok::<_, std::convert::Infallible>(())
    })
    .unwrap()
}

/// Enter capture mode for shortcut recording in the settings UI.
///
/// The single polling thread switches from monitoring to capturing.
/// No shortcuts will fire while in capture mode.
///
/// Returns "native" or "webview" to indicate the capture backend.
#[tauri::command]
pub async fn enter_capture_mode(app: AppHandle) -> Result<String, Error> {
    tauri_plugin_telemetry::traced("enter_capture_mode", async move {
        #[cfg(target_os = "macos")]
        {
            if !crate::platform::check_input_monitoring_permission() {
                tracing::warn!("Input Monitoring permission not granted");
                return Err(
                    "Input Monitoring permission required. Please grant permission in \
                 System Preferences > Privacy & Security > Input Monitoring"
                        .to_string()
                        .into(),
                );
            }
        }

        #[cfg(target_os = "linux")]
        {
            if crate::shortcuts::is_wayland() {
                tracing::info!("Running on Wayland - using webview keyboard capture");
                // Still set Capturing mode so GlobalShortcut callbacks are suppressed
                MODE.store(KeyboardMode::Capturing as u8, Ordering::Release);
                // Unregister GlobalShortcuts
                crate::shortcuts::manager::unregister_all(&app)
                    .map_err(|e| format!("Failed to unregister shortcuts: {}", e))?;
                return Ok("webview".to_string());
            }
        }

        // 1. Switch mode FIRST (atomic, instant)
        //    The polling thread will see this on its next mode check.
        MODE.store(KeyboardMode::Capturing as u8, Ordering::Release);

        // 2. Clear all key states (prevents stale monitoring state leaking in)
        clear_all_key_states();

        // 3. Unregister Tauri GlobalShortcuts (F13, Cmd+Shift+Space, etc.)
        //    Even if a queued callback fires after this, it checks MODE and discards.
        crate::shortcuts::manager::unregister_all(&app)
            .map_err(|e| format!("Failed to unregister shortcuts: {}", e))?;

        // 4. Ensure polling thread is running
        ensure_thread_running(app);

        tracing::info!("Entered capture mode");
        Ok("native".to_string())
    })
    .await
}

/// Exit capture mode, returning to normal operation.
///
/// Re-registers all shortcuts from config (clean slate).
/// The frontend MUST save config before calling this.
#[tauri::command]
pub async fn exit_capture_mode(app: AppHandle) -> Result<(), Error> {
    tauri_plugin_telemetry::traced("exit_capture_mode", async move {
        // 1. Switch mode back (atomic, instant)
        let has_modifier_shortcuts = !get_registry().read().shortcuts.is_empty();
        let new_mode = if has_modifier_shortcuts {
            KeyboardMode::Monitoring
        } else {
            KeyboardMode::Idle
        };
        MODE.store(new_mode as u8, Ordering::Release);

        // 2. Clear all key states (prevents stale capture state leaking back)
        clear_all_key_states();

        // 3. Re-register ALL shortcuts from config (clean slate)
        let cfg =
            crate::config::get_config().map_err(|e| format!("Failed to load config: {}", e))?;
        crate::register_shortcuts_from_config(&app, &cfg);

        tracing::info!("Exited capture mode, shortcuts re-registered from config");
        Ok(())
    })
    .await
}

/// Report a key event from the webview (used on Wayland where native capture doesn't work)
// All args are required by the Tauri IPC contract; grouping would change the JS call-site.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn report_key_event(
    app: AppHandle,
    key: String,
    code: String,
    ctrl: bool,
    shift: bool,
    alt: bool,
    meta: bool,
    event_type: String,
) -> Result<(), Error> {
    tauri_plugin_telemetry::traced("report_key_event", async move {
        if !is_capture_active() {
            return Ok(());
        }

        tracing::debug!(
            "Webview key event: key={}, code={}, modifiers=({}{}{}{}), type={}",
            key,
            code,
            if ctrl { "Ctrl " } else { "" },
            if shift { "Shift " } else { "" },
            if alt { "Alt " } else { "" },
            if meta { "Meta " } else { "" },
            event_type
        );

        if event_type == "keydown" {
            process_webview_keydown(&app, &key, &code, ctrl, shift, alt, meta);
        }

        Ok(())
    })
    .await
}

// ---------------------------------------------------------------------------
// Thread management
// ---------------------------------------------------------------------------

/// Ensure the polling thread is running. Spawns one if not.
///
/// On macOS, `DeviceState::new()` calls IOKit which aborts the process if
/// the Input Monitoring TCC permission is missing. We check permission both
/// here (fast-path to avoid a pointless spawn) and inside the thread (to
/// close the TOCTOU window if permission is revoked between check and use).
fn ensure_thread_running(app: AppHandle) {
    #[cfg(target_os = "macos")]
    if !crate::platform::check_input_monitoring_permission() {
        tracing::warn!("Skipping keyboard service: Input Monitoring permission not granted");
        return;
    }

    if THREAD_RUNNING.swap(true, Ordering::SeqCst) {
        return; // Thread already running
    }

    tracing::info!("Spawning keyboard service polling thread");

    thread::spawn(move || {
        // Drop guard ensures THREAD_RUNNING is reset even on panic
        struct ThreadGuard;
        impl Drop for ThreadGuard {
            fn drop(&mut self) {
                THREAD_RUNNING.store(false, Ordering::SeqCst);
                tracing::info!("Keyboard service polling thread exited");
            }
        }
        let _guard = ThreadGuard;

        // Re-check inside the thread to close the TOCTOU window: permission
        // could have been revoked between the outer check and this point.
        #[cfg(target_os = "macos")]
        if !crate::platform::check_input_monitoring_permission() {
            tracing::warn!(
                "Input Monitoring permission revoked before thread could start — exiting cleanly"
            );
            return;
        }

        polling_loop(app);
    });
}

/// The single polling loop. Reads keys, then checks mode, then processes.
fn polling_loop(app: AppHandle) {
    // Use checked_new() instead of new() to avoid the assert-panic inside
    // DeviceState::new() if Input Monitoring permission was revoked between
    // the pre-spawn check and this point (TOCTOU residual).
    let Some(device_state) = DeviceState::checked_new() else {
        tracing::error!(
            "DeviceState::checked_new() returned None — Input Monitoring permission not available"
        );
        return;
    };
    let mut previous_keys: HashSet<Keycode> = HashSet::new();
    let mut previous_mode = KeyboardMode::Idle;

    loop {
        // 1. Read hardware state
        let keys: HashSet<Keycode> = device_state.get_keys().into_iter().collect();

        // 2. Read mode AFTER keys (critical ordering for race-proofing)
        let mode = KeyboardMode::from_u8(MODE.load(Ordering::Acquire));

        // 3. Handle mode transitions: reset previous_keys appropriately
        if mode != previous_mode {
            tracing::debug!("Mode transition: {:?} -> {:?}", previous_mode, mode);

            if mode == KeyboardMode::Capturing {
                // Monitoring → Capturing: clear previous_keys so any key press
                // (even one already held) is detected as a new capture event.
                previous_keys.clear();
            } else if mode == KeyboardMode::Monitoring && previous_mode == KeyboardMode::Capturing {
                // Capturing → Monitoring: seed previous_keys with CURRENT keys
                // so that keys still physically held (e.g. RShift that was just
                // captured) are NOT treated as new presses. Also pre-mark held
                // modifier keys as already-pressed in the registry.
                previous_keys = keys.clone();
                preseed_monitoring_state(&keys);
            } else {
                previous_keys.clear();
            }

            previous_mode = mode;
        }

        match mode {
            KeyboardMode::Idle => {
                // Exit the thread
                break;
            }
            KeyboardMode::Monitoring => {
                process_monitoring(&app, &keys);
                previous_keys = keys;
            }
            KeyboardMode::Capturing => {
                if keys != previous_keys {
                    let should_stop = process_capture(&app, &keys);
                    if should_stop {
                        // Valid shortcut captured; the frontend will call
                        // exit_capture_mode after saving config.
                        // We stay in Capturing mode until then.
                    }
                    previous_keys = keys;
                }
            }
        }

        thread::sleep(Duration::from_millis(POLL_INTERVAL_MS));
    }
}

// ---------------------------------------------------------------------------
// Monitoring mode processing (ported from modifier_monitor.rs)
// ---------------------------------------------------------------------------

/// Process keys in monitoring mode: detect modifier shortcut presses/releases
fn process_monitoring(app: &AppHandle, keys: &HashSet<Keycode>) {
    let shortcuts: Vec<ModifierShortcut> = {
        let registry = get_registry().read();
        registry.shortcuts.values().cloned().collect()
    };

    for shortcut in shortcuts {
        let keycode = shortcut.modifier.to_keycode();
        let is_pressed = keys.contains(&keycode);

        // Get current state
        let (was_pressed, press_time, last_trigger, hands_free) = {
            let registry = get_registry().read();
            let key_state = registry.key_states.get(&shortcut.modifier);
            (
                key_state.map(|s| s.is_pressed).unwrap_or(false),
                key_state.and_then(|s| s.press_time),
                key_state.and_then(|s| s.last_trigger),
                key_state.map(|s| s.hands_free_mode).unwrap_or(false),
            )
        };

        // Check cooldown
        if let Some(last) = last_trigger {
            if last.elapsed().as_millis() < TRIGGER_COOLDOWN_MS as u128 {
                continue;
            }
        }

        if is_pressed && !was_pressed {
            // Key just pressed
            {
                let mut registry = get_registry().write();
                if let Some(key_state) = registry.key_states.get_mut(&shortcut.modifier) {
                    key_state.is_pressed = true;
                    key_state.press_time = Some(Instant::now());
                }
            }

            if hands_free {
                // In hands-free mode, pressing again stops recording
                {
                    let mut registry = get_registry().write();
                    if let Some(key_state) = registry.key_states.get_mut(&shortcut.modifier) {
                        key_state.hands_free_mode = false;
                        key_state.last_trigger = Some(Instant::now());
                    }
                }
                emit_shortcut_event(app, &shortcut.id, "pressed");
            } else {
                // Start recording on press
                {
                    let mut registry = get_registry().write();
                    if let Some(key_state) = registry.key_states.get_mut(&shortcut.modifier) {
                        key_state.last_trigger = Some(Instant::now());
                    }
                }
                emit_shortcut_event(app, &shortcut.id, "pressed");
            }
        } else if !is_pressed && was_pressed {
            // Key just released
            let press_duration = press_time.map(|t| t.elapsed().as_millis()).unwrap_or(0);
            // A modifier binding reaches this path instead of the global-shortcut
            // dispatcher, so hold-to-record (#111) has to be honoured here too or
            // the setting is inert for anyone whose recording key is a bare
            // modifier — silently, which is the failure the earlier push-to-talk
            // attempt was deleted for.
            let hold_to_record = crate::shortcuts::manager::release_acts(
                crate::config::get_config()
                    .map(|c| c.shortcuts.recording_mode)
                    .unwrap_or_default(),
                &shortcut.id,
            );

            {
                let mut registry = get_registry().write();
                if let Some(key_state) = registry.key_states.get_mut(&shortcut.modifier) {
                    key_state.is_pressed = false;
                    key_state.press_time = None;

                    if hold_to_record {
                        // The user asked for hold-to-record, so the release
                        // ends the recording however brief the hold was. The
                        // tap-vs-hold heuristic below is what this path does
                        // when no mode has been chosen; it is not a second
                        // opinion on one that has.
                        key_state.hands_free_mode = false;
                        key_state.last_trigger = Some(Instant::now());
                    } else if press_duration < BRIEF_PRESS_THRESHOLD_MS as u128 {
                        // Brief press: enter hands-free mode (don't stop yet)
                        key_state.hands_free_mode = true;
                        tracing::debug!(
                            "Brief press detected ({}ms), entering hands-free mode",
                            press_duration
                        );
                    } else {
                        // Long press: stop recording
                        key_state.hands_free_mode = false;
                        key_state.last_trigger = Some(Instant::now());
                    }
                }
            }

            if hold_to_record || press_duration >= BRIEF_PRESS_THRESHOLD_MS as u128 {
                emit_shortcut_event(app, &shortcut.id, "released");
            }
        }
    }
}

/// Emit a shortcut event to the frontend
fn emit_shortcut_event(app: &AppHandle, id: &str, state: &str) {
    tracing::info!("Modifier shortcut {}: {}", state, id);

    if state == "pressed" {
        if id.contains("toggle_recording") {
            crate::recording_indicator::maybe_play_start_indicator(app);
        }

        if let Err(e) = app.emit("shortcut-triggered", id.to_string()) {
            tracing::error!("Failed to emit shortcut-triggered: {}", e);
        }
        return;
    }

    // A release only reaches the frontend in hold-to-record mode, on the same
    // stop-only channel the global-shortcut path uses (#111). Before this it
    // reached nothing at all: the long-press branch above computed "stop
    // recording" and then emitted an event this function dropped on the floor,
    // so a held modifier key started a recording that only a second press
    // could end.
    if crate::shortcuts::manager::release_acts(
        crate::config::get_config()
            .map(|c| c.shortcuts.recording_mode)
            .unwrap_or_default(),
        id,
    ) && let Err(e) = app.emit("shortcut-released", id.to_string())
    {
        tracing::error!("Failed to emit shortcut-released: {}", e);
    }
}

// ---------------------------------------------------------------------------
// Capture mode processing (ported from keyboard_capture.rs)
// ---------------------------------------------------------------------------

/// Process keys in capture mode. Returns true if a valid shortcut was captured.
fn process_capture(app: &AppHandle, keys: &HashSet<Keycode>) -> bool {
    let (accelerator, key_names, is_valid) = format_keys(keys);

    let event = KeyCaptureEvent {
        keys: key_names,
        accelerator: accelerator.clone(),
        is_valid,
    };

    if let Err(e) = app.emit("key-capture-update", &event) {
        tracing::warn!("Failed to emit key capture event: {}", e);
    }

    let is_standalone_right_modifier = keys.len() == 1 && keys.iter().any(is_right_modifier);
    if is_valid && (has_non_modifier_key(keys) || is_standalone_right_modifier) {
        tracing::info!("Valid shortcut captured: {}", accelerator);

        let result = CapturedShortcut {
            accelerator,
            keys: event.keys,
            is_valid: true,
        };

        if let Err(e) = app.emit("key-capture-complete", &result) {
            tracing::warn!("Failed to emit key capture complete: {}", e);
        }

        return true;
    }

    false
}

// ---------------------------------------------------------------------------
// Webview capture (Wayland fallback, ported from keyboard_capture.rs)
// ---------------------------------------------------------------------------

/// Process a webview keydown event (Wayland fallback)
fn process_webview_keydown(
    app: &AppHandle,
    key: &str,
    code: &str,
    ctrl: bool,
    shift: bool,
    alt: bool,
    meta: bool,
) {
    let mut parts: Vec<String> = Vec::new();
    let mut key_names: Vec<String> = Vec::new();

    if meta || ctrl {
        parts.push("CommandOrControl".to_string());
        key_names.push(if meta { "Super" } else { "Ctrl" }.to_string());
    }
    if alt {
        parts.push("Alt".to_string());
        key_names.push("Alt".to_string());
    }
    if shift {
        parts.push("Shift".to_string());
        key_names.push("Shift".to_string());
    }

    let main_key = webview_key_to_accelerator(key, code);
    if let Some(key_str) = &main_key {
        parts.push(key_str.clone());
        key_names.push(webview_key_to_display(key, code));
    }

    let accelerator = parts.join("+");
    let is_valid = main_key.is_some();

    let event = KeyCaptureEvent {
        keys: key_names.clone(),
        accelerator: accelerator.clone(),
        is_valid,
    };

    if let Err(e) = app.emit("key-capture-update", &event) {
        tracing::warn!("Failed to emit key capture event: {}", e);
    }

    if is_valid {
        tracing::info!("Valid shortcut captured via webview: {}", accelerator);

        let result = CapturedShortcut {
            accelerator,
            keys: key_names,
            is_valid: true,
        };

        if let Err(e) = app.emit("key-capture-complete", &result) {
            tracing::warn!("Failed to emit key capture complete: {}", e);
        }
    }
}

/// Convert webview key/code to Tauri accelerator format
fn webview_key_to_accelerator(key: &str, code: &str) -> Option<String> {
    match code {
        "F1" | "F2" | "F3" | "F4" | "F5" | "F6" | "F7" | "F8" | "F9" | "F10" | "F11" | "F12"
        | "F13" | "F14" | "F15" | "F16" | "F17" | "F18" | "F19" | "F20" => {
            return Some(code.to_string());
        }
        "Space" => return Some("Space".to_string()),
        "Enter" => return Some("Enter".to_string()),
        "Tab" => return Some("Tab".to_string()),
        "Backspace" => return Some("Backspace".to_string()),
        "Delete" => return Some("Delete".to_string()),
        "Insert" => return Some("Insert".to_string()),
        "Home" => return Some("Home".to_string()),
        "End" => return Some("End".to_string()),
        "PageUp" => return Some("PageUp".to_string()),
        "PageDown" => return Some("PageDown".to_string()),
        "ArrowUp" => return Some("Up".to_string()),
        "ArrowDown" => return Some("Down".to_string()),
        "ArrowLeft" => return Some("Left".to_string()),
        "ArrowRight" => return Some("Right".to_string()),
        // Skip pure modifiers
        "ControlLeft" | "ControlRight" | "ShiftLeft" | "ShiftRight" | "AltLeft" | "AltRight"
        | "MetaLeft" | "MetaRight" => return None,
        _ => {}
    }

    let key_upper = key.to_uppercase();
    if key_upper.len() == 1 {
        let c = key_upper.chars().next()?;
        if c.is_ascii_alphabetic() || c.is_ascii_digit() {
            return Some(key_upper);
        }
    }

    match key {
        " " => Some("Space".to_string()),
        "+" => Some("Plus".to_string()),
        "-" => Some("Minus".to_string()),
        _ => None,
    }
}

/// Convert webview key/code to display string
fn webview_key_to_display(key: &str, code: &str) -> String {
    match code {
        "ArrowUp" => "↑".to_string(),
        "ArrowDown" => "↓".to_string(),
        "ArrowLeft" => "←".to_string(),
        "ArrowRight" => "→".to_string(),
        "Space" => "Space".to_string(),
        "Enter" => "Return".to_string(),
        "Backspace" => "⌫".to_string(),
        "Tab" => "⇥".to_string(),
        _ => {
            if key.len() == 1 {
                key.to_uppercase()
            } else {
                key.to_string()
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Key utility functions (ported from keyboard_capture.rs)
// ---------------------------------------------------------------------------

/// Clear all key states (called on mode transitions)
fn clear_all_key_states() {
    let mut registry = get_registry().write();
    for state in registry.key_states.values_mut() {
        state.is_pressed = false;
        state.press_time = None;
        state.last_trigger = None;
        state.hands_free_mode = false;
    }
}

/// Pre-seed monitoring state for keys that are currently held down.
///
/// Called when transitioning from Capturing back to Monitoring. Without this,
/// a key held during capture (e.g. the Right Shift just captured as a shortcut)
/// would be seen as a fresh press and immediately trigger the shortcut.
fn preseed_monitoring_state(keys: &HashSet<Keycode>) {
    let registry = get_registry().read();
    let shortcuts: Vec<ModifierShortcut> = registry.shortcuts.values().cloned().collect();
    drop(registry);

    let mut registry = get_registry().write();
    for shortcut in &shortcuts {
        let keycode = shortcut.modifier.to_keycode();
        if keys.contains(&keycode) {
            if let Some(key_state) = registry.key_states.get_mut(&shortcut.modifier) {
                key_state.is_pressed = true;
                key_state.press_time = Some(Instant::now());
                // Set a recent trigger time to also enforce cooldown
                key_state.last_trigger = Some(Instant::now());
                tracing::debug!(
                    "Pre-seeded monitoring state for {:?} (key held during capture→monitoring transition)",
                    shortcut.modifier
                );
            }
        }
    }
}

/// Check if the key set contains any non-modifier key
fn has_non_modifier_key(keys: &HashSet<Keycode>) -> bool {
    keys.iter().any(|k| !is_modifier(k))
}

/// Check if a keycode is a right-side modifier (can be used as standalone key)
fn is_right_modifier(key: &Keycode) -> bool {
    matches!(
        key,
        Keycode::RShift | Keycode::RControl | Keycode::RAlt | Keycode::RMeta
    )
}

/// Check if a keycode is any modifier
fn is_modifier(key: &Keycode) -> bool {
    matches!(
        key,
        Keycode::LShift
            | Keycode::LControl
            | Keycode::LAlt
            | Keycode::LMeta
            | Keycode::RShift
            | Keycode::RControl
            | Keycode::RAlt
            | Keycode::RMeta
    )
}

/// Convert right modifier keycode to Tauri Code name for use as primary key
fn right_modifier_to_code(key: &Keycode) -> Option<String> {
    match key {
        Keycode::RShift => Some("ShiftRight".to_string()),
        Keycode::RControl => Some("ControlRight".to_string()),
        Keycode::RAlt => Some("AltRight".to_string()),
        Keycode::RMeta => Some("MetaRight".to_string()),
        _ => None,
    }
}

/// Convert right modifier keycode to display string
fn right_modifier_to_display(key: &Keycode) -> String {
    match key {
        Keycode::RShift => "Right Shift".to_string(),
        Keycode::RControl => "Right Ctrl".to_string(),
        Keycode::RAlt => "Right Option".to_string(),
        Keycode::RMeta => "Right Cmd".to_string(),
        _ => format!("{:?}", key),
    }
}

/// Format keys into an accelerator string and key names
fn format_keys(keys: &HashSet<Keycode>) -> (String, Vec<String>, bool) {
    let mut parts: Vec<String> = Vec::new();
    let mut key_names: Vec<String> = Vec::new();
    let mut has_main_key = false;

    let only_right_modifier = keys.len() == 1 && keys.iter().any(is_right_modifier);

    if only_right_modifier {
        for key in keys {
            if let Some(name) = right_modifier_to_code(key) {
                parts.push(name);
                key_names.push(right_modifier_to_display(key));
                has_main_key = true;
            }
        }
    } else {
        // Check for modifiers first (in consistent order)
        if keys.contains(&Keycode::LMeta) || keys.contains(&Keycode::RMeta) {
            parts.push("CommandOrControl".to_string());
            key_names.push("Cmd".to_string());
        } else if keys.contains(&Keycode::LControl) || keys.contains(&Keycode::RControl) {
            parts.push("CommandOrControl".to_string());
            key_names.push("Ctrl".to_string());
        }

        if keys.contains(&Keycode::LAlt) || keys.contains(&Keycode::RAlt) {
            parts.push("Alt".to_string());
            key_names.push("Alt".to_string());
        }

        if keys.contains(&Keycode::LShift) || keys.contains(&Keycode::RShift) {
            parts.push("Shift".to_string());
            key_names.push("Shift".to_string());
        }

        // Add non-modifier keys
        for key in keys {
            if !is_modifier(key) {
                if let Some(name) = keycode_to_accelerator(key) {
                    parts.push(name);
                    key_names.push(keycode_to_display(key));
                    has_main_key = true;
                }
            }
        }
    }

    let accelerator = parts.join("+");
    let is_valid = has_main_key;

    (accelerator, key_names, is_valid)
}

/// Convert keycode to Tauri accelerator string
fn keycode_to_accelerator(key: &Keycode) -> Option<String> {
    Some(
        match key {
            // Letters
            Keycode::A => "A",
            Keycode::B => "B",
            Keycode::C => "C",
            Keycode::D => "D",
            Keycode::E => "E",
            Keycode::F => "F",
            Keycode::G => "G",
            Keycode::H => "H",
            Keycode::I => "I",
            Keycode::J => "J",
            Keycode::K => "K",
            Keycode::L => "L",
            Keycode::M => "M",
            Keycode::N => "N",
            Keycode::O => "O",
            Keycode::P => "P",
            Keycode::Q => "Q",
            Keycode::R => "R",
            Keycode::S => "S",
            Keycode::T => "T",
            Keycode::U => "U",
            Keycode::V => "V",
            Keycode::W => "W",
            Keycode::X => "X",
            Keycode::Y => "Y",
            Keycode::Z => "Z",
            // Numbers
            Keycode::Key0 => "0",
            Keycode::Key1 => "1",
            Keycode::Key2 => "2",
            Keycode::Key3 => "3",
            Keycode::Key4 => "4",
            Keycode::Key5 => "5",
            Keycode::Key6 => "6",
            Keycode::Key7 => "7",
            Keycode::Key8 => "8",
            Keycode::Key9 => "9",
            // Function keys
            Keycode::F1 => "F1",
            Keycode::F2 => "F2",
            Keycode::F3 => "F3",
            Keycode::F4 => "F4",
            Keycode::F5 => "F5",
            Keycode::F6 => "F6",
            Keycode::F7 => "F7",
            Keycode::F8 => "F8",
            Keycode::F9 => "F9",
            Keycode::F10 => "F10",
            Keycode::F11 => "F11",
            Keycode::F12 => "F12",
            Keycode::F13 => "F13",
            Keycode::F14 => "F14",
            Keycode::F15 => "F15",
            Keycode::F16 => "F16",
            Keycode::F17 => "F17",
            Keycode::F18 => "F18",
            Keycode::F19 => "F19",
            Keycode::F20 => "F20",
            // Special keys
            Keycode::Space => "Space",
            Keycode::Enter => "Enter",
            Keycode::Tab => "Tab",
            Keycode::Backspace => "Backspace",
            Keycode::Delete => "Delete",
            Keycode::Insert => "Insert",
            Keycode::Home => "Home",
            Keycode::End => "End",
            Keycode::PageUp => "PageUp",
            Keycode::PageDown => "PageDown",
            Keycode::Up => "Up",
            Keycode::Down => "Down",
            Keycode::Left => "Left",
            Keycode::Right => "Right",
            // Punctuation
            Keycode::Minus => "-",
            Keycode::Equal => "=",
            Keycode::LeftBracket => "[",
            Keycode::RightBracket => "]",
            Keycode::BackSlash => "\\",
            Keycode::Semicolon => ";",
            Keycode::Apostrophe => "'",
            Keycode::Grave => "`",
            Keycode::Comma => ",",
            Keycode::Dot => ".",
            Keycode::Slash => "/",
            _ => return None,
        }
        .to_string(),
    )
}

/// Convert keycode to display string
fn keycode_to_display(key: &Keycode) -> String {
    match key {
        Keycode::Space => "Space".to_string(),
        Keycode::Enter => "Return".to_string(),
        Keycode::Tab => "Tab".to_string(),
        Keycode::Backspace => "Delete".to_string(),
        Keycode::Delete => "Del".to_string(),
        Keycode::Up => "↑".to_string(),
        Keycode::Down => "↓".to_string(),
        Keycode::Left => "←".to_string(),
        Keycode::Right => "→".to_string(),
        _ => keycode_to_accelerator(key).unwrap_or_else(|| format!("{:?}", key)),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- Modifier key tests (from modifier_monitor.rs) --

    #[test]
    fn test_modifier_key_from_accelerator() {
        assert_eq!(
            ModifierKey::from_accelerator("ShiftRight"),
            Some(ModifierKey::ShiftRight)
        );
        assert_eq!(
            ModifierKey::from_accelerator("AltLeft"),
            Some(ModifierKey::AltLeft)
        );
        assert_eq!(ModifierKey::from_accelerator("F13"), None);
        assert_eq!(ModifierKey::from_accelerator("CommandOrControl+S"), None);
    }

    #[test]
    fn test_is_modifier_shortcut() {
        assert!(is_modifier_shortcut("ShiftRight"));
        assert!(is_modifier_shortcut("ControlLeft"));
        assert!(!is_modifier_shortcut("F13"));
        assert!(!is_modifier_shortcut("CommandOrControl+Space"));
    }

    #[test]
    fn test_register_unregister() {
        // Use unique IDs to avoid interference from parallel tests
        let id = "test_ks_register_unregister";

        assert!(register_modifier_shortcut(
            id.to_string(),
            "ShiftRight".to_string(),
            "Test".to_string()
        ));
        assert!(is_modifier_shortcut_registered(id));

        assert!(unregister_modifier_shortcut(id));
        assert!(!is_modifier_shortcut_registered(id));

        assert!(!unregister_modifier_shortcut(id));
    }

    #[test]
    fn test_register_invalid_accelerator() {
        assert!(!register_modifier_shortcut(
            "test_ks_invalid".to_string(),
            "F13".to_string(),
            "Test".to_string()
        ));
    }

    // -- Key formatting tests (from keyboard_capture.rs) --

    #[test]
    fn test_format_keys_cmd_y() {
        let mut keys = HashSet::new();
        keys.insert(Keycode::LMeta);
        keys.insert(Keycode::Y);

        let (accelerator, _, is_valid) = format_keys(&keys);
        assert_eq!(accelerator, "CommandOrControl+Y");
        assert!(is_valid);
    }

    #[test]
    fn test_format_keys_modifier_only() {
        let mut keys = HashSet::new();
        keys.insert(Keycode::LMeta);

        let (_, _, is_valid) = format_keys(&keys);
        assert!(!is_valid);
    }

    #[test]
    fn test_format_keys_function_key_alone() {
        let mut keys = HashSet::new();
        keys.insert(Keycode::F13);

        let (accelerator, _, is_valid) = format_keys(&keys);
        assert_eq!(accelerator, "F13");
        assert!(is_valid);
    }

    #[test]
    fn test_format_keys_single_letter() {
        let mut keys = HashSet::new();
        keys.insert(Keycode::H);

        let (accelerator, _, is_valid) = format_keys(&keys);
        assert_eq!(accelerator, "H");
        assert!(is_valid);
    }

    #[test]
    fn test_format_keys_right_shift_alone() {
        let mut keys = HashSet::new();
        keys.insert(Keycode::RShift);

        let (accelerator, key_names, is_valid) = format_keys(&keys);
        assert_eq!(accelerator, "ShiftRight");
        assert_eq!(key_names, vec!["Right Shift"]);
        assert!(is_valid);
    }

    #[test]
    fn test_format_keys_right_option_alone() {
        let mut keys = HashSet::new();
        keys.insert(Keycode::RAlt);

        let (accelerator, key_names, is_valid) = format_keys(&keys);
        assert_eq!(accelerator, "AltRight");
        assert_eq!(key_names, vec!["Right Option"]);
        assert!(is_valid);
    }

    #[test]
    fn test_format_keys_left_shift_alone() {
        let mut keys = HashSet::new();
        keys.insert(Keycode::LShift);

        let (_, _, is_valid) = format_keys(&keys);
        assert!(!is_valid);
    }

    // -- Mode tests --

    #[test]
    fn test_keyboard_mode_roundtrip() {
        for mode in [
            KeyboardMode::Idle,
            KeyboardMode::Monitoring,
            KeyboardMode::Capturing,
        ] {
            assert_eq!(KeyboardMode::from_u8(mode as u8), mode);
        }
    }

    #[test]
    fn test_is_capture_active_reflects_mode() {
        let original = MODE.load(Ordering::SeqCst);

        MODE.store(KeyboardMode::Capturing as u8, Ordering::SeqCst);
        assert!(is_capture_active());

        MODE.store(KeyboardMode::Monitoring as u8, Ordering::SeqCst);
        assert!(!is_capture_active());

        MODE.store(KeyboardMode::Idle as u8, Ordering::SeqCst);
        assert!(!is_capture_active());

        // Restore original
        MODE.store(original, Ordering::SeqCst);
    }
}
