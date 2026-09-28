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

use crate::TELEMETRY_TARGET;
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

/// Get the recording indicator window
fn get_indicator_window(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window(INDICATOR_WINDOW_LABEL)
}

/// Get the recording indicator window (generic version for use from shortcut handler)
fn get_indicator_window_generic<R: Runtime>(app: &AppHandle<R>) -> Option<tauri::WebviewWindow<R>> {
    app.get_webview_window(INDICATOR_WINDOW_LABEL)
}

/// Find the monitor containing a point (in logical pixels).
///
/// Returns `(mon_x, mon_y, mon_width, mon_height, scale_factor)` in logical pixels.
pub(crate) fn find_monitor_for_point(
    app: &AppHandle,
    x: f64,
    y: f64,
) -> Option<(f64, f64, f64, f64, f64)> {
    // Get all monitors - try main window first, then any other window
    let monitors = app
        .get_webview_window("main")
        .and_then(|w| w.available_monitors().ok())
        .or_else(|| {
            app.get_webview_window(INDICATOR_WINDOW_LABEL)
                .and_then(|w| w.available_monitors().ok())
        })?;

    for monitor in monitors {
        let scale_factor = monitor.scale_factor();
        let pos = monitor.position();
        let size = monitor.size();

        // Convert to logical pixels
        let mon_x = pos.x as f64 / scale_factor;
        let mon_y = pos.y as f64 / scale_factor;
        let mon_width = size.width as f64 / scale_factor;
        let mon_height = size.height as f64 / scale_factor;

        // Check if point is within this monitor
        if x >= mon_x && x < mon_x + mon_width && y >= mon_y && y < mon_y + mon_height {
            return Some((mon_x, mon_y, mon_width, mon_height, scale_factor));
        }
    }

    None
}

/// Get the window dimensions for a given indicator style.
pub fn dimensions_for_style(style: IndicatorStyle) -> (f64, f64) {
    match style {
        IndicatorStyle::CursorDot | IndicatorStyle::FixedFloat => (DOT_WIDTH, DOT_HEIGHT),
        IndicatorStyle::Pill => (PILL_WIDTH, PILL_HEIGHT),
    }
}

/// Resize the indicator window to match the current style.
fn resize_indicator(indicator: &WebviewWindow, style: IndicatorStyle) -> Result<(), String> {
    let (w, h) = dimensions_for_style(style);
    indicator
        .set_size(tauri::Size::Logical(LogicalSize::new(w, h)))
        .map_err(|e| e.to_string())
}

/// Position the indicator at a fixed location based on `RecorderPosition` config.
fn position_at_fixed(
    app: &AppHandle,
    indicator: &WebviewWindow,
    style: IndicatorStyle,
) -> Result<(), String> {
    let cfg = config::get_config().unwrap_or_default();
    let pos = cfg.recorder.position;
    let (iw, ih) = dimensions_for_style(style);

    let monitor = app
        .get_webview_window("main")
        .and_then(|w| w.current_monitor().ok().flatten())
        .or_else(|| indicator.current_monitor().ok().flatten())
        .or_else(|| indicator.primary_monitor().ok().flatten())
        .ok_or_else(|| "Could not determine current monitor".to_string())?;

    let scale = monitor.scale_factor();
    let mp = monitor.position();
    let ms = monitor.size();
    let mx = mp.x as f64 / scale;
    let my = mp.y as f64 / scale;
    let mw = ms.width as f64 / scale;
    let mh = ms.height as f64 / scale;

    let padding = 20.0;
    let (x, y) = match pos {
        config::RecorderPosition::Cursor => {
            // For fixed-float with cursor position: use centre-bottom fallback
            (mx + (mw / 2.0) - (iw / 2.0), my + mh - ih - BOTTOM_PADDING)
        }
        config::RecorderPosition::TrayIcon => {
            // Near top-right (where tray typically is)
            (mx + mw - iw - padding, my + padding + 30.0)
        }
        config::RecorderPosition::TopLeft => (mx + padding, my + padding + 30.0),
        config::RecorderPosition::TopRight => (mx + mw - iw - padding, my + padding + 30.0),
        config::RecorderPosition::BottomLeft => (mx + padding, my + mh - ih - BOTTOM_PADDING),
        config::RecorderPosition::BottomRight => {
            (mx + mw - iw - padding, my + mh - ih - BOTTOM_PADDING)
        }
        config::RecorderPosition::Centre => {
            (mx + (mw / 2.0) - (iw / 2.0), my + (mh / 2.0) - (ih / 2.0))
        }
    };

    indicator
        .set_position(tauri::Position::Logical(LogicalPosition::new(x, y)))
        .map_err(|e| e.to_string())?;

    tracing::debug!("Fixed-float indicator at ({}, {}) position={:?}", x, y, pos);
    Ok(())
}

