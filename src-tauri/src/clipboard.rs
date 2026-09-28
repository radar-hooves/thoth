//! Clipboard integration module
//!
//! Provides smart clipboard operations including auto-copy on transcription
//! completion, clipboard history, and configurable formatting options.

use crate::TELEMETRY_TARGET;
use crate::error::Error;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::OnceLock;
use tauri::AppHandle;
use tauri_plugin_clipboard_manager::ClipboardExt;
use tracing::{debug, error, info};

/// Maximum number of items to keep in clipboard history
const MAX_HISTORY_SIZE: usize = 50;

/// Clipboard format options for copying transcriptions
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ClipboardFormat {
    /// Plain text format (default)
    #[default]
    PlainText,
    /// Rich text with basic formatting preserved
    RichText,
    /// Markdown format
    Markdown,
}

/// Clipboard settings for auto-copy behaviour
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardSettings {
    /// Whether to automatically copy transcription on completion
    pub auto_copy_enabled: bool,
    /// Format to use when copying
    pub format: ClipboardFormat,
    /// Whether to show notification on copy
    pub show_notification: bool,
    /// Whether to preserve original clipboard content (restore after paste)
    pub preserve_clipboard: bool,
    /// Delay in milliseconds before restoring clipboard (default 1000ms)
    pub restore_delay_ms: u64,
    /// Whether to track clipboard history
    pub history_enabled: bool,
}

impl Default for ClipboardSettings {
    fn default() -> Self {
        Self {
            auto_copy_enabled: false,
            format: ClipboardFormat::PlainText,
            show_notification: false,
            preserve_clipboard: true,
            restore_delay_ms: 1000,
            history_enabled: true,
        }
    }
}

/// An entry in the clipboard history
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardHistoryEntry {
    /// Unique identifier for the entry
    pub id: String,
    /// The text content
    pub text: String,
    /// Timestamp when the entry was created (ISO 8601)
    pub timestamp: String,
    /// Optional source description (e.g., "transcription", "manual copy")
    pub source: String,
}

/// Clipboard contents captured before a paste, so the matching type can be put back.
///
/// Preserving only text is what caused #101: `get_text()` errors when the
/// clipboard holds an image, so the saved value was `None` and the image was
/// destroyed with nothing to restore. That breaks the clipboard-restore
/// invariant in `.claude/CLAUDE.md`, which is not qualified by content type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SavedClipboard {
    /// Plain text contents.
    Text(String),
    /// Raw RGBA image contents.
    Image {
        width: usize,
        height: usize,
        bytes: Vec<u8>,
    },
}

/// Decide what to preserve from what the clipboard offered.
///
/// Split from the I/O in [`SavedClipboard::capture`] so the rule is testable
/// without a system clipboard — CI is headless and has none. Text wins when both
/// are present, matching the behaviour before images were handled at all.
fn choose_saved_clipboard(
    text: Option<String>,
    image: Option<(usize, usize, Vec<u8>)>,
) -> Option<SavedClipboard> {
    // An empty string is treated as "no text": some platforms return Ok("")
    // rather than an error when the clipboard holds only non-text content, and
    // restoring an empty string would be indistinguishable from wiping it.
    if let Some(text) = text.filter(|t| !t.is_empty()) {
        return Some(SavedClipboard::Text(text));
    }
    image.map(|(width, height, bytes)| SavedClipboard::Image {
        width,
        height,
        bytes,
    })
}

impl SavedClipboard {
    /// Capture the current clipboard, preferring text and falling back to an image.
    ///
    /// Returns `None` when the clipboard is empty, unreadable, or holds a type
    /// Thoth cannot round-trip — in which case the caller must not restore, since
    /// writing back a wrong or empty value is worse than leaving the clipboard be.
    pub fn capture() -> Option<Self> {
        let mut clipboard = arboard::Clipboard::new().ok()?;
        let text = clipboard.get_text().ok();
        // Only pay for the image read when there is no text to save.
        let image = if text.as_deref().is_none_or(str::is_empty) {
            clipboard
                .get_image()
                .ok()
                .map(|img| (img.width, img.height, img.bytes.into_owned()))
        } else {
            None
        };
        choose_saved_clipboard(text, image)
    }

