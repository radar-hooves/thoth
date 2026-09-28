//! Remote model manifest for dynamic model discovery
//!
//! Fetches model information from a remote JSON manifest to keep
//! the model list up-to-date without requiring app updates.

use crate::TELEMETRY_TARGET;
use crate::error::Error;
use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// URL for the model manifest (can be changed to your own hosting)
const MANIFEST_URL: &str =
    "https://raw.githubusercontent.com/radar-hooves/thoth/main/models/manifest.json";

/// Cache duration for the manifest (24 hours)
const MANIFEST_CACHE_HOURS: u64 = 24;

/// Model manifest containing all available models
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelManifest {
    /// Manifest version (for future compatibility)
    pub version: u32,
    /// Last updated timestamp (ISO 8601)
    pub updated: String,
    /// List of available models
    pub models: Vec<RemoteModelInfo>,
}

/// Information about a model from the remote manifest
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteModelInfo {
    /// Unique identifier for the model (e.g., "parakeet-tdt-0.6b-v3-int8")
    pub id: String,
    /// Human-readable name
    pub name: String,
    /// Description of the model's capabilities
    pub description: String,
    /// Model version string
    pub version: String,
    /// Download URL for the model archive
    pub download_url: String,
    /// Expected download size in bytes
    pub download_size: u64,
    /// Expected extracted size in bytes
    pub extracted_size: u64,
    /// SHA256 checksum of the archive (for verification)
    pub sha256: Option<String>,
    /// Required files that must exist after extraction
    pub required_files: Vec<String>,
    /// Directory name inside the archive (for extraction)
    pub archive_directory: Option<String>,
    /// Supported languages (empty = all)
    pub languages: Vec<String>,
    /// Model type (e.g., "transducer", "ctc")
    pub model_type: String,
    /// Whether this is the recommended/default model
    #[serde(default)]
    pub recommended: bool,
    /// Minimum app version required (semver)
    pub min_app_version: Option<String>,
}

/// Combined model info for the frontend (remote + local status)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    /// Model ID
    pub id: String,
    /// Human-readable name
    pub name: String,
    /// Description
    pub description: String,
    /// Model version
    pub version: String,
    /// Download size in MB (approximate)
    pub size_mb: u32,
    /// Whether the model is downloaded locally
    pub downloaded: bool,
    /// Path to the model directory
    pub path: String,
    /// Actual size on disk in bytes (if downloaded)
    pub disk_size: Option<u64>,
    /// Whether this is the recommended model
    pub recommended: bool,
    /// Supported languages
    pub languages: Vec<String>,
    /// Whether an update is available
    pub update_available: bool,
    /// Whether this is the currently selected model
    pub selected: bool,
    /// Model type (e.g., "whisper_ggml", "nemo_transducer")
    pub model_type: String,
    /// Whether this model's backend is available in the current build
    pub backend_available: bool,
    /// Human-readable accelerator for this build and platform, e.g.
    /// "whisper.cpp (Vulkan GPU)". Derived — never hardcode this in the frontend.
    pub accelerator: String,
}

/// Cached manifest with timestamp
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedManifest {
    /// When the manifest was fetched
    fetched_at: u64,
    /// The manifest data
    manifest: ModelManifest,
}

/// Get the manifest cache file path
fn get_cache_path() -> PathBuf {
    home_dir_or_fallback()
        .join(".thoth")
        .join("models")
        .join("manifest_cache.json")
}

fn home_dir_or_fallback() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| {
        tracing::error!("Could not determine home directory, using /tmp");
        PathBuf::from("/tmp")
    })
}

/// Load cached manifest if it exists and is not expired
fn load_cached_manifest() -> Option<ModelManifest> {
    let cache_path = get_cache_path();
    if !cache_path.exists() {
        return None;
    }

    let data = std::fs::read_to_string(&cache_path).ok()?;
    let cached: CachedManifest = serde_json::from_str(&data).ok()?;

    // Check if cache is expired
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();

    let cache_age_hours = (now - cached.fetched_at) / 3600;
    if cache_age_hours >= MANIFEST_CACHE_HOURS {
        tracing::debug!("Manifest cache expired ({} hours old)", cache_age_hours);
        return None;
    }

    tracing::debug!("Using cached manifest ({} hours old)", cache_age_hours);
    Some(cached.manifest)
}

/// Save manifest to cache
fn save_manifest_cache(manifest: &ModelManifest) -> Result<()> {
    let cache_path = get_cache_path();

    // Ensure parent directory exists
    if let Some(parent) = cache_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();

    let cached = CachedManifest {
        fetched_at: now,
        manifest: manifest.clone(),
    };

    let data = serde_json::to_string_pretty(&cached)?;
    std::fs::write(&cache_path, data)?;

    tracing::debug!("Saved manifest to cache");
    Ok(())
}

