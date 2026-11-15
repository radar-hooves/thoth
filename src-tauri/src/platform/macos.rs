//! macOS-specific platform functionality

use objc2::runtime::AnyClass;
use objc2::{class, msg_send};
use objc2_foundation::NSString;
use std::process::Command;
use std::time::Duration;
use tauri::Emitter;

/// Whether Thoth may post synthetic keystrokes (paste, auto-submit) and read
/// the focused element's caret. macOS 27 shows this switch as "Device Control
/// and Data Access"; earlier releases call it Accessibility.
pub fn check_accessibility_permission() -> bool {
    unsafe {
        #[link(name = "ApplicationServices", kind = "framework")]
        unsafe extern "C" {
            fn AXIsProcessTrusted() -> bool;
        }

        AXIsProcessTrusted()
    }
}

/// Microphone authorization status values
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MicrophoneStatus {
    /// User has not yet made a choice
    NotDetermined,
    /// Access is restricted (e.g., parental controls)
    Restricted,
    /// User explicitly denied access
    Denied,
    /// User granted access
    Authorized,
    /// Unknown status
    Unknown,
}

impl From<i64> for MicrophoneStatus {
    fn from(value: i64) -> Self {
        match value {
            0 => MicrophoneStatus::NotDetermined,
            1 => MicrophoneStatus::Restricted,
            2 => MicrophoneStatus::Denied,
            3 => MicrophoneStatus::Authorized,
            _ => MicrophoneStatus::Unknown,
        }
    }
}

/// Check microphone permission status
///
/// Uses AVFoundation's AVCaptureDevice to check authorization status for audio.
pub fn check_microphone_permission() -> MicrophoneStatus {
    unsafe {
        #[link(name = "AVFoundation", kind = "framework")]
        unsafe extern "C" {}

        let cls: &AnyClass = class!(AVCaptureDevice);
        let media_type = NSString::from_str("soun");

        // authorizationStatusForMediaType: returns 0..3 — the NSString must stay
        // alive for the duration of the message send but is not retained by the
        // callee, so binding it above is sufficient.
        let status: i64 = msg_send![cls, authorizationStatusForMediaType: &*media_type];
        MicrophoneStatus::from(status)
    }
}

/// Ask for whatever Thoth is missing, one system dialog at a time: the
/// microphone first, then Accessibility once the microphone dialog is answered.
/// Nothing is asked twice and nothing needs an administrator password.
pub fn request_permissions(app: tauri::AppHandle) {
    let bundle_id = app.config().identifier.clone();
    request_microphone(&bundle_id.clone(), move || {
        request_accessibility(&bundle_id)
    });
}

fn request_microphone(bundle_id: &str, then: impl FnOnce() + Send + 'static) {
    match check_microphone_permission() {
        MicrophoneStatus::Authorized => then(),
        // Parental controls or MDM: nothing Thoth can ask for.
        MicrophoneStatus::Restricted => {
            open_privacy_pane("Privacy_Microphone");
            then();
        }
        // A refusal is remembered and the dialog never returns, so forget it
        // and ask again: the user pressed Grant to be asked.
        MicrophoneStatus::Denied => {
            forget_decision("Microphone", bundle_id);
            ask_microphone(then);
        }
        MicrophoneStatus::NotDetermined | MicrophoneStatus::Unknown => ask_microphone(then),
    }
}

/// Show the system microphone dialog; `then` runs once it is answered.
fn ask_microphone(then: impl FnOnce() + Send + 'static) {
    let then = parking_lot::Mutex::new(Some(then));
    unsafe {
        #[link(name = "AVFoundation", kind = "framework")]
        unsafe extern "C" {}

        let cls: &AnyClass = class!(AVCaptureDevice);
        let media_type = NSString::from_str("soun");

        // requestAccessForMediaType:completionHandler: requires a proper ObjC
        // block; it fires on a background thread when the user responds.
        let handler: block2::RcBlock<dyn Fn(objc2::runtime::Bool)> =
            block2::RcBlock::new(move |granted: objc2::runtime::Bool| {
                if !granted.as_bool() {
                    // No dialog was shown (macOS still remembers a refusal) or
                    // the user declined it: the switch is the only way left.
                    open_privacy_pane("Privacy_Microphone");
                }
                if let Some(then) = then.lock().take() {
                    then();
                }
            });

        let _: () = msg_send![
            cls,
            requestAccessForMediaType: &*media_type,
            completionHandler: &*handler
        ];
    }
}