    /// Write the captured contents back to the clipboard.
    pub fn restore(&self) -> Result<(), String> {
        let mut clipboard =
            arboard::Clipboard::new().map_err(|e| format!("Failed to open clipboard: {e}"))?;
        match self {
            Self::Text(text) => clipboard
                .set_text(text.clone())
                .map_err(|e| format!("Failed to restore clipboard text: {e}")),
            Self::Image {
                width,
                height,
                bytes,
            } => clipboard
                .set_image(arboard::ImageData {
                    width: *width,
                    height: *height,
                    bytes: std::borrow::Cow::Borrowed(bytes),
                })
                .map_err(|e| format!("Failed to restore clipboard image: {e}")),
        }
    }

    /// Short description for logging, without dumping clipboard contents.
    pub fn describe(&self) -> String {
        match self {
            Self::Text(text) => format!("{} chars of text", text.len()),
            Self::Image { width, height, .. } => format!("{width}x{height} image"),
        }
    }
}

/// Manages clipboard state and history
pub struct ClipboardManager {
    /// Current settings
    settings: ClipboardSettings,
    /// Clipboard history (most recent first)
    history: VecDeque<ClipboardHistoryEntry>,
    /// Preserved clipboard content for restore after paste
    preserved_content: Option<String>,
}

impl ClipboardManager {
    /// Create a new clipboard manager with default settings.
    pub fn new() -> Self {
        Self {
            settings: ClipboardSettings::default(),
            history: VecDeque::with_capacity(MAX_HISTORY_SIZE),
            preserved_content: None,
        }
    }

    /// Get current settings.
    pub fn settings(&self) -> &ClipboardSettings {
        &self.settings
    }

    /// Update settings.
    pub fn update_settings(&mut self, settings: ClipboardSettings) {
        debug!("Updating clipboard settings: {:?}", settings);
        self.settings = settings;

        // Clear history if disabled
        if !self.settings.history_enabled {
            self.history.clear();
        }
    }

    /// Add an entry to the clipboard history.
    pub fn add_to_history(&mut self, text: String, source: &str) {
        if !self.settings.history_enabled {
            return;
        }

        let entry = ClipboardHistoryEntry {
            id: uuid::Uuid::new_v4().to_string(),
            text,
            timestamp: chrono::Utc::now().to_rfc3339(),
            source: source.to_string(),
        };

        debug!(
            "Adding to clipboard history: {} chars from {}",
            entry.text.len(),
            source
        );

        // Add to front (most recent first)
        self.history.push_front(entry);

        // Trim to max size
        while self.history.len() > MAX_HISTORY_SIZE {
            self.history.pop_back();
        }
    }

    /// Get clipboard history.
    pub fn get_history(&self) -> Vec<ClipboardHistoryEntry> {
        self.history.iter().cloned().collect()
    }

    /// Clear clipboard history.
    pub fn clear_history(&mut self) {
        debug!("Clearing clipboard history");
        self.history.clear();
    }

    /// Remove a specific entry from history.
    pub fn remove_from_history(&mut self, id: &str) -> bool {
        if let Some(pos) = self.history.iter().position(|e| e.id == id) {
            self.history.remove(pos);
            debug!("Removed entry {} from clipboard history", id);
            true
        } else {
            false
        }
    }

    /// Preserve the current clipboard content for later restoration.
    pub fn preserve_content(&mut self, content: String) {
        debug!("Preserving clipboard content: {} chars", content.len());
        self.preserved_content = Some(content);
    }

    /// Get and clear the preserved clipboard content.
    pub fn take_preserved_content(&mut self) -> Option<String> {
        self.preserved_content.take()
    }
}

impl Default for ClipboardManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Global clipboard manager instance
static CLIPBOARD_MANAGER: OnceLock<Mutex<ClipboardManager>> = OnceLock::new();

fn get_manager() -> &'static Mutex<ClipboardManager> {
    CLIPBOARD_MANAGER.get_or_init(|| Mutex::new(ClipboardManager::new()))
}

