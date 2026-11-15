//! Linux-specific shortcut implementation.
//!
//! Global shortcuts on Linux take one of two routes depending on the display
//! server, detected at runtime:
//!
//! - **X11**: Tauri's GlobalShortcut plugin, which grabs the key combination
//!   directly. This works exactly as it does on macOS.
//! - **Wayland**: the XDG Desktop Portal `GlobalShortcuts` interface (see
//!   [`super::wayland_portal`]). Wayland has no client-side hotkey API, so the
//!   Tauri plugin cannot bind a global shortcut there. The portal is set up
//!   once at application start; per-shortcut `register` calls on Wayland do not
//!   drive the binding (the portal dialog does) but still record the shortcut
//!   so the rest of the app sees a consistent registered-shortcuts list.
//!
//! Where a route cannot work on the user's compositor, the failure is surfaced
//! (logged and emitted to the frontend), never silent.

use tauri::{AppHandle, Runtime};

use super::manager::{self, ShortcutInfo};

/// Detected display server type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayServer {
    /// X11 display server (Xorg)
    X11,
    /// Wayland compositor
    Wayland,
    /// Unknown or unable to detect
    Unknown,
}

impl std::fmt::Display for DisplayServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DisplayServer::X11 => write!(f, "X11"),
            DisplayServer::Wayland => write!(f, "Wayland"),
            DisplayServer::Unknown => write!(f, "Unknown"),
        }
    }
}

/// Detect the current display server
///
/// Checks environment variables and XDG session type to determine
/// whether we're running on X11 or Wayland.
pub fn detect_display_server() -> DisplayServer {
    // Check XDG_SESSION_TYPE first (most reliable)
    if let Ok(session_type) = std::env::var("XDG_SESSION_TYPE") {
        match session_type.to_lowercase().as_str() {
            "wayland" => {
                tracing::info!("Detected Wayland session via XDG_SESSION_TYPE");
                return DisplayServer::Wayland;
            }
            "x11" | "xorg" => {
                tracing::info!("Detected X11 session via XDG_SESSION_TYPE");
                return DisplayServer::X11;
            }
            _ => {}
        }
    }

    // Check WAYLAND_DISPLAY (Wayland-specific)
    if std::env::var("WAYLAND_DISPLAY").is_ok() {
        tracing::info!("Detected Wayland via WAYLAND_DISPLAY");
        return DisplayServer::Wayland;
    }

    // Check DISPLAY (X11-specific)
    if std::env::var("DISPLAY").is_ok() {
        tracing::info!("Detected X11 via DISPLAY");
        return DisplayServer::X11;
    }

    tracing::warn!("Could not detect display server type");
    DisplayServer::Unknown
}

/// Cached display server detection
static DISPLAY_SERVER: std::sync::OnceLock<DisplayServer> = std::sync::OnceLock::new();

/// Get the detected display server (cached after first call)
pub fn get_display_server() -> DisplayServer {
    *DISPLAY_SERVER.get_or_init(detect_display_server)
}

/// Set up the Wayland global-shortcut portal, once, at application start.
///
/// No-op on X11 (the Tauri plugin handles shortcuts there per-registration).
/// On Wayland this opens the portal session; the binding result is delivered to
/// the frontend via the `wayland-shortcuts-status` event.
pub fn init_global_shortcuts(app: &AppHandle) {
    if get_display_server() == DisplayServer::Wayland {
        // On Hyprland the XDG GlobalShortcuts portal registers a shortcut but
        // never binds a key (the binding comes back empty), so hotkeys never
        // fire. Use native hyprctl binds + a FIFO there instead of the portal.
        if super::hyprland::is_hyprland() {
            tracing::info!("Hyprland detected: using native hyprctl binds (skipping portal)");
            super::hyprland::setup(app);
        } else {
            tracing::info!("Wayland session: setting up the XDG GlobalShortcuts portal");
            super::wayland_portal::setup(app);
        }
    }
}

/// Register a shortcut on Linux.
///
/// On X11, binds the accelerator via Tauri's GlobalShortcut plugin. On Wayland,
/// the actual binding is owned by the portal (set up at app start via
/// [`init_global_shortcuts`]); here we only record the shortcut in the manager
/// so listing and unregistration stay consistent, because the Tauri plugin
/// cannot bind a global shortcut under Wayland.
pub fn register<R: Runtime>(
    app: &AppHandle<R>,
    id: String,
    accelerator: String,
    description: String,
) -> Result<(), String> {
    match get_display_server() {
        DisplayServer::Wayland => {
            tracing::info!(
                "Wayland session: shortcut '{}' is bound through the desktop portal, not the \
                 Tauri plugin; recording it for listing only",
                id
            );
            manager::record_shortcut(id, accelerator, description);
            Ok(())
        }
        _ => {
            // X11 (and Unknown — attempt the plugin, which works under X11/XWayland).
            manager::register(app, id, accelerator, description)
        }
    }
}

/// List all registered shortcuts on Linux
pub fn list_registered() -> Vec<ShortcutInfo> {
    manager::list_registered()
}

/// Unregister all shortcuts on Linux
pub fn unregister_all<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    if get_display_server() == DisplayServer::Wayland {
        manager::clear_shortcuts();
        return Ok(());
    }
    manager::unregister_all(app)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_display_server_detection() {
        // This test just ensures the detection doesn't panic
        let server = detect_display_server();
        tracing::info!("Detected display server: {:?}", server);
    }

    #[test]
    fn test_get_display_server_cached() {
        let server1 = get_display_server();
        let server2 = get_display_server();
        assert_eq!(server1, server2);
    }
}
