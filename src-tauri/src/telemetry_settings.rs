//! The Telemetry section of Settings: where this device sends its telemetry.
//!
//! A Dock-launched app inherits no fleet environment, so on a machine the fleet
//! does not configure the pane is the only door. Where
//! `OTEL_EXPORTER_OTLP_ENDPOINT` is set the environment wins:
//! `Guard::set_exporter` is a no-op there and the pane shows the values
//! read-only.
//!
//! The shared crate holds the whole export pipeline; this module only reads the
//! saved values, hands them over, and reports what came back. No header value is
//! returned, logged or stored — only the helper command that prints them.

use crate::TELEMETRY_TARGET;
use std::sync::Mutex;

use serde::Serialize;
use tauri::Manager;

use crate::config::TelemetryConfig;
use crate::error::Error;

/// The saved values as the crate's own type. An empty endpoint is no exporter,
/// and an empty helper is no helper.
fn exporter_of(cfg: &TelemetryConfig) -> Option<telemetry::Exporter> {
    let endpoint = cfg.endpoint.trim();
    if endpoint.is_empty() {
        return None;
    }
    let helper = cfg.headers_helper.trim();
    Some(telemetry::Exporter {
        endpoint: endpoint.to_owned(),
        headers_helper: (!helper.is_empty()).then(|| helper.to_owned()),
    })
}

/// Run `f` against the process's `Guard`, or return `None` where telemetry was
/// never installed (a unit test, or a second `init`).
fn with_guard<R>(app: &tauri::AppHandle, f: impl FnOnce(&telemetry::Guard) -> R) -> Option<R> {
    let state = app.try_state::<Mutex<Option<telemetry::Guard>>>()?;
    let guard = state.lock().unwrap_or_else(|e| e.into_inner());
    guard.as_ref().map(f)
}

/// Point the live log and span export at `cfg`, or at nothing.
///
/// Blocks while the header helper runs (up to ten seconds in the crate), so
/// callers reach it from a plain thread or `spawn_blocking`, never from a Tokio
/// worker or the main thread.
fn install(app: &tauri::AppHandle, cfg: &TelemetryConfig) {
    with_guard(app, |guard| guard.set_exporter(exporter_of(cfg)));
}

/// Apply the saved endpoint at startup, unless the environment already set one.
///
/// Spawned, because the header helper is a subprocess and the app is starting.
pub fn apply_saved(app: &tauri::AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        if with_guard(&app, telemetry::Guard::exporter_is_from_env) != Some(false) {
            return;
        }
        let Ok(cfg) = crate::config::get_config() else {
            return;
        };
        if cfg.telemetry.endpoint.trim().is_empty() {
            return;
        }
        install(&app, &cfg.telemetry);
    });
}

/// What the Telemetry card shows: the live exporter, whether the environment
/// owns it, and what the settings file holds underneath.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TelemetryStatus {
    /// The endpoint this process is exporting to; empty means local only.
    pub endpoint: String,
    /// The helper command behind it; empty means none.
    pub headers_helper: String,
    /// True when the environment set it, so the pane shows it read-only.
    pub from_env: bool,
    /// The saved endpoint, which the environment overrides where it is set.
    pub saved_endpoint: String,
    /// The saved helper command.
    pub saved_headers_helper: String,
}

/// Report the live exporter and the saved values.
#[tauri::command]
#[tracing::instrument(target = TELEMETRY_TARGET, skip_all, err)]
pub fn telemetry_get(app: tauri::AppHandle) -> Result<TelemetryStatus, Error> {
    let saved = crate::config::get_config()?.telemetry;
    let (live, from_env) =
        with_guard(&app, |g| (g.exporter(), g.exporter_is_from_env())).unwrap_or((None, false));
    Ok(TelemetryStatus {
        endpoint: live
            .as_ref()
            .map(|e| e.endpoint.clone())
            .unwrap_or_default(),
        headers_helper: live.and_then(|e| e.headers_helper).unwrap_or_default(),
        from_env,
        saved_endpoint: saved.endpoint,
        saved_headers_helper: saved.headers_helper,
    })
}

/// Save the endpoint and helper, then point the live exporter at them.
#[tauri::command]
#[tracing::instrument(target = TELEMETRY_TARGET, skip_all, err)]
pub async fn telemetry_set(
    app: tauri::AppHandle,
    endpoint: String,
    headers_helper: String,
) -> Result<TelemetryStatus, Error> {
    let cfg = TelemetryConfig {
        endpoint: endpoint.trim().to_owned(),
        headers_helper: headers_helper.trim().to_owned(),
    };
    crate::config::set_telemetry_config(cfg.clone())?;

    // `set_exporter` blocks while the header helper runs, and this is an async
    // command on a Tokio worker.
    let handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || install(&handle, &cfg))
        .await
        .map_err(|e| Error::Other(format!("Failed to apply the telemetry endpoint: {e}")))?;

    telemetry_get(app)
}

/// Send one record to the values given and report whether they were accepted,
/// without saving anything.
///
/// The error is the crate's own class or HTTP status — never a URL, a header
/// value or a response body.
#[tauri::command]
#[tracing::instrument(target = TELEMETRY_TARGET, skip_all, err)]
pub async fn telemetry_probe(endpoint: String, headers_helper: String) -> Result<(), Error> {
    let cfg = TelemetryConfig {
        endpoint,
        headers_helper,
    };
    let exporter = exporter_of(&cfg).ok_or_else(|| Error::Other("No endpoint to test".into()))?;

    // `probe` blocks until the collector answers or the OTLP timeout expires.
    tauri::async_runtime::spawn_blocking(move || telemetry::probe(&exporter))
        .await
        .map_err(|e| Error::Other(format!("The test did not run: {e}")))?
        .map_err(|e| Error::Other(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_endpoint_is_no_exporter() {
        assert!(exporter_of(&TelemetryConfig::default()).is_none());
        assert!(
            exporter_of(&TelemetryConfig {
                endpoint: "   ".into(),
                headers_helper: "signet headers otlp".into(),
            })
            .is_none()
        );
    }

    #[test]
    fn an_empty_helper_is_no_helper() {
        let exporter = exporter_of(&TelemetryConfig {
            endpoint: "https://otlp.example ".into(),
            headers_helper: "  ".into(),
        })
        .expect("an endpoint is an exporter");
        assert_eq!(exporter.endpoint, "https://otlp.example");
        assert_eq!(exporter.headers_helper, None);
    }

    #[test]
    fn a_helper_carries_the_command_only() {
        let exporter = exporter_of(&TelemetryConfig {
            endpoint: "https://otlp.example".into(),
            headers_helper: " signet headers otlp ".into(),
        })
        .expect("an endpoint is an exporter");
        assert_eq!(
            exporter.headers_helper.as_deref(),
            Some("signet headers otlp")
        );
    }
}
