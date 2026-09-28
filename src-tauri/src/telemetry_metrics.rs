//! Process RSS/CPU sampling for telemetry.
//!
//! Own-process only — never a whole-system scan — sampled every
//! [`SAMPLE_INTERVAL`] plus right after a model load and right after each
//! transcription, so a memory or CPU trend is visible without the operator
//! having to reproduce it live. The `System` is kept across samples so
//! `cpu_usage()` reads a real delta rather than the meaningless first-call
//! zero sysinfo returns before it has two readings to compare.

use crate::TELEMETRY_TARGET;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::Duration;
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

/// How often the idle sampler takes a reading.
const SAMPLE_INTERVAL: Duration = Duration::from_secs(60);

fn system() -> &'static Mutex<System> {
    static SYSTEM: OnceLock<Mutex<System>> = OnceLock::new();
    SYSTEM.get_or_init(|| Mutex::new(System::new()))
}

/// Refresh this process's own memory and CPU, and emit one `process_metrics`
/// event. `reason` says what triggered the sample (`"interval"`,
/// `"model_loaded"`, `"transcription_complete"`) so a spike can be attributed
/// to a phase rather than only a point in time.
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

/// Spawn the periodic sampler. Call once, at startup.
pub fn spawn_periodic_sampler() {
    tauri::async_runtime::spawn(async {
        loop {
            tokio::time::sleep(SAMPLE_INTERVAL).await;
            sample("interval");
        }
    });
}
