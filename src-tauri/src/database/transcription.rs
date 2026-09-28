//! Transcription CRUD operations.
//!
//! Provides functions for creating, reading, updating, and deleting transcriptions
//! in the SQLite database.

use chrono::Utc;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::database::{DatabaseError, open_connection};
use crate::error::Error;

/// A transcription record stored in the database.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Transcription {
    /// Unique identifier (UUID).
    pub id: String,
    /// The transcribed text (possibly enhanced).
    pub text: String,
    /// Original text before enhancement (if any).
    pub raw_text: Option<String>,
    /// Duration of the audio in seconds.
    pub duration_seconds: Option<f64>,
    /// When the transcription was created (ISO 8601).
    pub created_at: String,
    /// Path to the audio file (if retained).
    pub audio_path: Option<String>,
    /// Whether the text has been enhanced by AI.
    pub is_enhanced: bool,
    /// Which enhancement prompt was used (if enhanced).
    pub enhancement_prompt: Option<String>,
    /// Name of the transcription model used (e.g., "ggml-large-v3-turbo").
    pub transcription_model_name: Option<String>,
    /// Time taken to transcribe the audio, in seconds.
    pub transcription_duration_seconds: Option<f64>,
    /// Name of the AI model used for enhancement (e.g., "llama3.2:3b").
    pub enhancement_model_name: Option<String>,
    /// Time taken for AI enhancement, in seconds.
    pub enhancement_duration_seconds: Option<f64>,
}

impl Transcription {
    /// Creates a new transcription with a generated UUID and current timestamp.
    pub fn new(text: String) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            text,
            raw_text: None,
            duration_seconds: None,
            created_at: Utc::now().to_rfc3339(),
            audio_path: None,
            is_enhanced: false,
            enhancement_prompt: None,
            transcription_model_name: None,
            transcription_duration_seconds: None,
            enhancement_model_name: None,
            enhancement_duration_seconds: None,
        }
    }

    /// Creates a new transcription with all fields specified.
    #[allow(clippy::too_many_arguments)]
    pub fn with_details(
        text: String,
        raw_text: Option<String>,
        duration_seconds: Option<f64>,
        audio_path: Option<String>,
        is_enhanced: bool,
        enhancement_prompt: Option<String>,
        transcription_model_name: Option<String>,
        transcription_duration_seconds: Option<f64>,
        enhancement_model_name: Option<String>,
        enhancement_duration_seconds: Option<f64>,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            text,
            raw_text,
            duration_seconds,
            created_at: Utc::now().to_rfc3339(),
            audio_path,
            is_enhanced,
            enhancement_prompt,
            transcription_model_name,
            transcription_duration_seconds,
            enhancement_model_name,
            enhancement_duration_seconds,
        }
    }
}

// =============================================================================
// Database Functions
// =============================================================================