/// Copy text to the system clipboard.
///
/// Copies the provided text to the clipboard and optionally adds it to
/// clipboard history.
#[tauri::command]
#[tracing::instrument(target = TELEMETRY_TARGET, skip_all, err)]
pub async fn copy_to_clipboard(
    app: AppHandle,
    text: String,
    source: Option<String>,
) -> Result<(), Error> {
    if text.is_empty() {
        return Err("Cannot copy empty text to clipboard".to_string().into());
    }

    debug!("Copying {} chars to clipboard", text.len());

    app.clipboard().write_text(&text).map_err(|e| {
        error!("Failed to copy to clipboard: {}", e);
        format!("Failed to copy to clipboard: {}", e)
    })?;

    // Add to history
    let source = source.unwrap_or_else(|| "manual".to_string());
    let mut manager = get_manager().lock();
    manager.add_to_history(text, &source);

    info!("Text copied to clipboard from source: {}", source);
    Ok(())
}

/// Copy transcription to clipboard with auto-copy settings applied.
///
/// This is the main entry point for copying transcription results. It checks
/// the current settings and applies formatting as configured.
#[tauri::command]
#[tracing::instrument(target = TELEMETRY_TARGET, skip_all, err)]
pub async fn copy_transcription(
    app: AppHandle,
    text: String,
    enhanced: bool,
) -> Result<bool, Error> {
    let manager = get_manager().lock();
    let settings = manager.settings().clone();
    drop(manager);

    if !settings.auto_copy_enabled {
        debug!("Auto-copy disabled, skipping clipboard copy");
        return Ok(false);
    }

    // Preserve current clipboard content if configured
    if settings.preserve_clipboard {
        if let Ok(current) = app.clipboard().read_text() {
            let mut manager = get_manager().lock();
            manager.preserve_content(current);
        }
    }

    // Format the text according to settings
    let formatted_text = match settings.format {
        ClipboardFormat::PlainText => text.clone(),
        ClipboardFormat::RichText => text.clone(), // Rich text handled by clipboard plugin
        ClipboardFormat::Markdown => {
            // Wrap in code block if it looks like it might benefit
            if text.contains('\n') {
                format!("```\n{}\n```", text)
            } else {
                text.clone()
            }
        }
    };

    // Copy to clipboard
    app.clipboard().write_text(&formatted_text).map_err(|e| {
        error!("Failed to copy transcription: {}", e);
        format!("Failed to copy transcription: {}", e)
    })?;

    // Add to history
    let source = if enhanced {
        "enhanced_transcription"
    } else {
        "transcription"
    };
    let mut manager = get_manager().lock();
    manager.add_to_history(text, source);

    info!(
        "Transcription copied to clipboard (enhanced: {}, format: {:?})",
        enhanced, settings.format
    );

    Ok(true)
}

/// Get current clipboard settings.
#[tauri::command]
#[tracing::instrument(target = TELEMETRY_TARGET, skip_all)]
pub fn get_clipboard_settings() -> ClipboardSettings {
    let manager = get_manager().lock();
    manager.settings().clone()
}

/// Update clipboard settings.
#[tauri::command]
#[tracing::instrument(target = TELEMETRY_TARGET, skip_all, err)]
pub fn set_clipboard_settings(settings: ClipboardSettings) -> Result<(), Error> {
    let mut manager = get_manager().lock();
    manager.update_settings(settings);
    Ok(())
}

/// Get clipboard history.
#[tauri::command]
#[tracing::instrument(target = TELEMETRY_TARGET, skip_all)]
pub fn get_clipboard_history() -> Vec<ClipboardHistoryEntry> {
    let manager = get_manager().lock();
    manager.get_history()
}

/// Clear clipboard history.
#[tauri::command]
#[tracing::instrument(target = TELEMETRY_TARGET, skip_all)]
pub fn clear_clipboard_history() {
    let mut manager = get_manager().lock();
    manager.clear_history();
}

/// Remove a specific entry from clipboard history.
#[tauri::command]
#[tracing::instrument(target = TELEMETRY_TARGET, skip_all)]
pub fn remove_clipboard_history_entry(id: String) -> bool {
    let mut manager = get_manager().lock();
    manager.remove_from_history(&id)
}

