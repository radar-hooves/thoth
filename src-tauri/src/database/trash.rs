//! Reversible Trash quarantine for transcription recordings.
//!
//! Moves audio to `~/.thoth/Trash/` and snapshots the transcription row so
//! it can be restored later.  `purge_trash` is the only permanent deletion
//! path; all user-initiated deletes should route through `quarantine_recordings`.
//!
//! # Reference-count guard
//!
//! The WAV file is moved to the Trash dir **only when no other live
//! `transcriptions` row still references the same `audio_path`**.  This mirrors
//! the guard in `delete_transcription_with_conn` (transcription.rs ~line 246).
//! `audio_moved` records whether the file was physically relocated so
//! `restore_recordings` knows whether to move it back.
//!
//! # Startup auto-purge
//!
//! `auto_purge_expired` is called from `initialise_database` after migrations.
//! It removes trash entries (and their WAV files) older than
//! `TRASH_RETENTION_DAYS`.

use crate::TELEMETRY_TARGET;
use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::database::{DatabaseError, open_connection};
use crate::error::Error;

// =============================================================================
// Constants
// =============================================================================

/// Trash entries older than this are removed on startup.
const TRASH_RETENTION_DAYS: i64 = 30;

// =============================================================================
// Public types
// =============================================================================

/// A snapshot of a quarantined transcription in the `trash` table.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrashEntry {
    pub id: String,
    /// First ~80 characters of the original transcription text.
    pub text_preview: String,
    pub created_at: String,
    pub deleted_at: String,
    pub duration_seconds: Option<f64>,
    /// File size at the time of quarantine, in bytes.
    pub file_bytes: u64,
    /// `true` when the WAV was moved to the Trash dir; `false` when another
    /// live row still referenced the same path and the move was skipped.
    pub audio_moved: bool,
}

// =============================================================================
// Internal helpers
// =============================================================================

/// Path to `~/.thoth/Trash/` (or `$THOTH_DATA_DIR/Trash/` in tests).
fn trash_dir() -> Result<std::path::PathBuf, DatabaseError> {
    let base = super::get_thoth_directory()?;
    let dir = base.join("Trash");
    if !dir.exists() {
        std::fs::create_dir_all(&dir)?;
    }
    Ok(dir)
}

/// Destination path for a moved WAV: `Trash/<id>__<original_filename>`.
fn trash_dest(id: &str, original: &std::path::Path) -> Result<std::path::PathBuf, DatabaseError> {
    let filename = original
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("audio.wav");
    let dest_name = format!("{}___{}", id, filename);
    Ok(trash_dir()?.join(dest_name))
}

// =============================================================================
// Core operations (accept a connection — testable without touching the global DB)
// =============================================================================

