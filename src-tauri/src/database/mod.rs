//! Database module for Thoth.
//!
//! Provides SQLite database connection management and migrations.
//! Database is stored at `~/.thoth/thoth.db`.

pub mod insights;
pub mod migrations;
pub mod schema;
pub mod transcription;
pub mod trash;

use rusqlite::Connection;
use std::path::PathBuf;
use std::sync::OnceLock;

use crate::database::migrations::run_migrations;
use crate::error::Error;

/// Global database path, initialised once.
static DATABASE_PATH: OnceLock<PathBuf> = OnceLock::new();

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
}

impl From<std::io::Error> for DatabaseError {
    fn from(e: std::io::Error) -> Self {
        DatabaseError::DirectoryCreation(e)
    }
}

/// Returns the path to the Thoth database directory (~/.thoth).
///
/// `THOTH_DATA_DIR` overrides the location when set to a non-empty path. This
/// lets tests point the database at a throwaway directory, and lets users
/// relocate their data, without touching `~/.thoth`.
fn get_thoth_directory() -> Result<PathBuf, DatabaseError> {
    if let Ok(dir) = std::env::var("THOTH_DATA_DIR") {
        if !dir.is_empty() {
            return Ok(PathBuf::from(dir));
        }
    }

    let home = dirs::home_dir().ok_or_else(|| {
        DatabaseError::DirectoryCreation(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "Could not find home directory",
        ))
    })?;

    Ok(home.join(".thoth"))
}

/// Returns the path to the database file (~/.thoth/thoth.db).
pub fn get_database_path() -> Result<PathBuf, DatabaseError> {
    let thoth_dir = get_thoth_directory()?;
    Ok(thoth_dir.join("thoth.db"))
}

/// Ensures the database directory exists and returns the database path.
fn ensure_database_directory() -> Result<PathBuf, DatabaseError> {
    let thoth_dir = get_thoth_directory()?;

    if !thoth_dir.exists() {
        std::fs::create_dir_all(&thoth_dir)?;
        tracing::info!("Created Thoth directory at {:?}", thoth_dir);
    }

    Ok(thoth_dir.join("thoth.db"))
}

/// Opens a connection to the database.
///
/// Each call creates a new connection. For thread safety in Tauri commands,
/// create a new connection per command invocation.
pub fn open_connection() -> Result<Connection, DatabaseError> {
    // DATABASE_PATH is normally set by initialise_database() at startup.
    // The expect here is a safeguard for direct open_connection() calls.
    let db_path = DATABASE_PATH.get_or_init(|| {
        ensure_database_directory()
            .expect("database directory must be writable; called before initialise_database()?")
    });

    let conn = Connection::open(db_path)?;

    // Enable foreign keys
    conn.execute_batch("PRAGMA foreign_keys = ON;")?;

    Ok(conn)
}

/// Initialises the database, creating the directory and running migrations.
///
/// This should be called once on application startup.
pub fn initialise_database() -> Result<(), DatabaseError> {
    tracing::info!("Initialising database");

    // Ensure directory exists and get path
    let db_path = ensure_database_directory()?;
    DATABASE_PATH.get_or_init(|| db_path.clone());

    tracing::info!("Database path: {:?}", db_path);

    // Open connection and run migrations
    let mut conn = open_connection()?;
    run_migrations(&mut conn)?;

    // Remove trash entries that have exceeded the retention window.
    // Failures are logged but not fatal — a missed purge is recoverable next startup.
    match trash::auto_purge_expired(&mut conn) {
        Ok(0) => {}
        Ok(n) => tracing::info!("Auto-purged {} expired trash entries", n),
        Err(e) => tracing::warn!("auto_purge_expired failed (non-fatal): {}", e),
    }

    tracing::info!("Database initialised successfully");
    Ok(())
}

// =============================================================================
// Tauri Commands
// =============================================================================

/// Initialises the database. Call this on application startup.
#[tauri::command]
pub async fn init_database() -> Result<(), Error> {
    tauri_plugin_telemetry::traced("init_database", async move {
        initialise_database()
            .map_err(|e| {
                tracing::error!("Failed to initialise database: {}", e);
                format!("Failed to initialise database: {}", e)
            })
            .map_err(Into::into)
    })
    .await
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
    count_transcriptions_filtered, delete_transcription_by_id, get_transcription_by_id,
    get_transcription_stats_cmd, list_all_transcriptions, reconcile_orphaned_recordings_cmd,
    save_transcription, search_transcriptions_text,
};

// Re-export trash Tauri commands
pub use trash::{list_trash, purge_trash, quarantine_recordings, restore_recordings};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_database_path_format() {
        let path = get_database_path().unwrap();
        assert!(path.to_string_lossy().contains(".thoth"));
        assert!(path.to_string_lossy().ends_with("thoth.db"));
    }
}
