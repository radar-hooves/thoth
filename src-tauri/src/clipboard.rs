//! Clipboard integration module
//!
//! Provides smart clipboard operations including auto-copy on transcription
//! completion, clipboard history, and configurable formatting options.

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
    /// Image contents, in the platform clipboard's own form.
    Image(SavedImage),
}

/// An image exactly as the macOS pasteboard held it, put back byte for byte.
///
/// Never decoded to RGBA and re-encoded: writing an image through `NSImage`
/// (arboard's `set_image`) leaves a copy of it behind in AppKit on every write,
/// so each restore of a screenshot leaked its full size.
#[cfg(target_os = "macos")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedImage {
    /// Pasteboard type: `public.png` or `public.tiff`.
    pub(crate) uti: String,
    /// The encoded image bytes.
    pub(crate) bytes: Vec<u8>,
}

/// Raw RGBA image contents, arboard's image form.
#[cfg(not(target_os = "macos"))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedImage {
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) bytes: Vec<u8>,
}

/// Decide what to preserve from what the clipboard offered.
///
/// Split from the I/O in [`SavedClipboard::capture`] so the rule is testable
/// without a system clipboard — CI is headless and has none. Text wins when both
/// are present, matching the behaviour before images were handled at all.
fn choose_saved_clipboard(
    text: Option<String>,
    image: Option<SavedImage>,
) -> Option<SavedClipboard> {
    // An empty string is treated as "no text": some platforms return Ok("")
    // rather than an error when the clipboard holds only non-text content, and
    // restoring an empty string would be indistinguishable from wiping it.
    if let Some(text) = text.filter(|t| !t.is_empty()) {
        return Some(SavedClipboard::Text(text));
    }
    image.map(SavedClipboard::Image)
}

/// Runs `f` inside an Objective-C autorelease pool on macOS.
///
/// Capture and restore run on tokio worker threads, which never exit and so
/// never drain their implicit pool: anything AppKit autoreleases during a
/// pasteboard read or write would otherwise live as long as the process.
#[cfg(target_os = "macos")]
fn with_autorelease_pool<T>(f: impl FnOnce() -> T) -> T {
    objc2::rc::autoreleasepool(|_| f())
}

#[cfg(not(target_os = "macos"))]
fn with_autorelease_pool<T>(f: impl FnOnce() -> T) -> T {
    f()
}

/// Image read and write against one `NSPasteboard`: the general pasteboard in
/// the app, a private named one in tests.
#[cfg(target_os = "macos")]
mod pasteboard {
    use super::SavedImage;
    use objc2_app_kit::{NSPasteboard, NSPasteboardTypePNG, NSPasteboardTypeTIFF};
    use objc2_foundation::{NSArray, NSData, NSString};

    /// The pasteboard's image as encoded bytes, PNG preferred over TIFF.
    pub(super) fn read_image(pasteboard: &NSPasteboard) -> Option<SavedImage> {
        // SAFETY: AppKit's pasteboard type constants are immutable statics.
        let wanted = unsafe { [NSPasteboardTypePNG, NSPasteboardTypeTIFF] };
        let uti = pasteboard.availableTypeFromArray(&NSArray::from_slice(&wanted))?;
        let bytes = pasteboard.dataForType(&uti)?.to_vec();
        Some(SavedImage {
            uti: uti.to_string(),
            bytes,
        })
    }

    /// Replace the pasteboard's contents with `image`'s bytes under its type.
    pub(super) fn write_image(pasteboard: &NSPasteboard, image: &SavedImage) -> Result<(), String> {
        pasteboard.clearContents();
        let data = NSData::with_bytes(&image.bytes);
        if pasteboard.setData_forType(Some(&data), &NSString::from_str(&image.uti)) {
            Ok(())
        } else {
            Err(format!(
                "Failed to restore clipboard image: the pasteboard refused {}",
                image.uti
            ))
        }
    }
}

#[cfg(target_os = "macos")]
fn read_image(_clipboard: &mut arboard::Clipboard) -> Option<SavedImage> {
    pasteboard::read_image(&objc2_app_kit::NSPasteboard::generalPasteboard())
}

#[cfg(not(target_os = "macos"))]
fn read_image(clipboard: &mut arboard::Clipboard) -> Option<SavedImage> {
    let image = clipboard.get_image().ok()?;
    Some(SavedImage {
        width: image.width,
        height: image.height,
        bytes: image.bytes.into_owned(),
    })
}

