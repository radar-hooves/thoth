//! Mouse cursor tracking for the recording indicator window.
//!
//! When recording, the indicator window follows the mouse cursor so it's
//! always visible near where the user is working. Uses a background polling
//! thread at ~60fps to read the cursor position via Core Graphics and
//! reposition the indicator window.
//!
//! The indicator window is made click-through during tracking so it never
//! intercepts user clicks. The user stops recording via keyboard shortcut.
//!
//! Coordinate system: `CGEvent::location()` returns logical points with
//! origin at the top-left of the primary display, matching Tauri's
//! `LogicalPosition` directly.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use parking_lot::RwLock;
use tauri::{AppHandle, LogicalPosition, Manager};

use crate::recording_indicator;

/// Poll interval in milliseconds (~60fps)
const POLL_INTERVAL_MS: u64 = 16;

/// Offset from cursor: window centred directly above with 12px gap
const CURSOR_OFFSET_X: f64 = -29.0; // half of 58px width (centres horizontally)
const CURSOR_OFFSET_Y: f64 = -70.0; // 58px height + 12px gap (directly above)

/// Skip repositioning if cursor moved less than this (logical pixels)
const MOVE_THRESHOLD: f64 = 1.0;

/// Smoothing factor for position interpolation (0.0 = no movement, 1.0 = instant snap)
const SMOOTHING_FACTOR: f64 = 0.45;

/// Re-query monitor bounds when cursor is within this margin of cached edges
const MONITOR_REFRESH_MARGIN: f64 = 100.0;

/// Minimum interval between edge-triggered monitor bounds refreshes (milliseconds)
const MONITOR_REFRESH_COOLDOWN_MS: u64 = 100;

/// Maximum age of cached monitor bounds before forced refresh (milliseconds).
/// Only triggers if the cursor has actually moved since the last refresh.
const MONITOR_CACHE_TTL_MS: u64 = 2000;

/// Hide the indicator after this many consecutive position failures.
/// At 16ms poll interval, 90 failures = ~1.4s. This avoids false triggers
/// during display reconfiguration events (which can cause CG failures
/// for 500ms-2s).
const FAILURE_HIDE_THRESHOLD: u32 = 90;

/// Recording indicator window label (must match tauri.conf.json)
const INDICATOR_WINDOW_LABEL: &str = "recording-indicator";

/// Cached monitor bounds (x, y, width, height) in logical pixels
struct CachedMonitor {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

struct TrackerState {
    is_running: AtomicBool,
    /// Counter for consecutive mouse position failures (for rate-limited logging)
    consecutive_failures: AtomicU32,
    /// Set by the wake observer to invalidate caches
    wake_detected: AtomicBool,
    /// Incremented each time a tracking thread starts. Allows detection
    /// of stale threads from a previous generation.
    generation: AtomicU32,
}

impl Default for TrackerState {
    fn default() -> Self {
        Self {
            is_running: AtomicBool::new(false),
            consecutive_failures: AtomicU32::new(0),
            wake_detected: AtomicBool::new(false),
            generation: AtomicU32::new(0),
        }
    }
}

/// Global tracker state
static TRACKER: OnceLock<RwLock<TrackerState>> = OnceLock::new();

/// Global app handle, stored at init with concrete type
static APP_HANDLE: OnceLock<AppHandle> = OnceLock::new();

fn get_tracker() -> &'static RwLock<TrackerState> {
    TRACKER.get_or_init(|| RwLock::new(TrackerState::default()))
}

/// Initialise the mouse tracker with the app handle.
///
/// Must be called during `setup()` where the AppHandle type is concrete.
pub fn init(app: &AppHandle) {
    APP_HANDLE.set(app.clone()).ok();
    // Eagerly initialise tracker state
    let _ = get_tracker();
    tracing::info!("Mouse tracker initialised");
}

/// Notify the tracker that the system has woken from sleep.
///
/// Invalidates cached monitor bounds so they are refreshed on the next
/// poll cycle. Also discards the smoothed position so the indicator
/// snaps to the actual cursor location rather than lerping from the
/// stale pre-sleep position.
pub fn notify_wake() {
    let tracker = get_tracker();
    tracker.read().wake_detected.store(true, Ordering::SeqCst);
    tracing::info!("Mouse tracker notified of system wake");
}

