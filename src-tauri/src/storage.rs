//! Storage management for Thoth
//!
//! Provides disk usage reporting and cleanup commands for all data
//! locations: models, recordings, database, config, and
//! FluidAudio CoreML cache.

use crate::error::Error;
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};

/// Disk usage breakdown by category
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageUsage {
    /// Speech recognition models (~/.thoth/models/)
    pub models_bytes: u64,
    /// Audio recordings (~/.thoth/Recordings/)
    pub recordings_bytes: u64,
    /// SQLite database (~/.thoth/thoth.db)
    pub database_bytes: u64,
    /// Config + dictionary + prompts (small files)
    pub config_bytes: u64,
    /// FluidAudio CoreML cache (~/Library/Application Support/FluidAudio/Models/)
    pub fluidaudio_bytes: u64,
    /// Total across all categories
    pub total_bytes: u64,
    /// Number of recording files
    pub recording_count: u64,
}

/// Get the Thoth data directory (~/.thoth)
fn thoth_dir() -> PathBuf {
    dirs::home_dir()
        .expect("Could not determine home directory")
        .join(".thoth")
}

/// Get the FluidAudio model cache directory, if applicable on this platform.
///
/// FluidAudio runs only on macOS (Apple Neural Engine via CoreML) and stores its
/// compiled models under `~/Library/Application Support/FluidAudio/Models/`. That
/// path is macOS-specific; on Linux/Windows there is no FluidAudio cache, so this
/// returns `None` rather than constructing a bogus `~/Library/...` path that
/// would never exist. Only targets the Models subdirectory — FluidAudio may
/// store other data in the parent `Application Support/FluidAudio/` directory.
fn fluidaudio_models_dir() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        Some(
            dirs::home_dir()?
                .join("Library")
                .join("Application Support")
                .join("FluidAudio")
                .join("Models"),
        )
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

/// Calculate total size of a directory recursively
fn dir_size(path: &Path) -> u64 {
    if !path.exists() {
        return 0;
    }

    walkdir(path)
}

/// Recursive directory size calculation
fn walkdir(path: &Path) -> u64 {
    let mut total = 0;
    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries.flatten() {
            let entry_path = entry.path();
            if entry_path.is_dir() {
                total += walkdir(&entry_path);
            } else if let Ok(meta) = entry_path.metadata() {
                total += meta.len();
            }
        }
    }
    total
}

/// Count files in a directory (non-recursive)
fn file_count(path: &Path) -> u64 {
    if !path.exists() {
        return 0;
    }

    fs::read_dir(path)
        .map(|entries| entries.flatten().filter(|e| e.path().is_file()).count() as u64)
        .unwrap_or(0)
}

/// Calculate the size of known config files
fn config_file_sizes(base: &Path) -> u64 {
    let files = ["config.json", "dictionary.json", "prompts.json"];
    files
        .iter()
        .filter_map(|f| fs::metadata(base.join(f)).ok())
        .map(|m| m.len())
        .sum()
}

/// Get storage usage breakdown
#[tauri::command]
pub async fn get_storage_usage() -> Result<StorageUsage, Error> {
    tauri_plugin_telemetry::traced("get_storage_usage", async move {
        let base = thoth_dir();

        let models_bytes = dir_size(&base.join("models"));
        let recordings_bytes = dir_size(&base.join("Recordings"));
        let database_bytes = fs::metadata(base.join("thoth.db"))
            .map(|m| m.len())
            .unwrap_or(0);
        let config_bytes = config_file_sizes(&base);
        let fluidaudio_bytes = fluidaudio_models_dir().map(|d| dir_size(&d)).unwrap_or(0);

        let recording_count = file_count(&base.join("Recordings"));

        let total_bytes =
            models_bytes + recordings_bytes + database_bytes + config_bytes + fluidaudio_bytes;

        Ok(StorageUsage {
            models_bytes,
            recordings_bytes,
            database_bytes,
            config_bytes,
            fluidaudio_bytes,
            total_bytes,
            recording_count,
        })
    })
    .await
}

