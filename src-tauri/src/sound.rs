//! Sound feedback module for Thoth
//!
//! Provides audio feedback sounds for recording events.
//! Uses macOS dictation-style tones (dt-begin, dt-confirm) for recording
//! start/stop, and standard system sounds for other events. On Linux the
//! equivalent cues come from the freedesktop XDG sound theme (see
//! [`SoundEvent::theme_sound_names`]).
//!
//! Playback uses `AVAudioPlayer` on macOS. This is the path that satisfies both
//! constraints a recording cue has:
//!   - It plays as an ordinary mixable CoreAudio client, so it does NOT duck or
//!     pause the user's music (the System Sound server does — it routes through
//!     the single-slot system-alert path with no mix control on macOS, and
//!     NSSound on the shared output got clipped when the mic opened).
//!   - Each cue is an independent output stream, so opening the microphone to
//!     record does not clip or swallow it (the failure NSSound had: the start
//!     tone went silent or "cut in half" on the first record after the warm
//!     audio stream had torn down).
//!
//! A fresh player is created per cue on its own short-lived thread, which holds
//! it alive for the cue's duration and then drops it. Dropping is the only thing
//! that releases an `AVAudioPlayer`: nothing in AVFoundation releases it when
//! playback ends, so a player that is not dropped is retained for the life of the
//! process (see #170 — this leaked a decoded PCM buffer twice per dictation).

use crate::config;
use crate::error::Error;

/// Sound event types for different application states
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoundEvent {
    /// Recording has started
    RecordingStart,
    /// Recording has stopped
    RecordingStop,
    /// Transcription completed successfully
    TranscriptionComplete,
    /// An error occurred
    Error,
}

impl SoundEvent {
    /// Get the macOS sound file path for this event
    ///
    /// Recording start/stop use the dictation tones from AssistantServices
    /// (the same sounds macOS plays for dictation on/off).
    /// Other events use standard system sounds from /System/Library/Sounds/.
    #[cfg(target_os = "macos")]
    fn sound_path(&self) -> &'static str {
        match self {
            SoundEvent::RecordingStart => {
                "/System/Library/PrivateFrameworks/AssistantServices.framework/Versions/A/Resources/dt-begin.caf"
            }
            SoundEvent::RecordingStop => {
                "/System/Library/PrivateFrameworks/AssistantServices.framework/Versions/A/Resources/dt-confirm.caf"
            }
            SoundEvent::TranscriptionComplete => "/System/Library/Sounds/Glass.aiff",
            SoundEvent::Error => "/System/Library/Sounds/Basso.aiff",
        }
    }

    /// The tone sequence for this event on Linux, as `(frequency Hz, duration ms)`
    /// segments.
    ///
    /// Linux has no system cue equivalent to the macOS dictation tones, and no
    /// asset or sound-theme package is guaranteed to be installed on an arbitrary
    /// distribution, so the cues are synthesised (see [`render_cue`]). Start and
    /// stop are the same interval inverted — rising for "on", falling for "off" —
    /// which is the semantic the macOS `dt-begin` / `dt-confirm` pair carries.
    #[cfg(target_os = "linux")]
    fn cue(&self) -> &'static [(f32, u32)] {
        match self {
            SoundEvent::RecordingStart => &[(660.0, 80), (990.0, 90)],
            SoundEvent::RecordingStop => &[(990.0, 80), (660.0, 90)],
            SoundEvent::TranscriptionComplete => &[(880.0, 70), (1174.0, 70), (1318.0, 110)],
            SoundEvent::Error => &[(320.0, 110), (240.0, 160)],
        }
    }
}

/// Clamp a configured volume into the usable 0.0..=1.0 range.
///
/// NaN maps to silence rather than propagating into the waveform, where it would
/// produce a stream of NaN samples instead of a tone.
fn clamp_volume(volume: f32) -> f32 {
    if volume.is_nan() {
        0.0
    } else {
        volume.clamp(0.0, 1.0)
    }
}

