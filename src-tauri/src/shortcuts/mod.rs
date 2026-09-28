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

pub use conflict::{RegistrationResult, ShortcutConflict};
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
#[tauri::command]
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

/// Unregister a shortcut by its ID
///
/// # Arguments
/// * `id` - The unique identifier of the shortcut to unregister
///
/// # Returns
/// * `Ok(())` on success
/// * `Err(String)` if the shortcut is not registered or unregistration fails
#[tauri::command]
pub async fn unregister_shortcut(app: AppHandle, id: String) -> Result<(), Error> {
    tauri_plugin_telemetry::traced("unregister_shortcut", async move {
        // Try modifier monitor first, then regular shortcuts
        if keyboard_service::is_modifier_shortcut_registered(&id) {
            keyboard_service::unregister_modifier_shortcut(&id);
            Ok(())
        } else {
            // Platform-specific unregistration
            #[cfg(target_os = "linux")]
            {
                linux::unregister(&app, &id).map_err(Into::into)
            }
            #[cfg(not(target_os = "linux"))]
            {
                manager::unregister(&app, &id).map_err(Into::into)
            }
        }
    })
    .await
}

/// List all currently registered shortcuts
///
/// # Returns
/// A vector of `ShortcutInfo` for all registered shortcuts
#[tauri::command]
pub fn list_registered_shortcuts() -> Vec<ShortcutInfo> {
    tauri_plugin_telemetry::traced_sync("list_registered_shortcuts", || {
        Ok::<_, std::convert::Infallible>({
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
        })
    })
    .unwrap()
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
    tauri_plugin_telemetry::traced_sync("get_default_shortcuts", || {
        Ok::<_, std::convert::Infallible>(manager::get_defaults())
    })
    .unwrap()
}

/// Register all default shortcuts
///
/// Convenience command to register all default shortcuts at once.
///
/// # Returns
/// * `Ok(())` if all shortcuts were registered successfully
/// * `Err(String)` if any shortcuts failed to register (includes details)
#[tauri::command]
pub async fn register_default_shortcuts(app: AppHandle) -> Result<(), Error> {
    tauri_plugin_telemetry::traced("register_default_shortcuts", async move {
        // Register each default through `register_shortcut`, which routes modifier-only
        // accelerators (e.g. "ShiftRight") to the keyboard service and everything else to
        // the platform layer. Calling the platform layer directly here would hand a bare
        // modifier to tauri-plugin-global-shortcut, which cannot bind one, so it would
        // silently fail to register.
        let defaults = manager::get_defaults();
        let mut errors = Vec::new();

        for shortcut in defaults {
            if let Err(e) = register_shortcut(
                app.clone(),
                shortcut.id.clone(),
                shortcut.accelerator.clone(),
                shortcut.description.clone(),
            ) {
                tracing::warn!(
                    "Failed to register default shortcut '{}': {}",
                    shortcut.id,
                    e
                );
                errors.push(format!("{}: {}", shortcut.id, e));
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "Some default shortcuts failed to register: {}",
                errors.join("; ")
            )
            .into())
        }
    })
    .await
}

/// Unregister all shortcuts
///
/// Removes all registered global shortcuts, including modifier-only shortcuts
/// monitored by the modifier monitor.
///
/// # Returns
/// * `Ok(())` on success
/// * `Err(String)` if unregistration fails
#[tauri::command]
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

/// Register a shortcut with conflict detection
///
/// Attempts to register a shortcut and returns detailed information
/// about success or failure, including alternative suggestions on conflict.
///
/// # Arguments
/// * `id` - Unique identifier for the shortcut
/// * `accelerator` - Keyboard accelerator string (e.g., "F13", "Cmd+Shift+Space")
/// * `description` - Human-readable description of the shortcut's action
///
/// # Returns
/// A `RegistrationResult` indicating success or conflict with suggestions
#[tauri::command]
pub fn try_register_shortcut(
    app: AppHandle,
    id: String,
    accelerator: String,
    description: String,
) -> RegistrationResult {
    tauri_plugin_telemetry::traced_sync("try_register_shortcut", || {
        Ok::<_, std::convert::Infallible>(try_register_shortcut_impl(
            app,
            id,
            accelerator,
            description,
        ))
    })
    .unwrap()
}