/// Copy an entry from clipboard history to the clipboard.
#[tauri::command]
#[tracing::instrument(target = TELEMETRY_TARGET, skip_all, err)]
pub async fn copy_from_history(app: AppHandle, id: String) -> Result<(), Error> {
    let manager = get_manager().lock();
    let entry = manager
        .get_history()
        .into_iter()
        .find(|e| e.id == id)
        .ok_or_else(|| "History entry not found".to_string())?;
    drop(manager);

    app.clipboard().write_text(&entry.text).map_err(|e| {
        error!("Failed to copy from history: {}", e);
        format!("Failed to copy from history: {}", e)
    })?;

    info!("Copied history entry {} to clipboard", id);
    Ok(())
}

/// Restore the preserved clipboard content.
///
/// Call this after pasting to restore the user's original clipboard content.
#[tauri::command]
#[tracing::instrument(target = TELEMETRY_TARGET, skip_all, err)]
pub async fn restore_clipboard(app: AppHandle) -> Result<bool, Error> {
    let mut manager = get_manager().lock();
    if let Some(content) = manager.take_preserved_content() {
        drop(manager);

        app.clipboard().write_text(&content).map_err(|e| {
            error!("Failed to restore clipboard: {}", e);
            format!("Failed to restore clipboard: {}", e)
        })?;

        debug!("Restored preserved clipboard content");
        Ok(true)
    } else {
        Ok(false)
    }
}

/// Get the current restore delay setting in milliseconds.
#[tauri::command]
#[tracing::instrument(target = TELEMETRY_TARGET, skip_all)]
pub fn get_restore_delay() -> u64 {
    let manager = get_manager().lock();
    manager.settings().restore_delay_ms
}