/// Quarantine a batch of recordings by ID.
///
/// For each ID:
/// 1. Snapshot the `transcriptions` row into `trash`.
/// 2. Delete the row from `transcriptions`.
/// 3. Move the WAV to `~/.thoth/Trash/` **only if** no other live row still
///    references that path.
/// 4. Record `audio_moved` so restore knows whether to move the file back.
///
/// All steps are wrapped in a single `IMMEDIATE` transaction per ID so the
/// ref-count check is race-free.  Returns the number of IDs successfully
/// quarantined.
#[allow(clippy::type_complexity)]
pub(crate) fn quarantine_recordings_with_conn(
    conn: &mut Connection,
    ids: &[String],
) -> Result<u32, DatabaseError> {
    let mut quarantined: u32 = 0;

    for id in ids {
        // IMMEDIATE locks the writer slot so the DELETE → ref-count → trash
        // INSERT sequence is atomic.
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;

        // Read the full row before deletion.
        let row: Option<(
            String,         // text
            Option<String>, // raw_text
            Option<f64>,    // duration_seconds
            String,         // created_at
            Option<String>, // audio_path
            i32,            // is_enhanced
            Option<String>, // enhancement_prompt
            Option<String>, // transcription_model_name
            Option<f64>,    // transcription_duration_seconds
            Option<String>, // enhancement_model_name
            Option<f64>,    // enhancement_duration_seconds
        )> = {
            let mut stmt = tx.prepare(
                r#"SELECT text, raw_text, duration_seconds, created_at, audio_path,
                          is_enhanced, enhancement_prompt,
                          transcription_model_name, transcription_duration_seconds,
                          enhancement_model_name, enhancement_duration_seconds
                   FROM transcriptions WHERE id = ?1"#,
            )?;
            stmt.query_row(params![id], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                    r.get(8)?,
                    r.get(9)?,
                    r.get(10)?,
                ))
            })
            .optional()
            .map_err(DatabaseError::from)?
        };

        let (
            text,
            raw_text,
            duration_seconds,
            created_at,
            audio_path,
            is_enhanced,
            enhancement_prompt,
            transcription_model_name,
            transcription_duration_seconds,
            enhancement_model_name,
            enhancement_duration_seconds,
        ) = match row {
            Some(r) => r,
            None => {
                tracing::warn!("quarantine_recordings: id {} not found, skipping", id);
                continue;
            }
        };

        // Delete the live row.
        let rows_deleted = tx.execute("DELETE FROM transcriptions WHERE id = ?1", params![id])?;
        if rows_deleted == 0 {
            continue;
        }

        // After the delete, count remaining live references to this audio path
        // (inside the same transaction — race-free).
        let remaining_refs: i64 = if let Some(ref path) = audio_path {
            tx.query_row(
                "SELECT COUNT(*) FROM transcriptions WHERE audio_path = ?1",
                params![path],
                |row| row.get(0),
            )?
        } else {
            0
        };

        let deleted_at = Utc::now().to_rfc3339();

        // Decide, inside the transaction, whether the WAV will be relocated to
        // the Trash dir: only when it has a path and no other live row still
        // references it. When it will move, the trash row's `audio_path` records
        // the *destination* (so restore and purge know where the file actually
        // is); `original_path` always records where to put it back. Determining
        // this in-transaction (rather than patching `audio_moved` afterwards via
        // a second connection) keeps the row consistent if the process dies
        // before the move; restore tolerates a move that never completed.
        let will_move = remaining_refs == 0 && audio_path.is_some();
        let trash_location: Option<String> = if will_move {
            let original = std::path::PathBuf::from(audio_path.as_ref().unwrap());
            Some(trash_dest(id, &original)?.to_string_lossy().into_owned())
        } else {
            audio_path.clone()
        };

        tx.execute(
            r#"INSERT INTO trash (
                   id, text, raw_text, duration_seconds, created_at, audio_path,
                   is_enhanced, enhancement_prompt,
                   transcription_model_name, transcription_duration_seconds,
                   enhancement_model_name, enhancement_duration_seconds,
                   original_path, deleted_at, audio_moved
               ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)"#,
            params![
                id,
                text,
                raw_text,
                duration_seconds,
                created_at,
                trash_location, // audio_path = trash destination when moving, else original
                is_enhanced,
                enhancement_prompt,
                transcription_model_name,
                transcription_duration_seconds,
                enhancement_model_name,
                enhancement_duration_seconds,
                audio_path, // original_path = where restore puts the file back
                deleted_at,
                will_move as i32,
            ],
        )?;

        tx.commit()?;

        // Move the file after the commit. The DB already records the intended
        // destination, so restore/purge stay correct even if this move fails or
        // the process dies here (restore falls back to the original path).
        if will_move {
            // will_move implies both are Some.
            let src = std::path::PathBuf::from(audio_path.as_ref().unwrap());
            let dest = std::path::PathBuf::from(trash_location.as_ref().unwrap());
            match std::fs::rename(&src, &dest) {
                Ok(()) => {
                    tracing::debug!("Quarantined audio {} -> {}", src.display(), dest.display())
                }
                Err(e) => tracing::warn!(
                    "Failed to move audio {} to trash (restore will fall back to the original path): {}",
                    src.display(),
                    e
                ),
            }
        }

        quarantined += 1;
    }

    Ok(quarantined)
}

