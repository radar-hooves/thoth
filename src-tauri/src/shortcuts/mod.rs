//! Global shortcuts module for Thoth
//!
//! Provides global keyboard shortcut registration and management
//! for controlling recording and other application features.
//!
//! Platform support:
//! - macOS: Tauri's GlobalShortcut plugin
//! - Linux X11: Tauri's GlobalShortcut plugin
//! - Linux Wayland: the XDG Desktop Portal `GlobalShortcuts` interface, where
//!   the compositor implements it (KDE, wlroots-based compositors, GNOME 48+).
//!   On compositors without it the user is told that global shortcuts are
//!   unavailable so they can use a function-key shortcut instead.

pub mod conflict;
pub mod manager;

#[cfg(target_os = "linux")]
pub mod hyprland;
#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "linux")]
pub mod wayland_portal;
#[cfg(target_os = "linux")]
pub use linux::{DisplayServer, get_display_server};

pub use conflict::ShortcutConflict;
pub use manager::{ShortcutInfo, shortcut_ids};

use crate::error::Error;
use crate::keyboard_service;
use tauri::AppHandle;

/// Whether the current session uses the Wayland display server.
///
/// Always `false` off Linux. On Linux it consults the cached
/// [`get_display_server`] detector, so every caller shares one source of truth
/// instead of re-reading the environment.
pub fn is_wayland() -> bool {
    #[cfg(target_os = "linux")]
    {
        get_display_server() == DisplayServer::Wayland
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

/// Register a global shortcut
///
/// # Arguments
/// * `id` - Unique identifier for the shortcut
/// * `accelerator` - Keyboard accelerator string (e.g., "F13", "Cmd+Shift+Space")
/// * `description` - Human-readable description of the shortcut's action
///
/// # Returns
/// * `Ok(())` on success
/// * `Err(String)` with a user-friendly error message on failure
pub fn register_shortcut(
    app: AppHandle,
    id: String,
    accelerator: String,
    description: String,
) -> Result<(), Error> {
    tauri_plugin_telemetry::traced_sync("register_shortcut", || {
        // Route modifier-only shortcuts to the keyboard service
        if keyboard_service::is_modifier_shortcut(&accelerator) {
            if keyboard_service::register_modifier_shortcut(
                id.clone(),
                accelerator.clone(),
                description,
            ) {
                keyboard_service::restart_monitoring(app);
                Ok(())
            } else {
                Err(format!("Failed to register modifier shortcut: {}", accelerator).into())
            }
        } else {
            // Platform-specific registration
            #[cfg(target_os = "linux")]
            {
                linux::register(&app, id, accelerator, description).map_err(Into::into)
            }
            #[cfg(not(target_os = "linux"))]
            {
                manager::register(&app, id, accelerator, description).map_err(Into::into)
            }
        }
    })
}

/// List all currently registered shortcuts
///
/// # Returns
/// A vector of `ShortcutInfo` for all registered shortcuts
#[tauri::command]
pub fn list_registered_shortcuts() -> Vec<ShortcutInfo> {
    tauri_plugin_telemetry::traced_sync_value("list_registered_shortcuts", || {
        {
            // Platform-specific listing
            #[cfg(target_os = "linux")]
            let mut shortcuts = linux::list_registered();
            #[cfg(not(target_os = "linux"))]
            let mut shortcuts = manager::list_registered();

            // Add modifier shortcuts
            for (id, accelerator, description) in keyboard_service::list_modifier_shortcuts() {
                shortcuts.push(ShortcutInfo {
                    id,
                    accelerator,
                    description,
                    is_enabled: true,
                });
            }

            shortcuts
        }
    })
}

/// Get the default shortcuts for Thoth
///
/// Returns the pre-configured shortcuts that Thoth uses by default.
/// These are not automatically registered; use `register_shortcut` to enable them.
///
/// Default shortcuts:
/// - F13: Toggle recording (push-to-talk)
/// - F14: Copy last transcription to clipboard
/// - Cmd+Shift+Space: Toggle recording (alternative)
///
/// # Returns
/// A vector of `ShortcutInfo` describing the default shortcuts
#[tauri::command]
pub fn get_default_shortcuts() -> Vec<ShortcutInfo> {
    tauri_plugin_telemetry::traced_sync_value("get_default_shortcuts", manager::get_defaults)
}

/// Unregister all shortcuts
///
/// Removes all registered global shortcuts, including modifier-only shortcuts
/// monitored by the modifier monitor.
///
/// # Returns
/// * `Ok(())` on success
/// * `Err(String)` if unregistration fails
pub fn unregister_all_shortcuts(app: AppHandle) -> Result<(), Error> {
    tauri_plugin_telemetry::traced_sync("unregister_all_shortcuts", || {
        // Unregister all modifier shortcuts (thread stays alive for mode transitions)
        keyboard_service::unregister_all_modifier_shortcuts();

        // Platform-specific unregistration
        #[cfg(target_os = "linux")]
        {
            linux::unregister_all(&app).map_err(Into::into)
        }
        #[cfg(not(target_os = "linux"))]
        {
            manager::unregister_all(&app).map_err(Into::into)
        }
    })
}