/// Paste text at cursor with automatic clipboard restoration.
///
/// Implements the complete paste-with-restore flow:
/// 1. Save current clipboard contents (if preserve_clipboard enabled)
/// 2. Copy transcription to clipboard
/// 3. Paste at cursor position
/// 4. Restore original clipboard after configured delay (backend-owned)
#[tauri::command]
#[tracing::instrument(target = TELEMETRY_TARGET, skip_all, err)]
pub async fn paste_transcription(
    app: AppHandle,
    text: String,
    enhanced: bool,
) -> Result<(), Error> {
    if text.is_empty() {
        return Err("Cannot paste empty text".to_string().into());
    }

    let manager = get_manager().lock();
    let settings = manager.settings().clone();
    drop(manager);

    // Save current clipboard BEFORE any modification. Captures an image when
    // there is no text, so a screenshot on the clipboard survives a dictation
    // paste instead of being silently destroyed (#101).
    let saved_clipboard = if settings.preserve_clipboard {
        SavedClipboard::capture()
    } else {
        None
    };

    // Format the text according to settings
    let formatted_text = match settings.format {
        ClipboardFormat::PlainText => text.clone(),
        ClipboardFormat::RichText => text.clone(),
        ClipboardFormat::Markdown => {
            if text.contains('\n') {
                format!("```\n{}\n```", text)
            } else {
                text.clone()
            }
        }
    };

    // Copy to clipboard and paste
    app.clipboard().write_text(&formatted_text).map_err(|e| {
        error!("Failed to copy transcription for paste: {}", e);
        format!("Failed to copy transcription: {}", e)
    })?;

    // Add to history
    let source = if enhanced {
        "enhanced_transcription"
    } else {
        "transcription"
    };
    {
        let mut manager = get_manager().lock();
        manager.add_to_history(text, source);
    }

    // Trailing space and auto-submit are transcription settings rather than
    // clipboard settings, so they are read from the config here rather than
    // from ClipboardSettings (#112).
    let transcription_cfg = crate::config::get_config()
        .map(|c| c.transcription)
        .unwrap_or_default();

    // Perform paste
    let insert_text = crate::text_insert::apply_trailing_space(
        &formatted_text,
        transcription_cfg.append_trailing_space,
    );
    crate::text_insert::insert_text_by_paste(insert_text, Some(50)).map_err(|e| {
        error!("Failed to paste transcription: {}", e);
        format!("Failed to paste: {}", e)
    })?;

    // Only after the paste succeeded: submitting a failed paste would send an
    // empty or half-written message.
    if let Err(e) = crate::text_insert::send_auto_submit(transcription_cfg.auto_submit) {
        tracing::warn!("Failed to send auto-submit key: {}", e);
    }

    info!(
        "Transcription pasted (enhanced: {}, preserve: {})",
        enhanced, settings.preserve_clipboard
    );

    // Restore original clipboard in the background after configured delay.
    // This is backend-owned; the frontend does not need to call restore_clipboard.
    //
    // Ordering is safe across platforms: the paste above is synchronous, so the
    // transcription is guaranteed to be on the clipboard when the paste keystroke
    // fires; the restore only runs after the delay that follows. On Wayland,
    // clipboard ownership is tied to a focused client, so a delayed restore is
    // best-effort — if Thoth has lost focus by the time it fires the compositor
    // may refuse the write. That only affects restoring the user's *previous*
    // clipboard, never the correctness of the paste itself, and the failure is
    // logged.
    if let Some(original) = saved_clipboard {
        let delay = settings.restore_delay_ms;
        tokio::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_millis(delay)).await;
            let what = original.describe();
            match original.restore() {
                Ok(()) => debug!("Clipboard restored ({what}) after {}ms", delay),
                Err(e) => tracing::warn!("Failed to restore clipboard: {}", e),
            }
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clipboard_manager_creation() {
        let manager = ClipboardManager::new();
        assert!(!manager.settings().auto_copy_enabled);
        assert!(manager.get_history().is_empty());
    }

    #[test]
    fn test_clipboard_settings_default() {
        let settings = ClipboardSettings::default();
        assert!(!settings.auto_copy_enabled);
        assert_eq!(settings.format, ClipboardFormat::PlainText);
        assert!(!settings.show_notification);
        assert!(settings.preserve_clipboard);
        assert_eq!(settings.restore_delay_ms, 1000);
        assert!(settings.history_enabled);
    }

    #[test]
    fn test_add_to_history() {
        let mut manager = ClipboardManager::new();
        manager.add_to_history("Test text".to_string(), "test");

        let history = manager.get_history();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].text, "Test text");
        assert_eq!(history[0].source, "test");
    }

    #[test]
    fn test_history_max_size() {
        let mut manager = ClipboardManager::new();

        // Add more than MAX_HISTORY_SIZE entries
        for i in 0..MAX_HISTORY_SIZE + 10 {
            manager.add_to_history(format!("Text {}", i), "test");
        }

        let history = manager.get_history();
        assert_eq!(history.len(), MAX_HISTORY_SIZE);
        // Most recent should be first
        assert_eq!(history[0].text, format!("Text {}", MAX_HISTORY_SIZE + 9));
    }

    #[test]
    fn test_history_disabled() {
        let mut manager = ClipboardManager::new();
        let mut settings = manager.settings().clone();
        settings.history_enabled = false;
        manager.update_settings(settings);

        manager.add_to_history("Test text".to_string(), "test");

        let history = manager.get_history();
        assert!(history.is_empty());
    }

    #[test]
    fn test_remove_from_history() {
        let mut manager = ClipboardManager::new();
        manager.add_to_history("Test 1".to_string(), "test");
        manager.add_to_history("Test 2".to_string(), "test");

        let history = manager.get_history();
        let id = history[0].id.clone();

        assert!(manager.remove_from_history(&id));
        assert_eq!(manager.get_history().len(), 1);
        assert!(!manager.remove_from_history(&id)); // Already removed
    }

    #[test]
    fn test_clear_history() {
        let mut manager = ClipboardManager::new();
        manager.add_to_history("Test 1".to_string(), "test");
        manager.add_to_history("Test 2".to_string(), "test");

        manager.clear_history();
        assert!(manager.get_history().is_empty());
    }

    #[test]
    fn test_preserve_content() {
        let mut manager = ClipboardManager::new();

        manager.preserve_content("Original content".to_string());
        assert!(manager.preserved_content.is_some());

        let content = manager.take_preserved_content();
        assert_eq!(content, Some("Original content".to_string()));
        assert!(manager.preserved_content.is_none());
    }

    #[test]
    fn test_clipboard_format_serialisation() {
        let plain = ClipboardFormat::PlainText;
        let serialised = serde_json::to_string(&plain).unwrap();
        assert_eq!(serialised, r#""plain_text""#);

        let deserialised: ClipboardFormat = serde_json::from_str(&serialised).unwrap();
        assert_eq!(deserialised, ClipboardFormat::PlainText);
    }

    fn image(bytes: &[u8]) -> Option<(usize, usize, Vec<u8>)> {
        Some((2, 1, bytes.to_vec()))
    }

    /// The #101 regression: an image on the clipboard with no text must be
    /// preserved, not dropped.
    ///
    /// Previously the save was `read_text().ok()`, which returns `Err` for an
    /// image, so nothing was saved and the image was destroyed by the paste with
    /// nothing to restore.
    #[test]
    fn image_with_no_text_is_preserved() {
        let saved = choose_saved_clipboard(None, image(&[1, 2, 3, 4, 5, 6, 7, 8]));
        assert_eq!(
            saved,
            Some(SavedClipboard::Image {
                width: 2,
                height: 1,
                bytes: vec![1, 2, 3, 4, 5, 6, 7, 8],
            })
        );
    }

    /// Some platforms return `Ok("")` rather than an error when the clipboard
    /// holds only non-text content, so an empty string must not shadow an image.
    #[test]
    fn empty_text_does_not_shadow_an_image() {
        let saved = choose_saved_clipboard(Some(String::new()), image(&[9, 9, 9, 9]));
        assert!(
            matches!(saved, Some(SavedClipboard::Image { .. })),
            "empty text shadowed the image: {saved:?}"
        );
    }

    /// Text still wins when both are present — the behaviour before images were
    /// handled at all, so this change cannot regress the common path.
    #[test]
    fn text_wins_over_image() {
        let saved = choose_saved_clipboard(Some("hello".to_string()), image(&[1, 2, 3, 4]));
        assert_eq!(saved, Some(SavedClipboard::Text("hello".to_string())));
    }

    #[test]
    fn text_alone_is_preserved() {
        assert_eq!(
            choose_saved_clipboard(Some("hi".to_string()), None),
            Some(SavedClipboard::Text("hi".to_string()))
        );
    }

    /// Nothing to save must stay `None`. The caller skips the restore entirely in
    /// that case; writing back an empty value would wipe whatever arrived in the
    /// meantime, which is worse than leaving the clipboard alone.
    #[test]
    fn nothing_to_save_is_none() {
        assert_eq!(choose_saved_clipboard(None, None), None);
        assert_eq!(choose_saved_clipboard(Some(String::new()), None), None);
    }

    #[test]
    fn describe_does_not_leak_contents() {
        let secret = SavedClipboard::Text("hunter2 correct horse".to_string());
        let described = secret.describe();
        assert!(!described.contains("hunter2"), "leaked text: {described}");
        assert_eq!(described, "21 chars of text");

        assert_eq!(
            SavedClipboard::Image {
                width: 800,
                height: 600,
                bytes: vec![0; 16],
            }
            .describe(),
            "800x600 image"
        );
    }

    /// Round-trips real content through the system clipboard. Ignored by default:
    /// CI is headless and has no clipboard.
    ///
    /// Run with: `cargo test --lib clipboard -- --ignored --nocapture`
    #[test]
    #[ignore = "requires a system clipboard (headless CI has none)"]
    fn manual_image_round_trip() {
        // 2x1 RGBA, deliberately not uniform so a truncated round-trip shows up.
        let original = SavedClipboard::Image {
            width: 2,
            height: 1,
            bytes: vec![255, 0, 0, 255, 0, 0, 255, 255],
        };
        original.restore().expect("failed to write image");

        let recaptured = SavedClipboard::capture().expect("clipboard read back empty");
        match recaptured {
            SavedClipboard::Image {
                width,
                height,
                bytes,
            } => {
                assert_eq!((width, height), (2, 1));
                assert_eq!(bytes.len(), 8, "RGBA byte count changed in the round trip");
            }
            other => panic!("expected an image back, got {}", other.describe()),
        }
    }
}
