//! Recording indicator window management
//!
//! Handles showing/hiding the floating recording indicator pill near the text
//! cursor (caret), similar to macOS dictation. Falls back to bottom-centre of
//! the main window's monitor if caret position cannot be determined.
//!
//! The indicator window is pre-warmed at app startup to eliminate any delay
//! when showing it for the first time.
//!
//! Can be disabled via config (general.show_recording_indicator) for users
//! who prefer no visual indicator (e.g., tiling window manager users).
//!
//! Note: On Wayland, mouse tracking and precise window positioning don't work
//! reliably due to Wayland's security model. Users may want to disable the
//! indicator on Wayland.

use crate::config;
use crate::config::IndicatorStyle;
use crate::error::Error;
use crate::mouse_tracker;
use tauri::{AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, Runtime, WebviewWindow};

/// Label for the recording indicator window (must match tauri.conf.json)
const INDICATOR_WINDOW_LABEL: &str = "recording-indicator";

/// Cursor-dot/fixed-float dimensions in logical pixels
const DOT_WIDTH: f64 = 58.0;
const DOT_HEIGHT: f64 = 58.0;

/// Pill dimensions in logical pixels
const PILL_WIDTH: f64 = 280.0;
const PILL_HEIGHT: f64 = 44.0;

/// Fallback: padding from bottom of screen (above dock)
const BOTTOM_PADDING: f64 = 120.0;

/// Padding from screen edge for pill style
const PILL_EDGE_PADDING: f64 = 12.0;

/// A monitor's rectangle in logical pixels.
#[derive(Clone, Copy)]
struct MonitorRect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    scale_factor: f64,
}

impl MonitorRect {
    fn of(monitor: &tauri::window::Monitor) -> Self {
        let scale_factor = monitor.scale_factor();
        let pos = monitor.position();
        let size = monitor.size();
        Self {
            x: pos.x as f64 / scale_factor,
            y: pos.y as f64 / scale_factor,
            width: size.width as f64 / scale_factor,
            height: size.height as f64 / scale_factor,
            scale_factor,
        }
    }

    fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && x < self.x + self.width && y >= self.y && y < self.y + self.height
    }

    fn bottom_centre(&self, w: f64, h: f64) -> (f64, f64) {
        (
            self.x + self.width / 2.0 - w / 2.0,
            self.y + self.height - h - BOTTOM_PADDING,
        )
    }
}

/// The monitor the indicator is placed on when the cursor does not decide it:
/// the main window's monitor, else the indicator's own, else the primary.
fn placement_monitor<R: Runtime>(
    app: &AppHandle<R>,
    indicator: &WebviewWindow<R>,
) -> Result<MonitorRect, String> {
    app.get_webview_window("main")
        .and_then(|w| w.current_monitor().ok().flatten())
        .or_else(|| indicator.current_monitor().ok().flatten())
        .or_else(|| indicator.primary_monitor().ok().flatten())
        .map(|m| MonitorRect::of(&m))
        .ok_or_else(|| "Could not determine current monitor".to_string())
}

/// Find the monitor containing a point (in logical pixels).
///
/// Returns `(mon_x, mon_y, mon_width, mon_height, scale_factor)` in logical pixels.
pub(crate) fn find_monitor_for_point(
    app: &AppHandle,
    x: f64,
    y: f64,
) -> Option<(f64, f64, f64, f64, f64)> {
    let monitors = app
        .get_webview_window("main")
        .and_then(|w| w.available_monitors().ok())
        .or_else(|| {
            app.get_webview_window(INDICATOR_WINDOW_LABEL)
                .and_then(|w| w.available_monitors().ok())
        })?;

    monitors
        .iter()
        .map(MonitorRect::of)
        .find(|m| m.contains(x, y))
        .map(|m| (m.x, m.y, m.width, m.height, m.scale_factor))
}

/// Get the window dimensions for a given indicator style.
pub fn dimensions_for_style(style: IndicatorStyle) -> (f64, f64) {
    match style {
        IndicatorStyle::CursorDot | IndicatorStyle::FixedFloat => (DOT_WIDTH, DOT_HEIGHT),
        IndicatorStyle::Pill => (PILL_WIDTH, PILL_HEIGHT),
    }
}

fn set_logical_position<R: Runtime>(
    indicator: &WebviewWindow<R>,
    (x, y): (f64, f64),
) -> Result<(), String> {
    indicator
        .set_position(tauri::Position::Logical(LogicalPosition::new(x, y)))
        .map_err(|e| e.to_string())
}