/// Restore a batch of trash entries back to `transcriptions`.
///
/// For each ID:
/// 1. Re-insert the snapshotted row into `transcriptions`.
/// 2. If `audio_moved = true`, move the WAV back to `original_path`.
/// 3. Delete the trash entry.
///
/// Returns the count of successfully restored entries.
#[allow(clippy::type_complexity)]
pub(crate) fn restore_recordings_with_conn(
    conn: &mut Connection,
    ids: &[String],
) -> Result<u32, DatabaseError> {
    let mut restored: u32 = 0;

    for id in ids {
        // Read the trash snapshot.
        let snap: Option<(
            String,         // text
            Option<String>, // raw_text
            Option<f64>,    // duration_seconds
            String,         // created_at
            Option<String>, // audio_path (current location in Trash/)
            i32,            // is_enhanced
            Option<String>, // enhancement_prompt
            Option<String>, // transcription_model_name
            Option<f64>,    // transcription_duration_seconds
            Option<String>, // enhancement_model_name
            Option<f64>,    // enhancement_duration_seconds
            Option<String>, // original_path
            i32,            // audio_moved
        )> = {
            let mut stmt = conn.prepare(
                r#"SELECT text, raw_text, duration_seconds, created_at, audio_path,
                          is_enhanced, enhancement_prompt,
                          transcription_model_name, transcription_duration_seconds,
                          enhancement_model_name, enhancement_duration_seconds,
                          original_path, audio_moved
                   FROM trash WHERE id = ?1"#,
            )?;
            stmt.query_row(params![id], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                    r.get(8)?,
                    r.get(9)?,
                    r.get(10)?,
                    r.get(11)?,
                    r.get(12)?,
                ))
            })
            .optional()
            .map_err(DatabaseError::from)?
        };

        let (
            text,
            raw_text,
            duration_seconds,
            created_at,
            audio_path_in_trash,
            is_enhanced,
            enhancement_prompt,
            transcription_model_name,
            transcription_duration_seconds,
            enhancement_model_name,
            enhancement_duration_seconds,
            original_path,
            audio_moved,
        ) = match snap {
            Some(s) => s,
            None => {
                tracing::warn!("restore_recordings: id {} not in trash, skipping", id);
                continue;
            }
        };

        // Move the file back to original_path before re-inserting the row, so
        // the restored row immediately has a valid audio_path. Tolerate a move
        // that never completed: if the file is already at original_path (or the
        // trash copy is missing), skip the move and still reference the original.
        let restored_audio_path: Option<String> = if audio_moved != 0 {
            if let Some(orig) = original_path.as_deref() {
                let dst = std::path::PathBuf::from(orig);
                if !dst.exists() {
                    if let Some(src) = audio_path_in_trash.as_deref().map(std::path::PathBuf::from)
                    {
                        if src.exists() {
                            if let Some(parent) = dst.parent() {
                                let _ = std::fs::create_dir_all(parent);
                            }
                            match std::fs::rename(&src, &dst) {
                                Ok(()) => {
                                    tracing::debug!("Restored audio -> {}", dst.display())
                                }
                                Err(e) => {
                                    tracing::warn!("Failed to move audio back for {}: {}", id, e)
                                }
                            }
                        }
                    }
                }
            }
            // The restored row always references the original path.
            original_path.clone()
        } else {
            // File was never moved; the recorded audio_path is the original location.
            // Guard against the case where a sibling trash entry sharing the same
            // audio_path was purged earlier (removing the WAV): if the file no
            // longer exists, restore the row without an audio_path rather than
            // pointing at a nonexistent file.
            match &audio_path_in_trash {
                Some(p) if !std::path::Path::new(p).exists() => {
                    tracing::warn!(
                        "restore_recordings: audio file missing for {} (path: {}); \
                         restoring text only",
                        id,
                        p
                    );
                    None
                }
                other => other.clone(),
            }
        };

        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;

        tx.execute(
            r#"INSERT OR IGNORE INTO transcriptions (
                   id, text, raw_text, duration_seconds, created_at, audio_path,
                   is_enhanced, enhancement_prompt,
                   transcription_model_name, transcription_duration_seconds,
                   enhancement_model_name, enhancement_duration_seconds
               ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)"#,
            params![
                id,
                text,
                raw_text,
                duration_seconds,
                created_at,
                restored_audio_path,
                is_enhanced,
                enhancement_prompt,
                transcription_model_name,
                transcription_duration_seconds,
                enhancement_model_name,
                enhancement_duration_seconds,
            ],
        )?;

        tx.execute("DELETE FROM trash WHERE id = ?1", params![id])?;
        tx.commit()?;

        tracing::debug!("Restored transcription {} from trash", id);
        restored += 1;
    }

    Ok(restored)
}

