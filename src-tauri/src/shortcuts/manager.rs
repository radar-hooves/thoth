//! Shortcut manager for Thoth
//!
//! Handles registration and management of global keyboard shortcuts
//! for controlling recording and other application features.

use crate::config::RecordingMode;
use crate::recording_indicator;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Instant;
use tauri::{AppHandle, Emitter, Runtime};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

/// Information about a registered shortcut
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShortcutInfo {
    /// Unique identifier for the shortcut
    pub id: String,
    /// Keyboard accelerator string (e.g., "F13", "Cmd+Shift+Space")
    pub accelerator: String,
    /// Human-readable description of what the shortcut does
    pub description: String,
    /// Whether the shortcut is currently enabled
    pub is_enabled: bool,
}

/// Default shortcut identifiers
pub mod shortcut_ids {
    pub const TOGGLE_RECORDING: &str = "toggle_recording";
    pub const TOGGLE_RECORDING_ALT: &str = "toggle_recording_alt";
    pub const COPY_LAST_TRANSCRIPTION: &str = "copy_last_transcription";
    pub const TOGGLE_ENHANCEMENT: &str = "toggle_enhancement";
}

/// Global shortcut manager instance
static MANAGER: OnceLock<RwLock<ShortcutManagerState>> = OnceLock::new();

/// Minimum interval between press events for the same shortcut (debounce).
/// 50ms is enough to absorb electrical key bounce while allowing rapid intentional presses.
const PRESS_DEBOUNCE_MS: u64 = 50;

/// Internal state for the shortcut manager
struct ShortcutManagerState {
    /// Registered shortcuts by ID
    shortcuts: HashMap<String, ShortcutInfo>,
    /// Last press timestamp per shortcut ID (for debouncing key bounce)
    last_press_times: HashMap<String, Instant>,
}

impl ShortcutManagerState {
    fn new() -> Self {
        Self {
            shortcuts: HashMap::new(),
            last_press_times: HashMap::new(),
        }
    }
}

fn get_manager() -> &'static RwLock<ShortcutManagerState> {
    MANAGER.get_or_init(|| RwLock::new(ShortcutManagerState::new()))
}

/// Run the action bound to a shortcut id (toggle recording, copy last
/// transcription, toggle enhancement, or emit `shortcut-triggered` for the
/// frontend to handle).
///
/// This is the single source of truth for what a shortcut *does*, independent
/// of how the press was detected. The Tauri global-shortcut plugin (macOS and
/// Linux/X11) calls it from its `on_shortcut` callback; the Wayland XDG portal
/// activation loop calls it from its D-Bus signal stream. Both paths share this
/// debounce, lock-screen suppression, and dispatch so behaviour cannot drift
/// between platforms.
/// Which edge of a key press this is.
///
/// Only hold-to-record cares (#111): every other mode acts on the press and
/// ignores the release. Carried into the shared dispatcher rather than decided
/// in each transport's callback, so the Tauri and portal paths cannot drift
/// apart on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Edge {
    Press,
    Release,
}

/// Whether a release should start a recording action at all.
///
/// Releases are dispatched unconditionally by the transports and filtered
/// here. Two conditions, and both matter: the mode has to be hold-to-record,
/// and it has to be a recording shortcut — releasing the copy-last or
/// toggle-enhancement key must not do it a second time.
pub fn release_acts(mode: RecordingMode, shortcut_id: &str) -> bool {
    mode == RecordingMode::HoldToRecord && is_recording_shortcut(shortcut_id)
}

/// The debounce map key for one shortcut's one edge.
///
/// Press keeps the bare id so existing behaviour is byte-identical; release
/// gets a suffix that cannot occur in a shortcut id (a unit separator).
fn debounce_key_for(shortcut_id: &str, edge: Edge) -> String {
    match edge {
        Edge::Press => shortcut_id.to_string(),
        Edge::Release => format!("{shortcut_id}\u{1f}release"),
    }
}

/// Whether this shortcut starts or stops a recording.
fn is_recording_shortcut(shortcut_id: &str) -> bool {
    shortcut_id == shortcut_ids::TOGGLE_RECORDING
        || shortcut_id == shortcut_ids::TOGGLE_RECORDING_ALT
}

