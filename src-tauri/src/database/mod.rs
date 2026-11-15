//! Database module for Thoth.
//!
//! Provides SQLite database connection management and migrations.
//! Database is stored at `~/.thoth/thoth.db`.

pub mod insights;
pub mod migrations;
pub mod schema;
pub mod transcription;
pub mod trash;

use parking_lot::{Mutex, MutexGuard};
use rusqlite::Connection;
use std::sync::OnceLock;

use crate::database::migrations::run_migrations;
use crate::error::Error;

/// The one connection, opened by [`initialise_database`].
static CONNECTION: OnceLock<Mutex<Connection>> = OnceLock::new();

/// Database error types.
#[derive(Debug, thiserror::Error)]
pub enum DatabaseError {
    #[error("Failed to create database directory: {0}")]
    DirectoryCreation(std::io::Error),

    #[error("Failed to read recordings directory: {0}")]
    DirectoryRead(std::io::Error),

    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("Migration failed: {0}")]
    Migration(String),

    #[error("Database not initialised")]
    NotInitialised,
}

impl From<std::io::Error> for DatabaseError {
    fn from(e: std::io::Error) -> Self {
        DatabaseError::DirectoryCreation(e)
    }
}

/// The shared connection. Hold the guard only for the queries: file work
/// (decoding audio, walking directories) happens after it is dropped, and a
/// holder never calls another function that takes it.
pub fn connection() -> Result<MutexGuard<'static, Connection>, DatabaseError> {
    CONNECTION
        .get()
        .map(Mutex::lock)
        .ok_or(DatabaseError::NotInitialised)
}

/// Opens `thoth.db` in the data directory, migrates it and keeps the connection.
///
/// Idempotent: the frontend's `init_database` and startup both call it.
pub fn initialise_database() -> Result<(), DatabaseError> {
    if CONNECTION.get().is_some() {
        return Ok(());
    }
    let dir = crate::config::data_dir();
    std::fs::create_dir_all(&dir)?;
    let db_path = dir.join("thoth.db");
    tracing::info!("Initialising database at {:?}", db_path);

    let mut conn = Connection::open(&db_path)?;
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA busy_timeout = 5000;
         PRAGMA foreign_keys = ON;",
    )?;
    run_migrations(&mut conn)?;

    // A missed purge is recoverable next startup.
    match trash::auto_purge_expired(&mut conn) {
        Ok(0) => {}
        Ok(n) => tracing::info!("Auto-purged {} expired trash entries", n),
        Err(e) => tracing::warn!("auto_purge_expired failed (non-fatal): {}", e),
    }

    // A racing initialiser's connection is dropped; both migrated the same file.
    let _ = CONNECTION.set(Mutex::new(conn));
    Ok(())
}

// =============================================================================
// Tauri Commands
// =============================================================================

/// Initialises the database. Call this on application startup.
#[tauri::command]
pub async fn init_database() -> Result<(), Error> {
    initialise_database()
        .map_err(|e| {
            tracing::error!("Failed to initialise database: {}", e);
            format!("Failed to initialise database: {}", e)
        })
        .map_err(Into::into)
}

// =============================================================================
// Re-exports
// =============================================================================

// Re-export transcription types for convenience
pub use transcription::Transcription;

// Re-export transcription CRUD functions
pub use transcription::{
    count_transcriptions, create_transcription, delete_transcription, get_transcription,
    list_transcriptions, search_transcriptions, update_transcription,
};

// Re-export transcription Tauri commands
pub use transcription::{
    delete_transcription_by_id, get_transcription_by_id, get_transcription_stats_cmd,
    list_all_transcriptions,
};

// Re-export trash Tauri commands
pub use trash::{list_trash, purge_trash, quarantine_recordings, restore_recordings};