/// Delete all audio recordings
#[tauri::command]
pub async fn delete_all_recordings() -> Result<u64, Error> {
    tauri_plugin_telemetry::traced("delete_all_recordings", async move {
        let recordings_dir = thoth_dir().join("Recordings");
        if !recordings_dir.exists() {
            return Ok(0);
        }

        let mut deleted = 0u64;
        let entries = fs::read_dir(&recordings_dir).map_err(|e| e.to_string())?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                if let Err(e) = fs::remove_file(&path) {
                    tracing::warn!("Failed to delete recording {:?}: {}", path, e);
                } else {
                    deleted += 1;
                }
            }
        }

        tracing::info!("Deleted {} recording files", deleted);
        Ok(deleted)
    })
    .await
}

/// Delete the FluidAudio CoreML model cache
#[tauri::command]
pub async fn delete_fluidaudio_cache() -> Result<(), Error> {
    tauri_plugin_telemetry::traced("delete_fluidaudio_cache", async move {
        let Some(cache_dir) = fluidaudio_models_dir() else {
            return Ok(()); // No FluidAudio cache off macOS.
        };
        if !cache_dir.exists() {
            return Ok(());
        }

        fs::remove_dir_all(&cache_dir).map_err(|e| {
            format!(
                "Failed to delete FluidAudio cache at {}: {}",
                cache_dir.display(),
                e
            )
        })?;

        // Also remove the ready marker so Model Manager reflects the change
        let marker_dir = thoth_dir()
            .join("models")
            .join("fluidaudio-parakeet-tdt-coreml");
        let marker_path = marker_dir.join(".fluidaudio_ready");
        if marker_path.exists() {
            let _ = fs::remove_file(&marker_path);
        }

        tracing::info!("Deleted FluidAudio cache directory");
        Ok(())
    })
    .await
}

/// Delete ALL Thoth data (full reset / uninstall cleanup)
///
/// Removes ~/.thoth/ and ~/Library/Application Support/FluidAudio/Models/
#[tauri::command]
pub async fn delete_all_data() -> Result<(), Error> {
    tauri_plugin_telemetry::traced("delete_all_data", async move {
        let base = thoth_dir();
        if base.exists() {
            fs::remove_dir_all(&base)
                .map_err(|e| format!("Failed to delete Thoth data at {}: {}", base.display(), e))?;
            tracing::info!("Deleted Thoth data directory: {}", base.display());
        }

        if let Some(fluid_dir) = fluidaudio_models_dir() {
            if fluid_dir.exists() {
                fs::remove_dir_all(&fluid_dir).map_err(|e| {
                    format!(
                        "Failed to delete FluidAudio cache at {}: {}",
                        fluid_dir.display(),
                        e
                    )
                })?;
                tracing::info!(
                    "Deleted FluidAudio cache directory: {}",
                    fluid_dir.display()
                );
            }
        }

        Ok(())
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_thoth_dir_path() {
        let dir = thoth_dir();
        assert!(dir.to_string_lossy().contains(".thoth"));
    }

    #[test]
    fn test_fluidaudio_dir_path() {
        let dir = fluidaudio_models_dir();
        #[cfg(target_os = "macos")]
        assert!(
            dir.expect("FluidAudio dir should resolve on macOS")
                .to_string_lossy()
                .contains("FluidAudio")
        );
        #[cfg(not(target_os = "macos"))]
        assert!(dir.is_none(), "FluidAudio dir should be None off macOS");
    }

    #[test]
    fn test_dir_size_nonexistent() {
        let path = PathBuf::from("/nonexistent/path/that/doesnt/exist");
        assert_eq!(dir_size(&path), 0);
    }

    #[test]
    fn test_file_count_nonexistent() {
        let path = PathBuf::from("/nonexistent/path/that/doesnt/exist");
        assert_eq!(file_count(&path), 0);
    }
}