/// Creates a new transcription in the database.
pub fn create_transcription(transcription: &Transcription) -> Result<(), DatabaseError> {
    let conn = open_connection()?;

    conn.execute(
        r#"
        INSERT INTO transcriptions (
            id, text, raw_text, duration_seconds, created_at, audio_path,
            is_enhanced, enhancement_prompt,
            transcription_model_name, transcription_duration_seconds,
            enhancement_model_name, enhancement_duration_seconds
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
        "#,
        params![
            transcription.id,
            transcription.text,
            transcription.raw_text,
            transcription.duration_seconds,
            transcription.created_at,
            transcription.audio_path,
            transcription.is_enhanced as i32,
            transcription.enhancement_prompt,
            transcription.transcription_model_name,
            transcription.transcription_duration_seconds,
            transcription.enhancement_model_name,
            transcription.enhancement_duration_seconds,
        ],
    )?;

    tracing::debug!("Created transcription: {}", transcription.id);
    Ok(())
}

/// Column list for all SELECT queries.
const SELECT_COLUMNS: &str = r#"
    id, text, raw_text, duration_seconds, created_at, audio_path,
    is_enhanced, enhancement_prompt,
    transcription_model_name, transcription_duration_seconds,
    enhancement_model_name, enhancement_duration_seconds
"#;

/// Map a database row to a Transcription struct.
fn row_to_transcription(row: &rusqlite::Row) -> rusqlite::Result<Transcription> {
    Ok(Transcription {
        id: row.get(0)?,
        text: row.get(1)?,
        raw_text: row.get(2)?,
        duration_seconds: row.get(3)?,
        created_at: row.get(4)?,
        audio_path: row.get(5)?,
        is_enhanced: row.get::<_, i32>(6)? != 0,
        enhancement_prompt: row.get(7)?,
        transcription_model_name: row.get(8)?,
        transcription_duration_seconds: row.get(9)?,
        enhancement_model_name: row.get(10)?,
        enhancement_duration_seconds: row.get(11)?,
    })
}

/// Retrieves a transcription by its ID.
pub fn get_transcription(id: &str) -> Result<Option<Transcription>, DatabaseError> {
    let conn = open_connection()?;

    let result = conn.query_row(
        &format!(
            "SELECT {} FROM transcriptions WHERE id = ?1",
            SELECT_COLUMNS
        ),
        params![id],
        row_to_transcription,
    );

    match result {
        Ok(transcription) => Ok(Some(transcription)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Updates an existing transcription.
pub fn update_transcription(transcription: &Transcription) -> Result<(), DatabaseError> {
    let conn = open_connection()?;

    let rows_affected = conn.execute(
        r#"
        UPDATE transcriptions
        SET text = ?2, raw_text = ?3, duration_seconds = ?4, audio_path = ?5,
            is_enhanced = ?6, enhancement_prompt = ?7,
            transcription_model_name = ?8, transcription_duration_seconds = ?9,
            enhancement_model_name = ?10, enhancement_duration_seconds = ?11
        WHERE id = ?1
        "#,
        params![
            transcription.id,
            transcription.text,
            transcription.raw_text,
            transcription.duration_seconds,
            transcription.audio_path,
            transcription.is_enhanced as i32,
            transcription.enhancement_prompt,
            transcription.transcription_model_name,
            transcription.transcription_duration_seconds,
            transcription.enhancement_model_name,
            transcription.enhancement_duration_seconds,
        ],
    )?;

    if rows_affected == 0 {
        tracing::warn!("No transcription found with id: {}", transcription.id);
    } else {
        tracing::debug!("Updated transcription: {}", transcription.id);
    }

    Ok(())
}

/// Deletes a transcription by its ID, removing its audio file when it is the
/// sole DB reference to that path.
pub fn delete_transcription(id: &str) -> Result<bool, DatabaseError> {
    let mut conn = open_connection()?;
    delete_transcription_with_conn(&mut conn, id)
}

/// Inner implementation that accepts an existing connection (enables testing
/// against an in-memory DB without touching the global DATABASE_PATH).
fn delete_transcription_with_conn(
    conn: &mut rusqlite::Connection,
    id: &str,
) -> Result<bool, DatabaseError> {
    // IMMEDIATE locks the DB writer slot up-front, closing the TOCTOU window
    // between the audio_path read, the DELETE, and the post-delete ref count.
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;

    // Read the audio_path before deletion.
    let audio_path: Option<String> = tx
        .query_row(
            "SELECT audio_path FROM transcriptions WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )
        .optional()?
        .flatten();

    let rows_affected = tx.execute("DELETE FROM transcriptions WHERE id = ?1", params![id])?;

    // Count remaining references AFTER the delete, inside the same transaction,
    // so the file-removal decision is race-free.
    let remaining_refs: i64 = if let Some(ref path) = audio_path {
        tx.query_row(
            "SELECT COUNT(*) FROM transcriptions WHERE audio_path = ?1",
            params![path],
            |row| row.get(0),
        )?
    } else {
        0
    };

    tx.commit()?;

    if rows_affected == 0 {
        tracing::warn!("No transcription found with id: {}", id);
        return Ok(false);
    }

    tracing::debug!("Deleted transcription: {}", id);

    // Remove the audio file only when no DB row still references it.
    // Removal is best-effort; any file left behind is recovered by
    // reconcile_orphaned_recordings on the next explicit sweep.
    if let Some(path) = audio_path {
        if remaining_refs == 0 {
            match std::fs::remove_file(&path) {
                Ok(()) => tracing::debug!("Removed audio file: {}", path),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    tracing::warn!("Audio file already gone: {}", path);
                }
                Err(e) => tracing::warn!("Failed to remove audio file {}: {}", path, e),
            }
        } else {
            tracing::debug!(
                "Audio file still referenced by {} other rows; not removing: {}",
                remaining_refs,
                path
            );
        }
    }

    Ok(true)
}

/// Deletes all transcriptions from the database and removes their audio files.
pub fn delete_all_transcriptions() -> Result<usize, DatabaseError> {
    let mut conn = open_connection()?;
    delete_all_transcriptions_with_conn(&mut conn)
}

/// Inner implementation that accepts an existing connection (enables testing
/// against an in-memory DB without touching the global DATABASE_PATH).
fn delete_all_transcriptions_with_conn(
    conn: &mut rusqlite::Connection,
) -> Result<usize, DatabaseError> {
    // IMMEDIATE transaction: collect paths and delete rows atomically.
    let audio_paths: Vec<String>;
    let rows_affected: usize;

    {
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;

        let mut stmt = tx.prepare(
            "SELECT DISTINCT audio_path FROM transcriptions WHERE audio_path IS NOT NULL",
        )?;
        audio_paths = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        drop(stmt);

        rows_affected = tx.execute("DELETE FROM transcriptions", [])?;
        tx.commit()?;
    }

    tracing::info!("Deleted all transcriptions ({} rows)", rows_affected);

    // Filesystem removal is best-effort after commit; any file that survives
    // a crash here will be cleaned up by reconcile_orphaned_recordings.
    for path in &audio_paths {
        match std::fs::remove_file(path) {
            Ok(()) => tracing::debug!("Removed audio file: {}", path),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                tracing::warn!("Audio file already gone: {}", path);
            }
            Err(e) => tracing::warn!("Failed to remove audio file {}: {}", path, e),
        }
    }

    Ok(rows_affected)
}

/// Returns the recordings directory (~/.thoth/Recordings/).
fn recordings_dir() -> Option<std::path::PathBuf> {
    dirs::home_dir().map(|h| h.join(".thoth").join("Recordings"))
}

/// Scans ~/.thoth/Recordings/ for WAV files that are not referenced by any
/// DB row and removes them, returning a count and bytes freed.
pub fn reconcile_orphaned_recordings() -> Result<ReconcileResult, DatabaseError> {
    let conn = open_connection()?;
    let dir = match recordings_dir() {
        Some(d) => d,
        None => {
            return Ok(ReconcileResult {
                removed_count: 0,
                bytes_freed: 0,
            });
        }
    };
    reconcile_orphaned_recordings_with_conn(&conn, &dir)
}

/// Inner implementation used by tests.
///
/// `dir` is the recordings directory to scan. Passing it explicitly avoids the
/// footgun where a test or in-memory connection would scan (and potentially
/// delete files from) the real ~/.thoth/Recordings/ directory.
fn reconcile_orphaned_recordings_with_conn(
    conn: &rusqlite::Connection,
    dir: &std::path::Path,
) -> Result<ReconcileResult, DatabaseError> {
    if !dir.exists() {
        return Ok(ReconcileResult {
            removed_count: 0,
            bytes_freed: 0,
        });
    }

    // Build canonicalised set of DB-referenced audio paths.
    // Canonicalisation normalises trailing slashes, symlinks, and
    // home-directory representations so a referenced file is never
    // mistakenly identified as an orphan due to path format differences.
    let mut stmt = conn
        .prepare("SELECT DISTINCT audio_path FROM transcriptions WHERE audio_path IS NOT NULL")?;
    let referenced: std::collections::HashSet<std::path::PathBuf> = stmt
        .query_map([], |row| row.get::<_, String>(0))?
        .filter_map(|r| r.ok())
        .filter_map(|s| std::fs::canonicalize(&s).ok())
        .collect();

    let mut removed_count: u32 = 0;
    let mut bytes_freed: u64 = 0;

    let entries = std::fs::read_dir(dir).map_err(DatabaseError::DirectoryRead)?;
    for entry in entries.flatten() {
        let path = entry.path();
        let is_wav = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("wav"))
            .unwrap_or(false);
        if !is_wav || !path.is_file() {
            continue;
        }

        // Canonicalise the disk path before comparing. If canonicalisation
        // fails (e.g. a broken symlink), skip the entry — never delete on doubt.
        let canon = match std::fs::canonicalize(&path) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(
                    "Skipping unresolvable path during reconcile ({}): {}",
                    path.display(),
                    e
                );
                continue;
            }
        };

        if referenced.contains(&canon) {
            continue;
        }

        let path_str = path.to_string_lossy().to_string();
        let size = path.metadata().map(|m| m.len()).unwrap_or(0);
        // Removal is best-effort; a crash here leaves the file as an orphan
        // that will be cleaned on the next reconcile call.
        match std::fs::remove_file(&path) {
            Ok(()) => {
                tracing::info!("Orphan removed: {}", path_str);
                removed_count += 1;
                bytes_freed += size;
            }
            Err(e) => tracing::warn!("Failed to remove orphan {}: {}", path_str, e),
        }
    }

    Ok(ReconcileResult {
        removed_count,
        bytes_freed,
    })
}

