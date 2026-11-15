//! Shortcut conflict detection for Thoth
//!
//! Turns a failed shortcut registration into a message the user can read.

use serde::Serialize;

/// A shortcut the system refused to register, and why.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShortcutConflict {
    /// The shortcut that failed to register
    pub shortcut: String,
    /// The shortcut ID that was being registered
    pub shortcut_id: String,
    /// Human-readable reason for the failure
    pub reason: String,
}

/// Classify the type of registration error
///
/// # Arguments
/// * `error_message` - The error message from the registration attempt
///
/// # Returns
/// A user-friendly description of what went wrong
pub fn classify_error(error_message: &str) -> String {
    let error_lower = error_message.to_lowercase();

    if error_lower.contains("already registered") || error_lower.contains("in use") {
        "This shortcut is already registered by another application.".to_string()
    } else if error_lower.contains("invalid") || error_lower.contains("parse") {
        "This shortcut format is not recognised. Please use a valid key combination.".to_string()
    } else if error_lower.contains("permission") || error_lower.contains("access") {
        "Accessibility permission is required to register global shortcuts.".to_string()
    } else if error_lower.contains("reserved") || error_lower.contains("system") {
        "This shortcut is reserved by the operating system.".to_string()
    } else {
        format!(
            "Failed to register shortcut. The system reported: {}",
            error_message
        )
    }
}

/// Create a conflict result from a registration error
pub fn create_conflict(shortcut: &str, shortcut_id: &str, error: &str) -> ShortcutConflict {
    ShortcutConflict {
        shortcut: shortcut.to_string(),
        shortcut_id: shortcut_id.to_string(),
        reason: classify_error(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classify_error_already_registered() {
        let reason = classify_error("Shortcut already registered");
        assert!(reason.contains("already registered"));
    }

    #[test]
    fn test_classify_error_permission() {
        let reason = classify_error("Permission denied");
        assert!(reason.contains("Accessibility permission"));
    }

    #[test]
    fn test_classify_error_generic() {
        let reason = classify_error("Unknown error occurred");
        assert!(reason.contains("system reported"));
    }

    #[test]
    fn test_create_conflict() {
        let conflict = create_conflict("F13", "toggle_recording", "Shortcut already registered");
        assert_eq!(conflict.shortcut, "F13");
        assert_eq!(conflict.shortcut_id, "toggle_recording");
        assert!(conflict.reason.contains("already registered"));
    }
}