/// Fetch the model manifest from the remote URL
pub async fn fetch_manifest(force_refresh: bool) -> Result<ModelManifest> {
    // Try cache first unless forcing refresh
    if !force_refresh {
        if let Some(cached) = load_cached_manifest() {
            return Ok(cached);
        }
    }

    tracing::info!("Fetching model manifest from {}", MANIFEST_URL);

    crate::ensure_crypto_provider();
    let response = crate::http_client()
        .get(MANIFEST_URL)
        .header("User-Agent", "Thoth/1.0")
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await?;

    if !response.status().is_success() {
        return Err(anyhow!(
            "Failed to fetch manifest: HTTP {}",
            response.status()
        ));
    }

    let manifest: ModelManifest = response.json().await?;

    // Cache the manifest
    if let Err(e) = save_manifest_cache(&manifest) {
        tracing::warn!("Failed to cache manifest: {}", e);
    }

    tracing::info!(
        "Fetched manifest v{} with {} models",
        manifest.version,
        manifest.models.len()
    );

    Ok(manifest)
}

/// Bundled manifest JSON (embedded at compile time)
const BUNDLED_MANIFEST: &str = include_str!("../../../models/manifest.json");

/// Get the default/fallback manifest when remote is unavailable
pub fn get_fallback_manifest() -> ModelManifest {
    // Parse the bundled manifest
    match serde_json::from_str::<ModelManifest>(BUNDLED_MANIFEST) {
        Ok(manifest) => {
            tracing::info!(
                "Using bundled manifest v{} with {} models",
                manifest.version,
                manifest.models.len()
            );
            manifest
        }
        Err(e) => {
            tracing::error!(
                "Failed to parse bundled manifest: {}. Using minimal fallback.",
                e
            );
            // Minimal fallback if bundled manifest is somehow corrupted
            let now = chrono::Utc::now().to_rfc3339();
            ModelManifest {
                version: 10,
                updated: now,
                models: vec![RemoteModelInfo {
                    id: "fluidaudio-parakeet-tdt-coreml".to_string(),
                    name: "Parakeet TDT v3 (Neural Engine)".to_string(),
                    description: "Fastest on Apple Silicon (~210x real-time). Parakeet TDT 0.6B v3 on the Apple Neural Engine via CoreML, multilingual.".to_string(),
                    version: "3.0.0".to_string(),
                    download_url: String::new(),
                    download_size: 500_000_000,
                    extracted_size: 500_000_000,
                    sha256: None,
                    required_files: vec![".fluidaudio_ready".to_string()],
                    archive_directory: None,
                    languages: vec!["en".to_string(), "multilingual".to_string()],
                    model_type: "fluidaudio_coreml".to_string(),
                    recommended: true,
                    min_app_version: None,
                }],
            }
        }
    }
}

/// Check if a specific model is downloaded locally
pub fn is_model_downloaded(model: &RemoteModelInfo) -> bool {
    // FluidAudio's "downloaded" state is the compiled CoreML cache in
    // ~/Library/Application Support/FluidAudio/Models/, NOT the sentinel marker
    // in ~/.thoth/models/. The marker persists after the cache is cleared, so
    // trusting it makes the UI and tray claim a model that cannot actually load.
    if model.model_type == "fluidaudio_coreml" {
        return fluidaudio_cache_present();
    }

    let model_dir = get_model_directory(&model.id);

    for file in &model.required_files {
        let path = model_dir.join(file);
        if !path.exists() {
            return false;
        }

        // Verify file has content
        if let Ok(metadata) = std::fs::metadata(&path) {
            if metadata.len() == 0 {
                return false;
            }
        } else {
            return false;
        }
    }

    true
}

/// Get the directory for a specific model
pub fn get_model_directory(model_id: &str) -> PathBuf {
    // Use model ID as directory name (sanitized)
    let safe_id = model_id.replace(['/', '\\', ':', '*', '?', '"', '<', '>', '|'], "_");
    home_dir_or_fallback()
        .join(".thoth")
        .join("models")
        .join(safe_id)
}