/// Ask for Accessibility. macOS shows its own dialog, whose "Open System
/// Settings" lands on the switch, and lists Thoth there switched off.
fn request_accessibility(bundle_id: &str) {
    if check_accessibility_permission() {
        return;
    }
    // A row recorded against another signature (an ad-hoc build, an older
    // certificate) shows switched on yet is not honoured, and macOS will not
    // prompt over it. Clearing Thoth's own row makes the switch the user flips
    // the live one.
    forget_decision("Accessibility", bundle_id);
    unsafe {
        use core_foundation::base::TCFType;
        use core_foundation::boolean::CFBoolean;
        use core_foundation::dictionary::CFDictionary;
        use core_foundation::string::{CFString, CFStringRef};

        #[link(name = "ApplicationServices", kind = "framework")]
        unsafe extern "C" {
            static kAXTrustedCheckOptionPrompt: CFStringRef;
            fn AXIsProcessTrustedWithOptions(options: *const std::ffi::c_void) -> bool;
        }

        let options = CFDictionary::from_CFType_pairs(&[(
            CFString::wrap_under_get_rule(kAXTrustedCheckOptionPrompt),
            CFBoolean::true_value(),
        )]);
        AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef() as *const _);
    }
}

/// Watch both permissions for the life of the app: macOS sends no event when
/// either switch changes, in either direction. A change updates telemetry, the
/// settings window and the tray; a new Accessibility grant also starts the
/// record key. One cheap check a second, beside a keyboard poll fifty times as
/// frequent.
pub fn watch_permissions(app: tauri::AppHandle) {
    std::thread::spawn(move || {
        let mut last = super::permissions();
        loop {
            std::thread::sleep(Duration::from_secs(1));
            let now = objc2::rc::autoreleasepool(|_| super::permissions());
            if now == last {
                continue;
            }
            if now.accessibility && !last.accessibility {
                crate::keyboard_service::start_monitoring(app.clone());
            }
            super::log_permissions();
            let _ = app.emit("permission-changed", ());
            crate::tray::rebuild_tray_menu(&app);
            last = now;
        }
    });
}

/// Forget Thoth's recorded decision for one privacy service, so the next
/// request asks afresh. `tccutil reset` on the app's own row needs no
/// administrator rights.
fn forget_decision(service: &str, bundle_id: &str) {
    match Command::new("/usr/bin/tccutil")
        .args(["reset", service, bundle_id])
        .output()
    {
        Ok(out) if out.status.success() => tracing::info!(service, "Cleared Thoth's privacy row"),
        Ok(out) => tracing::warn!(
            service,
            stderr = %String::from_utf8_lossy(&out.stderr).trim(),
            "tccutil reset refused"
        ),
        Err(e) => tracing::warn!(service, error = %e, "tccutil could not run"),
    }
}

/// Open a Privacy & Security pane by its anchor. macOS 27 still resolves the
/// legacy Security pane identifier.
fn open_privacy_pane(anchor: &str) {
    let url = format!("x-apple.systempreferences:com.apple.preference.security?{anchor}");
    if let Err(e) = Command::new("/usr/bin/open").arg(&url).status() {
        tracing::error!(error = %e, "Failed to open System Settings");
    }
}

/// Make sure the shortcut editor can read ordinary keys, not just modifiers.
///
/// From macOS 27 the Accessibility switch ("Device Control and Data Access")
/// carries keyboard listening too, so this is a no-op once Thoth is trusted.
/// Earlier releases keep Input Monitoring separate: ask for it here, at the one
/// moment it matters, rather than on the Overview page. Capture proceeds either
/// way; bare modifiers such as right Shift never need it.
pub fn ensure_listen_access() {
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGPreflightListenEventAccess() -> bool;
        fn CGRequestListenEventAccess() -> bool;
    }
    // SAFETY: both take no arguments and only query or prompt TCC.
    unsafe {
        if !CGPreflightListenEventAccess() {
            CGRequestListenEventAccess();
        }
    }
}

