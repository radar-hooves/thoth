# Service Architecture Patterns

Patterns for Thoth's Rust backend service design.

## Module Organisation

Services are organised as Rust modules with clear public interfaces:

```
src-tauri/src/
├── lib.rs              # Tauri app entry, command registration
├── audio/              # Audio capture subsystem
│   └── mod.rs
├── database/           # SQLite persistence
│   ├── mod.rs          # Connection management, migrations
│   ├── migrations.rs
│   ├── schema.rs
│   └── transcription.rs
├── enhancement/        # AI enhancement
│   ├── mod.rs          # Ollama client, commands
│   ├── context.rs
│   ├── ollama.rs
│   └── prompts.rs
└── transcription/      # Speech-to-text
    ├── mod.rs          # Unified service, backend selection
    ├── whisper.rs      # Whisper with Metal GPU
    ├── parakeet.rs     # ONNX fallback
    └── manifest.rs
```

Each module exposes:

- **Types**: Public structs, enums via `pub use`
- **Tauri commands**: Functions annotated with `#[tauri::command]`
- **Internal functions**: Private implementation details

## Global State Management

### OnceLock for Singleton Services

Use `OnceLock` for services initialised once and used throughout the app lifetime:

```rust
use parking_lot::Mutex;
use std::sync::OnceLock;

/// Global transcription service instance
static TRANSCRIPTION_SERVICE: OnceLock<Mutex<Option<TranscriptionService>>> = OnceLock::new();

fn get_service() -> &'static Mutex<Option<TranscriptionService>> {
    TRANSCRIPTION_SERVICE.get_or_init(|| Mutex::new(None))
}
```

**When to use `OnceLock<Mutex<Option<T>>>`:**

- Service requires initialisation with runtime parameters
- Service needs mutable access (e.g., stateful transcription)
- Lazy initialisation is acceptable

**When to use `OnceLock<Mutex<T>>`:**

- Service can be constructed with defaults
- Service is always available after init

```rust
/// Global Ollama client (always available with defaults)
static OLLAMA_CLIENT: OnceLock<Mutex<OllamaClient>> = OnceLock::new();

fn get_client() -> &'static Mutex<OllamaClient> {
    OLLAMA_CLIENT.get_or_init(|| Mutex::new(OllamaClient::new()))
}
```

### Database Connection Pattern

One connection, opened by `initialise_database` with `journal_mode = WAL`, `busy_timeout = 5000` and `foreign_keys = ON`, behind a mutex (`database::connection()`), as Bragi's `db/mod.rs` does. Hold the guard only for the queries: decode audio or walk directories after dropping it (`insights.rs` `get_cruft_candidates`), and never call a function that takes it while holding it.

## Tauri Commands as Service Interface

Tauri commands are the primary interface between frontend and backend services.

### Basic Command Pattern

```rust
#[tauri::command]
pub fn is_transcription_ready() -> bool {
    get_service().lock().is_some()
}
```

### Fallible Commands with String Errors

Always return `Result<T, String>` with user-friendly messages:

```rust
#[tauri::command]
pub fn init_transcription(model_path: String) -> Result<(), String> {
    let service = TranscriptionService::new_whisper(&PathBuf::from(model_path))
        .map_err(|e| e.to_string())?;

    let mut guard = get_service().lock();
    *guard = Some(service);

    tracing::info!("Transcription service initialised");
    Ok(())
}
```

### Async Commands

For I/O-bound operations, use async commands:

```rust
#[tauri::command]
pub async fn check_ollama_available() -> bool {
    let client = get_client().lock().clone();
    client.is_available().await
}

#[tauri::command]
pub async fn enhance_text(text: String, model: String, prompt: String) -> Result<String, String> {
    if text.is_empty() {
        return Err("Text cannot be empty".to_string());
    }

    let client = get_client().lock().clone();

    tracing::info!(
        "Enhancing text with model '{}' ({} characters)",
        model,
        text.len()
    );

    client
        .enhance_text(&text, &model, &prompt)
        .await
        .map_err(|e| {
            tracing::error!("Enhancement failed: {}", e);
            format!("Enhancement failed: {}", e)
        })
}
```

**Note:** Clone the client before awaiting to release the lock during async operations.

### Command Registration

Register all commands in `lib.rs`:

```rust
tauri::Builder::default()
    .invoke_handler(tauri::generate_handler![
        // Transcription
        transcription::init_transcription,
        transcription::transcribe_file,
        transcription::is_transcription_ready,
        // Enhancement
        enhancement::check_ollama_available,
        enhancement::enhance_text,
        // Database
        database::init_database,
        database::transcription::save_transcription,
    ])
```

## Error Handling

### thiserror for Domain Errors

Define domain-specific errors with `thiserror`:

```rust
#[derive(Debug, thiserror::Error)]
pub enum DatabaseError {
    #[error("Failed to create database directory: {0}")]
    DirectoryCreation(#[from] std::io::Error),

    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("Migration failed: {0}")]
    Migration(String),
}
```

### anyhow for Service Implementations

Use `anyhow` for internal implementation where error context is more important than type:

```rust
use anyhow::{Context, Result};

impl TranscriptionService {
    pub fn new_whisper(model_path: &std::path::Path) -> anyhow::Result<Self> {
        let service = whisper::WhisperTranscriptionService::new(model_path)
            .context("Failed to create whisper service")?;
        Ok(Self::Whisper(service))
    }

    pub fn transcribe(&mut self, audio_path: &std::path::Path) -> anyhow::Result<String> {
        match self {
            Self::Whisper(service) => service.transcribe(audio_path),
            Self::Parakeet(service) => service.transcribe(audio_path),
        }
    }
}
```

