//! Tauri command handlers
//!
//! This module contains all IPC commands that can be invoked from the frontend.

use crate::TELEMETRY_TARGET;
use crate::error::Error;
use tauri::{AppHandle, Manager};

/// Greet command for testing
#[tauri::command]
pub fn greet(name: &str) -> String {
    format!("Hello, {}! Welcome to Thoth.", name)
}

/// Show a window by label
#[tauri::command]
pub async fn show_window(app: AppHandle, label: &str) -> Result<(), Error> {
    tauri_plugin_telemetry::traced("show_window", async move {
        if let Some(window) = app.get_webview_window(label) {
            window.show().map_err(|e| e.to_string())?;
            window.set_focus().map_err(|e| e.to_string())?;
            tracing::info!("Showed window: {}", label);
            Ok(())
        } else {
            Err(format!("Window '{}' not found", label).into())
        }
    })
    .await
}

/// Open a URL in the system's default browser
#[tauri::command]
pub async fn open_url(url: &str) -> Result<(), Error> {
    tauri_plugin_telemetry::traced("open_url", async move {
        // Only allow http/https URLs for security
        if !url.starts_with("https://") && !url.starts_with("http://") {
            return Err("Only http:// and https:// URLs are allowed"
                .to_string()
                .into());
        }

        // `open` selects the platform launcher (open / xdg-open / cmd start); the
        // detached variant returns without waiting on the spawned browser process,
        // matching the previous fire-and-forget behaviour.
        open::that_detached(url).map_err(|e| format!("Failed to open URL: {}", e).into())
    })
    .await
}

/// Set dock icon visibility (macOS) and persist to config
#[tauri::command]
pub async fn set_show_in_dock(app: AppHandle, show: bool) -> Result<(), Error> {
    tauri_plugin_telemetry::traced("set_show_in_dock", async move {
        // Update config
        let mut config =
            crate::config::get_config().map_err(|e| format!("Failed to load config: {}", e))?;
        config.general.show_in_dock = show;
        crate::config::set_config(config).map_err(|e| format!("Failed to save config: {}", e))?;

        // Apply immediately on macOS
        #[cfg(target_os = "macos")]
        {
            let policy = if show {
                tauri::ActivationPolicy::Regular
            } else {
                tauri::ActivationPolicy::Accessory
            };
            app.set_activation_policy(policy)
                .map_err(|e| format!("Failed to set activation policy: {}", e))?;
        }

        #[cfg(not(target_os = "macos"))]
        {
            let _ = app; // Suppress unused warning on non-macOS
        }

        tracing::info!("Dock visibility set to: {}", show);
        Ok(())
    })
    .await
}

/// Get current dock visibility setting
#[tauri::command]
pub fn get_show_in_dock() -> bool {
    crate::config::get_config()
        .map(|c| c.general.show_in_dock)
        .unwrap_or(false)
}

/// Set the audio input device and persist to config
///
/// Uses a dedicated command (rather than full config save) to prevent
/// the device_id from being accidentally overwritten by other config saves.
/// Also cools down the warm stream so the next recording opens the new device.
#[tauri::command]
pub async fn set_audio_device(device_id: Option<String>) -> Result<(), Error> {
    tauri_plugin_telemetry::traced("set_audio_device", async move {
        crate::config::set_audio_device_config(device_id.clone())
            .map_err(|e| format!("Failed to save audio device: {}", e))?;
        // Cool down the warm stream — the new device must be opened fresh.
        crate::audio::cool_down_recording();
        tracing::info!("Audio device set to: {:?}", device_id);
        Ok(())
    })
    .await
}

// ─── macOS Permission Helpers ─────────────────────────────────────────────────

/// Remove the macOS quarantine extended attribute from Thoth.app.
/// Safe to call on any version — no-ops if already cleared.
#[tauri::command]
pub async fn remove_quarantine() -> Result<(), Error> {
    tauri_plugin_telemetry::traced("remove_quarantine", async move {
        #[cfg(target_os = "macos")]
        {
            let output = std::process::Command::new("xattr")
                .args(["-dr", "com.apple.quarantine", "/Applications/Thoth.app"])
                .output()
                .map_err(|e| format!("Failed to run xattr: {}", e))?;

            if output.status.success() {
                tracing::info!("Quarantine attribute removed from Thoth.app");
                Ok(())
            } else {
                // xattr exits non-zero if the attribute doesn't exist — that's fine
                let stderr = String::from_utf8_lossy(&output.stderr);
                if stderr.contains("No such xattr") || stderr.is_empty() {
                    Ok(()) // Already clear
                } else {
                    Err(format!("xattr failed: {}", stderr).into())
                }
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            Ok(()) // No-op on non-macOS
        }
    })
    .await
}

/// Open a macOS Privacy & Security preference pane
#[tauri::command]
#[cfg_attr(not(target_os = "macos"), allow(unused_variables))]
pub async fn open_privacy_pane(pane: String) -> Result<(), Error> {
    tauri_plugin_telemetry::traced("open_privacy_pane", async move {
        #[cfg(target_os = "macos")]
        {
            let url = match pane.as_str() {
                "accessibility" => {
                    "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
                }
                "input-monitoring" => {
                    "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent"
                }
                "microphone" => {
                    "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone"
                }
                _ => return Err(format!("Unknown pane: {}", pane).into()),
            };
            std::process::Command::new("open")
                .arg(url)
                .spawn()
                .map_err(|e| format!("Failed to open settings: {}", e))?;
            Ok(())
        }
        #[cfg(not(target_os = "macos"))]
        {
            Ok(())
        }
    })
    .await
}

/// Quit and relaunch the application (used by troubleshooting flow)
#[tauri::command]
pub async fn relaunch_app(app: AppHandle) -> Result<(), Error> {
    tauri_plugin_telemetry::traced("relaunch_app", async move {
        app.restart();
    })
    .await
}

/// Record the outcome of the frontend's update check (`@tauri-apps/plugin-updater`'s
/// `check()`) on the host process, which is the one thing allowed to emit
/// telemetry — the webview itself ships nothing. `error` is the failure's own
/// message: this is the app reporting on its own update mechanism, never
/// arbitrary caller-supplied text, so carrying it is safe by construction.
#[tauri::command]
pub fn report_update_check(available: bool, version: Option<String>, error: Option<String>) {
    match &error {
        Some(e) => {
            telemetry::report_error("update_check_failed");
            tracing::warn!(
                target: TELEMETRY_TARGET,
                error = %e,
                "update_check_failed"
            );
        }
        None => {
            tracing::info!(
                target: TELEMETRY_TARGET,
                available,
                version = %version.as_deref().unwrap_or(""),
                "update_check_complete"
            );
        }
    }
}