/// Where the fixed-float style sits for a `RecorderPosition`.
fn fixed_position(
    mon: MonitorRect,
    pos: config::RecorderPosition,
    (iw, ih): (f64, f64),
) -> (f64, f64) {
    let padding = 20.0;
    let top = mon.y + padding + 30.0;
    match pos {
        config::RecorderPosition::Cursor => mon.bottom_centre(iw, ih),
        config::RecorderPosition::TrayIcon | config::RecorderPosition::TopRight => {
            (mon.x + mon.width - iw - padding, top)
        }
        config::RecorderPosition::TopLeft => (mon.x + padding, top),
        config::RecorderPosition::BottomLeft => {
            (mon.x + padding, mon.y + mon.height - ih - BOTTOM_PADDING)
        }
        config::RecorderPosition::BottomRight => (
            mon.x + mon.width - iw - padding,
            mon.y + mon.height - ih - BOTTOM_PADDING,
        ),
        config::RecorderPosition::Centre => (
            mon.x + mon.width / 2.0 - iw / 2.0,
            mon.y + mon.height / 2.0 - ih / 2.0,
        ),
    }
}

/// Position the indicator for `style`. Cursor-dot follows the cursor where the
/// platform allows reading it, else sits at the bottom centre; fixed-float uses
/// the configured `RecorderPosition`; the pill sits at top centre. Everything
/// but the cursor itself is placed on the main window's monitor.
fn position_for_style<R: Runtime>(
    app: &AppHandle<R>,
    indicator: &WebviewWindow<R>,
    style: IndicatorStyle,
    cursor_following_possible: bool,
) -> Result<(), String> {
    if style == IndicatorStyle::CursorDot {
        if let Some(xy) =
            mouse_tracker::get_initial_position().filter(|_| cursor_following_possible)
        {
            set_logical_position(indicator, xy)?;
            tracing::debug!("Cursor-dot indicator at {:?}", xy);
            return Ok(());
        }
        tracing::info!(
            "Cursor position unavailable (Wayland or no pointer); using bottom-centre fallback"
        );
    }

    let mon = placement_monitor(app, indicator)?;
    let xy = match style {
        IndicatorStyle::CursorDot => mon.bottom_centre(DOT_WIDTH, DOT_HEIGHT),
        IndicatorStyle::FixedFloat => {
            let pos = config::get_config().unwrap_or_default().recorder.position;
            fixed_position(mon, pos, dimensions_for_style(style))
        }
        IndicatorStyle::Pill => (
            mon.x + mon.width / 2.0 - PILL_WIDTH / 2.0,
            mon.y + PILL_EDGE_PADDING + 30.0, // below the menu bar
        ),
    };
    set_logical_position(indicator, xy)?;
    tracing::debug!("Indicator ({:?}) at {:?}", style, xy);
    Ok(())
}

/// Show the recording indicator, synchronously.
///
/// Positions it for the configured style and, for cursor-dot, starts
/// cursor-following tracking. Used by both the `show_recording_indicator`
/// command and the record key, so the two place it identically. Returns
/// silently if the indicator is disabled in config.
pub fn show_indicator_instant<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    let cfg = config::get_config().unwrap_or_default();
    let style = cfg.general.indicator_style;

    if !cfg.general.show_recording_indicator {
        tracing::info!("Recording indicator disabled in config, skipping show");
        return Ok(());
    }

    // On Wayland the compositor controls window placement: a client cannot read
    // the global cursor position or set an absolute window position. The
    // cursor-dot style therefore cannot follow the cursor; fall back to the
    // fixed position and skip cursor tracking. The window still shows; only its
    // placement is the compositor's choice.
    let cursor_following_possible = !crate::shortcuts::is_wayland();

    let indicator = app
        .get_webview_window(INDICATOR_WINDOW_LABEL)
        .ok_or_else(|| "Recording indicator window not found".to_string())?;

    let (w, h) = dimensions_for_style(style);
    let _ = indicator.set_size(tauri::Size::Logical(LogicalSize::new(w, h)));
    let _ = indicator.emit("indicator-style", style);

    // On Linux, show before positioning to ensure the window is mapped
    #[cfg(target_os = "linux")]
    {
        indicator.show().map_err(|e| e.to_string())?;
    }

    position_for_style(app, &indicator, style, cursor_following_possible)?;

    // On macOS, show after positioning
    #[cfg(not(target_os = "linux"))]
    {
        indicator.show().map_err(|e| e.to_string())?;
    }

    if style == IndicatorStyle::CursorDot && cursor_following_possible {
        mouse_tracker::start_tracking();
    }

    Ok(())
}

/// Show the recording indicator window (command form of `show_indicator_instant`).
#[tauri::command]
pub async fn show_recording_indicator(app: AppHandle) -> Result<(), Error> {
    tracing::info!("show_recording_indicator called");
    show_indicator_instant(&app).map_err(Into::into)
}

