# Data Model and Persistence

## Overview

Thoth uses SQLite for transcription records and JSON files for configuration and dictionary data. All data is stored locally in the `~/.thoth/` directory.

## Directory Structure

```
~/.thoth/
├── thoth.db           # SQLite database (transcriptions)
├── config.json        # Application configuration
├── dictionary.json    # Word replacement rules
└── Recordings/        # Audio WAV files
    └── thoth_recording_YYYYMMDD_HHMMSS.wav
```

## SQLite Database

### Location

The database is stored at `~/.thoth/thoth.db`.

### Connection Management

Each Tauri command creates a new database connection for thread safety. Foreign keys are enabled via pragma on connection open.

### Schema Migrations

Migrations are tracked in the `migrations` table and applied sequentially on application startup.

```sql
CREATE TABLE IF NOT EXISTS migrations (
    version INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    applied_at TEXT NOT NULL DEFAULT (datetime('now'))
);
```

### Transcriptions Table

Stores all transcription records with optional AI enhancement metadata.

```sql
CREATE TABLE IF NOT EXISTS transcriptions (
    id TEXT PRIMARY KEY,
    text TEXT NOT NULL,
    raw_text TEXT,
    duration_seconds REAL,
    created_at TEXT NOT NULL,
    audio_path TEXT,
    is_enhanced INTEGER NOT NULL DEFAULT 0,
    enhancement_prompt TEXT
);

CREATE INDEX IF NOT EXISTS idx_transcriptions_created_at ON transcriptions(created_at);
CREATE INDEX IF NOT EXISTS idx_transcriptions_is_enhanced ON transcriptions(is_enhanced);
```

#### Column Definitions

| Column               | Type    | Description                                 |
| -------------------- | ------- | ------------------------------------------- |
| `id`                 | TEXT    | UUID primary key                            |
| `text`               | TEXT    | Transcribed text (possibly enhanced)        |
| `raw_text`           | TEXT    | Original text before enhancement (nullable) |
| `duration_seconds`   | REAL    | Audio duration in seconds (nullable)        |
| `created_at`         | TEXT    | ISO 8601 timestamp                          |
| `audio_path`         | TEXT    | Path to audio file if retained (nullable)   |
| `is_enhanced`        | INTEGER | 1 if AI-enhanced, 0 otherwise               |
| `enhancement_prompt` | TEXT    | Enhancement prompt used (nullable)          |

## Rust Data Structures

### Transcription

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
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
}
```

## Configuration

### Location

Configuration is stored at `~/.thoth/config.json` with schema versioning for migrations.

### Structure

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Schema version for migrations
    pub version: u32,
    /// Audio recording settings
    pub audio: AudioConfig,
    /// Transcription settings
    pub transcription: TranscriptionConfig,
    /// Keyboard shortcut settings
    pub shortcuts: ShortcutConfig,
    /// AI enhancement settings
    pub enhancement: EnhancementConfig,
    /// General application settings
    pub general: GeneralConfig,
    /// Recorder window settings
    pub recorder: RecorderConfig,
}
```

### AudioConfig

| Field         | Type           | Default | Description                            |
| ------------- | -------------- | ------- | -------------------------------------- |
| `device_id`   | Option<String> | None    | Selected input device (None = default) |
| `sample_rate` | u32            | 16000   | Sample rate in Hz                      |
| `play_sounds` | bool           | true    | Play audio feedback sounds             |

### TranscriptionConfig

| Field               | Type           | Default | Description                  |
| ------------------- | -------------- | ------- | ---------------------------- |
| `model_id`          | Option<String> | None    | Selected model ID            |
| `language`          | String         | "en"    | Transcription language code  |
| `auto_copy`         | bool           | true    | Auto-copy to clipboard       |
| `auto_paste`        | bool           | true    | Auto-paste at cursor         |
| `add_leading_space` | bool           | false   | Add space before pasted text |

### ShortcutConfig

| Field                  | Type           | Default                        | Description                |
| ---------------------- | -------------- | ------------------------------ | -------------------------- |
| `toggle_recording`     | String         | "ShiftRight" on macOS, "F13" elsewhere | Primary recording shortcut |
| `toggle_recording_alt` | Option<String> | "CommandOrControl+Shift+Space" | Alternative shortcut       |
| `copy_last`            | Option<String> | "F14"                          | Copy last transcription    |
| `recording_mode`       | RecordingMode  | Toggle                         | Toggle or push-to-talk     |

### EnhancementConfig

| Field        | Type   | Default                  | Description            |
| ------------ | ------ | ------------------------ | ---------------------- |
| `enabled`    | bool   | false                    | AI enhancement enabled |
| `model`      | String | "llama3.2"               | Ollama model name      |
| `prompt_id`  | String | "fix-grammar"            | Enhancement prompt ID  |
| `ollama_url` | String | "http://localhost:11434" | Ollama server URL      |

