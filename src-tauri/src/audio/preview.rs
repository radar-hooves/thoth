//! Audio preview for device selection
//!
//! Provides audio level metering for previewing input devices in settings.
//! This is separate from recording; it only monitors levels for UI feedback.

use super::device::{get_device_display_name, get_recording_device};
use super::metering::AudioMeter;
use crate::error::Error;
use cpal::traits::{DeviceTrait, StreamTrait};
use parking_lot::Mutex;
use serde::Serialize;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, Emitter, Manager};

/// Audio level event emitted to the frontend
#[derive(Debug, Clone, Serialize)]
pub struct AudioLevelEvent {
    /// RMS level, normalised 0.0-1.0
    pub rms: f32,
    /// Peak level, normalised 0.0-1.0
    pub peak: f32,
}

/// Shared state for an audio metering stream (preview or recording indicator)
struct MeteringState {
    stream: Option<cpal::Stream>,
    stop_flag: Arc<AtomicBool>,
    emit_handle: Option<std::thread::JoinHandle<()>>,
}

impl Default for MeteringState {
    fn default() -> Self {
        Self {
            stream: None,
            stop_flag: Arc::new(AtomicBool::new(false)),
            emit_handle: None,
        }
    }
}

/// Global preview state (only one preview can run at a time)
static PREVIEW_STATE: Mutex<Option<MeteringState>> = Mutex::new(None);

/// Start audio preview for a specific device
///
/// Emits `audio-level` events to the frontend with RMS and peak levels.
#[tauri::command]
pub async fn start_audio_preview(app: AppHandle, device_id: Option<String>) -> Result<(), Error> {
    tauri_plugin_telemetry::traced("start_audio_preview", async move {
        // Stop any existing preview
        stop_audio_preview_inner();

        // Find the device using stable device IDs
        let device = get_recording_device(device_id.as_deref())
            .ok_or_else(|| "No audio input device available".to_string())?;

        let device_name = get_device_display_name(&device);
        tracing::info!("Starting audio preview for device: {}", device_name);

        let config = device.default_input_config().map_err(|e| e.to_string())?;
        let channels = config.channels() as usize;

        // Shared state for metering
        let meter = Arc::new(Mutex::new(AudioMeter::new()));
        let stop_flag = Arc::new(AtomicBool::new(false));

        // Channel for audio data
        let (tx, rx) = crossbeam_channel::bounded::<Vec<f32>>(16);

        // Build the input stream
        let stream = {
            let tx = tx.clone();
            device
                .build_input_stream(
                    config.into(),
                    move |data: &[f32], _: &cpal::InputCallbackInfo| {
                        // Mix to mono and send to emitter thread
                        let mono: Vec<f32> = data
                            .chunks(channels)
                            .map(|frame| frame.iter().sum::<f32>() / channels as f32)
                            .collect();
                        let _ = tx.try_send(mono);
                    },
                    |err| {
                        tracing::error!("Audio preview stream error: {}", err);
                    },
                    None,
                )
                .map_err(|e| e.to_string())?
        };

        stream.play().map_err(|e| e.to_string())?;

        // Spawn emitter thread to send levels to frontend
        let emit_stop_flag = stop_flag.clone();
        let emit_handle = std::thread::spawn(move || {
            while !emit_stop_flag.load(Ordering::Relaxed) {
                while let Ok(samples) = rx.try_recv() {
                    let mut meter = meter.lock();
                    let level = meter.process(&samples);

                    let event = AudioLevelEvent {
                        rms: level.rms,
                        peak: level.peak,
                    };

                    if let Err(e) = app.emit("audio-level", &event) {
                        tracing::warn!("Failed to emit audio level: {}", e);
                    }
                }

                // Rate limit to ~30fps
                std::thread::sleep(std::time::Duration::from_millis(33));
            }
        });

        // Store state
        let mut state_guard = PREVIEW_STATE.lock();
        *state_guard = Some(MeteringState {
            stream: Some(stream),
            stop_flag,
            emit_handle: Some(emit_handle),
        });

        tracing::info!("Audio preview started");
        Ok(())
    })
    .await
}

/// Stop audio preview
#[tauri::command]
pub fn stop_audio_preview() {
    tauri_plugin_telemetry::traced_sync_value("stop_audio_preview", || {
        stop_audio_preview_inner();
    })
}

/// Internal stop function (doesn't require Tauri command context)
fn stop_audio_preview_inner() {
    let mut state_guard = PREVIEW_STATE.lock();

    if let Some(mut state) = state_guard.take() {
        // Signal stop
        state.stop_flag.store(true, Ordering::Relaxed);

        // Drop stream to stop audio callback
        if let Some(stream) = state.stream.take() {
            drop(stream);
        }

        // Wait for emitter thread
        if let Some(handle) = state.emit_handle.take() {
            let _ = handle.join();
        }

        tracing::info!("Audio preview stopped");
    }
}

// =============================================================================
// Recording Metering (for the recording indicator overlay)
// =============================================================================

/// Global recording meter state
static RECORDING_METER_STATE: Mutex<Option<MeteringState>> = Mutex::new(None);

