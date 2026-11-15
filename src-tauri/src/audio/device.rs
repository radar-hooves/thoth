//! Audio device enumeration using cpal

use cpal::DeviceId;
use cpal::traits::{DeviceTrait, HostTrait};
use serde::Serialize;
use std::str::FromStr;

/// Represents an audio input device
#[derive(Debug, Clone, Serialize)]
pub struct AudioDevice {
    /// Unique identifier for the device (stable across restarts)
    pub id: String,
    /// Human-readable device name
    pub name: String,
    /// Whether this is the system default input device
    pub is_default: bool,
}

/// Get the display name for a device.
///
/// Uses `description()` (cpal 0.18 removed the old `name()` accessor), falling
/// back to "Unknown" when the description is unavailable.
pub fn get_device_display_name(device: &cpal::Device) -> String {
    device
        .description()
        .map(|desc| desc.name().to_string())
        .unwrap_or_else(|_| "Unknown".to_string())
}

/// List all available audio input devices
///
/// Returns a vector of available input devices with their names and
/// whether they are the default device.
///
/// Uses cpal's DeviceId for stable identification across restarts.
pub fn list_input_devices() -> Vec<AudioDevice> {
    let host = cpal::default_host();

    // Log host information for debugging Linux audio issues
    tracing::info!("CPAL host: {}", host.id().name());

    // Get default device ID for comparison
    let default_device = host.default_input_device();
    let default_device_id = default_device
        .as_ref()
        .and_then(|d| d.id().ok())
        .map(|id| id.to_string());

    if let Some(ref device) = default_device {
        let name = get_device_display_name(device);
        if let Ok(config) = device.default_input_config() {
            tracing::info!(
                "Default input device: '{}', {}Hz, {}ch, format={:?}",
                name,
                config.sample_rate(),
                config.channels(),
                config.sample_format()
            );
        }
    } else {
        tracing::warn!("No default input device found!");
    }

    // Enumerate all input devices
    let devices: Vec<AudioDevice> = host
        .input_devices()
        .map(|device_iter| {
            device_iter
                .filter_map(|device| {
                    // Use id() for stable identification (persists across restarts)
                    let device_id = device.id().ok()?.to_string();
                    let device_name = get_device_display_name(&device);

                    // Log each device for debugging
                    if let Ok(config) = device.default_input_config() {
                        tracing::debug!(
                            "Found input device: '{}' (id: {}), {}Hz, {}ch",
                            device_name,
                            device_id,
                            config.sample_rate(),
                            config.channels()
                        );
                    }

                    Some(AudioDevice {
                        id: device_id.clone(),
                        name: device_name,
                        is_default: Some(&device_id) == default_device_id.as_ref(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    tracing::info!("Found {} input devices", devices.len());
    devices
}

/// Get the default input device
pub fn get_default_input_device() -> Option<cpal::Device> {
    let host = cpal::default_host();
    host.default_input_device()
}

/// Find an input device by its stable ID
///
/// Uses cpal's DeviceId for reliable device lookup across restarts.
pub fn find_input_device_by_id(id_str: &str) -> Option<cpal::Device> {
    let host = cpal::default_host();

    // Parse the stored ID string back to a DeviceId
    let device_id = DeviceId::from_str(id_str).ok()?;

    // Use cpal's device_by_id for efficient lookup
    host.device_by_id(&device_id)
}

/// The configured-device id we last warned the user about, so we emit the
/// fallback toast only when a *different* device goes missing rather than on
/// every recording with the same missing device.
static LAST_FALLBACK_ID: std::sync::OnceLock<parking_lot::Mutex<Option<String>>> =
    std::sync::OnceLock::new();

/// Emit a user-facing notice that the configured input device was not found and
/// the default is being used, deduped per distinct missing id.
fn notify_device_fallback_once(missing_id: &str) {
    let cell = LAST_FALLBACK_ID.get_or_init(|| parking_lot::Mutex::new(None));
    let mut last = cell.lock();
    if last.as_deref() == Some(missing_id) {
        return; // Already notified for this device.
    }
    *last = Some(missing_id.to_string());
    drop(last);

    crate::app_handle::emit(
        "audio-device-fallback",
        "Your selected microphone is unavailable; recording from the system default device \
         instead. Re-select your microphone in Settings if you want to keep using it.",
    );
}

/// Get the input device to use for recording, based on config
///
/// If a device ID is configured and found, uses that device.
/// Otherwise falls back to the system default.
pub fn get_recording_device(device_id: Option<&str>) -> Option<cpal::Device> {
    tracing::info!(
        "get_recording_device called with device_id: {:?}",
        device_id
    );

    if let Some(id) = device_id {
        if let Some(device) = find_input_device_by_id(id) {
            let name = get_device_display_name(&device);
            tracing::info!("Using configured audio device: {}", name);
            return Some(device);
        }

        // Log available devices so it's easy to diagnose why the configured one wasn't found
        let available = list_input_devices();
        let device_list: Vec<String> = available
            .iter()
            .map(|d| format!("{} (id: {})", d.name, d.id))
            .collect();
        tracing::warn!(
            "Configured audio device '{}' not found. Available devices: [{}]. Falling back to default.",
            id,
            device_list.join(", ")
        );

        // Surface the fallback to the user once per distinct missing device, so a
        // mic that was unplugged or whose opaque cpal id changed (e.g. after an
        // ALSA/PulseAudio/PipeWire switch) does not silently record from the
        // default device. Deduped so it does not toast on every recording.
        notify_device_fallback_once(id);
    }

    // When falling back to the default, check whether it is a Bluetooth device.
    // If so, prefer the first non-Bluetooth input to avoid forcing the headset
    // from A2DP (high-quality stereo) into HFP "call" mode, which degrades the
    // user's music for the duration of the recording. This covers built-in mics,
    // USB mics, and any wired device — anything that won't trigger HFP.
    if crate::platform::default_input_transport_is_bluetooth() {
        let non_bt = cpal::default_host()
            .input_devices()
            .ok()
            .and_then(|mut iter| {
                iter.find(|d| {
                    let name = get_device_display_name(d);
                    !crate::platform::device_name_is_bluetooth(&name)
                })
            });

        if let Some(device) = non_bt {
            let name = get_device_display_name(&device);
            tracing::info!(
                "Default input is Bluetooth; recording from non-Bluetooth input '{}' \
                 to keep headphone audio in high-quality (A2DP) mode",
                name
            );
            return Some(device);
        } else {
            let msg = "Your Bluetooth headset mic will be used for recording, which reduces \
                       headphone audio to call quality during the recording. Connect another \
                       microphone if you want to avoid this.";
            tracing::warn!(
                "Default input is Bluetooth and no non-Bluetooth input is available; \
                 recording from Bluetooth mic — headphone audio will drop to call quality \
                 during recording"
            );
            crate::app_handle::emit("audio-device-fallback", msg);
        }
    }

    let device = get_default_input_device();
    if let Some(ref d) = device {
        let name = get_device_display_name(d);
        if let Ok(config) = d.default_input_config() {
            tracing::info!(
                "Using default audio device: '{}', {}Hz, {}ch, format={:?}",
                name,
                config.sample_rate(),
                config.channels(),
                config.sample_format()
            );
        } else {
            tracing::info!(
                "Using default audio device: '{}' (could not get config)",
                name
            );
        }
    } else {
        tracing::error!("No default input device available!");
    }
    device
}

/// Tauri command to list audio devices
#[tauri::command]
pub fn list_audio_devices() -> Vec<AudioDevice> {
    let devices = list_input_devices();
    tracing::debug!("Found {} audio input devices", devices.len());
    devices
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_list_input_devices() {
        let devices = list_input_devices();
        // Should return at least an empty list without panicking
        println!("Found {} devices", devices.len());
        for device in &devices {
            println!(
                "  - {} (id: {}, default: {})",
                device.name, device.id, device.is_default
            );
        }
    }

    /// Measure what a cold open actually spends on the device query (#104).
    ///
    /// Ignored: it is a measurement, not an assertion, and its numbers are
    /// this machine's. Run it deliberately:
    ///
    /// ```text
    /// cargo test --release --lib device_query_cost -- --ignored --nocapture
    /// ```
    ///
    /// The first call is reported separately from the rest: caching would only
    /// ever save the repeats, so an expensive first call and cheap repeats
    /// means a cache buys nothing after warm-up.
    #[test]
    #[ignore]
    fn device_query_cost() {
        use std::time::Instant;

        let Some(device) = get_default_input_device() else {
            println!("no input device; nothing to measure");
            return;
        };

        let start = Instant::now();
        let _ = device.default_input_config();
        let first = start.elapsed();

        const REPEATS: u32 = 50;
        let start = Instant::now();
        for _ in 0..REPEATS {
            let _ = device.default_input_config();
        }
        let repeats = start.elapsed() / REPEATS;

        // A genuine cold open resolves the device too, so measure that as well
        // rather than only the part a config cache would skip.
        let start = Instant::now();
        let _ = get_recording_device(None);
        let resolve = start.elapsed();

        println!(
            "default_input_config, first call : {:>8.3} ms",
            first.as_secs_f64() * 1000.0
        );
        println!(
            "default_input_config, subsequent : {:>8.3} ms (mean of {REPEATS})",
            repeats.as_secs_f64() * 1000.0
        );
        println!(
            "get_recording_device(None)       : {:>8.3} ms",
            resolve.as_secs_f64() * 1000.0
        );
    }

    #[test]
    fn test_get_default_device() {
        // Should not panic even if no device available
        let _device = get_default_input_device();
    }

    #[test]
    fn test_device_id_stable_format() {
        // Verify the ID format is parseable back to DeviceId
        let devices = list_input_devices();
        for device in &devices {
            let parsed = DeviceId::from_str(&device.id);
            assert!(
                parsed.is_ok(),
                "Device ID '{}' should be parseable as DeviceId",
                device.id
            );
        }
    }
}