/// Position the pill indicator at the top-centre of the screen.
fn position_pill(app: &AppHandle, indicator: &WebviewWindow) -> Result<(), String> {
    let monitor = app
        .get_webview_window("main")
        .and_then(|w| w.current_monitor().ok().flatten())
        .or_else(|| indicator.current_monitor().ok().flatten())
        .or_else(|| indicator.primary_monitor().ok().flatten())
        .ok_or_else(|| "Could not determine current monitor".to_string())?;

    let scale = monitor.scale_factor();
    let mp = monitor.position();
    let ms = monitor.size();
    let mx = mp.x as f64 / scale;
    let my = mp.y as f64 / scale;
    let mw = ms.width as f64 / scale;

    let x = mx + (mw / 2.0) - (PILL_WIDTH / 2.0);
    let y = my + PILL_EDGE_PADDING + 30.0; // Below menu bar

    indicator
        .set_position(tauri::Position::Logical(LogicalPosition::new(x, y)))
        .map_err(|e| e.to_string())?;

    tracing::debug!("Pill indicator at top-centre ({}, {})", x, y);
    Ok(())
}

/// Shared logic for showing the recording indicator.
///
/// Checks config, warns on Wayland, gets the window, resizes and positions
/// based on the current indicator style, then shows and optionally starts
/// mouse tracking (cursor-dot only).
fn show_indicator_common<F>(app: &AppHandle, fallback_position: F) -> Result<(), String>
where
    F: FnOnce(&AppHandle, &WebviewWindow) -> Result<(), String>,
{
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

    let indicator = get_indicator_window(app)
        .ok_or_else(|| "Recording indicator window not found".to_string())?;

    // Resize window for the current style
    resize_indicator(&indicator, style)?;

    // Emit style to frontend so it knows how to render
    let _ = indicator.emit("indicator-style", style);

    // On Linux, show before positioning to ensure the window is mapped
    #[cfg(target_os = "linux")]
    {
        indicator.show().map_err(|e| e.to_string())?;
    }

    match style {
        IndicatorStyle::CursorDot => {
            // Position at cursor where the platform allows reading it; otherwise
            // (Wayland, or no cursor available) use the provided fallback.
            if let Some((x, y)) =
                mouse_tracker::get_initial_position().filter(|_| cursor_following_possible)
            {
                indicator
                    .set_position(tauri::Position::Logical(LogicalPosition::new(x, y)))
                    .map_err(|e| e.to_string())?;
                tracing::debug!("Cursor-dot indicator at ({}, {})", x, y);
            } else {
                tracing::info!(
                    "Cursor position unavailable (Wayland or no pointer); using fallback position"
                );
                fallback_position(app, &indicator)?;
            }
        }
        IndicatorStyle::FixedFloat => {
            position_at_fixed(app, &indicator, style)?;
        }
        IndicatorStyle::Pill => {
            position_pill(app, &indicator)?;
        }
    }

    // On macOS, show after positioning
    #[cfg(not(target_os = "linux"))]
    {
        indicator.show().map_err(|e| e.to_string())?;
    }

    // Only track the mouse for cursor-dot style, and only where the cursor can
    // actually be followed (not Wayland). `start_tracking` also guards Wayland
    // internally; this avoids the call entirely.
    if style == IndicatorStyle::CursorDot && cursor_following_possible {
        mouse_tracker::start_tracking();
    }

    Ok(())
}

/// Position the indicator at the primary monitor's centre-bottom.
///
/// Used as fallback when mouse position is unavailable in the generic
/// (`show_indicator_instant`) code path.
fn position_at_primary_monitor<R: Runtime>(indicator: &tauri::WebviewWindow<R>) {
    if let Ok(Some(monitor)) = indicator.primary_monitor() {
        let scale = monitor.scale_factor();
        let pos = monitor.position();
        let size = monitor.size();
        let mx = pos.x as f64 / scale;
        let my = pos.y as f64 / scale;
        let mw = size.width as f64 / scale;
        let mh = size.height as f64 / scale;
        let x = mx + (mw / 2.0) - (DOT_WIDTH / 2.0);
        let y = my + mh - DOT_HEIGHT - BOTTOM_PADDING;
        let _ = indicator.set_position(tauri::Position::Logical(LogicalPosition::new(x, y)));
    }
}

