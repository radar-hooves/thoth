//! Platform-specific functionality

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(target_os = "linux")]
pub mod linux;

// Re-export GPU types for convenience
#[cfg(target_os = "linux")]
pub use linux::{GpuBackend, GpuDetectionResult, GpuInfo};

use crate::TELEMETRY_TARGET;
use crate::error::Error;

/// GPU backend type (re-exported for all platforms)
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GpuBackendType {
    /// NVIDIA CUDA
    Cuda,
    /// AMD HIP/ROCm
    Hipblas,
    /// Vulkan (cross-platform)
    Vulkan,
    /// Apple Metal
    Metal,
    /// CPU only
    Cpu,
}

impl std::fmt::Display for GpuBackendType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GpuBackendType::Cuda => write!(f, "CUDA"),
            GpuBackendType::Hipblas => write!(f, "HIP/ROCm"),
            GpuBackendType::Vulkan => write!(f, "Vulkan"),
            GpuBackendType::Metal => write!(f, "Metal"),
            GpuBackendType::Cpu => write!(f, "CPU"),
        }
    }
}

impl GpuBackendType {
    /// The GPU backend compiled into this build.
    ///
    /// macOS always builds with Metal; other platforms depend on which
    /// (mutually exclusive) GPU feature was enabled at compile time, defaulting
    /// to CPU. Single source of truth so the compile-time cfg ladder lives in
    /// one place instead of being duplicated across every logging site.
    pub fn compiled() -> Self {
        #[cfg(target_os = "macos")]
        {
            Self::Metal
        }
        #[cfg(all(not(target_os = "macos"), feature = "cuda"))]
        {
            Self::Cuda
        }
        #[cfg(all(not(target_os = "macos"), not(feature = "cuda"), feature = "hipblas"))]
        {
            Self::Hipblas
        }
        #[cfg(all(
            not(target_os = "macos"),
            not(feature = "cuda"),
            not(feature = "hipblas"),
            feature = "vulkan"
        ))]
        {
            Self::Vulkan
        }
        #[cfg(all(
            not(target_os = "macos"),
            not(feature = "cuda"),
            not(feature = "hipblas"),
            not(feature = "vulkan")
        ))]
        {
            Self::Cpu
        }
    }
}

/// GPU information for the current system
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SystemGpuInfo {
    /// The GPU backend compiled into this build
    pub compiled_backend: String,
    /// Whether GPU acceleration is available
    pub gpu_available: bool,
    /// Detected GPU name (if any)
    pub gpu_name: Option<String>,
    /// GPU VRAM in MB (if available)
    pub vram_mb: Option<u64>,
    /// List of all detected GPUs
    pub detected_gpus: Vec<DetectedGpu>,
}

/// Information about a detected GPU
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DetectedGpu {
    /// GPU backend type
    pub backend: String,
    /// GPU name/model
    pub name: String,
    /// VRAM in MB (if available)
    pub vram_mb: Option<u64>,
}

/// Get GPU information for the current system
#[tauri::command]
#[tracing::instrument(target = TELEMETRY_TARGET, skip_all, err)]
pub fn get_gpu_info() -> Result<SystemGpuInfo, Error> {
    #[cfg(target_os = "linux")]
    {
        let detection = linux::detect_gpus();
        Ok(SystemGpuInfo {
            compiled_backend: detection.compiled_backend.to_string(),
            gpu_available: detection.recommended_backend != linux::GpuBackend::Cpu,
            gpu_name: detection.gpus.first().map(|g| g.name.clone()),
            vram_mb: detection.gpus.first().and_then(|g| g.vram_mb),
            detected_gpus: detection
                .gpus
                .iter()
                .map(|g| DetectedGpu {
                    backend: g.backend.to_string(),
                    name: g.name.clone(),
                    vram_mb: g.vram_mb,
                })
                .collect(),
        })
    }

    #[cfg(target_os = "macos")]
    {
        Ok(SystemGpuInfo {
            compiled_backend: "Metal".to_string(),
            gpu_available: true,
            gpu_name: get_macos_gpu_name(),
            vram_mb: None,
            detected_gpus: vec![DetectedGpu {
                backend: "Metal".to_string(),
                name: get_macos_gpu_name().unwrap_or_else(|| "Apple GPU".to_string()),
                vram_mb: None,
            }],
        })
    }

    #[cfg(target_os = "windows")]
    {
        Ok(SystemGpuInfo {
            compiled_backend: if cfg!(feature = "cuda") {
                "CUDA".to_string()
            } else {
                "CPU".to_string()
            },
            gpu_available: cfg!(feature = "cuda"),
            gpu_name: None,
            vram_mb: None,
            detected_gpus: vec![],
        })
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        Ok(SystemGpuInfo {
            compiled_backend: "CPU".to_string(),
            gpu_available: false,
            gpu_name: None,
            vram_mb: None,
            detected_gpus: vec![],
        })
    }
}