/// Start recording metering - emits `recording-audio-level` events
///
/// Routes metering through the recorder's shared ring buffer so the meter sees
/// the exact samples being recorded, without opening a second device stream.
/// A second stream on USB mics (e.g. DJI MIC MINI) receives silence because
/// the device delivers audio to only one capture client on macOS.
///
/// Must be called AFTER `audio::start_recording()` so the metering buffer exists.
pub fn start_recording_metering(app: AppHandle) -> Result<(), String> {
    tracing::info!("[RECORDING METERING] starting via shared ring buffer");

    // Stop any existing metering
    stop_recording_metering_inner();

    // Obtain the metering ring buffer that the recorder's callback writes into
    let buf = crate::audio::current_metering_buffer().ok_or_else(|| {
        "Recording metering: no metering buffer available (recorder not started?)".to_string()
    })?;

    let stop_flag = Arc::new(AtomicBool::new(false));
    let mut meter = AudioMeter::new();

    // Scratch buffer allocated once outside the read loop — never inside
    let mut scratch = vec![0.0f32; 4096];

    // Spawn emitter thread to drain the ring buffer and emit levels
    let emit_stop_flag = stop_flag.clone();
    let emit_handle = std::thread::spawn(move || {
        while !emit_stop_flag.load(Ordering::Relaxed) {
            let n = buf.read(&mut scratch);
            if n > 0 {
                let level = meter.process(&scratch[..n]);

                let event = AudioLevelEvent {
                    rms: level.rms,
                    peak: level.peak,
                };

                // Try to emit directly to the recording-indicator window, fall back to global
                let emitted =
                    if let Some(indicator_window) = app.get_webview_window("recording-indicator") {
                        indicator_window
                            .emit("recording-audio-level", &event)
                            .is_ok()
                    } else {
                        false
                    };

                if !emitted {
                    let _ = app.emit("recording-audio-level", &event);
                }
            }

            // Rate limit to ~30fps
            std::thread::sleep(std::time::Duration::from_millis(33));
        }
    });

    // Store state — no cpal stream; metering follows the recorder's stream
    let mut state_guard = RECORDING_METER_STATE.lock();
    *state_guard = Some(MeteringState {
        stream: None,
        stop_flag,
        emit_handle: Some(emit_handle),
    });

    tracing::info!("Recording metering started");
    Ok(())
}

/// Stop recording metering
pub fn stop_recording_metering() {
    stop_recording_metering_inner();
}

/// Internal stop function
fn stop_recording_metering_inner() {
    let mut state_guard = RECORDING_METER_STATE.lock();

    if let Some(mut state) = state_guard.take() {
        // Signal stop
        state.stop_flag.store(true, Ordering::Relaxed);

        // Drop stream to stop audio callback
        if let Some(stream) = state.stream.take() {
            drop(stream);
        }

        // Wait for emitter thread
        if let Some(handle) = state.emit_handle.take() {
            let _ = handle.join();
        }

        tracing::debug!("Recording metering stopped");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_recording_device_nonexistent() {
        // Should fall back to default when device ID doesn't exist
        let device = get_recording_device(Some("Nonexistent Device 12345"));
        // May or may not have a default device, but shouldn't panic
        let _ = device;
    }

    #[test]
    fn test_metering_state_default() {
        let state = MeteringState::default();
        assert!(state.stream.is_none());
        assert!(!state.stop_flag.load(Ordering::Relaxed));
    }

    /// Proves the metering emitter's read-and-process path produces a real,
    /// non-zero level when the recorder's callback writes non-silent samples
    /// into the SAME shared buffer the emitter reads from. This mirrors the
    /// production flow without needing a live audio device:
    ///   recorder callback ──write──▶ shared AudioRingBuffer ──read──▶ AudioMeter
    /// If this passes but the on-screen waveform is flat, the break is on the
    /// frontend delivery/render side, not in this Rust level path.
    #[test]
    fn test_metering_emitter_reads_nonzero_from_shared_buffer() {
        use crate::audio::AudioRecorder;
        use crate::audio::AudioRingBuffer;
        use std::sync::Arc;

        // Wire a metering buffer into the recorder exactly as start_recording does.
        let metering_buf = Arc::new(AudioRingBuffer::new());
        let mut recorder = AudioRecorder::new();
        recorder.set_metering_buffer(metering_buf.clone());

        // Simulate the audio callback writing a non-silent block (a half-scale
        // square wave). We write directly to the SAME Arc the recorder holds,
        // which is what the cpal callback closure does in capture.rs.
        let block: Vec<f32> = (0..2048)
            .map(|i| if i % 2 == 0 { 0.5 } else { -0.5 })
            .collect();
        let written = metering_buf.write(&block);
        assert_eq!(written, block.len(), "ring buffer should accept the block");

        // The emitter reads from current_metering_buffer-equivalent (same Arc)
        // into a scratch buffer and runs the meter — identical to the live loop.
        let mut scratch = vec![0.0f32; 4096];
        let n = metering_buf.read(&mut scratch);
        assert!(n > 0, "emitter must read the written samples back");

        let mut meter = AudioMeter::new();
        let level = meter.process(&scratch[..n]);
        assert!(
            level.rms > 0.4,
            "RMS of a half-scale square wave should be ~0.5, got {}",
            level.rms
        );
        assert!(level.peak > 0.4, "peak should be ~0.5, got {}", level.peak);
    }
}