/// Permanently delete trash entries.
///
/// `ids = Some(...)` purges only those entries; `ids = None` purges everything.
/// Removes the WAV from `~/.thoth/Trash/` when `audio_moved = true`.
///
/// Returns the count of entries purged.
pub(crate) fn purge_trash_with_conn(
    conn: &mut Connection,
    ids: Option<&[String]>,
) -> Result<u32, DatabaseError> {
    // Collect the rows to purge.
    let to_purge: Vec<(String, Option<String>, bool)> = {
        let sql = match ids {
            Some(_) => "SELECT id, audio_path, audio_moved FROM trash WHERE id = ?1",
            None => "SELECT id, audio_path, audio_moved FROM trash",
        };

        if let Some(id_list) = ids {
            let mut out = Vec::new();
            for id in id_list {
                let mut stmt = conn.prepare(sql)?;
                let rows: Vec<_> = stmt
                    .query_map(params![id], |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, Option<String>>(1)?,
                            r.get::<_, i32>(2)? != 0,
                        ))
                    })?
                    .collect::<Result<Vec<_>, _>>()?;
                out.extend(rows);
            }
            out
        } else {
            let mut stmt = conn.prepare(sql)?;
            stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, i32>(2)? != 0,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?
        }
    };

    let mut purged: u32 = 0;

    for (id, audio_path, audio_moved) in &to_purge {
        // Remove the WAV from Trash/ when we physically moved it there.
        if *audio_moved {
            if let Some(path_str) = audio_path {
                let path = std::path::PathBuf::from(path_str);
                match std::fs::remove_file(&path) {
                    Ok(()) => tracing::debug!("Purged audio: {}", path.display()),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        // File already gone (e.g. crash between a previous
                        // purge's remove_file and DELETE FROM trash). The DB
                        // row is the zombie; proceed to delete it so the next
                        // auto-purge doesn't retry forever.
                        tracing::warn!("Audio already gone during purge: {}", path.display());
                    }
                    Err(e) => {
                        // Unexpected error: keep the DB row so the file can be
                        // retried on the next purge rather than being orphaned.
                        tracing::warn!(
                            "Failed to purge audio {} (keeping trash row for retry): {}",
                            path.display(),
                            e
                        );
                        continue;
                    }
                }
            }
        }

        conn.execute("DELETE FROM trash WHERE id = ?1", params![id])?;
        purged += 1;
    }

    Ok(purged)
}

/// List all entries in the trash table.
pub(crate) fn list_trash_with_conn(conn: &Connection) -> Result<Vec<TrashEntry>, DatabaseError> {
    let mut stmt = conn.prepare(
        r#"SELECT id,
                  SUBSTR(COALESCE(text, ''), 1, 80) AS text_preview,
                  created_at,
                  deleted_at,
                  duration_seconds,
                  COALESCE(audio_path, '') AS audio_path_str,
                  audio_moved
           FROM trash
           ORDER BY deleted_at DESC"#,
    )?;

    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<f64>>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, i32>(6)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    let entries = rows
        .into_iter()
        .map(
            |(
                id,
                text_preview,
                created_at,
                deleted_at,
                duration_seconds,
                audio_path_str,
                audio_moved,
            )| {
                let file_bytes = if !audio_path_str.is_empty() {
                    std::fs::metadata(&audio_path_str)
                        .map(|m| m.len())
                        .unwrap_or(0)
                } else {
                    0
                };
                TrashEntry {
                    id,
                    text_preview,
                    created_at,
                    deleted_at,
                    duration_seconds,
                    file_bytes,
                    audio_moved: audio_moved != 0,
                }
            },
        )
        .collect();

    Ok(entries)
}