pub(crate) fn dispatch_shortcut_action<R: Runtime>(
    app: &AppHandle<R>,
    shortcut_id: &str,
    edge: Edge,
) {
    if edge == Edge::Release {
        let mode = crate::config::get_config()
            .map(|c| c.shortcuts.recording_mode)
            .unwrap_or_default();
        if !release_acts(mode, shortcut_id) {
            tracing::debug!(
                "Shortcut released: {} (no action in {:?})",
                shortcut_id,
                mode
            );
            return;
        }
    }

    // Discard events during capture mode. Queued OS events may fire even after
    // unregistration; this guard prevents phantom triggers.
    if crate::keyboard_service::is_capture_active() {
        tracing::debug!(
            "Discarding shortcut event for '{}' — capture mode active",
            shortcut_id
        );
        return;
    }

    // Suppress shortcuts when the screen is locked or the screensaver is active.
    // Prevents accidental recording when the user presses a key to dismiss the
    // lock screen.
    if crate::platform::is_screen_locked() {
        tracing::debug!(
            "Discarding shortcut event for '{}' — screen is locked",
            shortcut_id
        );
        return;
    }

    // Debounce rapid events (key bounce protection). Only allow one per
    // PRESS_DEBOUNCE_MS window per shortcut PER EDGE: a hold-to-record release
    // arrives within milliseconds of its own press on a quick tap, and
    // debouncing the two against each other would swallow the stop and leave
    // the recording running.
    let debounce_key = debounce_key_for(shortcut_id, edge);
    let debounce_key = debounce_key.as_str();
    {
        let mut manager = get_manager().write();
        if let Some(last) = manager.last_press_times.get(debounce_key) {
            let elapsed = last.elapsed().as_millis();
            if elapsed < PRESS_DEBOUNCE_MS as u128 {
                tracing::info!(
                    "Debounced shortcut press for '{}' ({}ms since last, threshold {}ms)",
                    shortcut_id,
                    elapsed,
                    PRESS_DEBOUNCE_MS
                );
                return;
            }
        }
        manager
            .last_press_times
            .insert(debounce_key.to_string(), Instant::now());
    }

    tracing::info!("Shortcut {:?}: {}", edge, shortcut_id);

    // For recording shortcuts, show indicator and play the start cue IMMEDIATELY
    // in Rust before emitting to the frontend. This eliminates JS round-trip delay.
    // Press only. On a hold-to-record release the recording is ENDING, and
    // `pipeline_stop_and_process` plays the stop cue and hides the indicator.
    if edge == Edge::Press && is_recording_shortcut(shortcut_id) {
        recording_indicator::maybe_play_start_indicator(app);
    }

    // Handle copy-last-transcription directly in Rust (no frontend round-trip).
    if shortcut_id == shortcut_ids::COPY_LAST_TRANSCRIPTION {
        match crate::clipboard::copy_last_transcription() {
            Ok(true) => tracing::info!("Copied last transcription to clipboard"),
            Ok(false) => tracing::info!("No transcriptions to copy"),
            Err(e) => tracing::error!("Failed to copy last transcription: {}", e),
        }
        return;
    }

    // Handle toggle-enhancement directly in Rust (no frontend round-trip).
    if shortcut_id == shortcut_ids::TOGGLE_ENHANCEMENT {
        crate::tray::handle_toggle_enhancement_shortcut(app);
        return;
    }

    // A hold-to-record release goes out on its own channel, not as another
    // `shortcut-triggered` (#111). The frontend's toggle handler applies a
    // 300 ms cooldown against key bounce, which would swallow the key-up of
    // any hold shorter than that and leave the recording running with the key
    // already let go — and a toggle would also START a recording if the press
    // had failed. The release is a stop, and says so.
    let channel = match edge {
        Edge::Press => "shortcut-triggered",
        Edge::Release => "shortcut-released",
    };
    match app.emit(channel, shortcut_id.to_string()) {
        Ok(_) => tracing::info!("Emitted {} event for: {}", channel, shortcut_id),
        Err(e) => tracing::error!("Failed to emit {} event: {}", channel, e),
    }
}