/// Get disk size for a downloaded model
pub fn get_model_disk_size(model: &RemoteModelInfo) -> Option<u64> {
    // FluidAudio models: the marker file is tiny (~99 bytes) but the actual
    // CoreML compiled models live in ~/Library/Application Support/FluidAudio/Models/
    if model.model_type == "fluidaudio_coreml" {
        #[cfg(all(target_os = "macos", feature = "fluidaudio"))]
        {
            let cache_dir = super::fluidaudio::model_cache_directory();
            let size = dir_size_recursive(&cache_dir);
            return if size > 0 { Some(size) } else { None };
        }
        #[cfg(not(all(target_os = "macos", feature = "fluidaudio")))]
        {
            return None;
        }
    }

    let model_dir = get_model_directory(&model.id);

    model
        .required_files
        .iter()
        .filter_map(|file| {
            std::fs::metadata(model_dir.join(file))
                .ok()
                .map(|m| m.len())
        })
        .reduce(|a, b| a + b)
}

/// Whether FluidAudio's compiled CoreML model cache is present and populated.
///
/// Always `false` on builds without the FluidAudio backend, so the model simply
/// reads as not-downloaded there.
fn fluidaudio_cache_present() -> bool {
    #[cfg(all(target_os = "macos", feature = "fluidaudio"))]
    {
        super::fluidaudio::is_cached()
    }
    #[cfg(not(all(target_os = "macos", feature = "fluidaudio")))]
    {
        false
    }
}

/// Recursively calculate directory size in bytes
#[cfg(all(target_os = "macos", feature = "fluidaudio"))]
fn dir_size_recursive(path: &std::path::Path) -> u64 {
    if !path.exists() {
        return 0;
    }
    let mut total = 0;
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                total += dir_size_recursive(&p);
            } else if let Ok(meta) = p.metadata() {
                total += meta.len();
            }
        }
    }
    total
}

/// Check if the backend for a given model type is available in this build
// Arms return different cfg!() expressions, not uniform true — matches! would be wrong.
#[allow(clippy::match_like_matches_macro)]
pub fn is_backend_available(model_type: &str) -> bool {
    match model_type {
        "whisper_ggml" => true,
        "nemo_transducer" => cfg!(feature = "parakeet"),
        "fluidaudio_coreml" => cfg!(all(target_os = "macos", feature = "fluidaudio")),
        _ => false,
    }
}

/// Human-readable accelerator for a model type, reflecting the host platform and
/// the compiled feature set.
///
/// These strings used to be hardcoded for macOS in the frontend and shown
/// verbatim everywhere (#129), so a Linux CUDA+Vulkan build advertised
/// "whisper.cpp (Metal GPU)" — an accelerator that cannot exist on the platform —
/// and "Sherpa-ONNX (CPU)" for a build that links CUDA. Derived here because Rust
/// is where the cfg ladder lives; `GpuBackendType::compiled()` is already the
/// single definition of which GPU backend this build carries.
pub fn accelerator_label(model_type: &str) -> String {
    use crate::platform::GpuBackendType;

    match model_type {
        "whisper_ggml" => match GpuBackendType::compiled() {
            GpuBackendType::Cpu => "whisper.cpp (CPU)".to_string(),
            gpu => format!("whisper.cpp ({gpu} GPU)"),
        },
        // parakeet-cuda links a CUDA-enabled sherpa-onnx; the recognizer requests
        // the CUDA provider and falls back to CPU if the EP is missing at runtime,
        // which is why this says CUDA rather than promising it.
        "nemo_transducer" => {
            if cfg!(feature = "parakeet-cuda") {
                "Sherpa-ONNX (CUDA)".to_string()
            } else {
                "Sherpa-ONNX (CPU)".to_string()
            }
        }
        // CoreML/ANE only exists on macOS; the model is marked unavailable
        // elsewhere, so the label never misleads.
        "fluidaudio_coreml" => "Apple Neural Engine".to_string(),
        other => other.to_string(),
    }
}

/// Read the persisted version of an installed model, if present.
///
/// Returns `None` when the model predates version tracking or is not installed.
fn read_installed_version(model_dir: &std::path::Path) -> Option<String> {
    std::fs::read_to_string(model_dir.join(".version"))
        .ok()
        .map(|s| s.trim().to_string())
}

/// Whether an update is available for a model.
///
/// Returns `true` only when the model is downloaded and its persisted version
/// differs from the manifest version. Returns `false` conservatively when no
/// `.version` sidecar exists (model installed before version tracking).
fn is_update_available(downloaded: bool, installed: Option<&str>, manifest_version: &str) -> bool {
    downloaded && installed.map(|v| v != manifest_version).unwrap_or(false)
}

