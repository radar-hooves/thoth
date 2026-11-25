//! Watches the system's input devices so a saved microphone that was unplugged
//! is used again the moment it returns, and the Settings pane stays true.
//!
//! macOS listens to CoreAudio's own change notifications; elsewhere (Linux
//! PipeWire/ALSA has no equivalent cpal exposes) it compares the device-id set
//! every few seconds. Either way a change closes the idle warm stream, so the
//! next recording resolves the saved device afresh, and tells the webview.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

/// Quiet period that folds a burst of notifications (one plug-in fires several) into one.
const SETTLE: Duration = Duration::from_millis(500);

/// Start watching. Safe to call once, from app setup.
pub fn start() {
    let (tx, rx) = channel::<()>();
    std::thread::spawn(move || settle_loop(rx));
    platform_watch(tx);
}

fn settle_loop(rx: Receiver<()>) {
    while rx.recv().is_ok() {
        while rx.recv_timeout(SETTLE).is_ok() {}
        on_change();
    }
}

fn on_change() {
    tracing::info!("Audio: input devices changed");
    // Never touch a stream that is capturing; it reopens on the next press anyway.
    if !super::get_recorder().lock().is_recording() {
        super::cool_down_recording();
    }
    super::device::forget_fallback_notice();
    crate::app_handle::emit("audio-devices-changed", ());
}

#[cfg(target_os = "macos")]
fn platform_watch(tx: Sender<()>) {
    use objc2_core_audio::{
        AudioObjectAddPropertyListener, AudioObjectID, AudioObjectPropertyAddress,
        kAudioHardwareNoError, kAudioHardwarePropertyDefaultInputDevice,
        kAudioHardwarePropertyDevices, kAudioObjectPropertyElementMain,
        kAudioObjectPropertyScopeGlobal, kAudioObjectSystemObject,
    };
    use std::ptr::NonNull;

    unsafe extern "C-unwind" fn listener(
        _object: AudioObjectID,
        _count: u32,
        _addresses: NonNull<AudioObjectPropertyAddress>,
        client: *mut std::ffi::c_void,
    ) -> i32 {
        // SAFETY: `client` is the leaked Sender registered below; it lives for the process.
        let tx = unsafe { &*(client as *const Sender<()>) };
        let _ = tx.send(());
        0
    }

    // CoreAudio holds this pointer for the life of the process.
    let client = Box::into_raw(Box::new(tx)) as *mut std::ffi::c_void;
    for selector in [
        kAudioHardwarePropertyDevices,
        kAudioHardwarePropertyDefaultInputDevice,
    ] {
        let mut address = AudioObjectPropertyAddress {
            mSelector: selector,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMain,
        };
        // SAFETY: the address is valid for the call, the listener matches the
        // required signature and `client` outlives the process.
        let status = unsafe {
            AudioObjectAddPropertyListener(
                kAudioObjectSystemObject as u32,
                NonNull::from(&mut address),
                Some(listener),
                client,
            )
        };
        if status != kAudioHardwareNoError {
            tracing::warn!("CoreAudio device listener failed ({status})");
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn platform_watch(tx: Sender<()>) {
    use cpal::traits::{DeviceTrait, HostTrait};

    fn snapshot() -> Vec<String> {
        let host = cpal::default_host();
        let mut ids: Vec<String> = host
            .input_devices()
            .map(|it| {
                it.filter_map(|d| d.id().ok())
                    .map(|id| id.to_string())
                    .collect()
            })
            .unwrap_or_default();
        ids.sort();
        ids
    }

    std::thread::spawn(move || {
        let mut last = snapshot();
        loop {
            std::thread::sleep(Duration::from_secs(3));
            let now = snapshot();
            if now != last {
                last = now;
                if tx.send(()).is_err() {
                    return;
                }
            }
        }
    });
}