/// Position of the text caret (insertion point) on screen
#[derive(Debug, Clone, Copy)]
pub struct CaretPosition {
    /// X coordinate in screen pixels
    pub x: f64,
    /// Y coordinate in screen pixels
    pub y: f64,
    /// Height of the caret/text line in pixels
    pub height: f64,
}

/// Get the position of the text caret in the currently focused application
///
/// Uses macOS Accessibility APIs to find the focused text element and get
/// the bounds of the text insertion point. This is used to position the
/// recording indicator near where text will be inserted (like macOS dictation).
///
/// Returns None if:
/// - No text field is focused
/// - The focused element doesn't have a text insertion point
/// - Accessibility permission is not granted
pub fn get_caret_position() -> Option<CaretPosition> {
    use core_foundation::base::TCFType;
    use core_foundation::string::CFString;
    use core_graphics::geometry::CGRect;

    unsafe {
        #[link(name = "ApplicationServices", kind = "framework")]
        unsafe extern "C" {
            fn AXUIElementCreateSystemWide() -> *mut std::ffi::c_void;
            fn AXUIElementCopyAttributeValue(
                element: *mut std::ffi::c_void,
                attribute: *const std::ffi::c_void,
                value: *mut *mut std::ffi::c_void,
            ) -> i32;
            fn AXValueGetValue(
                value: *mut std::ffi::c_void,
                value_type: i32,
                value_ptr: *mut std::ffi::c_void,
            ) -> bool;
            fn CFRelease(cf: *const std::ffi::c_void);
        }

        // AXValue types
        const AX_VALUE_TYPE_CGRECT: i32 = 3;

        // Create system-wide accessibility element
        let system_wide = AXUIElementCreateSystemWide();
        if system_wide.is_null() {
            tracing::debug!("Failed to create system-wide AXUIElement");
            return None;
        }

        // Get the focused application
        let focused_app_attr = CFString::new("AXFocusedApplication");
        let mut focused_app: *mut std::ffi::c_void = std::ptr::null_mut();
        let result = AXUIElementCopyAttributeValue(
            system_wide,
            focused_app_attr.as_concrete_TypeRef() as *const _,
            &mut focused_app,
        );

        if result != 0 || focused_app.is_null() {
            CFRelease(system_wide as *const _);
            tracing::debug!("No focused application");
            return None;
        }

        // Get the focused UI element within the application
        let focused_element_attr = CFString::new("AXFocusedUIElement");
        let mut focused_element: *mut std::ffi::c_void = std::ptr::null_mut();
        let result = AXUIElementCopyAttributeValue(
            focused_app,
            focused_element_attr.as_concrete_TypeRef() as *const _,
            &mut focused_element,
        );

        CFRelease(focused_app as *const _);
        CFRelease(system_wide as *const _);

        if result != 0 || focused_element.is_null() {
            tracing::debug!("No focused UI element");
            return None;
        }

        // Try to get the insertion point bounds directly
        // Note: AXBoundsForRange could be used for more precise positioning but
        // requires creating an AXValueRef for the range. AXInsertionPointBounds
        // is simpler and works for our use case.
        // Some apps expose AXSelectedTextMarkerRange or similar
        let insertion_point_attr = CFString::new("AXInsertionPointBounds");
        let mut bounds_value: *mut std::ffi::c_void = std::ptr::null_mut();
        let result = AXUIElementCopyAttributeValue(
            focused_element,
            insertion_point_attr.as_concrete_TypeRef() as *const _,
            &mut bounds_value,
        );

        if result == 0 && !bounds_value.is_null() {
            // Got insertion point bounds directly
            let mut rect = CGRect::default();
            if AXValueGetValue(
                bounds_value,
                AX_VALUE_TYPE_CGRECT,
                &mut rect as *mut CGRect as *mut std::ffi::c_void,
            ) {
                CFRelease(bounds_value as *const _);
                CFRelease(focused_element as *const _);

                tracing::debug!(
                    "Got caret position from AXInsertionPointBounds: ({}, {})",
                    rect.origin.x,
                    rect.origin.y
                );

                return Some(CaretPosition {
                    x: rect.origin.x,
                    y: rect.origin.y,
                    height: rect.size.height.max(20.0), // Minimum height
                });
            }
            CFRelease(bounds_value as *const _);
        }

        // Fallback: try to get the position of the focused element itself
        let position_attr = CFString::new("AXPosition");
        let size_attr = CFString::new("AXSize");

        let mut position_value: *mut std::ffi::c_void = std::ptr::null_mut();
        let mut size_value: *mut std::ffi::c_void = std::ptr::null_mut();

        let pos_result = AXUIElementCopyAttributeValue(
            focused_element,
            position_attr.as_concrete_TypeRef() as *const _,
            &mut position_value,
        );

        let size_result = AXUIElementCopyAttributeValue(
            focused_element,
            size_attr.as_concrete_TypeRef() as *const _,
            &mut size_value,
        );

        CFRelease(focused_element as *const _);

        if pos_result == 0 && !position_value.is_null() {
            use core_graphics::geometry::CGPoint;
            use core_graphics::geometry::CGSize;

            let mut point = CGPoint::default();
            const AX_VALUE_TYPE_CGPOINT: i32 = 1;

            if AXValueGetValue(
                position_value,
                AX_VALUE_TYPE_CGPOINT,
                &mut point as *mut CGPoint as *mut std::ffi::c_void,
            ) {
                let mut height = 20.0; // Default height

                if size_result == 0 && !size_value.is_null() {
                    let mut size = CGSize::default();
                    const AX_VALUE_TYPE_CGSIZE: i32 = 2;
                    if AXValueGetValue(
                        size_value,
                        AX_VALUE_TYPE_CGSIZE,
                        &mut size as *mut CGSize as *mut std::ffi::c_void,
                    ) {
                        height = size.height;
                    }
                    CFRelease(size_value as *const _);
                }

                CFRelease(position_value as *const _);

                tracing::debug!(
                    "Got focused element position: ({}, {}), height={}",
                    point.x,
                    point.y,
                    height
                );

                // Position at the right edge of the element (where text cursor typically is)
                return Some(CaretPosition {
                    x: point.x,
                    y: point.y,
                    height,
                });
            }
            CFRelease(position_value as *const _);
        }

        if !size_value.is_null() {
            CFRelease(size_value as *const _);
        }

        tracing::debug!("Could not determine caret position");
        None
    }
}