/// Get the current mouse cursor position using Core Graphics.
///
/// Returns `(x, y)` in logical points with origin at top-left of primary display.
#[cfg(target_os = "macos")]
fn get_mouse_position() -> Option<(f64, f64)> {
    use core_graphics::event::CGEvent;
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState).ok()?;
    let event = CGEvent::new(source).ok()?;
    let point = event.location();
    Some((point.x, point.y))
}

/// Get the current mouse cursor position using X11.
///
/// Returns `(x, y)` in logical pixels with origin at top-left of primary display.
/// Returns `None` on Wayland (where X11 isn't available).
#[cfg(target_os = "linux")]
fn get_mouse_position() -> Option<(f64, f64)> {
    // Check if we're on Wayland - X11 won't work there
    if crate::shortcuts::is_wayland() {
        return None;
    }

    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::ConnectionExt;

    // Get the DISPLAY environment variable
    let display = std::env::var("DISPLAY").ok()?;

    // Connect to X11 display
    let (conn, screen_num) = x11rb::connect(Some(&display)).ok()?;

    // Get the root window of the default screen
    let screen = &conn.setup().roots[screen_num];
    let root = screen.root;

    // Query pointer position
    let pointer = conn.query_pointer(root).ok()?.reply().ok()?;

    // root_x and root_y are relative to the root window (entire screen)
    Some((pointer.root_x as f64, pointer.root_y as f64))
}

#[cfg(all(not(target_os = "macos"), not(target_os = "linux")))]
fn get_mouse_position() -> Option<(f64, f64)> {
    None
}

/// Get the initial indicator position based on the current cursor location.
///
/// Returns the offset position `(x, y)` ready for `set_position()`, or `None`
/// if the mouse position cannot be determined. Coordinates may be negative
/// on multi-monitor setups where a secondary monitor is above or to the
/// left of the primary display.
pub fn get_initial_position() -> Option<(f64, f64)> {
    let (cx, cy) = get_mouse_position()?;
    let x = cx + CURSOR_OFFSET_X;
    let y = cy + CURSOR_OFFSET_Y;
    Some((x, y))
}

/// Check if the cursor is near the edge of cached monitor bounds.
fn needs_monitor_refresh(cursor_x: f64, cursor_y: f64, monitor: &CachedMonitor) -> bool {
    cursor_x < monitor.x + MONITOR_REFRESH_MARGIN
        || cursor_x > monitor.x + monitor.width - MONITOR_REFRESH_MARGIN
        || cursor_y < monitor.y + MONITOR_REFRESH_MARGIN
        || cursor_y > monitor.y + monitor.height - MONITOR_REFRESH_MARGIN
}

/// Check if the cursor is within the cached monitor bounds.
///
/// Uses the same boundary convention as `find_monitor_for_point()`:
/// inclusive min (`>=`), exclusive max (`<`).
fn is_within_monitor(cursor_x: f64, cursor_y: f64, monitor: &CachedMonitor) -> bool {
    cursor_x >= monitor.x
        && cursor_x < monitor.x + monitor.width
        && cursor_y >= monitor.y
        && cursor_y < monitor.y + monitor.height
}

/// Refresh cached monitor bounds for the given cursor position.
fn refresh_monitor_bounds(app: &AppHandle, cursor_x: f64, cursor_y: f64) -> Option<CachedMonitor> {
    let (mon_x, mon_y, mon_w, mon_h, _scale) =
        recording_indicator::find_monitor_for_point(app, cursor_x, cursor_y)?;
    Some(CachedMonitor {
        x: mon_x,
        y: mon_y,
        width: mon_w,
        height: mon_h,
    })
}

/// Clamp the indicator position to stay within monitor bounds.
fn clamp_to_monitor(x: f64, y: f64, monitor: &CachedMonitor) -> (f64, f64) {
    let indicator_w = 58.0;
    let indicator_h = 58.0;

    let clamped_x = x
        .max(monitor.x)
        .min(monitor.x + monitor.width - indicator_w);
    let clamped_y = y
        .max(monitor.y)
        .min(monitor.y + monitor.height - indicator_h - 50.0);

    (clamped_x, clamped_y)
}