/// Play a sound for the given event if sounds are enabled in config
pub fn play_sound(event: SoundEvent) {
    // Check if sounds are enabled in config
    let (sounds_enabled, volume) = match config::get_config() {
        Ok(cfg) => (cfg.audio.play_sounds, cfg.audio.sound_volume),
        Err(e) => {
            tracing::warn!("Failed to get config for sound check: {}", e);
            (true, 1.0) // Default to playing sounds at full volume if config fails
        }
    };

    if !sounds_enabled {
        tracing::debug!("Sound disabled, skipping {:?}", event);
        return;
    }

    // Clamped rather than trusted: the value round-trips through config.json and
    // the IPC boundary, and a negative volume would invert the waveform while a
    // value above 1.0 would clip.
    let volume = clamp_volume(volume);
    if volume <= f32::EPSILON {
        tracing::debug!("Cue volume is zero, skipping {:?}", event);
        return;
    }

    #[cfg(target_os = "macos")]
    {
        play_macos_sound(event.sound_path(), volume);
    }

    #[cfg(target_os = "linux")]
    {
        play_linux_sound(event, volume);
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        tracing::debug!(
            "System sounds not implemented for this platform, skipping {:?}",
            event
        );
    }
}

/// Peak amplitude of a rendered cue. Quiet enough not to startle over headphones,
/// loud enough to hear over typing.
#[cfg(target_os = "linux")]
const CUE_AMPLITUDE: f32 = 0.22;

/// Attack/release ramp applied to each cue segment, in seconds. Without it the
/// waveform starts and stops at a non-zero value and the speaker clicks.
#[cfg(target_os = "linux")]
const CUE_RAMP_SECS: f32 = 0.006;

/// Render a `(frequency Hz, duration ms)` cue to mono `f32` samples at
/// `sample_rate`.
///
/// Each segment is an enveloped sine: a linear fade in and out so segment edges
/// (and the cue's start and end) are click-free.
#[cfg(target_os = "linux")]
fn render_cue(segments: &[(f32, u32)], sample_rate: f32, volume: f32) -> Vec<f32> {
    let mut samples = Vec::new();

    for &(frequency, duration_ms) in segments {
        let frames = ((sample_rate * duration_ms as f32) / 1000.0).round() as usize;
        if frames == 0 {
            continue;
        }
        // Never let attack and release overlap on a very short segment.
        let ramp = ((sample_rate * CUE_RAMP_SECS) as usize).clamp(1, (frames / 2).max(1));

        for i in 0..frames {
            let envelope = if i < ramp {
                i as f32 / ramp as f32
            } else if i + ramp >= frames {
                (frames - i) as f32 / ramp as f32
            } else {
                1.0
            };
            let phase = std::f32::consts::TAU * frequency * (i as f32 / sample_rate);
            samples.push(phase.sin() * envelope * CUE_AMPLITUDE * volume);
        }
    }

    samples
}

/// Build an output stream that plays `samples` (mono) across every channel,
/// then silence.
#[cfg(target_os = "linux")]
fn build_cue_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    samples: Vec<f32>,
) -> Result<cpal::Stream, String>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    use cpal::traits::DeviceTrait;

    let channels = config.channels as usize;
    let mut cursor = 0usize;

    device
        .build_output_stream::<T, _, _>(
            *config,
            move |out: &mut [T], _: &cpal::OutputCallbackInfo| {
                for frame in out.chunks_mut(channels) {
                    let value = samples.get(cursor).copied().unwrap_or(0.0);
                    cursor = cursor.saturating_add(1);
                    // Mono cue duplicated to every channel.
                    for slot in frame.iter_mut() {
                        *slot = T::from_sample(value);
                    }
                }
            },
            |e| tracing::warn!("Audio cue output stream error: {}", e),
            None,
        )
        .map_err(|e| format!("failed to build output stream: {e}"))
}

/// Play a UI cue on Linux by synthesising it and writing it to the default
/// output device via cpal.
///
/// Deliberately not the freedesktop sound theme or a `paplay`/`canberra` style
/// helper: both require a package that plenty of distributions and minimal
/// window-manager setups do not ship, which would make the cue silent on exactly
/// the systems least likely to have it. cpal is already a dependency (it drives
/// capture) and speaks ALSA/PulseAudio/PipeWire/JACK, so synthesising in-process
/// is the one path that behaves identically on every Linux install and needs no
/// bundled asset.
#[cfg(target_os = "linux")]
fn play_cue(segments: &'static [(f32, u32)], volume: f32) -> Result<(), String> {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use std::time::Duration;

    let device = cpal::default_host()
        .default_output_device()
        .ok_or_else(|| "no default audio output device".to_string())?;
    let supported = device
        .default_output_config()
        .map_err(|e| format!("no default output config: {e}"))?;

    let sample_format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();
    let sample_rate = config.sample_rate as f32;

    let samples = render_cue(segments, sample_rate, volume);
    if samples.is_empty() {
        return Ok(());
    }
    let duration = Duration::from_secs_f32(samples.len() as f32 / sample_rate);

    let stream = match sample_format {
        cpal::SampleFormat::F32 => build_cue_stream::<f32>(&device, &config, samples),
        cpal::SampleFormat::I16 => build_cue_stream::<i16>(&device, &config, samples),
        cpal::SampleFormat::U16 => build_cue_stream::<u16>(&device, &config, samples),
        other => Err(format!("unsupported output sample format {other:?}")),
    }?;

    stream
        .play()
        .map_err(|e| format!("failed to start cue playback: {e}"))?;

    // Dropping the stream stops playback, so hold it open for the cue's length.
    // The tail lets the device drain its last buffer before the drop.
    std::thread::sleep(duration + Duration::from_millis(60));
    Ok(())
}