/// Resolve which model is actually active, given the configured selection.
///
/// `configured` is `transcription.model_id` from config, which is `None` on a
/// fresh install and can also name a model this build cannot run (a config
/// copied from macOS to Linux, or a feature dropped from the build).
///
/// Resolution order, all filtered on `is_backend_available`:
///
/// 1. the configured model, if this build can run it;
/// 2. otherwise the first recommended model this build can run;
/// 3. otherwise the first model this build can run;
/// 4. otherwise `None` — nothing is active rather than something unusable.
///
/// Step 2 is the fix for #128. Selection previously fell back to
/// `remote.recommended` alone, and the recommended model
/// (`fluidaudio-parakeet-tdt-coreml`) is flagged unconditionally while its
/// backend is gated on `all(target_os = "macos", feature = "fluidaudio")`. On a
/// Linux build that produced a model showing an `Active` badge and an
/// `Unavailable` button simultaneously — the app had selected a model it could
/// not possibly run.
pub fn resolve_selected_id<'a>(
    models: &'a [RemoteModelInfo],
    configured: Option<&'a str>,
) -> Option<&'a str> {
    let runnable = |m: &&'a RemoteModelInfo| is_backend_available(&m.model_type);

    if let Some(id) = configured
        && let Some(m) = models.iter().find(|m| m.id == id).filter(runnable)
    {
        return Some(m.id.as_str());
    }

    models
        .iter()
        .find(|m| m.recommended && runnable(m))
        .or_else(|| models.iter().find(runnable))
        .map(|m| m.id.as_str())
}

/// Convert remote model info to frontend model info
///
/// `selected_id` should come from [`resolve_selected_id`]. The `selected` flag is
/// additionally gated on `backend_available` here, so the invariant "a model
/// whose backend is unavailable can never be Active" holds even if a caller
/// passes an unresolved id.
pub fn to_model_info(remote: &RemoteModelInfo, selected_id: Option<&str>) -> ModelInfo {
    let downloaded = is_model_downloaded(remote);
    let disk_size = if downloaded {
        get_model_disk_size(remote)
    } else {
        None
    };

    // Gated on backend availability, not just id equality: a model this build
    // cannot run must never carry the Active badge (#128).
    let backend_available = is_backend_available(&remote.model_type);
    let selected = backend_available && selected_id == Some(remote.id.as_str());

    // For FluidAudio models, show the actual cache directory
    let path = if remote.model_type == "fluidaudio_coreml" {
        #[cfg(all(target_os = "macos", feature = "fluidaudio"))]
        {
            super::fluidaudio::model_cache_directory()
                .to_string_lossy()
                .to_string()
        }
        #[cfg(not(all(target_os = "macos", feature = "fluidaudio")))]
        {
            get_model_directory(&remote.id)
                .to_string_lossy()
                .to_string()
        }
    } else {
        get_model_directory(&remote.id)
            .to_string_lossy()
            .to_string()
    };

    ModelInfo {
        id: remote.id.clone(),
        name: remote.name.clone(),
        description: remote.description.clone(),
        version: remote.version.clone(),
        size_mb: (remote.download_size / (1024 * 1024)) as u32,
        downloaded,
        path,
        disk_size,
        recommended: remote.recommended,
        languages: probed_languages(remote, downloaded).unwrap_or_else(|| remote.languages.clone()),
        update_available: is_update_available(
            downloaded,
            read_installed_version(&get_model_directory(&remote.id)).as_deref(),
            &remote.version,
        ),
        selected,
        accelerator: accelerator_label(&remote.model_type),
        model_type: remote.model_type.clone(),
        backend_available,
    }
}

/// What an installed Whisper model's own header says its languages are (#107).
///
/// `None` when there is nothing better to say than the manifest: a model that
/// is not downloaded yet, one that is not Whisper, or a file that cannot be
/// read. The manifest's hand-typed value is the fallback, not the authority.
///
/// A disagreement is logged at warn rather than quietly corrected, because it
/// means the manifest is wrong about a model people are downloading — the
/// exact drift that is otherwise invisible, since a wrong entry still
/// downloads and still loads.
fn probed_languages(remote: &RemoteModelInfo, downloaded: bool) -> Option<Vec<String>> {
    if !downloaded || remote.model_type != "whisper_ggml" {
        return None;
    }

    let path = crate::transcription::whisper::get_whisper_model_path(&remote.id);
    match crate::transcription::ggml_header::probe(&path) {
        Ok(header) => {
            let languages = header.languages();
            if languages != remote.languages {
                tracing::warn!(
                    "Manifest disagrees with the model file for {}: manifest says {:?}, the file says {:?} (n_vocab {}, {} layers). Using the file.",
                    remote.id,
                    remote.languages,
                    languages,
                    header.n_vocab,
                    header.n_audio_layer,
                );
            }
            Some(languages)
        }
        Err(e) => {
            tracing::warn!(
                "Could not read {}'s header ({}); using the manifest's languages",
                remote.id,
                e
            );
            None
        }
    }
}