/// Result returned by `reconcile_orphaned_recordings`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReconcileResult {
    /// Number of orphaned WAV files removed.
    pub removed_count: u32,
    /// Total bytes freed.
    pub bytes_freed: u64,
}

/// Lists all transcriptions, ordered by creation date (newest first).
pub fn list_transcriptions(
    limit: Option<i64>,
    offset: Option<i64>,
) -> Result<Vec<Transcription>, DatabaseError> {
    let conn = open_connection()?;
    list_transcriptions_with_conn(&conn, limit, offset)
}

/// Inner implementation that accepts an existing connection (enables testing
/// against an in-memory DB without touching the global DATABASE_PATH).
fn list_transcriptions_with_conn(
    conn: &rusqlite::Connection,
    limit: Option<i64>,
    offset: Option<i64>,
) -> Result<Vec<Transcription>, DatabaseError> {
    let limit = limit.unwrap_or(100);
    let offset = offset.unwrap_or(0);

    let mut stmt = conn.prepare(&format!(
        "SELECT {} FROM transcriptions ORDER BY created_at DESC LIMIT ?1 OFFSET ?2",
        SELECT_COLUMNS
    ))?;

    let transcriptions = stmt
        .query_map(params![limit, offset], row_to_transcription)?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(transcriptions)
}