/// Play a UI cue on Linux.
///
/// Runs on a detached thread: building the stream and holding it open for the
/// cue's duration both block, and the start cue fires on the hotkey path where
/// any delay is audible as lag.
#[cfg(target_os = "linux")]
fn play_linux_sound(event: SoundEvent, volume: f32) {
    let segments = event.cue();
    std::thread::spawn(move || {
        if let Err(e) = play_cue(segments, volume) {
            tracing::warn!("Failed to play {:?} cue: {}", event, e);
        }
    });
}

/// Longest a cue thread will hold a player alive, in seconds.
///
/// The cues are sub-second system assets, so a longer reported duration is a
/// fault rather than a long cue, and honouring it would pin a thread and a
/// decoded audio buffer for that long.
#[cfg(target_os = "macos")]
const CUE_MAX_HOLD_SECS: f64 = 5.0;

/// Hold time used when `AVAudioPlayer` reports a duration that cannot be trusted
/// (NaN, negative, infinite).
///
/// Sized against the real assets [`SoundEvent::sound_path`] returns, measured with
/// `afinfo`: `dt-begin` 0.315s, `dt-confirm` 0.339s, `Basso` 0.768s, `Glass`
/// 1.650s. The fallback has to clear the longest of them, or the case it exists to
/// handle is the case where the cue gets cut off.
#[cfg(target_os = "macos")]
const CUE_FALLBACK_SECS: f64 = 2.0;

/// Extra time held after the reported duration, so the output device drains its
/// last buffer before the player is released. Matches the Linux cue's tail.
#[cfg(target_os = "macos")]
const CUE_RELEASE_TAIL: std::time::Duration = std::time::Duration::from_millis(60);

/// Turn an `AVAudioPlayer` duration into the time its thread should hold it.
///
/// `duration` crosses FFI as an `NSTimeInterval`: a NaN or negative value would
/// panic `Duration::from_secs_f64`, and an absurd one would pin the thread. Both
/// fall back to a fixed hold rather than trusting the number.
#[cfg(target_os = "macos")]
fn cue_hold_time(duration_secs: f64) -> std::time::Duration {
    let secs = if duration_secs.is_finite() && duration_secs > 0.0 {
        duration_secs.min(CUE_MAX_HOLD_SECS)
    } else {
        CUE_FALLBACK_SECS
    };
    std::time::Duration::from_secs_f64(secs) + CUE_RELEASE_TAIL
}