/// Check if the screen is locked or the screensaver is active.
///
/// Uses `CGSessionCopyCurrentDictionary()` from ApplicationServices to query
/// the `CGSSessionScreenIsLocked` key. Returns `true` when the lock screen
/// or screensaver is showing, so global shortcuts can be suppressed.
pub fn is_screen_locked() -> bool {
    unsafe {
        #[link(name = "ApplicationServices", kind = "framework")]
        unsafe extern "C" {
            fn CGSessionCopyCurrentDictionary() -> *const std::ffi::c_void;
        }

        let dict = CGSessionCopyCurrentDictionary();
        if dict.is_null() {
            // Can't determine — assume not locked
            return false;
        }

        use core_foundation::base::TCFType;
        use core_foundation::boolean::CFBoolean;
        use core_foundation::dictionary::CFDictionary;
        use core_foundation::string::CFString;

        let cf_dict = CFDictionary::<CFString, CFBoolean>::wrap_under_create_rule(dict as *const _);

        let key = CFString::new("CGSSessionScreenIsLocked");

        let locked = cf_dict
            .find(key)
            .is_some_and(|val| bool::from((*val).clone()));

        if locked {
            tracing::debug!("Screen is locked — suppressing shortcut");
        }

        locked
    }
}

/// Milliseconds since the Unix epoch of the most recent wake event we acted on.
/// Used to debounce the two notifications (DidWake + ScreensDidWake) that both
/// fire on a full system wake. Zero means no wake has fired yet.
static LAST_WAKE_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Run the wake recovery sequence on a background thread.
///
/// Kept separate so the NSWorkspace notification block stays cheap
/// (grab atomic, maybe spawn, return) and this body can be called
/// identically for both DidWake and ScreensDidWake.
fn handle_wake(source: &str) {
    use std::sync::atomic::Ordering;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    // Milliseconds since epoch — fits in u64 for centuries.
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_millis() as u64;

    // Debounce: both DidWake and ScreensDidWake fire on a full system wake.
    // If we handled a wake within the last second, skip this duplicate.
    let prev = LAST_WAKE_MS.load(Ordering::Relaxed);
    if prev > 0 && now_ms.saturating_sub(prev) < 1000 {
        tracing::debug!("Wake notification '{}' suppressed (debounce)", source);
        return;
    }
    LAST_WAKE_MS.store(now_ms, Ordering::Relaxed);

    tracing::info!("System wake detected via '{}'", source);

    std::thread::spawn(|| {
        // Invalidate the mouse tracker's caches FIRST (a cheap atomic)
        // so the indicator can recover immediately, without waiting on
        // the multi-second model re-warm below.
        crate::mouse_tracker::notify_wake();
        // A held cpal stream handle goes stale across sleep; drop it so
        // the next recording opens a fresh device handle.
        crate::audio::cool_down_recording();
        // Then make sure a transcription model is loaded, so the first
        // recording after wake is not penalised by a cold load. This is a
        // no-op when one already is: the observer fires for display wake and
        // monitor hotplug too, and unconditionally rebuilding a 0.5-3.1 GB
        // model on each of those was #171.
        std::thread::sleep(std::time::Duration::from_secs(3));
        crate::transcription::warmup_transcription();
    });
}