/// Hide the recording indicator window.
///
/// Uses a real `hide()` on all platforms. A previous macOS optimisation kept
/// the window permanently visible but parked off-screen to avoid show/hide
/// animation latency; that left a visible-but-off-screen window that the macOS
/// window server could remap back on-screen at launch and on display wake,
/// producing a stray floating indicator. Hiding for real removes that whole
/// class of bug. The webview stays warm from pre-warm, so re-showing an
/// already-mapped 58px borderless window is fast.
#[tauri::command]
pub fn hide_recording_indicator(app: AppHandle) -> Result<(), Error> {
    tracing::info!("hide_recording_indicator called");

    // Stop mouse tracking before hiding
    mouse_tracker::stop_tracking();

    let window = app
        .get_webview_window(INDICATOR_WINDOW_LABEL)
        .ok_or_else(|| "Recording indicator window not found".to_string())?;

    window.hide().map_err(|e| e.to_string())?;
    tracing::info!("Recording indicator hidden");

    Ok(())
}

/// Show the indicator and play the start tone when a new recording is starting.
///
/// Gated ONLY on "not already recording" — the same authority the stop tone
/// uses. It is deliberately NOT gated on `is_transcription_ready()`: pressing
/// record starts a recording whether or not the model is already in memory (the
/// pipeline warms it in the background), so the audio cue must fire regardless.
/// Gating the bing on model-readiness was why it went silent after the ~45s
/// idle warm-stream teardown — the model check could lag a cold start, dropping
/// the tone even though recording began. The stop tone never had this gate,
/// which is why it was always reliable.
///
/// Called by the record key's queue (`pipeline::hotkey_toggle`) as it acts.
pub(crate) fn maybe_play_start_indicator<R: Runtime>(app: &AppHandle<R>) {
    if !crate::audio::is_recording() {
        if let Err(e) = show_indicator_instant(app) {
            tracing::warn!("Failed to show recording indicator: {}", e);
        }
        crate::sound::play_sound(crate::sound::SoundEvent::RecordingStart);
    }
}

/// Pre-warm the recording indicator window by loading its content.
///
/// This eliminates the delay on first show by ensuring the webview
/// content is fully loaded and rendered before the user triggers recording.
/// Should be called during app startup.
///
/// On macOS the window is shown briefly off-screen, then hidden for real. On
/// Linux it is left hidden (no show/hide cycle; see the comment in the body).
pub fn prewarm_indicator_window(app: &AppHandle) {
    if crate::shortcuts::is_wayland() {
        tracing::info!(
            "Wayland session: recording indicator uses a fixed position (the compositor controls \
             window placement); it does not follow the cursor"
        );
    }

    let app_handle = app.clone();

    // Spawn async task to pre-warm without blocking startup
    tauri::async_runtime::spawn(async move {
        // Small delay to let the main window initialise first
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        if let Some(window) = app_handle.get_webview_window(INDICATOR_WINDOW_LABEL) {
            tracing::info!("Pre-warming recording indicator window");

            // On Linux we deliberately do NOT show-then-hide to pre-warm. A
            // show()/hide() cycle risks leaving a stray indicator visible if the
            // hide() is not honoured by the compositor (a real failure mode on
            // some Wayland compositors). The webview is created and loads its
            // content with the window; the first real show() maps it. The minor
            // first-show latency is preferable to a stuck floating window.
            #[cfg(target_os = "linux")]
            {
                let _ = &window; // used only in the non-linux branch below
                tracing::info!(
                    "Recording indicator window ready (Linux - left hidden; no show/hide pre-warm)"
                );
            }

            // On macOS, briefly show off-screen to map the window and load the
            // webview, then hide it. A permanently-visible window would be
            // remapped on-screen by the window server at launch/display-wake,
            // causing a stray floating indicator.
            #[cfg(not(target_os = "linux"))]
            {
                // Park off-screen first so the brief pre-warm show is never visible.
                if let Err(e) = window.set_position(tauri::Position::Logical(LogicalPosition::new(
                    -10000.0, -10000.0,
                ))) {
                    tracing::warn!("Failed to position indicator off-screen: {}", e);
                }

                if let Err(e) = window.show() {
                    tracing::warn!("Failed to show indicator for pre-warming: {}", e);
                }

                // Wait for the webview to fully load and render while off-screen.
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;

                // Hide for real — the webview is now warm; subsequent shows are fast.
                if let Err(e) = window.hide() {
                    tracing::warn!("Failed to hide indicator after pre-warming: {}", e);
                }

                tracing::info!(
                    "Recording indicator window pre-warmed and ready (hidden until needed)"
                );
            }
        } else {
            tracing::warn!("Recording indicator window not found for pre-warming");
        }
    });
}