/// Play a short UI sound via `AVAudioPlayer`.
///
/// `AVAudioPlayer` plays as an ordinary mixable CoreAudio client: it does not
/// duck or pause other apps' audio (so the cue no longer interferes with music),
/// and it is an independent output stream so opening the microphone to record
/// does not clip it.
///
/// Runs on a detached thread so the player is created, played and released on one
/// thread. `Retained<AVAudioPlayer>` is not `Send`, so it cannot be parked in a
/// shared slot for a later release; keeping its whole life on one thread is what
/// makes the drop — and therefore the `objc_release` — possible at all. Building
/// and preparing the player also blocks, and the start cue fires on the hotkey
/// path where any delay is audible as lag.
#[cfg(target_os = "macos")]
fn play_macos_sound(path: &'static str, volume: f32) {
    std::thread::spawn(move || {
        // `NSString::from_str` and `NSURL::fileURLWithPath` return autoreleased
        // objects, and a thread with no pool in place just leaks them.
        objc2::rc::autoreleasepool(|_pool| {
            use objc2::AnyThread;
            use objc2_avf_audio::AVAudioPlayer;
            use objc2_foundation::{NSString, NSURL};

            let ns_path = NSString::from_str(path);
            let url = NSURL::fileURLWithPath(&ns_path);

            // SAFETY: `url` is a valid file URL; init returns None (Err) if the
            // file can't be opened as audio, which we handle.
            let player = match unsafe {
                AVAudioPlayer::initWithContentsOfURL_error(AVAudioPlayer::alloc(), &url)
            } {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!("Failed to load sound {} into AVAudioPlayer: {:?}", path, e);
                    return;
                }
            };

            // SAFETY: standard AVAudioPlayer calls; safe to call from any thread.
            let hold = unsafe {
                // AVAudioPlayer's volume is per-player and relative to the system
                // volume, so this scales the cue without touching the user's output.
                player.setVolume(volume);
                player.prepareToPlay();
                if !player.play() {
                    tracing::warn!("AVAudioPlayer failed to start playing {}", path);
                    return;
                }
                cue_hold_time(player.duration())
            };

            tracing::debug!(
                "Playing sound via AVAudioPlayer: {} (hold {:?})",
                path,
                hold
            );

            // Hold the player alive for the cue: AVAudioPlayer stops if it is
            // deallocated mid-play. Dropping it at the end of this scope is what
            // releases it.
            std::thread::sleep(hold);
        });
    });
}

/// Play a sound for recording start
#[tauri::command]
pub fn play_recording_start_sound() {
    tauri_plugin_telemetry::traced_sync("play_recording_start_sound", || {
        play_sound(SoundEvent::RecordingStart);

        Ok::<_, std::convert::Infallible>(())
    })
    .unwrap()
}

/// Play a sound for recording stop
#[tauri::command]
pub fn play_recording_stop_sound() {
    tauri_plugin_telemetry::traced_sync("play_recording_stop_sound", || {
        play_sound(SoundEvent::RecordingStop);

        Ok::<_, std::convert::Infallible>(())
    })
    .unwrap()
}

/// Play a sound for transcription complete
#[tauri::command]
pub fn play_transcription_complete_sound() {
    tauri_plugin_telemetry::traced_sync("play_transcription_complete_sound", || {
        play_sound(SoundEvent::TranscriptionComplete);

        Ok::<_, std::convert::Infallible>(())
    })
    .unwrap()
}

/// Play a sound for error
#[tauri::command]
pub fn play_error_sound() {
    tauri_plugin_telemetry::traced_sync("play_error_sound", || {
        play_sound(SoundEvent::Error);

        Ok::<_, std::convert::Infallible>(())
    })
    .unwrap()
}

/// Check if sounds are enabled
#[tauri::command]
pub async fn are_sounds_enabled() -> Result<bool, Error> {
    tauri_plugin_telemetry::traced("are_sounds_enabled", async move {
        let cfg = config::get_config()?;
        Ok(cfg.audio.play_sounds)
    })
    .await
}

/// Set sounds enabled state
#[tauri::command]
pub async fn set_sounds_enabled(enabled: bool) -> Result<(), Error> {
    tauri_plugin_telemetry::traced("set_sounds_enabled", async move {
        let mut cfg = config::get_config()?;
        cfg.audio.play_sounds = enabled;
        config::set_config(cfg)?;
        tracing::info!("Sounds enabled: {}", enabled);
        Ok(())
    })
    .await
}

/// Get the recording cue volume (0.0 to 1.0).
#[tauri::command]
pub async fn get_sound_volume() -> Result<f32, Error> {
    tauri_plugin_telemetry::traced("get_sound_volume", async move {
        Ok(clamp_volume(config::get_config()?.audio.sound_volume))
    })
    .await
}