/// Register NSWorkspace observers for system wake and display-wake events.
///
/// Uses `NSWorkspaceDidWakeNotification` (full wake from sleep) and
/// `NSWorkspaceScreensDidWakeNotification` (display wake, e.g. lid open or
/// HDMI reconnect). The poll-based heuristic missed display-only wakes and
/// short lid-close events; real OS notifications are exact.
///
/// Observer tokens are leaked for process lifetime — the returned object is
/// not Send, so it cannot live in a static Mutex; leaking is the correct
/// pattern for a once-registered, never-removed observer.
pub fn register_wake_observer() {
    use objc2::runtime::AnyObject;
    use objc2_app_kit::{
        NSWorkspace, NSWorkspaceDidWakeNotification, NSWorkspaceScreensDidWakeNotification,
    };
    use objc2_foundation::NSOperationQueue;
    use std::ptr::NonNull;

    unsafe {
        let workspace = NSWorkspace::sharedWorkspace();
        let center = workspace.notificationCenter();
        let queue = NSOperationQueue::mainQueue();

        // Build one block per notification name. Each block is cheap: debounce
        // check + optional thread spawn. The `move` captures nothing heap-heavy.
        let did_wake_block: block2::RcBlock<dyn Fn(NonNull<objc2_foundation::NSNotification>)> =
            block2::RcBlock::new(move |_notif: NonNull<objc2_foundation::NSNotification>| {
                handle_wake("NSWorkspaceDidWakeNotification");
            });

        let screens_did_wake_block: block2::RcBlock<
            dyn Fn(NonNull<objc2_foundation::NSNotification>),
        > = block2::RcBlock::new(move |_notif: NonNull<objc2_foundation::NSNotification>| {
            handle_wake("NSWorkspaceScreensDidWakeNotification");
        });

        let token_did_wake = center.addObserverForName_object_queue_usingBlock(
            Some(NSWorkspaceDidWakeNotification),
            None::<&AnyObject>,
            Some(&*queue),
            &did_wake_block,
        );

        let token_screens_wake = center.addObserverForName_object_queue_usingBlock(
            Some(NSWorkspaceScreensDidWakeNotification),
            None::<&AnyObject>,
            Some(&*queue),
            &screens_did_wake_block,
        );

        // Leak the tokens for process lifetime — removing these observers is
        // not needed (the app exits, taking the notification centre with it).
        Box::leak(Box::new(token_did_wake));
        Box::leak(Box::new(token_screens_wake));
    }

    tracing::info!("Registered NSWorkspace wake observers (DidWake + ScreensDidWake)");
}

