//! Regression tests for the MCP `transcription` tool — issue #82.
//!
//! The `list` action once returned `[]` despite thousands of records. These
//! tests drive the real database-backed functions the MCP actions call:
//!
//!   • `list`  → `database::list_transcriptions(...)`  (primary assertion; this
//!               is the function `mcp_server/mod.rs` calls for the `list` action)
//!   • `stats` → `database::transcription::get_transcription_stats_cmd()`
//!
//! and assert that `list` returns seeded records and that both counts
//! agree (the consistency the issue asked us to guard).
//!
//! The whole database layer talks to a process-global connection path, so the
//! suite runs in its own integration-test binary and points `THOTH_DATA_DIR`
//! at a throwaway directory. All assertions live in one test so the single
//! process-global path is set exactly once.

use tempfile::TempDir;
use thoth_lib::database::{self, Transcription};

#[test]
fn list_returns_seeded_records_and_matches_stats() {
    // Point the database at a throwaway dir BEFORE any connection is opened.
    let temp = TempDir::new().expect("create temp dir");
    // SAFETY: single-threaded test binary; no other threads read this var.
    unsafe { std::env::set_var("THOTH_DATA_DIR", temp.path()) };

    database::initialise_database().expect("initialise database");

    // Seed a handful of records via the normal create path.
    const SEEDED: usize = 5;
    for i in 0..SEEDED {
        let record = Transcription::new(format!("regression record number {i}"));
        database::create_transcription(&record).expect("insert transcription");
    }

    // Primary assertion (#82 guard) — `list` path via the function MCP actually calls.
    let listed =
        database::list_transcriptions(None, None).expect("list_transcriptions should succeed");
    assert_eq!(
        listed.len(),
        SEEDED,
        "list returned {} records, expected {SEEDED}",
        listed.len()
    );

    // `stats` path — count must be consistent with `list`.
    let stats = database::transcription::get_transcription_stats_cmd()
        .expect("get_transcription_stats_cmd (stats) should succeed");
    assert_eq!(stats.total_count, SEEDED, "stats total_count mismatch");

    assert_eq!(
        stats.total_count,
        listed.len(),
        "list and stats counts disagree"
    );

    // Keep the temp dir alive until every query has run.
    drop(temp);
}