/// Start tracking the mouse cursor and repositioning the indicator window.
///
/// The indicator window is made click-through during tracking. Calling this
/// when already tracking is a no-op (uses atomic compare_exchange).
pub fn start_tracking() {
    // On Wayland the compositor does not expose the global cursor position, so
    // `get_mouse_position()` returns `None` on every poll. Spawning the tracking
    // thread would burn wake-ups polling for a value it can never read and the
    // indicator would never move. Refuse to start and let the caller fall back
    // to a fixed indicator position instead.
    #[cfg(target_os = "linux")]
    if crate::shortcuts::is_wayland() {
        tracing::info!(
            "Wayland session: cursor-following indicator disabled (Wayland does not expose \
             global cursor position); using a fixed indicator position instead"
        );
        return;
    }

    let tracker = get_tracker();

    // Atomic compare_exchange to prevent double-start
    if tracker
        .read()
        .is_running
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        tracing::debug!("Mouse tracking already running, skipping start");
        return;
    }

    // Bump generation so any lingering old thread knows to exit
    let generation = tracker.read().generation.fetch_add(1, Ordering::SeqCst) + 1;

    // Reset counters and clear stale wake flag
    tracker
        .read()
        .consecutive_failures
        .store(0, Ordering::SeqCst);
    tracker.read().wake_detected.store(false, Ordering::SeqCst);

    let Some(app) = APP_HANDLE.get().cloned() else {
        tracing::warn!("Mouse tracker not initialised, cannot start tracking");
        tracker.read().is_running.store(false, Ordering::SeqCst);
        return;
    };

    // Cache the window handle up front instead of looking it up every frame
    let Some(window) = app.get_webview_window(INDICATOR_WINDOW_LABEL) else {
        tracing::warn!("Recording indicator window not found, cannot start tracking");
        tracker.read().is_running.store(false, Ordering::SeqCst);
        return;
    };

    // Make indicator window click-through
    if let Err(e) = window.set_ignore_cursor_events(true) {
        tracing::warn!("Failed to set ignore cursor events: {}", e);
        // Non-fatal: continue tracking even if click-through fails
    }

    tracing::info!(
        "Starting mouse cursor tracking for recording indicator (generation {})",
        generation
    );

    let builder = thread::Builder::new().name("mouse-tracker".to_string());
    if let Err(e) = builder.spawn(move || {
        // Drop guard ensures is_running resets even on panic or early return.
        // Only resets if this generation still owns the flag.
        let guard_window = window.clone();
        struct TrackingGuard {
            generation: u32,
            window: tauri::WebviewWindow,
        }
        impl Drop for TrackingGuard {
            fn drop(&mut self) {
                let tracker = get_tracker();
                let current_gen = tracker.read().generation.load(Ordering::SeqCst);
                if current_gen == self.generation {
                    // We still own the flag; reset it
                    tracker.read().is_running.store(false, Ordering::SeqCst);
                    // Restore cursor events
                    if let Err(e) = self.window.set_ignore_cursor_events(false) {
                        tracing::warn!("Failed to restore cursor events in guard: {}", e);
                    }
                    tracing::info!(
                        "Mouse tracking thread exited (generation {}, guard dropped)",
                        self.generation
                    );
                } else {
                    tracing::info!(
                        "Mouse tracking thread exited (generation {} superseded by {})",
                        self.generation,
                        current_gen
                    );
                }
            }
        }
        let _guard = TrackingGuard {
            generation,
            window: guard_window,
        };

        let mut last_x: f64 = f64::NAN;
        let mut last_y: f64 = f64::NAN;
        // Smoothed position (what's actually set on the window)
        let mut smooth_x: f64 = f64::NAN;
        let mut smooth_y: f64 = f64::NAN;
        let mut cached_monitor: Option<CachedMonitor> = None;
        let mut last_monitor_refresh: Option<Instant> = None;
        // Track cursor position at last TTL refresh to avoid pointless refreshes
        // when the cursor is stationary
        let mut last_refresh_cx: f64 = f64::NAN;
        let mut last_refresh_cy: f64 = f64::NAN;

        while get_tracker().read().is_running.load(Ordering::SeqCst) {
            // Check generation: if a newer thread was started, exit this one
            let current_gen = get_tracker().read().generation.load(Ordering::SeqCst);
            if current_gen != generation {
                tracing::info!(
                    "Tracking thread generation {} superseded by {}, exiting",
                    generation,
                    current_gen
                );
                break;
            }

            // Check for wake event: invalidate all caches
            if get_tracker()
                .read()
                .wake_detected
                .swap(false, Ordering::SeqCst)
            {
                tracing::info!("Wake detected in tracking loop, invalidating monitor cache");
                cached_monitor = None;
                last_monitor_refresh = None;
                last_refresh_cx = f64::NAN;
                last_refresh_cy = f64::NAN;
                // Reset smoothed position so we snap to actual cursor
                smooth_x = f64::NAN;
                smooth_y = f64::NAN;
                last_x = f64::NAN;
                last_y = f64::NAN;
                // Brief pause for CG to stabilise after wake
                thread::sleep(Duration::from_millis(200));
                continue;
            }

            if let Some((cx, cy)) = get_mouse_position() {
                // Reset failure counter on success
                let failures = get_tracker()
                    .read()
                    .consecutive_failures
                    .swap(0, Ordering::SeqCst);
                if failures > 0 {
                    tracing::info!(
                        "Mouse position polling recovered after {} failures",
                        failures
                    );
                }

                // Self-heal after a sustained failure run. A long gap of failed
                // polls is the signature of a sleep/wake (or display
                // reconfiguration): CGEvent returns nothing for hundreds of ms,
                // the indicator gets parked off-screen, and the smoothed/cached
                // state is now stale. Recovery must not depend on notify_wake()
                // firing — the wake observer is a heuristic that misses short
                // and display-only sleeps. Treat the recovery itself as a wake:
                // discard the stale position and monitor cache so the next
                // reposition snaps straight to the live cursor instead of
                // lerping from the off-screen/pre-sleep position (or never
                // moving because last_x is unchanged).
                if failures >= FAILURE_HIDE_THRESHOLD {
                    tracing::info!(
                        "Self-healing tracker state after {} failed polls (wake/reconfig)",
                        failures
                    );
                    smooth_x = f64::NAN;
                    smooth_y = f64::NAN;
                    last_x = f64::NAN;
                    last_y = f64::NAN;
                    cached_monitor = None;
                    last_monitor_refresh = None;
                    last_refresh_cx = f64::NAN;
                    last_refresh_cy = f64::NAN;
                }

                let dx = (cx - last_x).abs();
                let dy = (cy - last_y).abs();

                // First iteration (NaN) or cursor moved: reposition
                if dx > MOVE_THRESHOLD || dy > MOVE_THRESHOLD || last_x.is_nan() {
                    last_x = cx;
                    last_y = cy;

                    // Determine if monitor cache needs refreshing
                    let cache_expired = last_monitor_refresh
                        .map(|t| t.elapsed().as_millis() >= MONITOR_CACHE_TTL_MS as u128)
                        .unwrap_or(true);

                    let cursor_outside_cached = cached_monitor
                        .as_ref()
                        .is_some_and(|m| !is_within_monitor(cx, cy, m));

                    let near_edge = cached_monitor
                        .as_ref()
                        .is_some_and(|m| needs_monitor_refresh(cx, cy, m));

                    let cooldown_elapsed = last_monitor_refresh
                        .map(|t| t.elapsed().as_millis() >= MONITOR_REFRESH_COOLDOWN_MS as u128)
                        .unwrap_or(true);

                    // Has the cursor moved since we last refreshed? (for TTL)
                    let cursor_moved_since_refresh = last_refresh_cx.is_nan()
                        || (cx - last_refresh_cx).abs() > MOVE_THRESHOLD
                        || (cy - last_refresh_cy).abs() > MOVE_THRESHOLD;

                    // Refresh if:
                    // 1. No cache at all, OR
                    // 2. Cursor is outside cached bounds (immediate, bypass cooldown), OR
                    // 3. Cache TTL expired AND cursor has moved since last refresh, OR
                    // 4. Near edge AND cooldown elapsed
                    let should_refresh = cached_monitor.is_none()
                        || cursor_outside_cached
                        || (cache_expired && cursor_moved_since_refresh)
                        || (near_edge && cooldown_elapsed);

                    if should_refresh {
                        if let Some(new_monitor) = refresh_monitor_bounds(&app, cx, cy) {
                            cached_monitor = Some(new_monitor);
                        } else if cursor_outside_cached {
                            // Cursor is outside all known monitors (can happen during
                            // sleep/wake transitions). Clear cache so we don't clamp
                            // to wrong bounds.
                            cached_monitor = None;
                        }
                        last_monitor_refresh = Some(Instant::now());
                        last_refresh_cx = cx;
                        last_refresh_cy = cy;
                    }

                    // Calculate indicator position (centred above cursor)
                    let target_x = cx + CURSOR_OFFSET_X;
                    let target_y = cy + CURSOR_OFFSET_Y;

                    // Clamp to monitor bounds if available
                    let (clamped_x, clamped_y) = if let Some(ref monitor) = cached_monitor {
                        clamp_to_monitor(target_x, target_y, monitor)
                    } else {
                        (target_x, target_y)
                    };

                    // Apply smoothing (lerp). On first frame, snap instantly.
                    if smooth_x.is_nan() {
                        smooth_x = clamped_x;
                        smooth_y = clamped_y;
                    } else {
                        smooth_x += (clamped_x - smooth_x) * SMOOTHING_FACTOR;
                        smooth_y += (clamped_y - smooth_y) * SMOOTHING_FACTOR;
                    }

                    // Reposition the indicator window
                    let _ = window.set_position(tauri::Position::Logical(LogicalPosition::new(
                        smooth_x, smooth_y,
                    )));
                }
            } else {
                // Mouse position unavailable: log once, then rate-limit
                let count = get_tracker()
                    .read()
                    .consecutive_failures
                    .fetch_add(1, Ordering::SeqCst);
                if count == 0 {
                    tracing::warn!("Failed to get mouse position, indicator tracking paused");
                } else if count == 100 {
                    tracing::warn!("Mouse position polling has failed 100 consecutive times");
                }

                // After sustained failures, move indicator off-screen to avoid
                // it being stuck in a stale position
                if count == FAILURE_HIDE_THRESHOLD {
                    tracing::warn!(
                        "Mouse position unavailable for {} polls, moving indicator off-screen",
                        FAILURE_HIDE_THRESHOLD
                    );
                    let _ = window.set_position(tauri::Position::Logical(LogicalPosition::new(
                        -10000.0, -10000.0,
                    )));
                }
            }

            thread::sleep(Duration::from_millis(POLL_INTERVAL_MS));
        }

        // _guard drops here, resetting is_running if generation still matches
    }) {
        // thread::spawn failed: reset is_running so future calls can try again
        tracing::error!("Failed to spawn mouse tracking thread: {}", e);
        tracker.read().is_running.store(false, Ordering::SeqCst);
        // Restore cursor events since we set click-through before spawn
        if let Some(app_handle) = APP_HANDLE.get()
            && let Some(window) = app_handle.get_webview_window(INDICATOR_WINDOW_LABEL)
            && let Err(e2) = window.set_ignore_cursor_events(false)
        {
            tracing::warn!(
                "Failed to restore cursor events after spawn failure: {}",
                e2
            );
        }
    }
}