// ---------------------------------------------------------------------------
// Bluetooth transport-type detection
// ---------------------------------------------------------------------------

/// CoreAudio transport-type constants (verified in objc2-core-audio 0.3.2
/// AudioHardware.rs — all values confirmed by grepping the generated source).
mod transport {
    pub const BLUETOOTH: u32 = 0x626c7565; // kAudioDeviceTransportTypeBluetooth
    pub const BLUETOOTH_LE: u32 = 0x626c6561; // kAudioDeviceTransportTypeBluetoothLE
}

/// Read a single `u32` CoreAudio property from a device object.
///
/// Returns `None` on any FFI error so callers degrade gracefully.
///
/// # Safety
/// Only safe to call after verifying `property_size` matches the u32 layout via
/// `AudioObjectGetPropertyDataSize`. This wrapper handles both size-check and read.
fn read_audio_property_u32(device_id: u32, selector: u32, scope: u32) -> Option<u32> {
    use core::ffi::c_void;
    use core::ptr::NonNull;
    use objc2_core_audio::{
        AudioObjectGetPropertyData, AudioObjectGetPropertyDataSize, AudioObjectPropertyAddress,
        kAudioHardwareNoError, kAudioObjectPropertyElementMain,
    };

    let address = AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: scope,
        mElement: kAudioObjectPropertyElementMain,
    };

    let mut address = address;

    // SAFETY: address is a valid stack allocation; qualifier args are null/0 (none required).
    let mut data_size: u32 = 0;
    let status = unsafe {
        AudioObjectGetPropertyDataSize(
            device_id,
            NonNull::new_unchecked(&mut address),
            0,
            std::ptr::null(),
            NonNull::new_unchecked(&mut data_size),
        )
    };
    if status != kAudioHardwareNoError {
        return None;
    }
    if data_size as usize != std::mem::size_of::<u32>() {
        return None;
    }

    let mut value: u32 = 0;
    // SAFETY: value is a valid u32 on the stack; size confirmed above.
    let status = unsafe {
        AudioObjectGetPropertyData(
            device_id,
            NonNull::new_unchecked(&mut address),
            0,
            std::ptr::null(),
            NonNull::new_unchecked(&mut data_size),
            NonNull::new_unchecked(&mut value as *mut u32 as *mut c_void),
        )
    };
    if status != kAudioHardwareNoError {
        None
    } else {
        Some(value)
    }
}

/// Read the transport type of a CoreAudio device object (by numeric ID).
fn device_transport_type(device_id: u32) -> Option<u32> {
    use objc2_core_audio::{kAudioDevicePropertyTransportType, kAudioObjectPropertyScopeGlobal};
    read_audio_property_u32(
        device_id,
        kAudioDevicePropertyTransportType,
        kAudioObjectPropertyScopeGlobal,
    )
}