/// Searches transcriptions using LIKE pattern matching on the text field.
pub fn search_transcriptions(
    query: &str,
    limit: Option<i64>,
) -> Result<Vec<Transcription>, DatabaseError> {
    let conn = open_connection()?;

    let limit = limit.unwrap_or(50);
    let search_pattern = format!("%{}%", query);

    let mut stmt = conn.prepare(&format!(
        "SELECT {} FROM transcriptions WHERE text LIKE ?1 ORDER BY created_at DESC LIMIT ?2",
        SELECT_COLUMNS
    ))?;

    let transcriptions = stmt
        .query_map(params![search_pattern, limit], row_to_transcription)?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(transcriptions)
}

/// Counts the total number of transcriptions, optionally filtered by a search query.
pub fn count_transcriptions(query: Option<&str>) -> Result<usize, DatabaseError> {
    let conn = open_connection()?;
    count_transcriptions_with_conn(&conn, query)
}

/// Inner implementation that accepts an existing connection (enables testing
/// against an in-memory DB without touching the global DATABASE_PATH).
fn count_transcriptions_with_conn(
    conn: &rusqlite::Connection,
    query: Option<&str>,
) -> Result<usize, DatabaseError> {
    let count: i64 = match query {
        Some(q) if !q.is_empty() => {
            let search_pattern = format!("%{}%", q);
            conn.query_row(
                "SELECT COUNT(*) FROM transcriptions WHERE text LIKE ?1",
                params![search_pattern],
                |row| row.get(0),
            )?
        }
        _ => conn.query_row("SELECT COUNT(*) FROM transcriptions", [], |row| row.get(0))?,
    };

    Ok(count as usize)
}

// =============================================================================
// Statistics
// =============================================================================

/// Aggregated statistics for the performance analysis dashboard.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptionStats {
    /// Total number of transcriptions.
    pub total_count: usize,
    /// Number of transcriptions with performance metadata.
    pub analysable_count: usize,
    /// Number of enhanced transcriptions.
    pub enhanced_count: usize,
    /// Total audio duration across all transcriptions (seconds).
    pub total_audio_duration: f64,
    /// Per-model transcription performance stats.
    pub transcription_models: Vec<ModelStats>,
    /// Per-model enhancement performance stats.
    pub enhancement_models: Vec<ModelStats>,
}

/// Performance stats for a single model.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelStats {
    /// Model name.
    pub name: String,
    /// Number of transcriptions using this model.
    pub count: usize,
    /// Average audio duration (seconds).
    pub avg_audio_duration: f64,
    /// Average processing time (seconds).
    pub avg_processing_time: f64,
    /// Real-time factor (audio duration / processing time). Higher is faster.
    pub speed_factor: f64,
}