#[cfg(target_os = "macos")]
fn write_image(image: &SavedImage) -> Result<(), String> {
    pasteboard::write_image(&objc2_app_kit::NSPasteboard::generalPasteboard(), image)
}

#[cfg(not(target_os = "macos"))]
fn write_image(image: &SavedImage) -> Result<(), String> {
    arboard::Clipboard::new()
        .map_err(|e| format!("Failed to open clipboard: {e}"))?
        .set_image(arboard::ImageData {
            width: image.width,
            height: image.height,
            bytes: std::borrow::Cow::Borrowed(&image.bytes),
        })
        .map_err(|e| format!("Failed to restore clipboard image: {e}"))
}

impl SavedClipboard {
    /// Capture the current clipboard, preferring text and falling back to an image.
    ///
    /// Returns `None` when the clipboard is empty, unreadable, or holds a type
    /// Thoth cannot round-trip — in which case the caller must not restore, since
    /// writing back a wrong or empty value is worse than leaving the clipboard be.
    pub fn capture() -> Option<Self> {
        with_autorelease_pool(|| {
            let mut clipboard = arboard::Clipboard::new().ok()?;
            let text = clipboard.get_text().ok();
            // Only pay for the image read when there is no text to save.
            let image = if text.as_deref().is_none_or(str::is_empty) {
                read_image(&mut clipboard)
            } else {
                None
            };
            choose_saved_clipboard(text, image)
        })
    }

    /// Write the captured contents back to the clipboard.
    pub fn restore(&self) -> Result<(), String> {
        with_autorelease_pool(|| match self {
            Self::Text(text) => arboard::Clipboard::new()
                .map_err(|e| format!("Failed to open clipboard: {e}"))?
                .set_text(text.clone())
                .map_err(|e| format!("Failed to restore clipboard text: {e}")),
            Self::Image(image) => write_image(image),
        })
    }