/// Query the default input device's transport type and return `true` if it is
/// Bluetooth (classic or LE). Returns `false` on any FFI failure so callers
/// degrade gracefully — an FFI error never blocks recording.
pub fn default_input_transport_is_bluetooth() -> bool {
    use core::ffi::c_void;
    use core::ptr::NonNull;
    use objc2_core_audio::{
        AudioObjectGetPropertyData, AudioObjectGetPropertyDataSize, AudioObjectPropertyAddress,
        kAudioHardwareNoError, kAudioHardwarePropertyDefaultInputDevice,
        kAudioObjectPropertyElementMain, kAudioObjectPropertyScopeGlobal, kAudioObjectSystemObject,
    };

    let address = AudioObjectPropertyAddress {
        mSelector: kAudioHardwarePropertyDefaultInputDevice,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain,
    };
    let mut address = address;
    let sys_obj = kAudioObjectSystemObject as u32;

    let mut data_size: u32 = 0;
    // SAFETY: address and data_size are valid stack allocations.
    let status = unsafe {
        AudioObjectGetPropertyDataSize(
            sys_obj,
            NonNull::new_unchecked(&mut address),
            0,
            std::ptr::null(),
            NonNull::new_unchecked(&mut data_size),
        )
    };
    if status != kAudioHardwareNoError {
        tracing::warn!(
            "CoreAudio: AudioObjectGetPropertyDataSize for default input failed ({}); assuming non-Bluetooth",
            status
        );
        return false;
    }

    let mut default_device_id: u32 = 0;
    // SAFETY: default_device_id is a valid u32 stack allocation; size confirmed above.
    let status = unsafe {
        AudioObjectGetPropertyData(
            sys_obj,
            NonNull::new_unchecked(&mut address),
            0,
            std::ptr::null(),
            NonNull::new_unchecked(&mut data_size),
            NonNull::new_unchecked(&mut default_device_id as *mut u32 as *mut c_void),
        )
    };
    if status != kAudioHardwareNoError {
        tracing::warn!(
            "CoreAudio: AudioObjectGetPropertyData for default input failed ({}); assuming non-Bluetooth",
            status
        );
        return false;
    }

    let transport = match device_transport_type(default_device_id) {
        Some(t) => t,
        None => {
            tracing::warn!(
                "CoreAudio: could not read transport type of default input device; assuming non-Bluetooth"
            );
            return false;
        }
    };

    let is_bt = transport == transport::BLUETOOTH || transport == transport::BLUETOOTH_LE;
    if is_bt {
        tracing::debug!(
            "Default input device (id={}) has Bluetooth transport type (0x{:x})",
            default_device_id,
            transport
        );
    }
    is_bt
}

/// Read a CoreAudio device's display name (`kAudioObjectPropertyName`) as a
/// Rust `String`. Returns `None` on any FFI failure.
#[cfg(target_os = "macos")]
fn read_device_name(dev_id: u32) -> Option<String> {
    use core::ffi::c_void;
    use core::ptr::NonNull;
    use objc2_core_audio::{
        AudioObjectGetPropertyData, AudioObjectPropertyAddress, kAudioHardwareNoError,
        kAudioObjectPropertyElementMain, kAudioObjectPropertyName, kAudioObjectPropertyScopeGlobal,
    };
    use objc2_core_foundation::{CFRetained, CFString, CFStringBuiltInEncodings};

    let mut name_addr = AudioObjectPropertyAddress {
        mSelector: kAudioObjectPropertyName,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain,
    };
    let mut name_size: u32 = std::mem::size_of::<*const c_void>() as u32;
    let mut cf_str_ptr: *const CFString = std::ptr::null();

    // SAFETY: cf_str_ptr is a valid pointer-sized out-param; name_addr is a valid stack address.
    let status = unsafe {
        AudioObjectGetPropertyData(
            dev_id,
            NonNull::new_unchecked(&mut name_addr),
            0,
            std::ptr::null(),
            NonNull::new_unchecked(&mut name_size),
            NonNull::new_unchecked(&mut cf_str_ptr as *mut *const CFString as *mut c_void),
        )
    };
    if status != kAudioHardwareNoError || cf_str_ptr.is_null() {
        return None;
    }

    // SAFETY: cf_str_ptr is a non-null, just-retained CFStringRef from CoreAudio.
    let cf_string: CFRetained<CFString> =
        unsafe { CFRetained::from_raw(NonNull::new_unchecked(cf_str_ptr as *mut CFString)) };

    let len = cf_string.length();
    let buf_len = (len as usize) * 4 + 1;
    let mut buf: Vec<u8> = vec![0u8; buf_len];
    // SAFETY: buf is a valid allocation of buf_len bytes; EncodingUTF8 is valid.
    let ok = unsafe {
        cf_string.c_string(
            buf.as_mut_ptr() as *mut i8,
            buf_len as isize,
            CFStringBuiltInEncodings::EncodingUTF8.0,
        )
    };
    if !ok {
        return None;
    }
    let c_str = std::ffi::CStr::from_bytes_until_nul(&buf).ok()?;
    Some(c_str.to_str().ok()?.to_string())
}