### Error Conversion in Commands

Convert domain errors to user-friendly strings at the command boundary:

```rust
#[tauri::command]
pub fn init_database() -> Result<(), String> {
    initialise_database().map_err(|e| {
        tracing::error!("Failed to initialise database: {}", e);
        format!("Failed to initialise database: {}", e)
    })
}
```

## Service Initialisation Patterns

### Explicit Initialisation via Command

For services requiring runtime configuration:

```rust
#[tauri::command]
pub fn init_transcription(model_path: String) -> Result<(), String> {
    let path = PathBuf::from(&model_path);

    // Auto-detect backend based on model files
    if path.extension().map(|e| e == "bin").unwrap_or(false) {
        return init_whisper_transcription(model_path);
    }

    if path.is_dir() {
        // Check for whisper .bin files (Metal GPU priority)
        if let Ok(entries) = std::fs::read_dir(&path) {
            for entry in entries.filter_map(|e| e.ok()) {
                let entry_path = entry.path();
                if entry_path.extension().map(|ext| ext == "bin").unwrap_or(false) {
                    tracing::info!("Found whisper model, using Metal GPU backend");
                    return init_whisper_transcription(entry_path.to_string_lossy().to_string());
                }
            }
        }

        // Fall back to ONNX/Parakeet
        let encoder = path.join("encoder.int8.onnx");
        if encoder.exists() {
            tracing::info!("Found ONNX model, using Parakeet backend");
            return init_parakeet_transcription(model_path);
        }
    }

    Err(format!("No valid model found: {}", path.display()))
}
```

### Lazy Initialisation with Defaults

For services that can start with sensible defaults:

```rust
fn get_client() -> &'static Mutex<OllamaClient> {
    OLLAMA_CLIENT.get_or_init(|| Mutex::new(OllamaClient::new()))
}
```

### App Startup Initialisation

For services requiring early initialisation, use the Tauri setup hook:

```rust
tauri::Builder::default()
    .setup(|app| {
        tracing::info!("Thoth starting");

        // Load config and register shortcuts
        if let Ok(cfg) = config::get_config() {
            let app_handle = app.handle().clone();
            register_shortcuts_from_config(&app_handle, &cfg);
        }

        Ok(())
    })
```

## Logging Pattern

Use `tracing` for structured logging:

```rust
use tracing::{info, warn, error};

#[tauri::command]
pub fn init_whisper_transcription(model_path: String) -> Result<(), String> {
    let service = TranscriptionService::new_whisper(&PathBuf::from(model_path))
        .map_err(|e| e.to_string())?;

    let mut guard = get_service().lock();
    *guard = Some(service);

    tracing::info!("Whisper transcription service initialised with Metal GPU");
    Ok(())
}
```

**Logging conventions:**

- `info!` for successful operations and state changes
- `warn!` for recoverable issues
- `error!` for failures that propagate to the frontend

## Enum-Based Backend Selection

When supporting multiple implementations, use enums:

```rust
/// Transcription backend type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptionBackend {
    /// Whisper with Metal GPU acceleration (primary, fastest)
    Whisper,
    /// Sherpa-ONNX with Parakeet models (fallback)
    Parakeet,
}

impl Default for TranscriptionBackend {
    fn default() -> Self {
        Self::Whisper
    }
}

/// Unified transcription service
pub enum TranscriptionService {
    Whisper(whisper::WhisperTranscriptionService),
    Parakeet(parakeet::TranscriptionService),
}

impl TranscriptionService {
    pub fn backend(&self) -> TranscriptionBackend {
        match self {
            Self::Whisper(_) => TranscriptionBackend::Whisper,
            Self::Parakeet(_) => TranscriptionBackend::Parakeet,
        }
    }
}
```

## Module Re-exports

Use `pub use` to expose a clean public API:

```rust
// In database/mod.rs
pub use transcription::Transcription;
pub use transcription::{
    count_transcriptions, create_transcription, delete_transcription,
    get_transcription, list_transcriptions, search_transcriptions,
    update_transcription,
};
```

## Testing Pattern

Include unit tests in each module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_initialisation() {
        let client = get_client();
        let _guard = client.lock();
        // Client should be initialised without panicking
    }

    #[test]
    fn test_database_path_format() {
        let path = get_database_path().unwrap();
        assert!(path.to_string_lossy().contains(".thoth"));
        assert!(path.to_string_lossy().ends_with("thoth.db"));
    }
}
```

## Summary

| Pattern                      | Use Case                | Example                |
| ---------------------------- | ----------------------- | ---------------------- |
| `OnceLock<Mutex<Option<T>>>` | Lazy init with params   | Transcription service  |
| `OnceLock<Mutex<T>>`         | Lazy init with defaults | Ollama client          |
| `OnceLock<PathBuf>`          | Immutable config        | Database path          |
| Per-command connection       | Thread-safe DB access   | rusqlite Connection    |
| `#[tauri::command]`          | Frontend interface      | All public operations  |
| `Result<T, String>`          | Command error handling  | User-friendly messages |
| `thiserror`                  | Domain errors           | DatabaseError          |
| `anyhow`                     | Internal implementation | Service construction   |
