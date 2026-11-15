//! Clipboard helpers for the output pipeline: the saved-clipboard round trip
//! that restores what the user had after a paste, and the auto-copy and
//! Copy Last writes.

use crate::error::Error;
use tauri::AppHandle;
use tauri_plugin_clipboard_manager::ClipboardExt;

/// How long after a paste the user's previous clipboard is put back.
pub(crate) const RESTORE_DELAY_MS: u64 = 1000;

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

/// Write `text` to the system clipboard (the user's auto-copy setting).
pub(crate) fn copy_transcription(app: &AppHandle, text: &str) -> Result<(), Error> {
    app.clipboard()
        .write_text(text)
        .map_err(|e| format!("Failed to copy transcription: {e}").into())
}

#[cfg(test)]
mod tests {
    use super::*;

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