/// Stop tracking the mouse cursor.
///
/// The indicator window is restored to accept cursor events.
pub fn stop_tracking() {
    let tracker = get_tracker();
    let was_running = tracker.read().is_running.swap(false, Ordering::SeqCst);

    if !was_running {
        return;
    }

    tracing::info!("Stopping mouse cursor tracking");

    // Restore cursor events on the indicator window
    if let Some(app) = APP_HANDLE.get()
        && let Some(window) = app.get_webview_window(INDICATOR_WINDOW_LABEL)
        && let Err(e) = window.set_ignore_cursor_events(false)
    {
        tracing::warn!("Failed to restore cursor events: {}", e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clamp_to_monitor() {
        let monitor = CachedMonitor {
            x: 0.0,
            y: 0.0,
            width: 1920.0,
            height: 1080.0,
        };

        // Normal position (within bounds)
        let (x, y) = clamp_to_monitor(500.0, 300.0, &monitor);
        assert_eq!(x, 500.0);
        assert_eq!(y, 300.0);

        // Left edge
        let (x, _y) = clamp_to_monitor(-20.0, 300.0, &monitor);
        assert_eq!(x, 0.0);

        // Right edge (58px indicator width)
        let (x, _y) = clamp_to_monitor(1900.0, 300.0, &monitor);
        assert_eq!(x, 1862.0); // 1920 - 58

        // Top edge
        let (_x, y) = clamp_to_monitor(500.0, -20.0, &monitor);
        assert_eq!(y, 0.0);

        // Bottom edge (58px indicator height + 50px dock padding)
        let (_x, y) = clamp_to_monitor(500.0, 1050.0, &monitor);
        assert_eq!(y, 972.0); // 1080 - 58 - 50
    }

    #[test]
    fn test_needs_monitor_refresh() {
        let monitor = CachedMonitor {
            x: 0.0,
            y: 0.0,
            width: 1920.0,
            height: 1080.0,
        };

        // Centre of screen: no refresh needed
        assert!(!needs_monitor_refresh(960.0, 540.0, &monitor));

        // Near left edge: refresh needed
        assert!(needs_monitor_refresh(50.0, 540.0, &monitor));

        // Near right edge: refresh needed
        assert!(needs_monitor_refresh(1870.0, 540.0, &monitor));

        // Near top edge: refresh needed
        assert!(needs_monitor_refresh(960.0, 50.0, &monitor));

        // Near bottom edge: refresh needed
        assert!(needs_monitor_refresh(960.0, 1030.0, &monitor));
    }

    #[test]
    fn test_is_within_monitor() {
        let monitor = CachedMonitor {
            x: 0.0,
            y: 0.0,
            width: 1920.0,
            height: 1080.0,
        };

        // Centre: within
        assert!(is_within_monitor(960.0, 540.0, &monitor));

        // Origin: within (inclusive min)
        assert!(is_within_monitor(0.0, 0.0, &monitor));

        // Just outside right edge (exclusive max)
        assert!(!is_within_monitor(1920.0, 540.0, &monitor));

        // Just outside bottom edge
        assert!(!is_within_monitor(960.0, 1080.0, &monitor));

        // Negative coordinates (outside)
        assert!(!is_within_monitor(-1.0, 540.0, &monitor));

        // Secondary monitor with negative origin
        let secondary = CachedMonitor {
            x: -1920.0,
            y: 0.0,
            width: 1920.0,
            height: 1080.0,
        };
        assert!(is_within_monitor(-960.0, 540.0, &secondary));
        assert!(!is_within_monitor(0.0, 540.0, &secondary)); // exclusive max
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_get_mouse_position_returns_some() {
        // Skip in headless/CI environments where no GUI session exists.
        // CGEvent requires an active HID session (WindowServer).
        if std::env::var("CI").is_ok() || std::env::var("GITHUB_ACTIONS").is_ok() {
            eprintln!("Skipping test_get_mouse_position_returns_some in CI (no GUI session)");
            return;
        }

        let pos = get_mouse_position();
        assert!(
            pos.is_some(),
            "get_mouse_position should return a position on macOS with an active GUI session"
        );
        let (x, y) = pos.unwrap();
        // On multi-monitor setups, cursor position can be negative when
        // a secondary monitor is above or to the left of the primary.
        assert!(x.is_finite(), "x should be finite, got {}", x);
        assert!(y.is_finite(), "y should be finite, got {}", y);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_get_mouse_position_returns_some_on_x11() {
        // On Linux with X11, this should succeed if DISPLAY is set
        // May return None on Wayland without XWayland
        if std::env::var("DISPLAY").is_ok() {
            let pos = get_mouse_position();
            assert!(
                pos.is_some(),
                "get_mouse_position should return a position on Linux X11"
            );
            let (x, y) = pos.unwrap();
            assert!(x.is_finite(), "x should be finite, got {}", x);
            assert!(y.is_finite(), "y should be finite, got {}", y);
        }
    }
}