/// Tauri command: Fetch model manifest
///
/// Uses the higher-versioned manifest between remote and bundled, so that
/// new models added in app updates are visible even before the remote
/// manifest on GitHub is updated.
#[tauri::command]
#[tracing::instrument(target = TELEMETRY_TARGET, skip_all, err)]
pub async fn fetch_model_manifest(force_refresh: bool) -> Result<Vec<ModelInfo>, Error> {
    let remote_manifest = match fetch_manifest(force_refresh).await {
        Ok(m) => Some(m),
        Err(e) => {
            tracing::warn!("Failed to fetch remote manifest: {}", e);
            None
        }
    };

    let bundled = get_fallback_manifest();

    let manifest = match remote_manifest {
        Some(remote) if remote.version >= bundled.version => remote,
        Some(remote) => {
            tracing::info!(
                "Bundled manifest v{} is newer than remote v{}, using bundled",
                bundled.version,
                remote.version
            );
            bundled
        }
        None => {
            tracing::info!("Using bundled manifest v{}", bundled.version);
            bundled
        }
    };

    let selected_id = crate::config::get_config()
        .ok()
        .and_then(|c| c.transcription.model_id.clone());

    let resolved_id = resolve_selected_id(&manifest.models, selected_id.as_deref());

    let models: Vec<ModelInfo> = manifest
        .models
        .iter()
        .map(|m| to_model_info(m, resolved_id))
        .collect();

    Ok(models)
}

/// Tauri command: Get manifest last update time
#[tauri::command]
#[tracing::instrument(target = TELEMETRY_TARGET, skip_all)]
pub fn get_manifest_update_time() -> Option<String> {
    let cache_path = get_cache_path();
    if !cache_path.exists() {
        return None;
    }

    let data = std::fs::read_to_string(&cache_path).ok()?;
    let cached: CachedManifest = serde_json::from_str(&data).ok()?;

    // Convert timestamp to ISO 8601
    let datetime = chrono::DateTime::from_timestamp(cached.fetched_at as i64, 0)?;
    Some(datetime.to_rfc3339())
}

#[cfg(test)]
mod tests {

    /// A Whisper manifest entry, for the probe tests below.
    fn whisper_entry(languages: Vec<String>) -> RemoteModelInfo {
        RemoteModelInfo {
            id: "ggml-small.en".to_string(),
            name: "Whisper Small (English)".to_string(),
            description: String::new(),
            version: "1.0.0".to_string(),
            download_url: "https://example.com/m.bin".to_string(),
            download_size: 1024,
            extracted_size: 2048,
            sha256: None,
            required_files: vec![],
            archive_directory: None,
            languages,
            model_type: "whisper_ggml".to_string(),
            recommended: false,
            min_app_version: None,
        }
    }

    /// A model that is not downloaded has no file to probe, so the manifest
    /// stands. Anything else would blank the language column for every model
    /// the user has not installed.
    #[test]
    fn an_undownloaded_model_keeps_the_manifest_languages() {
        let remote = whisper_entry(vec!["en".to_string()]);
        assert_eq!(probed_languages(&remote, false), None);
    }

    /// Only Whisper models carry this header. Probing a Parakeet or CoreML
    /// entry would read the wrong bytes out of the wrong file.
    #[test]
    fn a_non_whisper_model_is_not_probed() {
        let mut remote = whisper_entry(vec!["en".to_string()]);
        remote.model_type = "nemo_transducer".to_string();
        assert_eq!(probed_languages(&remote, true), None);
    }

    /// A downloaded Whisper model whose file is missing (deleted underneath
    /// the app) must fall back rather than fail the whole model list.
    #[test]
    fn an_unreadable_file_falls_back_to_the_manifest() {
        let mut remote = whisper_entry(vec!["en".to_string()]);
        remote.id = "definitely-not-a-real-model-id-for-tests".to_string();
        assert_eq!(probed_languages(&remote, true), None);
    }
    use super::*;

    #[test]
    fn test_fallback_manifest() {
        let manifest = get_fallback_manifest();
        assert_eq!(manifest.version, 10);
        assert_eq!(manifest.models.len(), 6);

        // FluidAudio is the recommended model
        let recommended = manifest.models.iter().find(|m| m.recommended);
        assert!(
            recommended.is_some(),
            "Manifest should have a recommended model"
        );
        assert_eq!(recommended.unwrap().id, "fluidaudio-parakeet-tdt-coreml");
    }