/// Return `true` if the CoreAudio input device whose name matches `target_name`
/// has a Bluetooth (classic or LE) transport type.
///
/// Used at stop time to decide whether to release the recording stream
/// immediately (Bluetooth — must not be pinned in HFP call mode) or keep it warm
/// (built-in / USB). Matching by NAME — the device we actually recorded from,
/// stored in `LAST_DEVICE_NAME` — is correct even when the *default* input
/// differs (e.g. the default is AirPods but recording was redirected to the
/// built-in mic). Returns `false` on any FFI failure so the warm path is kept.
pub fn device_name_is_bluetooth(target_name: &str) -> bool {
    use core::ffi::c_void;
    use core::ptr::NonNull;
    use objc2_core_audio::{
        AudioObjectGetPropertyData, AudioObjectGetPropertyDataSize, AudioObjectPropertyAddress,
        kAudioHardwareNoError, kAudioHardwarePropertyDevices, kAudioObjectPropertyElementMain,
        kAudioObjectPropertyScopeGlobal, kAudioObjectSystemObject,
    };

    let sys_obj = kAudioObjectSystemObject as u32;
    let mut devices_address = AudioObjectPropertyAddress {
        mSelector: kAudioHardwarePropertyDevices,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain,
    };

    let mut devices_size: u32 = 0;
    // SAFETY: devices_address and devices_size are valid stack allocations.
    let status = unsafe {
        AudioObjectGetPropertyDataSize(
            sys_obj,
            NonNull::new_unchecked(&mut devices_address),
            0,
            std::ptr::null(),
            NonNull::new_unchecked(&mut devices_size),
        )
    };
    if status != kAudioHardwareNoError {
        return false;
    }
    let device_count = devices_size as usize / std::mem::size_of::<u32>();
    if device_count == 0 {
        return false;
    }
    let mut device_ids: Vec<u32> = vec![0u32; device_count];
    // SAFETY: device_ids matches the confirmed byte size.
    let status = unsafe {
        AudioObjectGetPropertyData(
            sys_obj,
            NonNull::new_unchecked(&mut devices_address),
            0,
            std::ptr::null(),
            NonNull::new_unchecked(&mut devices_size),
            NonNull::new_unchecked(device_ids.as_mut_ptr() as *mut c_void),
        )
    };
    if status != kAudioHardwareNoError {
        return false;
    }

    for &dev_id in &device_ids {
        let Some(transport) = device_transport_type(dev_id) else {
            continue;
        };
        if transport != transport::BLUETOOTH && transport != transport::BLUETOOTH_LE {
            continue;
        }
        if read_device_name(dev_id).as_deref() == Some(target_name) {
            tracing::debug!(
                "CoreAudio: recording device '{}' has Bluetooth transport",
                target_name
            );
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_check_accessibility() {
        // This test just ensures the function doesn't panic
        let _result = check_accessibility_permission();
    }

    #[test]
    fn test_check_microphone() {
        // This test just ensures the function doesn't panic
        let _result = check_microphone_permission();
    }

    #[test]
    fn test_get_caret_position() {
        // This test just ensures the function doesn't panic
        // It will return None if run without a focused text field
        let _result = get_caret_position();
    }

    #[test]
    fn test_microphone_status_from_i64() {
        assert_eq!(MicrophoneStatus::from(0), MicrophoneStatus::NotDetermined);
        assert_eq!(MicrophoneStatus::from(1), MicrophoneStatus::Restricted);
        assert_eq!(MicrophoneStatus::from(2), MicrophoneStatus::Denied);
        assert_eq!(MicrophoneStatus::from(3), MicrophoneStatus::Authorized);
        // Out-of-range values map to Unknown
        assert_eq!(MicrophoneStatus::from(99), MicrophoneStatus::Unknown);
        assert_eq!(MicrophoneStatus::from(-1), MicrophoneStatus::Unknown);
    }

    #[test]
    fn test_default_input_transport_is_bluetooth() {
        // Verifies the function completes without panicking on real hardware.
        // Value depends on the test runner's connected devices — not asserted.
        let _result = default_input_transport_is_bluetooth();
    }
}