/// [`try_register_shortcut`]'s own body, kept as its own function so the
/// early `return`s inside stay plain `RegistrationResult` — wrapping them in
/// `Ok(...)` to fit `traced_sync`'s `Result` bound would touch every one of
/// them for no behavioural change.
fn try_register_shortcut_impl(
    app: AppHandle,
    id: String,
    accelerator: String,
    description: String,
) -> RegistrationResult {
    // Handle modifier-only shortcuts separately
    if keyboard_service::is_modifier_shortcut(&accelerator) {
        if keyboard_service::register_modifier_shortcut(
            id.clone(),
            accelerator.clone(),
            description,
        ) {
            keyboard_service::restart_monitoring(app);
            return RegistrationResult::Success {
                shortcut: accelerator,
                shortcut_id: id,
            };
        } else {
            return RegistrationResult::Conflict(conflict::ShortcutConflict {
                shortcut: accelerator.clone(),
                shortcut_id: id,
                reason: "Failed to register modifier shortcut".to_string(),
                suggestions: vec![],
            });
        }
    }

    // Validate format first
    if let Err(e) = conflict::validate_shortcut_format(&accelerator) {
        return RegistrationResult::Conflict(conflict::ShortcutConflict {
            shortcut: accelerator.clone(),
            shortcut_id: id,
            reason: e,
            suggestions: conflict::suggest_alternatives(&accelerator),
        });
    }

    // Attempt registration (platform-specific)
    #[cfg(target_os = "linux")]
    let result = linux::register(&app, id.clone(), accelerator.clone(), description);
    #[cfg(not(target_os = "linux"))]
    let result = manager::register(&app, id.clone(), accelerator.clone(), description);

    match result {
        Ok(()) => RegistrationResult::Success {
            shortcut: accelerator,
            shortcut_id: id,
        },
        Err(e) => RegistrationResult::Conflict(conflict::create_conflict(&accelerator, &id, &e)),
    }
}

/// Check if a shortcut can be registered without actually registering it
///
/// Performs validation and checks if the shortcut is already registered
/// by this application, but does not register with the system.
///
/// # Arguments
/// * `accelerator` - Keyboard accelerator string to check
///
/// # Returns
/// * `Ok(true)` if the shortcut appears to be available
/// * `Ok(false)` if the shortcut is already registered by this app
/// * `Err(String)` if the format is invalid
#[tauri::command]
pub async fn check_shortcut_available(app: AppHandle, accelerator: String) -> Result<bool, Error> {
    tauri_plugin_telemetry::traced("check_shortcut_available", async move {
        // Modifier-only shortcuts are always "available" (we handle them ourselves)
        if keyboard_service::is_modifier_shortcut(&accelerator) {
            // Check if already registered as a modifier shortcut
            let modifier_shortcuts = keyboard_service::list_modifier_shortcuts();
            let in_use = modifier_shortcuts
                .iter()
                .any(|(_, acc, _)| acc == &accelerator);
            return Ok(!in_use);
        }

        // Validate format
        conflict::validate_shortcut_format(&accelerator)?;

        // Check if already registered by us (platform-specific)
        #[cfg(target_os = "linux")]
        let registered = linux::list_registered();
        #[cfg(not(target_os = "linux"))]
        let registered = manager::list_registered();

        let in_use = registered.iter().any(|s| s.accelerator == accelerator);

        if in_use {
            return Ok(false);
        }

        // On Wayland the compositor (and the user, via the portal dialog) decides
        // the actual binding, so there is nothing to check ahead of time: the
        // requested accelerator is only a preferred trigger. Report it as available
        // and let the portal assign the real key (surfaced via the
        // `wayland-shortcuts-status` event).
        #[cfg(target_os = "linux")]
        {
            if linux::get_display_server() == linux::DisplayServer::Wayland {
                return Ok(true);
            }
        }

        use tauri_plugin_global_shortcut::GlobalShortcutExt;
        let is_registered = app.global_shortcut().is_registered(accelerator.as_str());

        Ok(!is_registered)
    })
    .await
}

/// Get alternative shortcut suggestions
///
/// Returns a list of suggested shortcuts that might work as alternatives
/// to the specified shortcut.
///
/// # Arguments
/// * `shortcut` - The shortcut to find alternatives for
///
/// # Returns
/// A vector of suggested alternative shortcuts
#[tauri::command]
pub fn get_shortcut_suggestions(shortcut: String) -> Vec<String> {
    tauri_plugin_telemetry::traced_sync("get_shortcut_suggestions", || {
        Ok::<_, std::convert::Infallible>(conflict::suggest_alternatives(&shortcut))
    })
    .unwrap()
}
