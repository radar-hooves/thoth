//! Curated process RSS/CPU samples, right after a model load and right after
//! each transcription, on top of the plugin's own interval sampler
//! (`telemetry::sample_process_metrics`, spawned by `tauri_plugin_telemetry`).
//!
//! Own-process only — never a whole-system scan. The `System` is kept across
//! samples so `cpu_usage()` reads a real delta rather than the meaningless
//! first-call zero sysinfo returns before it has two readings to compare.

use crate::TELEMETRY_TARGET;
use std::sync::Mutex;
use std::sync::OnceLock;
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

fn system() -> &'static Mutex<System> {
    static SYSTEM: OnceLock<Mutex<System>> = OnceLock::new();
    SYSTEM.get_or_init(|| Mutex::new(System::new()))
}

/// Refresh this process's own memory and CPU, and emit one `process_metrics`
/// event. `reason` says what triggered the sample (`"model_loaded"`,
/// `"transcription_complete"`) so a spike can be attributed to a phase rather
/// than only a point in time.
pub fn sample(reason: &'static str) {
    let pid = Pid::from_u32(std::process::id());
    let mut sys = system().lock().unwrap_or_else(|e| e.into_inner());
    sys.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]),
        false,
        ProcessRefreshKind::nothing().with_memory().with_cpu(),
    );
    let Some(process) = sys.process(pid) else {
        return;
    };
    tracing::info!(
        target: TELEMETRY_TARGET,
        rss_bytes = process.memory(),
        cpu_percent = process.cpu_usage(),
        reason,
        "process_metrics"
    );
}
