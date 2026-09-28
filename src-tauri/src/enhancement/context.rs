//! Context capture for AI enhancement
//!
//! Provides functionality to capture clipboard content and build context
//! for AI-enhanced transcription processing.

use arboard::Clipboard;
use tracing::{debug, warn};

/// Captures context from various sources for AI enhancement.
///
/// Currently supports clipboard capture, with placeholder for future
/// selected text capture functionality.
pub struct ContextCapture {
    clipboard: Option<Clipboard>,
}

impl ContextCapture {
    /// Create a new context capture instance.
    ///
    /// Initialises the clipboard access. If clipboard initialisation fails,
    /// the instance will still be created but clipboard capture will be unavailable.
    pub fn new() -> Self {
        let clipboard = match Clipboard::new() {
            Ok(cb) => {
                debug!("Clipboard access initialised");
                Some(cb)
            }
            Err(e) => {
                warn!("Failed to initialise clipboard access: {}", e);
                None
            }
        };

        Self { clipboard }
    }

    /// Capture the current clipboard text content.
    ///
    /// Returns `None` if clipboard access is unavailable or if the clipboard
    /// does not contain text content.
    pub fn capture_clipboard(&mut self) -> Option<String> {
        let clipboard = self.clipboard.as_mut()?;

        match clipboard.get_text() {
            Ok(text) => {
                if text.is_empty() {
                    debug!("Clipboard is empty");
                    None
                } else {
                    debug!("Captured {} characters from clipboard", text.len());
                    Some(text)
                }
            }
            Err(e) => {
                debug!("No text content in clipboard: {}", e);
                None
            }
        }
    }

    /// Placeholder for future selected text capture functionality.
    ///
    /// This will capture the currently selected text from the active application.
    /// Currently returns `None` as this feature requires accessibility APIs.
    pub fn capture_selected_text(&self) -> Option<String> {
        // TODO: Implement using accessibility APIs (macOS) or platform-specific methods
        // This would involve:
        // 1. Getting the frontmost application
        // 2. Sending Cmd+C to copy selection
        // 3. Reading clipboard (with restore of previous content)
        // For now, this is a placeholder for future implementation.
        debug!("Selected text capture not yet implemented");
        None
    }
}

impl Default for ContextCapture {
    fn default() -> Self {
        Self::new()
    }
}

/// Build a formatted context string for AI enhancement.
///
/// Combines clipboard content (if provided) with the transcription text
/// in a structured format that AI providers can use for context-aware
/// enhancement.
///
/// # Arguments
///
/// * `transcription` - The transcription text to enhance
/// * `clipboard` - Optional clipboard content to provide as context
///
/// # Returns
///
/// A formatted string combining context and transcription.
pub fn build_context(transcription: &str, clipboard: Option<&str>) -> String {
    match clipboard {
        Some(clip) if !clip.is_empty() => {
            format!(
                "[Context from clipboard]\n{}\n\n[Transcription to enhance]\n{}",
                clip, transcription
            )
        }
        _ => transcription.to_string(),
    }
}

// ============================================================================
// Tauri Commands
// ============================================================================

use parking_lot::Mutex;
use std::sync::OnceLock;

/// Global context capture instance
static CONTEXT_CAPTURE: OnceLock<Mutex<ContextCapture>> = OnceLock::new();

fn get_context_capture() -> &'static Mutex<ContextCapture> {
    CONTEXT_CAPTURE.get_or_init(|| Mutex::new(ContextCapture::new()))
}

/// Get the current clipboard text content.
///
/// Returns the clipboard content if available and contains text,
/// otherwise returns `None`.
#[tauri::command]
pub fn get_clipboard_context() -> Option<String> {
    let mut capture = get_context_capture().lock();
    capture.capture_clipboard()
}

/// Build an enhancement context combining transcription with optional clipboard content.
///
/// # Arguments
///
/// * `transcription` - The transcription text to enhance
/// * `include_clipboard` - Whether to include clipboard content as context
///
/// # Returns
///
/// A formatted context string ready for AI enhancement.
#[tauri::command]
pub fn build_enhancement_context(transcription: String, include_clipboard: bool) -> String {
    let clipboard_content = if include_clipboard {
        let mut capture = get_context_capture().lock();
        capture.capture_clipboard()
    } else {
        None
    };

    build_context(&transcription, clipboard_content.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_context_with_clipboard() {
        let transcription = "Hello world";
        let clipboard = Some("Some context");

        let result = build_context(transcription, clipboard);

        assert!(result.contains("[Context from clipboard]"));
        assert!(result.contains("Some context"));
        assert!(result.contains("[Transcription to enhance]"));
        assert!(result.contains("Hello world"));
    }

    #[test]
    fn test_build_context_without_clipboard() {
        let transcription = "Hello world";

        let result = build_context(transcription, None);

        assert_eq!(result, "Hello world");
        assert!(!result.contains("[Context from clipboard]"));
    }

    #[test]
    fn test_build_context_with_empty_clipboard() {
        let transcription = "Hello world";
        let clipboard = Some("");

        let result = build_context(transcription, clipboard);

        assert_eq!(result, "Hello world");
        assert!(!result.contains("[Context from clipboard]"));
    }

    #[test]
    fn test_context_capture_creation() {
        // This test verifies ContextCapture can be created
        // Actual clipboard access may not work in all test environments
        let _capture = ContextCapture::new();
    }

    #[test]
    fn test_selected_text_placeholder() {
        let capture = ContextCapture::new();
        let result = capture.capture_selected_text();
        assert!(result.is_none());
    }
}