    #[test]
    fn test_parakeet_models_in_manifest() {
        let manifest = get_fallback_manifest();
        let parakeet_models: Vec<_> = manifest
            .models
            .iter()
            .filter(|m| m.model_type == "nemo_transducer")
            .collect();
        assert_eq!(parakeet_models.len(), 2);
        assert!(
            parakeet_models
                .iter()
                .any(|m| m.id == "parakeet-tdt-0.6b-v2-int8")
        );
        assert!(
            parakeet_models
                .iter()
                .any(|m| m.id == "parakeet-tdt-0.6b-v3-int8")
        );
    }

    #[test]
    fn test_fluidaudio_model_in_manifest() {
        let manifest = get_fallback_manifest();
        let fa_model = manifest
            .models
            .iter()
            .find(|m| m.model_type == "fluidaudio_coreml");
        assert!(fa_model.is_some(), "FluidAudio model should be in manifest");
        let fa = fa_model.unwrap();
        assert_eq!(fa.id, "fluidaudio-parakeet-tdt-coreml");
        assert_eq!(fa.required_files, vec![".fluidaudio_ready"]);
        assert!(fa.recommended);
    }

    #[test]
    fn test_backend_availability() {
        assert!(is_backend_available("whisper_ggml"));
        assert!(!is_backend_available("unknown_type"));
    }

    #[test]
    fn test_model_directory() {
        let dir = get_model_directory("parakeet-tdt-0.6b-v3-int8");
        assert!(dir.to_string_lossy().contains(".thoth"));
        assert!(dir.to_string_lossy().contains("models"));
    }