/// Remove trash entries whose `deleted_at` is older than `TRASH_RETENTION_DAYS`.
///
/// Called from `initialise_database` after migrations.
pub fn auto_purge_expired(conn: &mut Connection) -> Result<u32, DatabaseError> {
    let cutoff = (Utc::now() - chrono::Duration::days(TRASH_RETENTION_DAYS)).to_rfc3339();

    // Collect expired rows before deleting so we can remove WAVs.
    let expired: Vec<(String, Option<String>, bool)> = {
        let mut stmt =
            conn.prepare("SELECT id, audio_path, audio_moved FROM trash WHERE deleted_at < ?1")?;
        stmt.query_map(params![cutoff], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, i32>(2)? != 0,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?
    };

    if expired.is_empty() {
        return Ok(0);
    }

    let ids: Vec<String> = expired.iter().map(|(id, _, _)| id.clone()).collect();
    let purged = purge_trash_with_conn(conn, Some(&ids))?;

    if purged > 0 {
        tracing::info!(
            "Auto-purged {} trash entries older than {} days",
            purged,
            TRASH_RETENTION_DAYS
        );
    }

    Ok(purged)
}

// =============================================================================
// Tauri commands
// =============================================================================

/// Move recordings to the reversible trash.
///
/// Returns the count of recordings successfully quarantined.
#[tauri::command]
#[tracing::instrument(target = TELEMETRY_TARGET, skip_all, err)]
pub fn quarantine_recordings(ids: Vec<String>) -> Result<u32, Error> {
    let mut conn = open_connection()?;
    quarantine_recordings_with_conn(&mut conn, &ids).map_err(|e| {
        tracing::error!("quarantine_recordings failed: {}", e);
        Error::Database(e)
    })
}

/// Restore quarantined recordings back to the live history.
///
/// Returns the count of recordings successfully restored.
#[tauri::command]
#[tracing::instrument(target = TELEMETRY_TARGET, skip_all, err)]
pub fn restore_recordings(ids: Vec<String>) -> Result<u32, Error> {
    let mut conn = open_connection()?;
    restore_recordings_with_conn(&mut conn, &ids).map_err(|e| {
        tracing::error!("restore_recordings failed: {}", e);
        Error::Database(e)
    })
}

/// Permanently delete trash entries.
///
/// `ids = None` purges the entire trash.  Returns the count purged.
#[tauri::command]
#[tracing::instrument(target = TELEMETRY_TARGET, skip_all, err)]
pub fn purge_trash(ids: Option<Vec<String>>) -> Result<u32, Error> {
    let mut conn = open_connection()?;
    let id_slice = ids.as_deref();
    purge_trash_with_conn(&mut conn, id_slice).map_err(|e| {
        tracing::error!("purge_trash failed: {}", e);
        Error::Database(e)
    })
}

/// List all entries currently in the trash.
#[tauri::command]
#[tracing::instrument(target = TELEMETRY_TARGET, skip_all, err)]
pub fn list_trash() -> Result<Vec<TrashEntry>, Error> {
    let conn = open_connection()?;
    list_trash_with_conn(&conn).map_err(|e| {
        tracing::error!("list_trash failed: {}", e);
        Error::Database(e)
    })
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::migrations::run_migrations;
    use hound::{SampleFormat, WavSpec, WavWriter};
    use rusqlite::Connection;

    fn make_test_db() -> Connection {
        let mut conn = Connection::open_in_memory().expect("in-memory DB");
        conn.execute_batch("PRAGMA foreign_keys = ON;")
            .expect("pragmas");
        run_migrations(&mut conn).expect("migrations");
        conn
    }

    /// Write a minimal 16-bit mono WAV to `path` so file-move assertions work.
    fn write_dummy_wav(path: &std::path::Path) {
        let spec = WavSpec {
            channels: 1,
            sample_rate: 16000,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        };
        let mut w = WavWriter::create(path, spec).expect("create wav");
        for _ in 0..160 {
            w.write_sample(1000i16).expect("write sample");
        }
        w.finalize().expect("finalize wav");
    }

    /// Seed a row into the `transcriptions` table, optionally with a WAV at `audio_path`.
    fn seed_transcription(
        conn: &Connection,
        id: &str,
        text: &str,
        audio_path: Option<&str>,
        duration: Option<f64>,
    ) {
        conn.execute(
            r#"INSERT INTO transcriptions
               (id, text, raw_text, duration_seconds, created_at, audio_path, is_enhanced)
               VALUES (?1, ?2, NULL, ?3, '2024-01-01T00:00:00Z', ?4, 0)"#,
            params![id, text, duration, audio_path],
        )
        .expect("seed transcription");
    }

    // -------------------------------------------------------------------------
    // Quarantine → restore round-trip
    // -------------------------------------------------------------------------

    #[serial_test::serial]
    #[test]
    fn quarantine_and_restore_round_trip() {
        let dir = tempfile::tempdir().expect("tempdir");
        // SAFETY: `#[serial]` guarantees no other test in this binary modifies
        // THOTH_DATA_DIR concurrently, making the mutation race-free.
        unsafe { std::env::set_var("THOTH_DATA_DIR", dir.path()) };

        let wav_path = dir.path().join("Recordings").join("test.wav");
        std::fs::create_dir_all(wav_path.parent().unwrap()).unwrap();
        write_dummy_wav(&wav_path);

        let mut conn = make_test_db();
        let wav_str = wav_path.to_str().unwrap();
        seed_transcription(&conn, "t1", "hello world", Some(wav_str), Some(5.0));

        // Quarantine.
        let count =
            quarantine_recordings_with_conn(&mut conn, &["t1".to_string()]).expect("quarantine");
        assert_eq!(count, 1);

        // Row must be gone from transcriptions.
        let live: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM transcriptions WHERE id = 't1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(live, 0, "row must be removed from transcriptions");

        // Row must be in trash.
        let in_trash: i64 = conn
            .query_row("SELECT COUNT(*) FROM trash WHERE id = 't1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(in_trash, 1, "row must be in trash");

        // Original WAV must no longer exist at source.
        assert!(
            !wav_path.exists(),
            "WAV must have been moved out of Recordings/"
        );

        // audio_moved flag must be set.
        let audio_moved: i32 = conn
            .query_row("SELECT audio_moved FROM trash WHERE id = 't1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(audio_moved, 1, "audio_moved must be 1");

        // Restore.
        let restored =
            restore_recordings_with_conn(&mut conn, &["t1".to_string()]).expect("restore");
        assert_eq!(restored, 1);

        // Row must be back in transcriptions.
        let back: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM transcriptions WHERE id = 't1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(back, 1, "row must be back in transcriptions after restore");

        // Trash must be empty.
        let still_in_trash: i64 = conn
            .query_row("SELECT COUNT(*) FROM trash", [], |r| r.get(0))
            .unwrap();
        assert_eq!(still_in_trash, 0, "trash must be empty after restore");

        // WAV must be back at original path.
        assert!(wav_path.exists(), "WAV must be restored to original path");

        // Clean up env var so other tests in the same run are unaffected.
        unsafe { std::env::remove_var("THOTH_DATA_DIR") };
    }

    // -------------------------------------------------------------------------
    // Shared audio path: file NOT moved, other row intact
    // -------------------------------------------------------------------------

    #[serial_test::serial]
    #[test]
    fn quarantine_shared_audio_does_not_move_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        // SAFETY: `#[serial]` guarantees no other test in this binary modifies
        // THOTH_DATA_DIR concurrently, making the mutation race-free.
        unsafe { std::env::set_var("THOTH_DATA_DIR", dir.path()) };

        let wav_path = dir.path().join("Recordings").join("shared.wav");
        std::fs::create_dir_all(wav_path.parent().unwrap()).unwrap();
        write_dummy_wav(&wav_path);

        let mut conn = make_test_db();
        let wav_str = wav_path.to_str().unwrap();

        // Two rows referencing the same audio file.
        seed_transcription(&conn, "a", "text a", Some(wav_str), Some(3.0));
        seed_transcription(&conn, "b", "text b", Some(wav_str), Some(3.0));

        // Quarantine only "a".
        let count =
            quarantine_recordings_with_conn(&mut conn, &["a".to_string()]).expect("quarantine");
        assert_eq!(count, 1);

        // WAV must still exist (row "b" still references it).
        assert!(
            wav_path.exists(),
            "WAV must NOT be moved while another row references it"
        );

        // audio_moved must be false.
        let audio_moved: i32 = conn
            .query_row("SELECT audio_moved FROM trash WHERE id = 'a'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(audio_moved, 0, "audio_moved must be 0 for shared audio");

        // Row "b" must be untouched.
        let b_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM transcriptions WHERE id = 'b'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(b_count, 1, "row 'b' must still be in transcriptions");

        unsafe { std::env::remove_var("THOTH_DATA_DIR") };
    }

    // -------------------------------------------------------------------------
    // Purge: removes WAV and row
    // -------------------------------------------------------------------------

    #[serial_test::serial]
    #[test]
    fn purge_removes_wav_and_row() {
        let dir = tempfile::tempdir().expect("tempdir");
        // SAFETY: `#[serial]` guarantees no other test in this binary modifies
        // THOTH_DATA_DIR concurrently, making the mutation race-free.
        unsafe { std::env::set_var("THOTH_DATA_DIR", dir.path()) };

        let wav_path = dir.path().join("Recordings").join("purge.wav");
        std::fs::create_dir_all(wav_path.parent().unwrap()).unwrap();
        write_dummy_wav(&wav_path);

        let mut conn = make_test_db();
        let wav_str = wav_path.to_str().unwrap();
        seed_transcription(&conn, "p1", "purge me", Some(wav_str), Some(2.0));

        // Quarantine.
        quarantine_recordings_with_conn(&mut conn, &["p1".to_string()]).expect("quarantine");

        // The WAV moved to Trash/.
        assert!(!wav_path.exists(), "WAV moved to Trash");

        // Determine the trash WAV path.
        let trash_audio: String = conn
            .query_row("SELECT audio_path FROM trash WHERE id = 'p1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        let trash_wav = std::path::PathBuf::from(&trash_audio);
        assert!(trash_wav.exists(), "Trash WAV must exist before purge");

        // Purge.
        let purged = purge_trash_with_conn(&mut conn, Some(&["p1".to_string()])).expect("purge");
        assert_eq!(purged, 1);

        // Trash WAV must be gone.
        assert!(!trash_wav.exists(), "Trash WAV must be removed after purge");

        // Trash row must be gone.
        let in_trash: i64 = conn
            .query_row("SELECT COUNT(*) FROM trash WHERE id = 'p1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(in_trash, 0, "trash row must be gone after purge");

        unsafe { std::env::remove_var("THOTH_DATA_DIR") };
    }

    // -------------------------------------------------------------------------
    // Auto-purge respects the retention cutoff
    // -------------------------------------------------------------------------

    #[serial_test::serial]
    #[test]
    fn auto_purge_respects_retention_cutoff() {
        let dir = tempfile::tempdir().expect("tempdir");
        // SAFETY: `#[serial]` guarantees no other test in this binary modifies
        // THOTH_DATA_DIR concurrently, making the mutation race-free.
        unsafe { std::env::set_var("THOTH_DATA_DIR", dir.path()) };

        let mut conn = make_test_db();

        // Insert an old trash entry directly (no audio, simpler).
        let old_deleted_at =
            (Utc::now() - chrono::Duration::days(TRASH_RETENTION_DAYS + 1)).to_rfc3339();
        let recent_deleted_at = Utc::now().to_rfc3339();

        conn.execute(
            r#"INSERT INTO trash (id, text, raw_text, duration_seconds, created_at,
                                  audio_path, is_enhanced, enhancement_prompt,
                                  transcription_model_name, transcription_duration_seconds,
                                  enhancement_model_name, enhancement_duration_seconds,
                                  original_path, deleted_at, audio_moved)
               VALUES ('old', 'old text', NULL, NULL, '2023-01-01T00:00:00Z',
                       NULL, 0, NULL, NULL, NULL, NULL, NULL, NULL, ?1, 0)"#,
            params![old_deleted_at],
        )
        .expect("insert old trash");

        conn.execute(
            r#"INSERT INTO trash (id, text, raw_text, duration_seconds, created_at,
                                  audio_path, is_enhanced, enhancement_prompt,
                                  transcription_model_name, transcription_duration_seconds,
                                  enhancement_model_name, enhancement_duration_seconds,
                                  original_path, deleted_at, audio_moved)
               VALUES ('recent', 'recent text', NULL, NULL, '2024-01-01T00:00:00Z',
                       NULL, 0, NULL, NULL, NULL, NULL, NULL, NULL, ?1, 0)"#,
            params![recent_deleted_at],
        )
        .expect("insert recent trash");

        let purged = auto_purge_expired(&mut conn).expect("auto_purge");
        assert_eq!(purged, 1, "only the old entry should be purged");

        let remaining: i64 = conn
            .query_row("SELECT COUNT(*) FROM trash", [], |r| r.get(0))
            .unwrap();
        assert_eq!(remaining, 1, "recent entry must survive auto-purge");

        let surviving_id: String = conn
            .query_row("SELECT id FROM trash", [], |r| r.get(0))
            .unwrap();
        assert_eq!(surviving_id, "recent", "the recent entry must survive");

        unsafe { std::env::remove_var("THOTH_DATA_DIR") };
    }
}