/// Computes aggregated transcription statistics.
pub fn get_transcription_stats() -> Result<TranscriptionStats, DatabaseError> {
    let conn = open_connection()?;

    // Summary counts
    let total_count: i64 =
        conn.query_row("SELECT COUNT(*) FROM transcriptions", [], |row| row.get(0))?;

    let analysable_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM transcriptions WHERE transcription_duration_seconds IS NOT NULL",
        [],
        |row| row.get(0),
    )?;

    let enhanced_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM transcriptions WHERE is_enhanced = 1",
        [],
        |row| row.get(0),
    )?;

    let total_audio_duration: f64 = conn
        .query_row(
            "SELECT COALESCE(SUM(duration_seconds), 0) FROM transcriptions",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0.0);

    // Per-model transcription stats
    let mut stmt = conn.prepare(
        r#"
        SELECT
            transcription_model_name,
            COUNT(*) as cnt,
            COALESCE(AVG(duration_seconds), 0) as avg_audio,
            AVG(transcription_duration_seconds) as avg_proc
        FROM transcriptions
        WHERE transcription_model_name IS NOT NULL
          AND transcription_duration_seconds IS NOT NULL
        GROUP BY transcription_model_name
        ORDER BY cnt DESC
        "#,
    )?;

    let transcription_models = stmt
        .query_map([], |row| {
            let name: String = row.get(0)?;
            let count: i64 = row.get(1)?;
            let avg_audio: f64 = row.get(2)?;
            let avg_proc: f64 = row.get(3)?;
            let speed_factor = if avg_proc > 0.0 {
                avg_audio / avg_proc
            } else {
                0.0
            };
            Ok(ModelStats {
                name,
                count: count as usize,
                avg_audio_duration: avg_audio,
                avg_processing_time: avg_proc,
                speed_factor,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    // Per-model enhancement stats
    let mut stmt = conn.prepare(
        r#"
        SELECT
            enhancement_model_name,
            COUNT(*) as cnt,
            COALESCE(AVG(duration_seconds), 0) as avg_audio,
            AVG(enhancement_duration_seconds) as avg_proc
        FROM transcriptions
        WHERE enhancement_model_name IS NOT NULL
          AND enhancement_duration_seconds IS NOT NULL
        GROUP BY enhancement_model_name
        ORDER BY cnt DESC
        "#,
    )?;

    let enhancement_models = stmt
        .query_map([], |row| {
            let name: String = row.get(0)?;
            let count: i64 = row.get(1)?;
            let avg_audio: f64 = row.get(2)?;
            let avg_proc: f64 = row.get(3)?;
            Ok(ModelStats {
                name,
                count: count as usize,
                avg_audio_duration: avg_audio,
                avg_processing_time: avg_proc,
                speed_factor: 0.0, // Not meaningful for enhancement
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(TranscriptionStats {
        total_count: total_count as usize,
        analysable_count: analysable_count as usize,
        enhanced_count: enhanced_count as usize,
        total_audio_duration,
        transcription_models,
        enhancement_models,
    })
}

// =============================================================================
// Tauri Commands
// =============================================================================

/// Returns aggregated transcription statistics for the performance dashboard.
#[tauri::command]
pub fn get_transcription_stats_cmd() -> Result<TranscriptionStats, Error> {
    get_transcription_stats()
        .map_err(|e| {
            tracing::error!("Failed to get transcription stats: {}", e);
            format!("Failed to get stats: {}", e)
        })
        .map_err(Into::into)
}

/// Saves a new transcription to the database.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn save_transcription(
    text: String,
    raw_text: Option<String>,
    duration_seconds: Option<f64>,
    audio_path: Option<String>,
    is_enhanced: bool,
    enhancement_prompt: Option<String>,
    transcription_model_name: Option<String>,
    transcription_duration_seconds: Option<f64>,
    enhancement_model_name: Option<String>,
    enhancement_duration_seconds: Option<f64>,
) -> Result<Transcription, Error> {
    tauri_plugin_telemetry::traced("save_transcription", async move {
        let transcription = Transcription::with_details(
            text,
            raw_text,
            duration_seconds,
            audio_path,
            is_enhanced,
            enhancement_prompt,
            transcription_model_name,
            transcription_duration_seconds,
            enhancement_model_name,
            enhancement_duration_seconds,
        );

        create_transcription(&transcription).map_err(|e| {
            tracing::error!("Failed to save transcription: {}", e);
            format!("Failed to save transcription: {}", e)
        })?;

        Ok(transcription)
    })
    .await
}

/// Retrieves a transcription by its ID.
#[tauri::command]
pub fn get_transcription_by_id(id: String) -> Result<Option<Transcription>, Error> {
    get_transcription(&id)
        .map_err(|e| {
            tracing::error!("Failed to get transcription {}: {}", id, e);
            format!("Failed to get transcription: {}", e)
        })
        .map_err(Into::into)
}

/// Lists all transcriptions with optional pagination.
#[tauri::command]
pub async fn list_all_transcriptions(
    limit: Option<i64>,
    offset: Option<i64>,
) -> Result<Vec<Transcription>, Error> {
    tauri_plugin_telemetry::traced("list_all_transcriptions", async move {
        list_transcriptions(limit, offset)
            .map_err(|e| {
                tracing::error!("Failed to list transcriptions: {}", e);
                format!("Failed to list transcriptions: {}", e)
            })
            .map_err(Into::into)
    })
    .await
}

/// Deletes a transcription by its ID.
#[tauri::command]
pub async fn delete_transcription_by_id(id: String) -> Result<bool, Error> {
    tauri_plugin_telemetry::traced("delete_transcription_by_id", async move {
        delete_transcription(&id)
            .map_err(|e| {
                tracing::error!("Failed to delete transcription {}: {}", id, e);
                format!("Failed to delete transcription: {}", e)
            })
            .map_err(Into::into)
    })
    .await
}

/// Deletes all transcriptions.
#[tauri::command]
pub async fn delete_all_transcriptions_cmd() -> Result<usize, Error> {
    tauri_plugin_telemetry::traced("delete_all_transcriptions_cmd", async move {
        delete_all_transcriptions()
            .map_err(|e| {
                tracing::error!("Failed to delete all transcriptions: {}", e);
                format!("Failed to delete all transcriptions: {}", e)
            })
            .map_err(Into::into)
    })
    .await
}

/// Scans ~/.thoth/Recordings/ for WAV files not referenced by any DB row and
/// removes them. Returns the number of files removed and bytes freed.
#[tauri::command]
pub async fn reconcile_orphaned_recordings_cmd() -> Result<ReconcileResult, Error> {
    tauri_plugin_telemetry::traced("reconcile_orphaned_recordings_cmd", async move {
        reconcile_orphaned_recordings()
            .map_err(|e| {
                tracing::error!("Failed to reconcile orphaned recordings: {}", e);
                format!("Failed to reconcile orphaned recordings: {}", e)
            })
            .map_err(Into::into)
    })
    .await
}

/// Searches transcriptions by text content.
#[tauri::command]
pub async fn search_transcriptions_text(
    query: String,
    limit: Option<i64>,
) -> Result<Vec<Transcription>, Error> {
    tauri_plugin_telemetry::traced("search_transcriptions_text", async move {
        search_transcriptions(&query, limit)
            .map_err(|e| {
                tracing::error!("Failed to search transcriptions: {}", e);
                format!("Failed to search transcriptions: {}", e)
            })
            .map_err(Into::into)
    })
    .await
}

/// Counts transcriptions, optionally filtered by a search query.
#[tauri::command]
pub async fn count_transcriptions_filtered(query: Option<String>) -> Result<usize, Error> {
    tauri_plugin_telemetry::traced("count_transcriptions_filtered", async move {
        count_transcriptions(query.as_deref())
            .map_err(|e| {
                tracing::error!("Failed to count transcriptions: {}", e);
                format!("Failed to count transcriptions: {}", e)
            })
            .map_err(Into::into)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::migrations::run_migrations;
    use rusqlite::Connection;

    /// Open an in-memory SQLite DB and apply all migrations so the schema is ready.
    fn make_test_db() -> Connection {
        let mut conn = Connection::open_in_memory().expect("in-memory DB");
        conn.execute_batch("PRAGMA foreign_keys = ON;")
            .expect("pragmas");
        run_migrations(&mut conn).expect("migrations");
        conn
    }

    /// Insert a minimal transcription row with the given audio_path.
    fn insert_row(conn: &Connection, id: &str, audio_path: Option<&str>) {
        conn.execute(
            "INSERT INTO transcriptions (id, text, created_at, audio_path, is_enhanced) VALUES (?1, ?2, ?3, ?4, 0)",
            rusqlite::params![id, "test", "2024-01-01T00:00:00Z", audio_path],
        )
        .expect("insert row");
    }

    /// Insert a transcription row with an explicit created_at, for order-sensitive tests.
    fn insert_row_at(conn: &Connection, id: &str, created_at: &str) {
        conn.execute(
            "INSERT INTO transcriptions (id, text, created_at, audio_path, is_enhanced) VALUES (?1, ?2, ?3, ?4, 0)",
            rusqlite::params![id, "test", created_at, None::<&str>],
        )
        .expect("insert row");
    }

    // -------------------------------------------------------------------------
    // Struct construction (unchanged from before)
    // -------------------------------------------------------------------------

    #[test]
    fn test_transcription_new() {
        let t = Transcription::new("Hello world".to_string());
        assert!(!t.id.is_empty());
        assert_eq!(t.text, "Hello world");
        assert!(!t.is_enhanced);
        assert!(t.raw_text.is_none());
    }

    #[test]
    fn test_transcription_with_details() {
        let t = Transcription::with_details(
            "Enhanced text".to_string(),
            Some("original text".to_string()),
            Some(5.5),
            Some("/path/to/audio.wav".to_string()),
            true,
            Some("grammar".to_string()),
            Some("ggml-large-v3-turbo".to_string()),
            Some(1.2),
            Some("llama3.2:3b".to_string()),
            Some(0.8),
        );

        assert_eq!(t.text, "Enhanced text");
        assert_eq!(t.raw_text, Some("original text".to_string()));
        assert_eq!(t.duration_seconds, Some(5.5));
        assert!(t.is_enhanced);
        assert_eq!(t.enhancement_prompt, Some("grammar".to_string()));
        assert_eq!(
            t.transcription_model_name,
            Some("ggml-large-v3-turbo".to_string())
        );
        assert_eq!(t.transcription_duration_seconds, Some(1.2));
        assert_eq!(t.enhancement_model_name, Some("llama3.2:3b".to_string()));
        assert_eq!(t.enhancement_duration_seconds, Some(0.8));
    }

    // -------------------------------------------------------------------------
    // delete_transcription: audio file cleanup
    // -------------------------------------------------------------------------

    /// Single row with a real WAV file: deleting the row removes the file.
    #[test]
    fn test_delete_transcription_removes_audio_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let wav = dir.path().join("rec.wav");
        std::fs::write(&wav, b"RIFF....").expect("write wav");

        let mut conn = make_test_db();
        insert_row(&conn, "id-1", Some(wav.to_str().unwrap()));

        let deleted = delete_transcription_with_conn(&mut conn, "id-1").expect("delete");
        assert!(deleted);
        assert!(!wav.exists(), "WAV file should have been removed");
    }

    /// Two rows share one WAV: deleting the first row leaves the file; deleting
    /// the second removes it.
    #[test]
    fn test_delete_shared_audio_path_only_removes_on_last_reference() {
        let dir = tempfile::tempdir().expect("tempdir");
        let wav = dir.path().join("shared.wav");
        std::fs::write(&wav, b"RIFF....").expect("write wav");

        let mut conn = make_test_db();
        insert_row(&conn, "id-a", Some(wav.to_str().unwrap()));
        insert_row(&conn, "id-b", Some(wav.to_str().unwrap()));

        // Delete first row — file must survive.
        let deleted = delete_transcription_with_conn(&mut conn, "id-a").expect("delete a");
        assert!(deleted);
        assert!(
            wav.exists(),
            "WAV should still exist while second row references it"
        );

        // Delete second row — file must be removed now.
        let deleted = delete_transcription_with_conn(&mut conn, "id-b").expect("delete b");
        assert!(deleted);
        assert!(
            !wav.exists(),
            "WAV should be removed after last reference deleted"
        );
    }

    /// Deleting a row whose WAV is already missing must not return an error.
    #[test]
    fn test_delete_transcription_wav_already_gone_no_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let wav = dir.path().join("gone.wav");
        // Deliberately do NOT create the file.

        let mut conn = make_test_db();
        insert_row(&conn, "id-gone", Some(wav.to_str().unwrap()));

        let result = delete_transcription_with_conn(&mut conn, "id-gone");
        assert!(result.is_ok(), "Should succeed even when file is missing");
        assert!(result.unwrap());
    }

    // -------------------------------------------------------------------------
    // delete_all_transcriptions: bulk file cleanup
    // -------------------------------------------------------------------------

    /// delete_all cleans up all referenced WAV files.
    #[test]
    fn test_delete_all_removes_audio_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let wav1 = dir.path().join("a.wav");
        let wav2 = dir.path().join("b.wav");
        std::fs::write(&wav1, b"RIFF").expect("write a");
        std::fs::write(&wav2, b"RIFF").expect("write b");

        let mut conn = make_test_db();
        insert_row(&conn, "id-1", Some(wav1.to_str().unwrap()));
        insert_row(&conn, "id-2", Some(wav2.to_str().unwrap()));

        let count = delete_all_transcriptions_with_conn(&mut conn).expect("delete all");
        assert_eq!(count, 2);
        assert!(!wav1.exists());
        assert!(!wav2.exists());
    }

    // -------------------------------------------------------------------------
    // reconcile_orphaned_recordings
    // -------------------------------------------------------------------------

    /// Files not referenced by any row are removed; referenced files are kept.
    #[test]
    fn test_reconcile_removes_orphans_keeps_referenced() {
        let dir = tempfile::tempdir().expect("tempdir");
        let orphan = dir.path().join("orphan.wav");
        let referenced = dir.path().join("referenced.wav");
        std::fs::write(&orphan, b"RIFF").expect("write orphan");
        std::fs::write(&referenced, b"RIFF").expect("write referenced");

        let conn = make_test_db();
        insert_row(&conn, "id-ref", Some(referenced.to_str().unwrap()));

        let result = reconcile_orphaned_recordings_with_conn(&conn, dir.path()).expect("reconcile");

        assert_eq!(result.removed_count, 1, "one orphan should be removed");
        assert!(!orphan.exists(), "orphan should be gone");
        assert!(referenced.exists(), "referenced file should survive");
        assert!(result.bytes_freed > 0);
    }

    /// A referenced file stored with a path that differs only in representation
    /// (e.g. via a symlink to the same tempdir) must NOT be deleted.
    #[test]
    fn test_reconcile_keeps_referenced_via_symlink_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let real_file = dir.path().join("rec.wav");
        std::fs::write(&real_file, b"RIFF").expect("write wav");

        // Create a symlink directory that resolves to the same tempdir.
        let link_dir = tempfile::tempdir().expect("link tempdir");
        let link_path = link_dir.path().join("rec_link.wav");
        std::os::unix::fs::symlink(&real_file, &link_path).expect("symlink");

        let conn = make_test_db();
        // Store the path via the symlink; the file on disk is in `dir`.
        insert_row(&conn, "id-via-link", Some(link_path.to_str().unwrap()));

        // Reconcile against the real directory; the file must not be deleted
        // because its canonicalised path matches the DB entry's canonical path.
        let result = reconcile_orphaned_recordings_with_conn(&conn, dir.path()).expect("reconcile");

        assert_eq!(
            result.removed_count, 0,
            "file referenced via symlink path must not be removed"
        );
        assert!(real_file.exists(), "real file must survive");
    }

    // -------------------------------------------------------------------------
    // list_transcriptions: regression tests for the MCP list action
    // -------------------------------------------------------------------------

    /// Seeding rows and calling list returns them (newest-first order).
    #[test]
    fn test_list_transcriptions_returns_seeded_records() {
        let conn = make_test_db();
        // Strictly increasing timestamps so newest-first order is well-defined.
        insert_row_at(&conn, "id-oldest", "2024-01-01T00:00:00Z");
        insert_row_at(&conn, "id-middle", "2024-06-01T00:00:00Z");
        insert_row_at(&conn, "id-newest", "2025-01-01T00:00:00Z");

        let results = list_transcriptions_with_conn(&conn, None, None).expect("list");
        assert_eq!(results.len(), 3, "expected all 3 seeded rows");

        // Must be newest-first (ORDER BY created_at DESC). This fails if the
        // ordering is removed or reversed.
        let ids: Vec<&str> = results.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, vec!["id-newest", "id-middle", "id-oldest"]);

        // Fields are populated, not blank.
        assert!(!results[0].text.is_empty(), "text should be populated");
    }

    /// list count and count_transcriptions must agree on the same in-memory DB.
    #[test]
    fn test_list_and_stats_counts_are_consistent() {
        let conn = make_test_db();
        insert_row(&conn, "cons-1", None);
        insert_row(&conn, "cons-2", None);

        let listed = list_transcriptions_with_conn(&conn, None, None).expect("list");
        let counted = count_transcriptions_with_conn(&conn, None).expect("count");

        assert_eq!(listed.len(), counted, "list length and count must agree");
    }
}