/// Get the default shortcuts for Thoth
pub fn get_defaults() -> Vec<ShortcutInfo> {
    vec![
        ShortcutInfo {
            id: shortcut_ids::TOGGLE_RECORDING.to_string(),
            accelerator: "F13".to_string(),
            description: "Toggle recording (push-to-talk)".to_string(),
            is_enabled: false,
        },
        ShortcutInfo {
            id: shortcut_ids::COPY_LAST_TRANSCRIPTION.to_string(),
            accelerator: "F14".to_string(),
            description: "Copy last transcription to clipboard".to_string(),
            is_enabled: false,
        },
        ShortcutInfo {
            id: shortcut_ids::TOGGLE_RECORDING_ALT.to_string(),
            accelerator: "ShiftRight".to_string(),
            description: "Toggle recording (alternative)".to_string(),
            is_enabled: false,
        },
        ShortcutInfo {
            id: shortcut_ids::TOGGLE_ENHANCEMENT.to_string(),
            accelerator: String::new(),
            description: "Toggle AI enhancement".to_string(),
            is_enabled: false,
        },
    ]
}

/// Register a global shortcut with the given ID and accelerator
///
/// The shortcut will emit a "shortcut-triggered" event to the frontend on key down.
pub fn register<R: Runtime>(
    app: &AppHandle<R>,
    id: String,
    accelerator: String,
    description: String,
) -> Result<(), String> {
    let global_shortcut = app.global_shortcut();

    // Check if already registered with the system
    if global_shortcut.is_registered(accelerator.as_str()) {
        tracing::debug!(
            "Shortcut '{}' already registered, skipping duplicate registration",
            accelerator
        );
        return Err(format!("Shortcut '{}' is already registered", accelerator));
    }

    // Clone values for the closure
    let shortcut_id = id.clone();
    let app_handle = app.clone();

    tracing::debug!(
        "Registering shortcut handler for '{}' (accelerator: '{}')",
        id,
        accelerator
    );

    // Register with the global shortcut plugin. The callback only routes the
    // press to the shared dispatcher; all debounce/suppression/action logic
    // lives in `dispatch_shortcut_action` so the Wayland portal path behaves
    // identically.
    global_shortcut
        .on_shortcut(
            accelerator.as_str(),
            move |_app, _shortcut, event| match event.state {
                ShortcutState::Pressed => {
                    dispatch_shortcut_action(&app_handle, &shortcut_id, Edge::Press);
                }
                ShortcutState::Released => {
                    // Only hold-to-record acts on a release (#111); every other
                    // mode leaves the key-up alone, and the dispatcher decides
                    // that rather than this callback, so the Wayland portal path
                    // reaches the same conclusion.
                    dispatch_shortcut_action(&app_handle, &shortcut_id, Edge::Release);
                }
            },
        )
        .map_err(|e| format!("Failed to register shortcut '{}': {}", accelerator, e))?;

    // Store in our manager state
    let info = ShortcutInfo {
        id: id.clone(),
        accelerator: accelerator.clone(),
        description,
        is_enabled: true,
    };

    {
        let mut manager = get_manager().write();
        manager.shortcuts.insert(id.clone(), info);
    }

    tracing::info!(
        "Registered shortcut '{}' with accelerator '{}'",
        id,
        accelerator
    );
    Ok(())
}

/// Record a shortcut in the manager state without binding it with the OS.
///
/// Used on Wayland, where the XDG portal owns the actual binding: the manager
/// still needs to know the shortcut exists so listing and unregistration behave
/// consistently with the X11/macOS paths.
pub fn record_shortcut(id: String, accelerator: String, description: String) {
    let info = ShortcutInfo {
        id: id.clone(),
        accelerator,
        description,
        is_enabled: true,
    };
    get_manager().write().shortcuts.insert(id, info);
}

/// Forget a recorded shortcut without an OS unbind call (Wayland book-keeping).
pub fn forget_shortcut(id: &str) {
    get_manager().write().shortcuts.remove(id);
}

/// Clear all recorded shortcuts without OS unbind calls (Wayland book-keeping).
pub fn clear_shortcuts() {
    get_manager().write().shortcuts.clear();
}

/// Unregister a shortcut by its ID
pub fn unregister<R: Runtime>(app: &AppHandle<R>, id: &str) -> Result<(), String> {
    let accelerator = {
        let manager = get_manager().read();
        manager
            .shortcuts
            .get(id)
            .map(|info| info.accelerator.clone())
            .ok_or_else(|| format!("Shortcut '{}' is not registered", id))?
    };

    let global_shortcut = app.global_shortcut();

    global_shortcut
        .unregister(accelerator.as_str())
        .map_err(|e| format!("Failed to unregister shortcut '{}': {}", accelerator, e))?;

    {
        let mut manager = get_manager().write();
        manager.shortcuts.remove(id);
    }

    tracing::info!("Unregistered shortcut '{}'", id);
    Ok(())
}

