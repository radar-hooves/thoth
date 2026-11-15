//! Tauri command handlers
//!
//! This module contains all IPC commands that can be invoked from the frontend.

use crate::TELEMETRY_TARGET;
use crate::error::Error;
use tauri::{AppHandle, Manager};

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
        crate::config::patch_config(serde_json::json!({ "general": { "show_in_dock": show } }))
            .map_err(|e| format!("Failed to save config: {}", e))?;

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
    tauri_plugin_telemetry::traced_sync_value("get_show_in_dock", || {
        {
            crate::config::get_config()
                .map(|c| c.general.show_in_dock)
                .unwrap_or(false)
        }
    })
}

/// Set the audio input device and persist to config
///
/// Also cools down the warm stream so the next recording opens the new device.
#[tauri::command]
pub async fn set_audio_device(device_id: Option<String>) -> Result<(), Error> {
    tauri_plugin_telemetry::traced("set_audio_device", async move {
        crate::config::patch_config(serde_json::json!({ "audio": { "device_id": device_id } }))
            .map_err(|e| format!("Failed to save audio device: {}", e))?;
        // Cool down the warm stream — the new device must be opened fresh.
        crate::audio::cool_down_recording();
        tracing::info!("Audio device set to: {:?}", device_id);
        Ok(())
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
    tauri_plugin_telemetry::traced_sync_value("report_update_check", || match &error {
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
    })
}