/// Show the recording indicator window at the current cursor position.
///
/// Positions the indicator near the mouse cursor and starts cursor-following
/// tracking. Falls back to bottom-centre of the main window's monitor if
/// the mouse position cannot be determined.
///
/// Returns silently if the recording indicator is disabled in config.
#[tauri::command]
#[tracing::instrument(target = TELEMETRY_TARGET, skip_all, err)]
pub fn show_recording_indicator(app: AppHandle) -> Result<(), Error> {
    tracing::info!("show_recording_indicator called");
    show_indicator_common(&app, |app_handle, indicator| {
        position_at_bottom_centre(app_handle, indicator)
    })
    .map_err(Into::into)
}

/// Position the indicator at the bottom centre of the main window's monitor
fn position_at_bottom_centre(app: &AppHandle, indicator: &WebviewWindow) -> Result<(), String> {
    // Get monitor for positioning - try main window first, then indicator window, then primary monitor
    let monitor = if let Some(main_window) = app.get_webview_window("main") {
        main_window
            .current_monitor()
            .ok()
            .flatten()
            .or_else(|| indicator.current_monitor().ok().flatten())
            .or_else(|| indicator.primary_monitor().ok().flatten())
    } else {
        // Main window not available, use indicator window or primary monitor
        indicator
            .current_monitor()
            .ok()
            .flatten()
            .or_else(|| indicator.primary_monitor().ok().flatten())
    }
    .ok_or_else(|| "Could not determine current monitor".to_string())?;

    let monitor_pos = monitor.position();
    let monitor_size = monitor.size();
    let scale_factor = monitor.scale_factor();

    // Convert to logical pixels
    let monitor_x = monitor_pos.x as f64 / scale_factor;
    let monitor_y = monitor_pos.y as f64 / scale_factor;
    let monitor_width = monitor_size.width as f64 / scale_factor;
    let monitor_height = monitor_size.height as f64 / scale_factor;

    tracing::info!(
        "Monitor: pos=({}, {}), size={}x{}, scale={}",
        monitor_x,
        monitor_y,
        monitor_width,
        monitor_height,
        scale_factor
    );

    // Calculate centre-bottom position
    let x = monitor_x + (monitor_width / 2.0) - (DOT_WIDTH / 2.0);
    let y = monitor_y + monitor_height - DOT_HEIGHT - BOTTOM_PADDING;

    tracing::info!("Setting indicator position to ({}, {})", x, y);

    indicator
        .set_position(tauri::Position::Logical(LogicalPosition::new(x, y)))
        .map_err(|e| e.to_string())?;

    Ok(())
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
#[tracing::instrument(target = TELEMETRY_TARGET, skip_all, err)]
pub fn hide_recording_indicator(app: AppHandle) -> Result<(), Error> {
    tracing::info!("hide_recording_indicator called");

    // Stop mouse tracking before hiding
    mouse_tracker::stop_tracking();

    let window = get_indicator_window(&app)
        .ok_or_else(|| "Recording indicator window not found".to_string())?;

    window.hide().map_err(|e| e.to_string())?;
    tracing::info!("Recording indicator hidden");

    Ok(())
}

/// Show the recording indicator immediately (generic version for shortcut handler).
///
/// Positions the indicator based on the configured style. For cursor-dot,
/// positions at the current mouse cursor position and starts cursor-following
/// tracking. For fixed-float and pill, uses static positioning.
///
/// The window is kept always-visible but positioned off-screen when "hidden"
/// to avoid macOS window show/hide animation delays.
///
/// On Linux/Wayland, uses actual show()/hide() since compositor controls positioning.
///
/// Returns silently if the recording indicator is disabled in config.
pub fn show_indicator_instant<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    tracing::info!("show_indicator_instant called (fast path)");

    let indicator = get_indicator_window_generic(app)
        .ok_or_else(|| "Recording indicator window not found".to_string())?;

    // Check config
    let cfg = config::get_config().unwrap_or_default();
    let style = cfg.general.indicator_style;

    if !cfg.general.show_recording_indicator {
        tracing::info!("Recording indicator disabled in config, skipping instant show");
        return Ok(());
    }

    // On Wayland the compositor controls placement and the global cursor
    // position is unreadable, so cursor-following is impossible; fall back to a
    // fixed position and skip tracking.
    let cursor_following_possible = !crate::shortcuts::is_wayland();

    // Resize window for the current style
    let (w, h) = dimensions_for_style(style);
    let _ = indicator.set_size(tauri::Size::Logical(LogicalSize::new(w, h)));

    // Emit style to frontend
    let _ = indicator.emit("indicator-style", style);

    // On Linux, show before positioning to ensure the window is mapped
    #[cfg(target_os = "linux")]
    {
        indicator.show().map_err(|e| e.to_string())?;
    }

    match style {
        IndicatorStyle::CursorDot => {
            // Position at cursor where the platform allows it; otherwise fall
            // back to primary monitor centre-bottom.
            if let Some((x, y)) =
                mouse_tracker::get_initial_position().filter(|_| cursor_following_possible)
            {
                indicator
                    .set_position(tauri::Position::Logical(LogicalPosition::new(x, y)))
                    .map_err(|e| e.to_string())?;
                tracing::debug!("Cursor-dot indicator at ({}, {})", x, y);
            } else {
                position_at_primary_monitor(&indicator);
                tracing::debug!("Cursor-dot indicator at fallback (bottom-centre)");
            }
        }
        IndicatorStyle::FixedFloat => {
            // Position at the configured fixed location
            position_fixed_generic(&indicator)?;
        }
        IndicatorStyle::Pill => {
            // Position at top-centre of screen
            position_pill_generic(&indicator)?;
        }
    }

    // On macOS, show after positioning
    #[cfg(not(target_os = "linux"))]
    {
        indicator.show().map_err(|e| e.to_string())?;
    }

    // Only track the mouse for cursor-dot style where the cursor can be followed.
    if style == IndicatorStyle::CursorDot && cursor_following_possible {
        mouse_tracker::start_tracking();
    }

    Ok(())
}