/// List all currently registered shortcuts
pub fn list_registered() -> Vec<ShortcutInfo> {
    let manager = get_manager().read();
    manager.shortcuts.values().cloned().collect()
}

/// Unregister all shortcuts
pub fn unregister_all<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    let global_shortcut = app.global_shortcut();

    global_shortcut
        .unregister_all()
        .map_err(|e| format!("Failed to unregister all shortcuts: {}", e))?;

    {
        let mut manager = get_manager().write();
        manager.shortcuts.clear();
    }

    tracing::info!("Unregistered all shortcuts");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_defaults_returns_expected_shortcuts() {
        let defaults = get_defaults();

        assert_eq!(defaults.len(), 4);

        let toggle = defaults
            .iter()
            .find(|s| s.id == shortcut_ids::TOGGLE_RECORDING);
        assert!(toggle.is_some());
        assert_eq!(toggle.unwrap().accelerator, "F13");

        let copy = defaults
            .iter()
            .find(|s| s.id == shortcut_ids::COPY_LAST_TRANSCRIPTION);
        assert!(copy.is_some());
        assert_eq!(copy.unwrap().accelerator, "F14");

        let alt = defaults
            .iter()
            .find(|s| s.id == shortcut_ids::TOGGLE_RECORDING_ALT);
        assert!(alt.is_some());
        assert_eq!(alt.unwrap().accelerator, "ShiftRight");

        let enh = defaults
            .iter()
            .find(|s| s.id == shortcut_ids::TOGGLE_ENHANCEMENT);
        assert!(enh.is_some());
        assert_eq!(enh.unwrap().accelerator, "");
    }

    #[test]
    fn test_shortcut_info_serialisation() {
        let info = ShortcutInfo {
            id: "test".to_string(),
            accelerator: "Ctrl+T".to_string(),
            description: "Test shortcut".to_string(),
            is_enabled: true,
        };

        let json = serde_json::to_string(&info).unwrap();
        let deserialised: ShortcutInfo = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialised.id, info.id);
        assert_eq!(deserialised.accelerator, info.accelerator);
        assert_eq!(deserialised.description, info.description);
        assert_eq!(deserialised.is_enabled, info.is_enabled);
    }

    /// Hold-to-record is the ONLY mode that acts on a key release. Toggle
    /// acting on one would stop the recording the same press just started.
    #[test]
    fn only_hold_to_record_acts_on_a_release() {
        for id in [
            shortcut_ids::TOGGLE_RECORDING,
            shortcut_ids::TOGGLE_RECORDING_ALT,
        ] {
            assert!(release_acts(RecordingMode::HoldToRecord, id), "{id}");
            assert!(!release_acts(RecordingMode::Toggle, id), "{id}");
            assert!(!release_acts(RecordingMode::HandsFree, id), "{id}");
        }
    }

    /// Releasing a non-recording shortcut must do nothing in any mode —
    /// otherwise letting go of the copy-last key copies a second time.
    #[test]
    fn releasing_a_non_recording_shortcut_never_acts() {
        for id in [
            shortcut_ids::COPY_LAST_TRANSCRIPTION,
            shortcut_ids::TOGGLE_ENHANCEMENT,
        ] {
            assert!(!release_acts(RecordingMode::HoldToRecord, id), "{id}");
            assert!(!release_acts(RecordingMode::Toggle, id), "{id}");
        }
    }

    /// Press and release must debounce independently. They share one map, and
    /// a quick tap puts the release inside the press's debounce window — if
    /// they shared a key the stop would be swallowed and the recording would
    /// run on with the key already let go.
    #[test]
    fn a_release_is_not_debounced_against_its_own_press() {
        let press = debounce_key_for(shortcut_ids::TOGGLE_RECORDING, Edge::Press);
        let release = debounce_key_for(shortcut_ids::TOGGLE_RECORDING, Edge::Release);
        assert_ne!(press, release);
        assert_eq!(press, shortcut_ids::TOGGLE_RECORDING);
    }

    /// Two shortcuts must not share a debounce key through the release
    /// suffix, or holding one would debounce the other.
    #[test]
    fn debounce_keys_stay_distinct_between_shortcuts() {
        let a = debounce_key_for(shortcut_ids::TOGGLE_RECORDING, Edge::Release);
        let b = debounce_key_for(shortcut_ids::TOGGLE_RECORDING_ALT, Edge::Release);
        assert_ne!(a, b);
    }
}