    #[test]
    fn test_read_installed_version_missing() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        assert_eq!(read_installed_version(dir.path()), None);
    }

    #[test]
    fn test_read_installed_version_present() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        std::fs::write(dir.path().join(".version"), "  2.0.0\n").unwrap();
        assert_eq!(read_installed_version(dir.path()).as_deref(), Some("2.0.0"));
    }

    #[test]
    fn test_is_update_available_version_differs() {
        assert!(is_update_available(true, Some("1.0.0"), "2.0.0"));
    }

    #[test]
    fn test_is_update_available_version_same() {
        assert!(!is_update_available(true, Some("2.0.0"), "2.0.0"));
    }

    #[test]
    fn test_is_update_available_no_sidecar() {
        // Conservative: no false positive when sidecar is absent
        assert!(!is_update_available(true, None, "2.0.0"));
    }

    #[test]
    fn test_is_update_available_not_downloaded() {
        assert!(!is_update_available(false, Some("1.0.0"), "2.0.0"));
    }

    #[test]
    fn test_to_model_info() {
        let remote = RemoteModelInfo {
            id: "test-model".to_string(),
            name: "Test Model".to_string(),
            description: "A test model".to_string(),
            version: "1.0.0".to_string(),
            download_url: "https://example.com/model.tar.bz2".to_string(),
            download_size: 100 * 1024 * 1024,
            extracted_size: 110 * 1024 * 1024,
            sha256: None,
            required_files: vec!["model.onnx".to_string()],
            archive_directory: None,
            languages: vec!["en".to_string()],
            model_type: "test".to_string(),
            recommended: false,
            min_app_version: None,
        };

        let info = to_model_info(&remote, None);
        assert_eq!(info.id, "test-model");
        assert_eq!(info.size_mb, 100);
        assert!(!info.downloaded);
        assert!(!info.selected);
        assert_eq!(info.model_type, "test");
        assert!(!info.backend_available);

        // Naming an unrunnable model no longer makes it Active. model_type "test"
        // has no backend, and #128 made backend availability a hard precondition
        // for the badge rather than something the id check could bypass.
        let info_selected = to_model_info(&remote, Some("test-model"));
        assert!(!info_selected.selected);

        // A model whose backend this build does have is selected as normal.
        let runnable = RemoteModelInfo {
            model_type: "whisper_ggml".to_string(),
            ..remote.clone()
        };
        assert!(to_model_info(&runnable, Some("test-model")).selected);
    }

    /// Fixture spanning the three backends, mirroring the real manifest: the
    /// recommended model is the macOS-only FluidAudio one.
    fn selection_fixture() -> Vec<RemoteModelInfo> {
        let base = RemoteModelInfo {
            id: String::new(),
            name: "M".to_string(),
            description: String::new(),
            version: "1.0.0".to_string(),
            download_url: "https://example.com/m.tar.bz2".to_string(),
            download_size: 1024,
            extracted_size: 2048,
            sha256: None,
            required_files: vec![],
            archive_directory: None,
            languages: vec!["en".to_string()],
            model_type: String::new(),
            recommended: false,
            min_app_version: None,
        };
        vec![
            RemoteModelInfo {
                id: "fluidaudio-parakeet-tdt-coreml".to_string(),
                model_type: "fluidaudio_coreml".to_string(),
                recommended: true,
                ..base.clone()
            },
            RemoteModelInfo {
                id: "whisper-large-v3-turbo".to_string(),
                model_type: "whisper_ggml".to_string(),
                ..base.clone()
            },
            RemoteModelInfo {
                id: "parakeet-tdt-0.6b-v3".to_string(),
                model_type: "nemo_transducer".to_string(),
                ..base.clone()
            },
        ]
    }

    /// The #128 regression: `model_id = null` must not select a model whose
    /// backend this build lacks.
    ///
    /// On a build without `fluidaudio` (every Linux build, and any macOS build
    /// with the feature off) the recommended model is unrunnable, so resolution
    /// must fall through to one that is. Previously selection used
    /// `remote.recommended` alone and produced a model carrying `Active` and
    /// `Unavailable` at the same time.
    #[test]
    fn null_model_id_never_selects_an_unavailable_backend() {
        let models = selection_fixture();
        let resolved = resolve_selected_id(&models, None).expect("something must be selectable");

        assert!(
            is_backend_available(
                &models
                    .iter()
                    .find(|m| m.id == resolved)
                    .expect("resolved id must exist in the manifest")
                    .model_type
            ),
            "resolved {resolved} but its backend is unavailable in this build"
        );

        #[cfg(not(all(target_os = "macos", feature = "fluidaudio")))]
        assert_ne!(
            resolved, "fluidaudio-parakeet-tdt-coreml",
            "selected the macOS-only model on a build without the fluidaudio feature"
        );

        // On a build that *can* run it, the recommended model still wins.
        #[cfg(all(target_os = "macos", feature = "fluidaudio"))]
        assert_eq!(resolved, "fluidaudio-parakeet-tdt-coreml");
    }

    /// The invariant from #128, stated directly: whatever the config says, no
    /// model with an unavailable backend may come back Active.
    #[test]
    fn unavailable_backend_can_never_be_active() {
        let models = selection_fixture();

        for configured in [
            None,
            Some("fluidaudio-parakeet-tdt-coreml"),
            Some("whisper-large-v3-turbo"),
            Some("parakeet-tdt-0.6b-v3"),
            Some("a-model-that-does-not-exist"),
        ] {
            let resolved = resolve_selected_id(&models, configured);
            for model in &models {
                let info = to_model_info(model, resolved);
                if info.selected {
                    assert!(
                        info.backend_available,
                        "{} is Active but its backend is unavailable (configured: {configured:?})",
                        info.id
                    );
                }
            }

            // Exactly one model is Active — never zero (with a runnable model
            // present) and never several.
            let active = models
                .iter()
                .filter(|m| to_model_info(m, resolved).selected)
                .count();
            assert_eq!(
                active, 1,
                "expected one Active model (configured: {configured:?})"
            );
        }
    }

    /// A configured model this build cannot run is ignored rather than honoured —
    /// e.g. a config.json carried from macOS to Linux.
    #[test]
    fn unrunnable_configured_model_falls_back() {
        let models = selection_fixture();
        let resolved = resolve_selected_id(&models, Some("fluidaudio-parakeet-tdt-coreml"));

        #[cfg(not(all(target_os = "macos", feature = "fluidaudio")))]
        {
            assert!(resolved.is_some());
            assert_ne!(resolved.unwrap(), "fluidaudio-parakeet-tdt-coreml");
        }
        #[cfg(all(target_os = "macos", feature = "fluidaudio"))]
        assert_eq!(resolved.unwrap(), "fluidaudio-parakeet-tdt-coreml");
    }

    /// The same check against the real bundled manifest rather than a fixture, so
    /// it fails if the shipped manifest ever marks an unrunnable model recommended.
    ///
    /// This is the exact reported case: `transcription.model_id = null` on a build
    /// without `fluidaudio` showed "Parakeet TDT v3 (Neural Engine)" with an
    /// `Active` badge and an `Unavailable` button at once.
    #[test]
    fn bundled_manifest_default_is_runnable() {
        let manifest = get_fallback_manifest();
        let resolved =
            resolve_selected_id(&manifest.models, None).expect("bundled manifest must be usable");

        let model = manifest
            .models
            .iter()
            .find(|m| m.id == resolved)
            .expect("resolved id must exist in the bundled manifest");

        assert!(
            is_backend_available(&model.model_type),
            "bundled manifest defaults to {} ({}), whose backend this build lacks",
            model.id,
            model.model_type
        );

        #[cfg(not(all(target_os = "macos", feature = "fluidaudio")))]
        assert_ne!(
            model.model_type, "fluidaudio_coreml",
            "selected a CoreML model without the fluidaudio feature"
        );

        // And no model in the real manifest may be both Active and unavailable.
        for m in &manifest.models {
            let info = to_model_info(m, Some(resolved));
            if info.selected {
                assert!(
                    info.backend_available,
                    "{} is Active but unavailable",
                    info.id
                );
            }
        }
    }

    /// Accelerator text must describe the build actually running, not macOS (#129).
    ///
    /// Asserted as properties of the compiled cfg rather than fixed strings, so
    /// the test is meaningful on every feature combination CI builds.
    #[test]
    fn accelerator_label_tracks_platform_and_features() {
        let whisper = accelerator_label("whisper_ggml");
        assert!(
            whisper.contains("whisper.cpp"),
            "whisper_ggml should name its runtime: {whisper}"
        );

        // Metal exists only on macOS. This is the reported Linux symptom.
        #[cfg(not(target_os = "macos"))]
        assert!(
            !whisper.contains("Metal"),
            "advertised Metal on a non-macOS build: {whisper}"
        );
        #[cfg(target_os = "macos")]
        assert!(
            whisper.contains("Metal"),
            "macOS build should report Metal: {whisper}"
        );

        // The GPU feature compiled in must be the one named.
        #[cfg(all(not(target_os = "macos"), feature = "vulkan"))]
        assert!(
            whisper.contains("Vulkan"),
            "vulkan build should report Vulkan: {whisper}"
        );
        #[cfg(all(not(target_os = "macos"), feature = "cuda"))]
        assert!(
            whisper.contains("CUDA"),
            "cuda build should report CUDA: {whisper}"
        );
        #[cfg(all(
            not(target_os = "macos"),
            not(feature = "vulkan"),
            not(feature = "cuda"),
            not(feature = "hipblas")
        ))]
        assert!(
            whisper.contains("CPU"),
            "CPU-only build should report CPU: {whisper}"
        );

        // Always asserted, so this binding is used on every feature combination.
        // CI's Linux leg builds `--no-default-features --features vulkan`, i.e.
        // with no parakeet feature at all; without an unconditional assertion
        // both cfg arms below vanish and `-D warnings` fails on the unused
        // binding rather than on anything real.
        let parakeet = accelerator_label("nemo_transducer");
        assert!(
            parakeet.contains("Sherpa-ONNX"),
            "nemo_transducer should name its runtime: {parakeet}"
        );

        // A CUDA-linked sherpa-onnx must not be described as CPU.
        #[cfg(feature = "parakeet-cuda")]
        assert!(
            parakeet.contains("CUDA") && !parakeet.contains("CPU"),
            "parakeet-cuda build claims CPU: {parakeet}"
        );
        #[cfg(all(feature = "parakeet", not(feature = "parakeet-cuda")))]
        assert!(
            parakeet.contains("CPU"),
            "plain parakeet build should report CPU: {parakeet}"
        );

        assert_eq!(
            accelerator_label("fluidaudio_coreml"),
            "Apple Neural Engine"
        );
        // Unknown types fall back to the raw type rather than inventing hardware.
        assert_eq!(accelerator_label("something_new"), "something_new");
    }

    /// Every model in the real manifest gets a non-empty accelerator, so the UI
    /// never renders a blank detail row.
    #[test]
    fn every_bundled_model_has_an_accelerator() {
        for model in get_fallback_manifest().models {
            let info = to_model_info(&model, None);
            assert!(
                !info.accelerator.is_empty(),
                "{} has no accelerator label",
                info.id
            );
            #[cfg(not(target_os = "macos"))]
            assert!(
                !info.accelerator.contains("Metal"),
                "{} advertises Metal on a non-macOS build",
                info.id
            );
        }
    }

    /// A runnable configured model is honoured over the recommended one.
    #[test]
    fn runnable_configured_model_wins_over_recommended() {
        let models = selection_fixture();
        assert_eq!(
            resolve_selected_id(&models, Some("whisper-large-v3-turbo")),
            Some("whisper-large-v3-turbo")
        );
    }

    /// Nothing runnable means nothing Active, rather than defaulting to something
    /// the build cannot execute.
    #[test]
    fn no_runnable_model_selects_nothing() {
        let models: Vec<RemoteModelInfo> = selection_fixture()
            .into_iter()
            .filter(|m| m.model_type == "fluidaudio_coreml")
            .collect();

        #[cfg(not(all(target_os = "macos", feature = "fluidaudio")))]
        assert_eq!(resolve_selected_id(&models, None), None);
        #[cfg(all(target_os = "macos", feature = "fluidaudio"))]
        assert!(resolve_selected_id(&models, None).is_some());
    }
}