/// Position indicator at fixed location (generic version for shortcut handler).
fn position_fixed_generic<R: Runtime>(indicator: &tauri::WebviewWindow<R>) -> Result<(), String> {
    let cfg = config::get_config().unwrap_or_default();
    let pos = cfg.recorder.position;
    let style = cfg.general.indicator_style;
    let (iw, ih) = dimensions_for_style(style);

    let monitor = indicator
        .primary_monitor()
        .ok()
        .flatten()
        .ok_or_else(|| "Could not determine primary monitor".to_string())?;

    let scale = monitor.scale_factor();
    let mp = monitor.position();
    let ms = monitor.size();
    let mx = mp.x as f64 / scale;
    let my = mp.y as f64 / scale;
    let mw = ms.width as f64 / scale;
    let mh = ms.height as f64 / scale;

    let padding = 20.0;
    let (x, y) = match pos {
        config::RecorderPosition::Cursor => {
            (mx + (mw / 2.0) - (iw / 2.0), my + mh - ih - BOTTOM_PADDING)
        }
        config::RecorderPosition::TrayIcon => (mx + mw - iw - padding, my + padding + 30.0),
        config::RecorderPosition::TopLeft => (mx + padding, my + padding + 30.0),
        config::RecorderPosition::TopRight => (mx + mw - iw - padding, my + padding + 30.0),
        config::RecorderPosition::BottomLeft => (mx + padding, my + mh - ih - BOTTOM_PADDING),
        config::RecorderPosition::BottomRight => {
            (mx + mw - iw - padding, my + mh - ih - BOTTOM_PADDING)
        }
        config::RecorderPosition::Centre => {
            (mx + (mw / 2.0) - (iw / 2.0), my + (mh / 2.0) - (ih / 2.0))
        }
    };

    indicator
        .set_position(tauri::Position::Logical(LogicalPosition::new(x, y)))
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Position pill at top-centre (generic version for shortcut handler).
fn position_pill_generic<R: Runtime>(indicator: &tauri::WebviewWindow<R>) -> Result<(), String> {
    let monitor = indicator
        .primary_monitor()
        .ok()
        .flatten()
        .ok_or_else(|| "Could not determine primary monitor".to_string())?;

    let scale = monitor.scale_factor();
    let mp = monitor.position();
    let ms = monitor.size();
    let mx = mp.x as f64 / scale;
    let my = mp.y as f64 / scale;
    let mw = ms.width as f64 / scale;

    let x = mx + (mw / 2.0) - (PILL_WIDTH / 2.0);
    let y = my + PILL_EDGE_PADDING + 30.0;

    indicator
        .set_position(tauri::Position::Logical(LogicalPosition::new(x, y)))
        .map_err(|e| e.to_string())?;
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
/// Extracted from the identical guard blocks in shortcuts/manager.rs,
/// keyboard_service.rs, and tray.rs so the logic lives in one place.
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
/// On macOS, the window is left visible but off-screen - we never hide() it
/// to avoid show/hide animation delays.
///
/// On Linux/Wayland, we show then hide the window during pre-warm to ensure
/// it's properly mapped with the compositor. Subsequent hide/show should work.
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