    /// Short description for logging, without dumping clipboard contents.
    pub fn describe(&self) -> String {
        match self {
            Self::Text(text) => format!("{} chars of text", text.len()),
            #[cfg(target_os = "macos")]
            Self::Image(image) => format!("{} KB {} image", image.bytes.len() / 1024, image.uti),
            #[cfg(not(target_os = "macos"))]
            Self::Image(image) => format!("{}x{} image", image.width, image.height),
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
pub async fn copy_to_clipboard(
    app: AppHandle,
    text: String,
    source: Option<String>,
) -> Result<(), Error> {
    tauri_plugin_telemetry::traced("copy_to_clipboard", async move {
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
    })
    .await
}

/// Put the most recent history transcription on the clipboard.
///
/// The one implementation behind both the Copy Last shortcut and the tray menu
/// item. An explicit request, so it ignores the auto-copy setting. Returns
/// `Ok(false)` when there is no transcription yet.
pub(crate) fn copy_last_transcription() -> Result<bool, String> {
    let latest = crate::database::transcription::list_transcriptions(Some(1), Some(0))
        .map_err(|e| e.to_string())?
        .into_iter()
        .next();
    let Some(t) = latest else {
        return Ok(false);
    };
    arboard::Clipboard::new()
        .and_then(|mut cb| cb.set_text(t.text))
        .map_err(|e| e.to_string())?;
    Ok(true)
}

/// Copy transcription to clipboard with auto-copy settings applied.
///
/// This is the main entry point for copying transcription results. It checks
/// the current settings and applies formatting as configured.
#[tauri::command]
pub async fn copy_transcription(
    app: AppHandle,
    text: String,
    enhanced: bool,
) -> Result<bool, Error> {
    tauri_plugin_telemetry::traced("copy_transcription", async move {
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
    })
    .await
}

/// Get current clipboard settings.
#[tauri::command]
pub fn get_clipboard_settings() -> ClipboardSettings {
    tauri_plugin_telemetry::traced_sync_value("get_clipboard_settings", || {
        let manager = get_manager().lock();
        manager.settings().clone()
    })
}

/// Update clipboard settings.
#[tauri::command]
pub async fn set_clipboard_settings(settings: ClipboardSettings) -> Result<(), Error> {
    tauri_plugin_telemetry::traced("set_clipboard_settings", async move {
        let mut manager = get_manager().lock();
        manager.update_settings(settings);
        Ok(())
    })
    .await
}

/// Get clipboard history.
#[tauri::command]
pub fn get_clipboard_history() -> Vec<ClipboardHistoryEntry> {
    tauri_plugin_telemetry::traced_sync_value("get_clipboard_history", || {
        let manager = get_manager().lock();
        manager.get_history()
    })
}

/// Clear clipboard history.
#[tauri::command]
pub fn clear_clipboard_history() {
    tauri_plugin_telemetry::traced_sync_value("clear_clipboard_history", || {
        let mut manager = get_manager().lock();
        manager.clear_history();
    })
}

/// Remove a specific entry from clipboard history.
#[tauri::command]
pub fn remove_clipboard_history_entry(id: String) -> bool {
    tauri_plugin_telemetry::traced_sync_value("remove_clipboard_history_entry", || {
        let mut manager = get_manager().lock();
        manager.remove_from_history(&id)
    })
}

/// Copy an entry from clipboard history to the clipboard.
#[tauri::command]
pub async fn copy_from_history(app: AppHandle, id: String) -> Result<(), Error> {
    tauri_plugin_telemetry::traced("copy_from_history", async move {
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
    })
    .await
}

/// Restore the preserved clipboard content.
///
/// Call this after pasting to restore the user's original clipboard content.
#[tauri::command]
pub async fn restore_clipboard(app: AppHandle) -> Result<bool, Error> {
    tauri_plugin_telemetry::traced("restore_clipboard", async move {
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
    })
    .await
}

/// Get the current restore delay setting in milliseconds.
#[tauri::command]
pub fn get_restore_delay() -> u64 {
    tauri_plugin_telemetry::traced_sync_value("get_restore_delay", || {
        let manager = get_manager().lock();
        manager.settings().restore_delay_ms
    })
}

/// Paste text at cursor with automatic clipboard restoration.
///
/// Implements the complete paste-with-restore flow:
/// 1. Save current clipboard contents (if preserve_clipboard enabled)
/// 2. Copy transcription to clipboard
/// 3. Paste at cursor position
/// 4. Restore original clipboard after configured delay (backend-owned)
#[tauri::command]
pub async fn paste_transcription(
    app: AppHandle,
    text: String,
    enhanced: bool,
) -> Result<(), Error> {
    tauri_plugin_telemetry::traced("paste_transcription", async move {
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
    })
    .await
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

    #[cfg(target_os = "macos")]
    fn test_image(bytes: &[u8]) -> SavedImage {
        SavedImage {
            uti: "public.png".to_string(),
            bytes: bytes.to_vec(),
        }
    }

    #[cfg(not(target_os = "macos"))]
    fn test_image(bytes: &[u8]) -> SavedImage {
        SavedImage {
            width: 2,
            height: 1,
            bytes: bytes.to_vec(),
        }
    }

    fn image(bytes: &[u8]) -> Option<SavedImage> {
        Some(test_image(bytes))
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
            Some(SavedClipboard::Image(test_image(&[1, 2, 3, 4, 5, 6, 7, 8])))
        );
    }

    /// Some platforms return `Ok("")` rather than an error when the clipboard
    /// holds only non-text content, so an empty string must not shadow an image.
    #[test]
    fn empty_text_does_not_shadow_an_image() {
        let saved = choose_saved_clipboard(Some(String::new()), image(&[9, 9, 9, 9]));
        assert!(
            matches!(saved, Some(SavedClipboard::Image(_))),
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

        let described = SavedClipboard::Image(test_image(&[0; 4096])).describe();
        #[cfg(target_os = "macos")]
        assert_eq!(described, "4 KB public.png image");
        #[cfg(not(target_os = "macos"))]
        assert_eq!(described, "2x1 image");
    }

    /// A private named pasteboard, so tests never touch the user's clipboard.
    #[cfg(target_os = "macos")]
    fn private_pasteboard(name: &str) -> objc2::rc::Retained<objc2_app_kit::NSPasteboard> {
        objc2_app_kit::NSPasteboard::pasteboardWithName(&objc2_foundation::NSString::from_str(name))
    }

    /// The image comes back byte for byte under its own type.
    #[cfg(target_os = "macos")]
    #[test]
    fn image_round_trips_byte_for_byte() {
        let pb = private_pasteboard("com.thoth.test.round-trip");
        let original = SavedImage {
            uti: "public.tiff".to_string(),
            bytes: (0..=255u8).cycle().take(10_000).collect(),
        };
        pasteboard::write_image(&pb, &original).expect("write");
        assert_eq!(pasteboard::read_image(&pb), Some(original));
        pb.clearContents();
    }

    /// This process's physical footprint, the number Activity Monitor shows.
    #[cfg(target_os = "macos")]
    fn phys_footprint() -> u64 {
        // `rusage_info_v0` from <libproc.h>.
        #[repr(C)]
        #[derive(Default)]
        struct RusageInfoV0 {
            uuid: [u8; 16],
            user_time: u64,
            system_time: u64,
            pkg_idle_wkups: u64,
            interrupt_wkups: u64,
            pageins: u64,
            wired_size: u64,
            resident_size: u64,
            phys_footprint: u64,
            proc_start_abstime: u64,
            proc_exit_abstime: u64,
        }
        unsafe extern "C" {
            fn proc_pid_rusage(pid: i32, flavor: i32, buffer: *mut RusageInfoV0) -> i32;
        }
        let mut info = RusageInfoV0::default();
        // SAFETY: flavor 0 (RUSAGE_INFO_V0) fills exactly this struct, for our own pid.
        let rc = unsafe { proc_pid_rusage(std::process::id() as i32, 0, &mut info) };
        assert_eq!(rc, 0, "proc_pid_rusage failed");
        info.phys_footprint
    }

    /// Repeated capture and restore of a screenshot-sized image must not grow
    /// the process. Runs on a thread that outlives the loop, as a tokio worker
    /// does, so anything left in its autorelease pool would accumulate.
    #[cfg(target_os = "macos")]
    #[test]
    #[serial_test::serial(clipboard_footprint)]
    fn repeated_image_restore_does_not_grow_the_process() {
        const PAYLOAD: usize = 16 * 1024 * 1024;
        const CYCLES: u64 = 8;
        let growth = std::thread::spawn(|| {
            let pb = private_pasteboard("com.thoth.test.footprint");
            let screenshot = SavedImage {
                uti: "public.tiff".to_string(),
                bytes: vec![0x7f; PAYLOAD],
            };
            with_autorelease_pool(|| pasteboard::write_image(&pb, &screenshot)).expect("seed");
            // One warm-up cycle so allocator caches are already sized.
            let saved = with_autorelease_pool(|| pasteboard::read_image(&pb)).expect("read");
            with_autorelease_pool(|| pasteboard::write_image(&pb, &saved)).expect("write");
            drop(saved);
            let before = phys_footprint();
            for _ in 0..CYCLES {
                let saved = with_autorelease_pool(|| pasteboard::read_image(&pb)).expect("read");
                with_autorelease_pool(|| pasteboard::write_image(&pb, &saved)).expect("write");
            }
            let growth = phys_footprint().saturating_sub(before);
            pb.clearContents();
            growth
        })
        .join()
        .expect("worker panicked");
        // A per-cycle leak of the image adds at least CYCLES payloads.
        assert!(
            growth < 2 * PAYLOAD as u64,
            "footprint grew {} MB over {CYCLES} restores of a {} MB image",
            growth / 1_048_576,
            PAYLOAD / 1_048_576
        );
    }

    /// Round-trips real content through the system clipboard. Ignored by default:
    /// CI is headless and has no clipboard.
    ///
    /// Run with: `cargo test --lib clipboard -- --ignored --nocapture`
    #[cfg(not(target_os = "macos"))]
    #[test]
    #[ignore = "requires a system clipboard (headless CI has none)"]
    fn manual_image_round_trip() {
        // 2x1 RGBA, deliberately not uniform so a truncated round-trip shows up.
        let original = SavedClipboard::Image(SavedImage {
            width: 2,
            height: 1,
            bytes: vec![255, 0, 0, 255, 0, 0, 255, 255],
        });
        original.restore().expect("failed to write image");

        let recaptured = SavedClipboard::capture().expect("clipboard read back empty");
        match recaptured {
            SavedClipboard::Image(image) => {
                assert_eq!((image.width, image.height), (2, 1));
                assert_eq!(
                    image.bytes.len(),
                    8,
                    "RGBA byte count changed in the round trip"
                );
            }
            other => panic!("expected an image back, got {}", other.describe()),
        }
    }
}