### GeneralConfig

| Field              | Type | Default | Description            |
| ------------------ | ---- | ------- | ---------------------- |
| `launch_at_login`  | bool | false   | Launch at system start |
| `show_in_menu_bar` | bool | true    | Show menu bar icon     |
| `show_in_dock`     | bool | false   | Show dock icon (macOS) |

### RecorderConfig

| Field             | Type             | Default  | Description                              |
| ----------------- | ---------------- | -------- | ---------------------------------------- |
| `position`        | RecorderPosition | TopRight | Window position preference               |
| `offset_x`        | i32              | -20      | Horizontal offset from position (pixels) |
| `offset_y`        | i32              | 20       | Vertical offset from position (pixels)   |
| `auto_hide_delay` | u32              | 3000     | Auto-hide delay in ms (0 = no auto-hide) |

### RecorderPosition Enum

| Value       | Description                       |
| ----------- | --------------------------------- |
| Cursor      | Near cursor when recording starts |
| TrayIcon    | Near the tray icon                |
| TopLeft     | Top-left corner of screen         |
| TopRight    | Top-right corner of screen        |
| BottomLeft  | Bottom-left corner of screen      |
| BottomRight | Bottom-right corner of screen     |
| Centre      | Centre of the screen              |

### RecordingMode Enum

| Value      | Description                         |
| ---------- | ----------------------------------- |
| Toggle     | Press to start, press again to stop |
| PushToTalk | Hold to record, release to stop     |

## Dictionary

### Location

Dictionary entries are stored at `~/.thoth/dictionary.json`.

### Structure

```rust
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Dictionary {
    /// The dictionary entries
    pub entries: Vec<DictionaryEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DictionaryEntry {
    /// The text to search for and replace
    pub from: String,
    /// The replacement text
    pub to: String,
    /// Whether the match should be case-sensitive
    pub case_sensitive: bool,
}
```

### Example

```json
{
  "entries": [
    {
      "from": "teh",
      "to": "the",
      "caseSensitive": false
    },
    {
      "from": "recieve",
      "to": "receive",
      "caseSensitive": false
    }
  ]
}
```

## Audio Files

### Location

Audio recordings are stored in `~/.thoth/Recordings/`.

### Naming Convention

Files are named with the pattern: `thoth_recording_YYYYMMDD_HHMMSS.wav`

Example: `thoth_recording_20250214_143022.wav`

### Format

- Format: WAV (uncompressed)
- Sample rate: 16000 Hz (optimised for transcription)
- Channels: Mono

## Data Lifecycle

```
1. Recording Starts  → Audio capture begins
2. Recording Stops   → WAV file written to ~/.thoth/Recordings/
3. Transcription     → Text extracted from audio
4. Enhancement       → AI enhancement applied (if enabled)
5. Dictionary        → Word replacements applied
6. Persistence       → Record saved to SQLite database
7. Output            → Text copied to clipboard / pasted at cursor
```

## Tauri Commands

### Database Commands

| Command                         | Description                               |
| ------------------------------- | ----------------------------------------- |
| `init_database`                 | Initialise database and run migrations    |
| `save_transcription`            | Save a new transcription                  |
| `get_transcription_by_id`       | Get a transcription by ID                 |
| `list_all_transcriptions`       | List transcriptions with pagination       |
| `delete_transcription_by_id`    | Delete a transcription                    |
| `search_transcriptions_text`    | Search transcriptions by text             |
| `count_transcriptions_filtered` | Count transcriptions with optional filter |

### Configuration Commands

| Command               | Description                    |
| --------------------- | ------------------------------ |
| `get_config`          | Get current configuration      |
| `patch_config`        | Apply a partial update         |
| `reset_config`        | Reset to default configuration |
| `get_config_path_cmd` | Get config file path           |

The tables above name the fields as `config.json` stores them (snake_case). `get_config`, `get_default_config`, `patch_config` and `reset_config` return the same document with camelCase keys (`device_id` is `deviceId`); `patch_config` accepts either spelling. MCP `setting` and HTTP `/settings` stay snake_case.

### Dictionary Commands

| Command                    | Description                |
| -------------------------- | -------------------------- |
| `get_dictionary_entries`   | Get all dictionary entries |
| `add_dictionary_entry`     | Add a new entry            |
| `update_dictionary_entry`  | Update an existing entry   |
| `remove_dictionary_entry`  | Remove an entry by index   |
| `import_dictionary`        | Import entries from JSON   |
| `export_dictionary`        | Export entries as JSON     |
| `apply_dictionary_to_text` | Apply replacements to text |