/// Set the recording cue volume.
///
/// Clamped before persisting, so an out-of-range value from the IPC boundary is
/// corrected once here rather than needing to be defended against at every
/// playback site.
#[tauri::command]
pub async fn set_sound_volume(volume: f32) -> Result<(), Error> {
    tauri_plugin_telemetry::traced("set_sound_volume", async move {
        let volume = clamp_volume(volume);
        let mut cfg = config::get_config()?;
        cfg.audio.sound_volume = volume;
        config::set_config(cfg)?;
        tracing::info!("Sound volume: {:.2}", volume);
        Ok(())
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn test_sound_event_paths() {
        assert!(SoundEvent::RecordingStart.sound_path().contains("dt-begin"));
        assert!(
            SoundEvent::RecordingStop
                .sound_path()
                .contains("dt-confirm")
        );
        assert!(
            SoundEvent::TranscriptionComplete
                .sound_path()
                .contains("Glass")
        );
        assert!(SoundEvent::Error.sound_path().contains("Basso"));
    }

    #[test]
    fn test_sound_event_equality() {
        assert_eq!(SoundEvent::RecordingStart, SoundEvent::RecordingStart);
        assert_ne!(SoundEvent::RecordingStart, SoundEvent::RecordingStop);
    }

    /// Start and stop must not sound the same, or the user cannot tell by ear
    /// whether recording began or ended — the whole point of the pair. They are
    /// the same interval inverted: rising for on, falling for off.
    #[cfg(target_os = "linux")]
    #[test]
    fn test_start_and_stop_cues_are_inverses() {
        let start = SoundEvent::RecordingStart.cue();
        let stop = SoundEvent::RecordingStop.cue();

        assert_ne!(start, stop);
        assert!(
            start[0].0 < start[1].0,
            "start cue should rise, got {start:?}"
        );
        assert!(stop[0].0 > stop[1].0, "stop cue should fall, got {stop:?}");
        // Same two pitches, opposite order.
        assert_eq!(start[0].0, stop[1].0);
        assert_eq!(start[1].0, stop[0].0);
    }

    /// Every event must produce an audible, non-empty cue at any plausible
    /// device sample rate — a silent cue is indistinguishable from the bug this
    /// replaced.
    #[cfg(target_os = "linux")]
    #[test]
    fn test_every_event_renders_audible_samples() {
        for rate in [8_000.0, 44_100.0, 48_000.0, 192_000.0] {
            for event in [
                SoundEvent::RecordingStart,
                SoundEvent::RecordingStop,
                SoundEvent::TranscriptionComplete,
                SoundEvent::Error,
            ] {
                let samples = render_cue(event.cue(), rate, 1.0);
                assert!(!samples.is_empty(), "{event:?} rendered nothing at {rate}");

                let peak = samples.iter().fold(0.0f32, |a, s| a.max(s.abs()));
                assert!(peak > 0.05, "{event:?} is inaudible at {rate}: peak {peak}");
                assert!(
                    peak <= CUE_AMPLITUDE + 1e-6,
                    "{event:?} exceeds the amplitude ceiling at {rate}: peak {peak}"
                );
                assert!(
                    samples.iter().all(|s| s.is_finite()),
                    "{event:?} produced a non-finite sample at {rate}"
                );
            }
        }
    }

    /// The envelope must start and end at silence, or the speaker clicks.
    #[cfg(target_os = "linux")]
    #[test]
    fn test_cue_fades_in_and_out() {
        let samples = render_cue(SoundEvent::RecordingStart.cue(), 48_000.0, 1.0);

        assert!(
            samples[0].abs() < 1e-6,
            "cue does not start at silence: {}",
            samples[0]
        );
        let last = samples[samples.len() - 1];
        assert!(last.abs() < 0.02, "cue does not fade out: {last}");
    }

    /// Duration must track the requested segment lengths, so a cue can't
    /// silently become a click or run long enough to overlap the next one.
    #[cfg(target_os = "linux")]
    #[test]
    fn test_cue_duration_matches_segments() {
        const RATE: f32 = 48_000.0;

        for event in [SoundEvent::RecordingStart, SoundEvent::RecordingStop] {
            let expected_ms: u32 = event.cue().iter().map(|(_, ms)| ms).sum();
            let frames = render_cue(event.cue(), RATE, 1.0).len() as f32;
            let actual_ms = (frames / RATE * 1000.0).round() as u32;
            assert!(
                actual_ms.abs_diff(expected_ms) <= 1,
                "{event:?}: expected ~{expected_ms}ms, rendered {actual_ms}ms"
            );
        }
    }

    /// Volume must scale the waveform linearly, so the slider does what it says.
    #[cfg(target_os = "linux")]
    #[test]
    fn volume_scales_the_waveform() {
        let peak = |volume: f32| {
            render_cue(SoundEvent::RecordingStart.cue(), 48_000.0, volume)
                .iter()
                .fold(0.0f32, |a, s| a.max(s.abs()))
        };

        let full = peak(1.0);
        let half = peak(0.5);
        let quiet = peak(0.1);

        assert!((half / full - 0.5).abs() < 0.01, "half was {half}/{full}");
        assert!(
            (quiet / full - 0.1).abs() < 0.01,
            "tenth was {quiet}/{full}"
        );
        assert_eq!(peak(0.0), 0.0, "zero volume still produced signal");
    }

    /// A configured volume can arrive from config.json or IPC, so out-of-range and
    /// NaN values must be corrected rather than reaching the waveform. A negative
    /// volume would invert the tone; NaN would fill the buffer with NaN samples.
    #[test]
    fn volume_is_clamped() {
        assert_eq!(clamp_volume(0.5), 0.5);
        assert_eq!(clamp_volume(0.0), 0.0);
        assert_eq!(clamp_volume(1.0), 1.0);
        assert_eq!(clamp_volume(-1.0), 0.0, "negative volume not clamped");
        assert_eq!(clamp_volume(5.0), 1.0, "over-unity volume not clamped");
        assert_eq!(clamp_volume(f32::NAN), 0.0, "NaN volume not handled");
        assert_eq!(clamp_volume(f32::INFINITY), 1.0);
        assert_eq!(clamp_volume(f32::NEG_INFINITY), 0.0);
    }

    /// A cue thread holds its player for the reported duration, so the hold must
    /// track it — a hold shorter than the cue cuts the sound off (#170's fix must
    /// not reintroduce the bug the leak was papering over).
    #[cfg(target_os = "macos")]
    #[test]
    fn cue_hold_covers_the_reported_duration() {
        let hold = cue_hold_time(0.35);
        assert!(
            hold >= std::time::Duration::from_secs_f64(0.35),
            "hold {hold:?} would cut a 350ms cue short"
        );
        assert_eq!(
            hold,
            std::time::Duration::from_secs_f64(0.35) + CUE_RELEASE_TAIL
        );
    }

    /// The fallback hold exists for the case where `duration()` cannot be
    /// trusted, so it has to clear the longest cue Thoth actually plays — or the
    /// one case it exists to handle is the one where the sound gets cut off.
    ///
    /// Durations are read from the real files rather than hardcoded, so a macOS
    /// release that ships a longer asset fails here instead of silently
    /// truncating the cue.
    #[cfg(target_os = "macos")]
    #[test]
    fn cue_fallback_covers_every_real_cue() {
        for event in [
            SoundEvent::RecordingStart,
            SoundEvent::RecordingStop,
            SoundEvent::TranscriptionComplete,
            SoundEvent::Error,
        ] {
            let path = event.sound_path();
            let out = std::process::Command::new("afinfo")
                .arg(path)
                .output()
                .expect("afinfo(1) not runnable");
            let info = String::from_utf8_lossy(&out.stdout);
            let secs: f64 = info
                .lines()
                .find_map(|l| l.trim().strip_prefix("estimated duration:"))
                .and_then(|v| v.split_whitespace().next())
                .and_then(|v| v.parse().ok())
                .unwrap_or_else(|| panic!("no duration for {path} in:\n{info}"));

            assert!(
                CUE_FALLBACK_SECS >= secs,
                "{event:?} is {secs}s but the fallback hold is only \
                 {CUE_FALLBACK_SECS}s — an untrusted duration would cut it off"
            );
            assert!(
                CUE_MAX_HOLD_SECS >= secs,
                "{event:?} is {secs}s, past the {CUE_MAX_HOLD_SECS}s ceiling"
            );
        }
    }

    /// `duration()` crosses FFI, so an untrustworthy value must not panic
    /// `Duration::from_secs_f64` or pin the thread for minutes.
    #[cfg(target_os = "macos")]
    #[test]
    fn cue_hold_rejects_untrustworthy_durations() {
        let fallback = std::time::Duration::from_secs_f64(CUE_FALLBACK_SECS) + CUE_RELEASE_TAIL;

        for bad in [f64::NAN, -1.0, 0.0, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(cue_hold_time(bad), fallback, "duration {bad} not handled");
        }

        let ceiling = std::time::Duration::from_secs_f64(CUE_MAX_HOLD_SECS) + CUE_RELEASE_TAIL;
        assert_eq!(
            cue_hold_time(3_600.0),
            ceiling,
            "absurd duration not capped"
        );
    }

    /// The default must be full volume, so existing installs sound unchanged.
    #[test]
    fn default_volume_is_full() {
        assert_eq!(crate::config::AudioConfig::default().sound_volume, 1.0);
    }

    /// A zero-length segment must not panic or emit samples.
    #[cfg(target_os = "linux")]
    #[test]
    fn test_render_cue_handles_degenerate_input() {
        assert!(render_cue(&[], 48_000.0, 1.0).is_empty());
        assert!(render_cue(&[(440.0, 0)], 48_000.0, 1.0).is_empty());
        // A one-frame segment is inaudible but must not panic on the ramp maths.
        let _ = render_cue(&[(440.0, 1)], 1_000.0, 1.0);
    }

    /// Count live `AVAudioPlayer` instances in this process via `heap(1)`.
    ///
    /// `leaks(1)` is the wrong instrument and reports zero either way: a
    /// `mem::forget`-ed player is still reachable from AVFoundation's own
    /// structures, so it is retained, not leaked. That is precisely why #170 grew
    /// to tens of GB of swap without ever showing up as a leak. `heap` counts live
    /// objects regardless of reachability, which is the number that matters.
    #[cfg(target_os = "macos")]
    fn live_av_audio_players() -> usize {
        let out = std::process::Command::new("heap")
            .arg(std::process::id().to_string())
            .output()
            .expect("heap(1) not runnable");
        assert!(
            out.status.success(),
            "heap(1) failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let report = String::from_utf8_lossy(&out.stdout);

        // `heap`'s object table is: COUNT  BYTES  AVG  CLASS_NAME  TYPE  BINARY.
        let rows: Vec<(usize, &str)> = report
            .lines()
            .filter_map(|line| {
                let fields: Vec<&str> = line.split_whitespace().collect();
                let [count, _bytes, _avg, class, ..] = fields.as_slice() else {
                    return None;
                };
                Some((count.replace(',', "").parse::<usize>().ok()?, *class))
            })
            .collect();

        // Without this the test is vacuous: an unparsed report yields no rows,
        // both counts read zero, and the assertion passes whether or not the
        // players were released. That already happened once, with the class name
        // read from the wrong column. The header is checked as well as the rows,
        // because it is what pins the column order this parse assumes.
        assert!(
            report.contains("COUNT") && report.contains("CLASS_NAME"),
            "heap(1) output is not the table this parses:\n{report}"
        );
        assert!(
            !rows.is_empty(),
            "heap(1) reported a table but no row parsed — the column order \
             changed, and this test would pass vacuously"
        );

        rows.iter()
            .filter(|(_, class)| *class == "AVAudioPlayer")
            .map(|(count, _)| count)
            .sum()
    }

    /// Drives the real macOS cue path and counts the players still alive
    /// afterwards — the acceptance criterion for #170, which a unit test on
    /// [`cue_hold_time`] cannot reach.
    ///
    /// Ignored by default: it plays 40 cues, takes ~30s, and `heap` needs a
    /// locally built (unsigned) binary to inspect.
    ///
    /// Run with: `cargo test --lib sound -- --ignored --nocapture`
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "plays 40 cues and shells out to heap(1)"]
    fn macos_cue_players_are_released() {
        const CUES: usize = 40;

        let before = live_av_audio_players();

        for _ in 0..CUES {
            play_macos_sound(SoundEvent::RecordingStart.sound_path(), 0.0);
            std::thread::sleep(std::time::Duration::from_millis(120));
        }
        // Outlast the longest hold so every cue thread has dropped its player.
        std::thread::sleep(cue_hold_time(CUE_MAX_HOLD_SECS) + std::time::Duration::from_secs(1));

        let after = live_av_audio_players();
        println!("live AVAudioPlayer instances: {before} before, {after} after {CUES} cues");
        assert_eq!(
            after,
            before,
            "{CUES} cues left {} AVAudioPlayer instance(s) alive — the players are \
             being retained, not released (#170)",
            after.saturating_sub(before)
        );
    }

    /// Audible end-to-end check against the real default output device.
    /// Ignored by default: CI has no audio server.
    ///
    /// Run with: `cargo test --lib sound -- --ignored --nocapture`
    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "requires a running audio server and an output device"]
    fn manual_play_recording_cues() {
        for event in [SoundEvent::RecordingStart, SoundEvent::RecordingStop] {
            println!("playing {event:?}: {:?}", event.cue());
            play_cue(event.cue(), 1.0).unwrap_or_else(|e| panic!("{event:?} failed to play: {e}"));
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
    }
}