/// Get macOS GPU name via system_profiler
#[cfg(target_os = "macos")]
fn get_macos_gpu_name() -> Option<String> {
    use std::process::Command;

    let output = Command::new("system_profiler")
        .args(["SPDisplaysDataType"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Parse GPU name from output
    for line in stdout.lines() {
        if line.contains("Chipset Model:") {
            return line.split(':').nth(1).map(|s| s.trim().to_string());
        }
    }

    None
}

/// Return `true` if the default macOS input device is Bluetooth (classic or LE).
///
/// Used to decide whether to redirect recording to the built-in microphone to
/// avoid forcing AirPods (or other Bluetooth headsets) out of A2DP into HFP
/// "call" mode, which degrades audio quality for the user.
///
/// Always returns `false` on non-macOS platforms.
pub fn default_input_transport_is_bluetooth() -> bool {
    #[cfg(target_os = "macos")]
    {
        macos::default_input_transport_is_bluetooth()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// Return `true` if the audio input device named `name` has a Bluetooth
/// transport type on macOS. Used to decide whether the device we actually
/// recorded from must be released immediately (Bluetooth) rather than held warm.
///
/// Always returns `false` on non-macOS platforms.
pub fn device_name_is_bluetooth(name: &str) -> bool {
    #[cfg(target_os = "macos")]
    {
        macos::device_name_is_bluetooth(name)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = name;
        false
    }
}

/// Check if the screen is locked or the screensaver is active.
///
/// Used to suppress global shortcuts when the user is on the lock screen,
/// preventing accidental recording triggers when dismissing the screensaver.
pub fn is_screen_locked() -> bool {
    #[cfg(target_os = "macos")]
    {
        macos::is_screen_locked()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false // Not implemented on other platforms
    }
}

/// Check if accessibility permissions are available
#[tauri::command]
#[tracing::instrument(target = TELEMETRY_TARGET, skip_all)]
pub fn check_accessibility() -> bool {
    #[cfg(target_os = "macos")]
    {
        macos::check_accessibility_permission()
    }
    #[cfg(target_os = "linux")]
    {
        linux::check_accessibility_permission()
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        true // Not needed on other platforms
    }
}

/// Request accessibility permission (opens settings if needed)
#[tauri::command]
#[tracing::instrument(target = TELEMETRY_TARGET, skip_all)]
pub fn request_accessibility() -> bool {
    #[cfg(target_os = "macos")]
    {
        if !macos::check_accessibility_permission() {
            macos::open_accessibility_settings();
            false
        } else {
            true
        }
    }
    #[cfg(target_os = "linux")]
    {
        // X11 allows key grabbing and Wayland uses the XDG portal, so there is
        // no per-app accessibility grant to request; it is always available.
        true
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        true
    }
}

/// Check if Input Monitoring permission is granted
///
/// This is required for capturing keyboard input at the system level.
pub fn check_input_monitoring_permission() -> bool {
    #[cfg(target_os = "macos")]
    {
        macos::check_input_monitoring_permission()
    }
    #[cfg(target_os = "linux")]
    {
        linux::check_input_monitoring_permission()
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        true // Not needed on other platforms
    }
}

/// Open Input Monitoring settings
pub fn open_input_monitoring_settings() {
    #[cfg(target_os = "macos")]
    {
        macos::open_input_monitoring_settings();
    }
    #[cfg(target_os = "linux")]
    {
        linux::open_input_monitoring_settings();
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        // No input-monitoring concept on other platforms.
    }
}

/// Verify that accessibility permission is functionally working, not just granted.
///
/// `AXIsProcessTrusted()` can return `true` for stale TCC entries (e.g., after
/// reinstall with a different code signature). This performs an actual AX API
/// call to confirm the permission is live.
#[tauri::command]
#[tracing::instrument(target = TELEMETRY_TARGET, skip_all)]
pub fn verify_accessibility_functional() -> bool {
    #[cfg(target_os = "macos")]
    {
        macos::verify_accessibility_functional()
    }
    #[cfg(not(target_os = "macos"))]
    {
        true // Not needed on other platforms
    }
}

/// Reset the permissions an app update is likely to have invalidated.
///
/// macOS keys TCC grants to the code-signing identity, which changes on each
/// build, so after an update the old grants silently go stale. Called once when
/// a version change is detected at startup. No-op on non-macOS platforms.
pub fn reset_permissions_after_update() -> Result<String, String> {
    #[cfg(target_os = "macos")]
    {
        macos::reset_permissions_after_update()
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok("No permission reset needed on this platform.".to_string())
    }
}

/// Reset TCC permission entries for Thoth.
///
/// Prompts for administrator privileges via macOS dialog, then runs
/// `tccutil reset` for each specified service.
///
/// Valid services: "Accessibility", "ListenEvent", "Microphone", "All"
#[tauri::command]
#[tracing::instrument(target = TELEMETRY_TARGET, skip_all, err)]
pub async fn reset_tcc_permissions(services: Vec<String>) -> Result<String, Error> {
    #[cfg(target_os = "macos")]
    {
        macos::reset_tcc_permissions(&services).map_err(Into::into)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = services;
        Ok("Permission reset is only supported on macOS.".to_string())
    }
}

/// Check microphone permission status
///
/// Returns the permission status as a string:
/// - "granted" - Permission has been granted
/// - "denied" - Permission was explicitly denied
/// - "not_determined" - User hasn't been asked yet
/// - "restricted" - Access is restricted (e.g., parental controls)
/// - "unknown" - Unable to determine status
#[tauri::command]
#[tracing::instrument(target = TELEMETRY_TARGET, skip_all)]
pub fn check_microphone_permission() -> String {
    #[cfg(target_os = "macos")]
    {
        macos::check_microphone_permission().to_string()
    }
    #[cfg(target_os = "linux")]
    {
        // Probes PulseAudio/PipeWire for an available capture source.
        linux::check_microphone_permission().to_string()
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        "granted".to_string() // Not needed on other platforms
    }
}

/// Caret (text cursor) position on screen
#[derive(Debug, Clone, Copy, serde::Serialize)]
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
/// Uses platform accessibility APIs to find the focused text element and get
/// the position of the text insertion point. This is used to position the
/// recording indicator near where text will be inserted (like macOS dictation).
///
/// Returns None if no text field is focused or caret position cannot be determined.
pub fn get_caret_position() -> Option<CaretPosition> {
    #[cfg(target_os = "macos")]
    {
        macos::get_caret_position().map(|p| CaretPosition {
            x: p.x,
            y: p.y,
            height: p.height,
        })
    }
    #[cfg(target_os = "linux")]
    {
        linux::get_caret_position().map(|p| CaretPosition {
            x: p.x,
            y: p.y,
            height: p.height,
        })
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        None // Not implemented on other platforms yet
    }
}

/// Request microphone permission
///
/// Triggers the system permission dialog. If permission was already denied,
/// this will open System Preferences instead.
#[tauri::command]
#[tracing::instrument(target = TELEMETRY_TARGET, skip_all)]
pub fn request_microphone_permission(app: tauri::AppHandle) {
    #[cfg(target_os = "macos")]
    {
        let status = macos::check_microphone_permission();
        match status {
            macos::MicrophoneStatus::NotDetermined => {
                // First time — trigger the system dialog. The completion
                // handler emits permission-changed when the user responds.
                macos::request_microphone_permission(app);
            }
            macos::MicrophoneStatus::Denied | macos::MicrophoneStatus::Restricted => {
                // Already denied - open System Preferences
                macos::open_microphone_settings();
            }
            macos::MicrophoneStatus::Authorized => {
                // Already granted, nothing to do
                tracing::info!("Microphone permission already granted");
            }
            macos::MicrophoneStatus::Unknown => {
                // Try requesting anyway
                macos::request_microphone_permission(app);
            }
        }
    }
    #[cfg(target_os = "linux")]
    {
        let _ = app;
        // PulseAudio/PipeWire grant capture access without a system dialogue.
        linux::request_microphone_permission();
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = app;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gpu_backend_type_compiled_matches_features() {
        // GpuBackendType::compiled() is the single resolver used for backend
        // logging; assert it matches the compiled feature set on every target.
        let backend = GpuBackendType::compiled();
        #[cfg(target_os = "macos")]
        assert_eq!(backend, GpuBackendType::Metal);
        #[cfg(all(not(target_os = "macos"), feature = "cuda"))]
        assert_eq!(backend, GpuBackendType::Cuda);
        #[cfg(all(not(target_os = "macos"), not(feature = "cuda"), feature = "hipblas"))]
        assert_eq!(backend, GpuBackendType::Hipblas);
        #[cfg(all(
            not(target_os = "macos"),
            not(feature = "cuda"),
            not(feature = "hipblas"),
            feature = "vulkan"
        ))]
        assert_eq!(backend, GpuBackendType::Vulkan);
        #[cfg(all(
            not(target_os = "macos"),
            not(feature = "cuda"),
            not(feature = "hipblas"),
            not(feature = "vulkan")
        ))]
        assert_eq!(backend, GpuBackendType::Cpu);
    }
}
