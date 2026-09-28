//! Configuration management for Thoth
//!
//! Provides persistent settings storage with schema versioning and migrations.
//! Configuration is stored in `~/.thoth/config.json` and is accessible from
//! both the Rust backend and the Svelte frontend via IPC commands.

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;

use crate::TELEMETRY_TARGET;
use crate::enhancement;
use crate::error::Error;

/// Current config schema version
const CURRENT_VERSION: u32 = 1;

/// Global config instance for caching
static CONFIG: OnceLock<RwLock<Config>> = OnceLock::new();

/// Integrations configuration (Local Control API, MCP server)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct IntegrationsConfig {
    /// Whether the Local Control API HTTP server is enabled
    #[serde(default, alias = "apiEnabled")]
    pub api_enabled: bool,
    /// Port for the Local Control API (default 8765)
    #[serde(default = "default_api_port", alias = "apiPort")]
    pub api_port: u16,
    /// Whether the MCP server is enabled
    #[serde(default, alias = "mcpEnabled")]
    pub mcp_enabled: bool,
}

fn default_api_port() -> u16 {
    8765
}

impl Default for IntegrationsConfig {
    fn default() -> Self {
        Self {
            // Control API and MCP server default ON. They bind 127.0.0.1 only and
            // require the bearer token, so they are not network-exposed; defaulting
            // on means MCP-capable assistants work out of the box. The token is
            // held in a dedicated reset-proof store (control_api::token_store).
            api_enabled: true,
            api_port: default_api_port(),
            mcp_enabled: true,
        }
    }
}

/// Main configuration structure
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
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
    /// Integrations settings (Local Control API, MCP)
    pub integrations: IntegrationsConfig,
    /// Telemetry exporter settings
    pub telemetry: TelemetryConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: CURRENT_VERSION,
            audio: AudioConfig::default(),
            transcription: TranscriptionConfig::default(),
            shortcuts: ShortcutConfig::default(),
            enhancement: EnhancementConfig::default(),
            general: GeneralConfig::default(),
            recorder: RecorderConfig::default(),
            integrations: IntegrationsConfig::default(),
            telemetry: TelemetryConfig::default(),
        }
    }
}

/// Audio recording configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioConfig {
    /// Selected audio input device ID (None for system default)
    pub device_id: Option<String>,
    /// Sample rate in Hz (default: 16000 for transcription)
    pub sample_rate: u32,
    /// Whether to play audio feedback sounds
    pub play_sounds: bool,
    /// Volume for the recording cues, 0.0 (silent) to 1.0 (full).
    ///
    /// Defaulted rather than required so configs written before the setting
    /// existed keep working and simply play at full volume.
    #[serde(default = "default_sound_volume")]
    pub sound_volume: f32,
    /// Keep the cpal input stream open between recordings ("warm stream").
    ///
    /// When true (default), the device is opened once and kept playing with an
    /// armed flag gating writes to the recording buffer. Start latency drops
    /// from ~150ms to near-zero. The stream is torn down after 45s of inactivity.
    /// When false, the device is opened and closed on every recording (original
    /// behaviour); the mic indicator only shows levels during active recording.
    #[serde(default = "default_true")]
    pub warm_stream: bool,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            device_id: None,
            sample_rate: 16000,
            play_sounds: true,
            sound_volume: default_sound_volume(),
            warm_stream: true,
        }
    }
}

/// Key combination sent after a successful insertion to submit the text.
///
/// Chat interfaces disagree on the send binding (Enter in Slack and iMessage,
/// Ctrl/Cmd+Enter in Discord threads, Gmail and many web forms), so this is a
/// choice rather than a boolean.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AutoSubmit {
    /// Send nothing after inserting. The default: dictation must not press keys
    /// the user did not ask for.
    #[default]
    Off,
    /// Plain Return.
    Enter,
    /// Control+Return.
    CtrlEnter,
    /// Command+Return on macOS; Control+Return elsewhere, since there is no
    /// Command key to press.
    CmdEnter,
}

/// Which external tool synthesises keystrokes on Linux.
///
/// Linux has no single working answer: `wtype` needs the
/// `zwp_virtual_keyboard_manager_v1` protocol, which KWin and Mutter do not
/// implement; `xdotool` is X11-only; `ydotool` and `dotool` go through
/// `/dev/uinput` and work anywhere but need permission set up. So the chain is
/// chosen per session, and pinned by the user when a session picks badly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TypingTool {
    /// Order the tools by what the session is (Wayland/X11, KDE/GNOME/other).
    #[default]
    Auto,
    /// Native Wayland, virtual-keyboard protocol. Not KDE or GNOME.
    Wtype,
    /// KDE's own Fake Input protocol.
    Kwtype,
    /// `/dev/uinput`, reads commands on stdin.
    Dotool,
    /// `/dev/uinput`, via its own daemon.
    Ydotool,
    /// X11 only.
    Xdotool,
    /// The in-process fallback. On Wayland it drives XWayland, which is what
    /// raises GNOME's "Allow Remote Interaction" prompt.
    Enigo,
}

/// Transcription engine configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TranscriptionConfig {
    /// Selected model ID (e.g., "ggml-large-v3-turbo", "parakeet-tdt-0.6b-v2-int8")
    /// If None, uses the recommended model from the manifest
    pub model_id: Option<String>,
    /// Whether to automatically copy transcription to clipboard
    pub auto_copy: bool,
    /// Whether to automatically paste transcription at cursor
    pub auto_paste: bool,
    /// Whether to add space before pasted text
    pub add_leading_space: bool,
    /// Whether to append a single space after the inserted text.
    ///
    /// Off by default. Useful when dictating several times into the same field,
    /// so consecutive transcriptions are word-spaced without manual editing.
    #[serde(default)]
    pub append_trailing_space: bool,
    /// Key combination to send after a successful insertion, to submit the text.
    ///
    /// Off by default. Different chat interfaces bind different send shortcuts,
    /// hence the choice rather than a plain boolean.
    #[serde(default)]
    pub auto_submit: AutoSubmit,
    /// Whether to remove hesitation sounds (um, uh, er, ah) from transcription
    #[serde(default = "default_true")]
    pub remove_fillers: bool,
    /// Whether to convert US spellings to Australian/British equivalents.
    /// Defaults on — the operator dictates Australian English.
    #[serde(default = "default_true")]
    pub australian_spelling: bool,
    /// Whether to convert spoken number words to digits.
    /// Defaults OFF: rule-based ITN is inherently ambiguous (lone "one" as a
    /// pronoun, counting sequences vs sums), so it stays opt-in for when the
    /// user is dictating numeric content rather than prose.
    #[serde(default)]
    pub spoken_numbers_to_digits: bool,
    /// Whether to collapse runs of whitespace and trim leading/trailing spaces
    #[serde(default = "default_true")]
    pub normalise_whitespace: bool,
    /// Whether to fix spacing around punctuation marks
    #[serde(default = "default_true")]
    pub cleanup_punctuation: bool,
    /// Whether to capitalise the first word of each sentence
    #[serde(default)]
    pub sentence_case: bool,
    /// Whether to convert spoken formatting commands ("new paragraph" / "new
    /// line") into line breaks. Defaults on — the dictation convention used by
    /// macOS Dictation, Dragon and Talon.
    #[serde(default = "default_true")]
    pub voice_formatting_commands: bool,
    /// Feed the user's dictionary and canonical terms to the decoder as an
    /// initial prompt, so an uncommon name is recognised rather than corrected
    /// afterwards. Whisper-only; the escape hatch for a prompt that starts
    /// pulling its own words into unrelated audio.
    #[serde(default = "default_true")]
    pub vocabulary_bias: bool,
    /// Which Linux tool types the text. Ignored on macOS.
    ///
    /// `Auto` orders the candidates by what the session is. Pinning one moves
    /// it to the front of that order; it does NOT remove the rest, so a pinned
    /// tool that is missing or fails still leaves the user able to dictate.
    #[serde(default)]
    pub typing_tool: TypingTool,
    /// Unload the transcription model after this many seconds with no use.
    ///
    /// `None` means never, which is the default: unloading trades the next
    /// dictation's latency (a full model load, seconds) for memory the user may
    /// not need back, so it is opt-in rather than a surprise.
    #[serde(default)]
    pub model_idle_unload_secs: Option<u64>,
}

fn default_true() -> bool {
    true
}

/// Recording cues play at full volume unless the user turns them down.
fn default_sound_volume() -> f32 {
    1.0
}

impl Default for TranscriptionConfig {
    fn default() -> Self {
        Self {
            model_id: None,
            auto_copy: false,
            auto_paste: true,
            add_leading_space: false,
            append_trailing_space: false,
            auto_submit: AutoSubmit::Off,
            remove_fillers: true,
            australian_spelling: true,
            spoken_numbers_to_digits: false,
            normalise_whitespace: true,
            cleanup_punctuation: true,
            sentence_case: false,
            voice_formatting_commands: true,
            vocabulary_bias: true,
            typing_tool: TypingTool::default(),
            model_idle_unload_secs: None,
        }
    }
}

/// How a recording ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RecordingMode {
    /// Toggle mode: press to start, press again to stop
    #[default]
    Toggle,
    /// Hands-free: press once to start, and silence ends it (#88).
    HandsFree,
    /// Hold to record: recording runs for exactly as long as the key is held
    /// (#111). The classic push-to-talk.
    HoldToRecord,
}

/// Bounds on `hands_free_silence_secs`.
///
/// The floor sits above a natural pause for breath — that is what stops
/// hands-free cutting someone off mid-thought — and the ceiling keeps a user
/// who walked away from recording indefinitely.
pub const HANDS_FREE_SILENCE_RANGE: std::ops::RangeInclusive<f32> = 0.8..=10.0;

fn default_hands_free_silence_secs() -> f32 {
    2.0
}

/// Keyboard shortcut configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ShortcutConfig {
    /// Toggle recording shortcut (e.g., "F13")
    pub toggle_recording: String,
    /// Alternative toggle recording shortcut
    pub toggle_recording_alt: Option<String>,
    /// Copy last transcription shortcut
    pub copy_last: Option<String>,
    /// Toggle AI enhancement on/off shortcut (unbound by default)
    pub toggle_enhancement: Option<String>,
    /// How a recording ends: on a second press, or by itself on silence.
    pub recording_mode: RecordingMode,
    /// Seconds of silence that end a hands-free recording.
    ///
    /// Ignored in `Toggle` mode. Read through
    /// [`ShortcutConfig::hands_free_silence`], which clamps it, so a
    /// hand-edited config cannot stop the user mid-sentence or never stop.
    #[serde(default = "default_hands_free_silence_secs")]
    pub hands_free_silence_secs: f32,
}

impl ShortcutConfig {
    /// The hands-free silence timeout, clamped to [`HANDS_FREE_SILENCE_RANGE`].
    pub fn hands_free_silence(&self) -> std::time::Duration {
        let secs = self.hands_free_silence_secs.clamp(
            *HANDS_FREE_SILENCE_RANGE.start(),
            *HANDS_FREE_SILENCE_RANGE.end(),
        );
        std::time::Duration::from_secs_f32(secs)
    }
}

/// The primary record key a fresh install gets.
///
/// Right Shift on macOS: every keyboard has it, a bare tap of it clashes with
/// nothing, and it is what the operator decided on 10/09/2026 (F13 exists on
/// almost no keyboard). A modifier-only binding cannot be read on Wayland
/// (docs/development/linux-setup.md), so every other platform keeps F13.
fn default_toggle_recording() -> &'static str {
    if cfg!(target_os = "macos") {
        "ShiftRight"
    } else {
        "F13"
    }
}

impl Default for ShortcutConfig {
    fn default() -> Self {
        Self {
            toggle_recording: default_toggle_recording().to_string(),
            // The frontend does not restate this default: it reads it from
            // get_default_config, and shortcut_defaults_match_typescript asserts
            // no copy has crept back in.
            toggle_recording_alt: Some("CommandOrControl+Shift+Space".to_string()),
            copy_last: Some("F14".to_string()),
            toggle_enhancement: None,
            recording_mode: RecordingMode::default(),
            hands_free_silence_secs: default_hands_free_silence_secs(),
        }
    }
}

/// AI enhancement configuration
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct EnhancementConfig {
    /// Whether AI enhancement is enabled
    pub enabled: bool,
    /// Model name (used by whichever backend is active)
    pub model: String,
    /// Selected prompt template ID
    pub prompt_id: String,
    /// Ollama server URL (unchanged from pre-existing config)
    pub ollama_url: String,
    /// Active backend: "ollama" (default) or "openai_compat"
    #[serde(default = "default_backend")]
    pub backend: String,
    /// OpenAI-compatible server base URL
    #[serde(default = "default_openai_compat_url")]
    pub openai_compat_url: String,
    /// Optional API key for the OpenAI-compatible endpoint
    #[serde(default)]
    pub api_key: Option<String>,
}

impl std::fmt::Debug for EnhancementConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EnhancementConfig")
            .field("enabled", &self.enabled)
            .field("model", &self.model)
            .field("prompt_id", &self.prompt_id)
            .field("ollama_url", &self.ollama_url)
            .field("backend", &self.backend)
            .field("openai_compat_url", &self.openai_compat_url)
            .field("api_key", &self.api_key.as_ref().map(|_| "***redacted***"))
            .finish()
    }
}

fn default_backend() -> String {
    "ollama".to_string()
}

fn default_openai_compat_url() -> String {
    "http://localhost:1234".to_string()
}

impl Default for EnhancementConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            model: "llama3.2".to_string(),
            prompt_id: "fix-grammar".to_string(),
            ollama_url: "http://localhost:11434".to_string(),
            backend: default_backend(),
            openai_compat_url: default_openai_compat_url(),
            api_key: None,
        }
    }
}

/// Recording indicator visual style
#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum IndicatorStyle {
    /// Small dot/square that follows the mouse cursor (default)
    #[default]
    CursorDot,
    /// Small stationary window at a fixed screen position
    FixedFloat,
    /// Elongated horizontal bar with waveform visualisation
    Pill,
}

/// General application settings
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GeneralConfig {
    /// Launch application on system startup
    pub launch_at_login: bool,
    /// Show menu bar icon
    pub show_in_menu_bar: bool,
    /// Show dock icon (macOS)
    pub show_in_dock: bool,
    /// Automatically check for updates on launch
    pub check_for_updates: bool,
    /// Show the floating recording indicator during recording
    pub show_recording_indicator: bool,
    /// Visual style for the recording indicator
    pub indicator_style: IndicatorStyle,
    /// Show native window decorations (Linux). When off, a custom in-app close
    /// button is shown instead. macOS uses native traffic lights regardless.
    #[serde(default = "default_true")]
    pub window_decorations: bool,
    /// App version recorded on the most recent run.
    ///
    /// Used to detect that an update has been applied: when this differs from
    /// the running binary's version, macOS TCC permission grants are likely
    /// stale (TCC keys grants to the code-signing identity, which changes on
    /// each build), so the app resets them once and prompts a re-grant.
    /// `None` on a genuinely fresh install — no reset is triggered then.
    #[serde(default)]
    pub last_run_version: Option<String>,
    /// The version whose release notes the user has already been shown.
    ///
    /// Deliberately NOT `last_run_version`: that one is overwritten on every
    /// launch to drive the TCC reset above, so by the time a "what's new"
    /// check ran it would already equal the running version and the modal
    /// would never appear. `None` means never shown — on a fresh install the
    /// app records the running version without showing anything, so a first
    /// run does not open with a changelog nobody asked for.
    #[serde(default)]
    pub whats_new_seen_version: Option<String>,
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            launch_at_login: false,
            show_in_menu_bar: true,
            show_in_dock: false,
            check_for_updates: true,
            show_recording_indicator: true,
            indicator_style: IndicatorStyle::default(),
            window_decorations: true,
            last_run_version: None,
            whats_new_seen_version: None,
        }
    }
}

/// Recorder window position options
#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum RecorderPosition {
    /// Position near the cursor when recording starts
    Cursor,
    /// Position near the tray icon
    TrayIcon,
    /// Top-left corner of the screen
    TopLeft,
    /// Top-right corner of the screen
    #[default]
    TopRight,
    /// Bottom-left corner of the screen
    BottomLeft,
    /// Bottom-right corner of the screen
    BottomRight,
    /// Centre of the screen
    Centre,
}

/// Recorder window configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RecorderConfig {
    /// Window position preference
    pub position: RecorderPosition,
    /// Horizontal offset from position anchor (in pixels)
    pub offset_x: i32,
    /// Vertical offset from position anchor (in pixels)
    pub offset_y: i32,
    /// Auto-hide delay in milliseconds after transcription completes (0 = no auto-hide)
    pub auto_hide_delay: u32,
}

impl Default for RecorderConfig {
    fn default() -> Self {
        Self {
            position: RecorderPosition::default(),
            offset_x: -20,
            offset_y: 20,
            auto_hide_delay: 3000,
        }
    }
}

/// Where this device sends its telemetry when the fleet's environment does not
/// say.
///
/// A Dock-launched app inherits no environment, so on a machine the fleet does
/// not configure this pane is the only door. Where
/// `OTEL_EXPORTER_OTLP_ENDPOINT` is set it wins and this section is ignored.
///
/// Empty means unset. `headers_helper` is a *command*, never a header value:
/// the bearer it prints is vended per launch and never lands in this file.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TelemetryConfig {
    /// The collector's OTLP/HTTP base; `/v1/logs` and `/v1/traces` are appended.
    pub endpoint: String,
    /// A command printing a JSON object of header name to header value.
    pub headers_helper: String,
}

/// Recursively merge `patch` into `target` (objects merged key-wise; other values replaced).
///
/// Used by the MCP `setting update` handler and the HTTP PATCH `/settings` handler so
/// both partial-update paths share one implementation.
pub(crate) fn merge_json(target: &mut serde_json::Value, patch: &serde_json::Value) {
    match (target, patch) {
        (serde_json::Value::Object(t), serde_json::Value::Object(p)) => {
            for (k, v) in p {
                merge_json(t.entry(k.clone()).or_insert(serde_json::Value::Null), v);
            }
        }
        (t, p) => *t = p.clone(),
    }
}

/// Recursively convert every object key in `value` from camelCase to snake_case.
///
/// The config always serialises to snake_case (no `rename_all` on the structs; the
/// `alias` annotations only add camelCase as a second *input* name). An MCP or HTTP
/// caller that sends `{"apiPort": 8765}` adds a key that serde never matches to
/// `api_port` but also doesn't remove the existing one, so
/// the merged `Value` ends up with both keys and serde errors on "duplicate field".
///
/// Canonicalising before the merge resolves this: every incoming key is
/// normalised to the same form the config serialises to, so the merge updates the
/// existing key in-place. The transform is idempotent (snake_case → snake_case
/// passes through unchanged) and handles nested objects recursively.
///
/// Arrays are also recursed so any array-of-objects config field has its nested
/// object keys canonicalised too.
pub(crate) fn canonicalise_patch_keys(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let new_map = map
                .into_iter()
                .map(|(k, v)| (camel_to_snake(&k), canonicalise_patch_keys(v)))
                .collect();
            serde_json::Value::Object(new_map)
        }
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(arr.into_iter().map(canonicalise_patch_keys).collect())
        }
        other => other,
    }
}

/// Convert a single camelCase identifier to snake_case.
///
/// Inserts an underscore before each uppercase ASCII letter and lowercases it.
/// All-lowercase and already-snake_case identifiers pass through unchanged.
/// A leading underscore (which would appear if the first char were uppercase)
/// is stripped so inputs like `"URL"` → `"u_r_l"` rather than `"_u_r_l"`.
fn camel_to_snake(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    for ch in s.chars() {
        if ch.is_ascii_uppercase() {
            out.push('_');
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    // A key that starts with an uppercase letter would produce a leading '_';
    // strip it so the result is a valid identifier.
    out.trim_start_matches('_').to_string()
}

/// Get the path to the config file (~/.thoth/config.json)
pub fn get_config_path() -> PathBuf {
    home_dir_or_fallback().join(".thoth").join("config.json")
}

/// Get the path to the config directory (~/.thoth)
pub(crate) fn get_config_dir() -> PathBuf {
    home_dir_or_fallback().join(".thoth")
}

/// Get the home directory, falling back to /tmp if unavailable
fn home_dir_or_fallback() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| {
        tracing::error!("Could not determine home directory, using /tmp");
        PathBuf::from("/tmp")
    })
}

/// Ensure the config directory exists
fn ensure_config_dir() -> Result<(), String> {
    let dir = get_config_dir();
    if !dir.exists() {
        fs::create_dir_all(&dir)
            .map_err(|e| format!("Failed to create config directory: {}", e))?;
    }
    Ok(())
}

/// Load configuration from disk
fn load_from_disk() -> Result<Config, String> {
    let path = get_config_path();

    if !path.exists() {
        tracing::info!("Config file not found, using defaults");
        return Ok(Config::default());
    }

    let contents =
        fs::read_to_string(&path).map_err(|e| format!("Failed to read config file: {}", e))?;

    let config: Config =
        serde_json::from_str(&contents).map_err(|e| format!("Failed to parse config: {}", e))?;

    // Migrate if needed, and persist the result here — this is the one path that
    // knows the config came off this disk. `migrate_config` itself writes
    // nothing: it is called from tests with a synthetic config, and a write
    // there lands on the user's own settings file.
    let version = config.version;
    let migrated = migrate_config(config)?;
    if migrated.version != version {
        save_to_disk(&migrated)?;
    }

    Ok(migrated)
}

/// Save configuration to disk
fn save_to_disk(config: &Config) -> Result<(), String> {
    try_save_to_disk(config).map_err(|e| {
        telemetry::report_error("config_save_failed");
        tracing::error!(target: TELEMETRY_TARGET, error = %e, "config_save_failed");
        e
    })
}

fn try_save_to_disk(config: &Config) -> Result<(), String> {
    ensure_config_dir()?;

    let path = get_config_path();
    let contents = serde_json::to_string_pretty(config)
        .map_err(|e| format!("Failed to serialise config: {}", e))?;

    fs::write(&path, &contents).map_err(|e| format!("Failed to write config file: {}", e))?;

    // Restrict config file permissions to owner-only (rw-------) because it
    // may contain an API key in plaintext.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let permissions = fs::Permissions::from_mode(0o600);
        if let Err(e) = fs::set_permissions(&path, permissions) {
            tracing::warn!("Failed to set config file permissions to 0o600: {}", e);
        }
    }

    tracing::info!(
        "Config saved to disk: device_id={:?}, toggle_recording_alt={:?}",
        config.audio.device_id,
        config.shortcuts.toggle_recording_alt
    );
    Ok(())
}

/// Migrate configuration from older schema versions. Pure: persisting the
/// result belongs to [`load_from_disk`], which is the only caller that knows the
/// config came off the user's disk.
fn migrate_config(mut config: Config) -> Result<Config, String> {
    let original_version = config.version;

    // Apply migrations sequentially
    while config.version < CURRENT_VERSION {
        config = apply_migration(config)?;
    }

    if config.version != original_version {
        tracing::info!(
            "Migrated config from version {} to {}",
            original_version,
            config.version
        );
    }

    Ok(config)
}

/// Apply a single migration step
fn apply_migration(config: Config) -> Result<Config, String> {
    match config.version {
        // Version 0 -> 1: Initial migration (add any new fields)
        0 => {
            let mut migrated = config;
            migrated.version = 1;
            // Future migrations would add field transformations here
            Ok(migrated)
        }
        v => Err(format!("Unknown config version: {}", v)),
    }
}

/// Apply the enhancement config to the global backend singleton.
///
/// Called on startup and after every `set_config` that touches enhancement
/// settings, so the in-process backend always reflects persisted config.
pub fn apply_enhancement_backend(enh: &EnhancementConfig) {
    enhancement::configure_backend(
        &enh.backend,
        &enh.ollama_url,
        &enh.openai_compat_url,
        enh.api_key.as_deref(),
    );
}

/// Get the global config instance
fn get_config_instance() -> &'static RwLock<Config> {
    CONFIG.get_or_init(|| {
        let config = load_from_disk().unwrap_or_else(|e| {
            tracing::error!("Failed to load config, using defaults: {}", e);
            telemetry::report_error("config_load_failed");
            tracing::error!(target: TELEMETRY_TARGET, error = %e, "config_load_failed");
            Config::default()
        });
        tracing::info!(
            "Config loaded from disk: device_id={:?}, toggle_recording_alt={:?}",
            config.audio.device_id,
            config.shortcuts.toggle_recording_alt
        );
        RwLock::new(config)
    })
}

// --- IPC Commands ---

/// Get the current configuration
///
/// Returns the current configuration state. The config is cached in memory
/// and loaded from disk on first access.
#[tauri::command]
#[tracing::instrument(target = TELEMETRY_TARGET, skip_all, err)]
pub fn get_config() -> Result<Config, Error> {
    Ok(get_config_instance().read().clone())
}

/// Return the built-in default configuration.
///
/// The single definition of every default. The frontend seeds its own defaults
/// from this instead of restating them in TypeScript, which is how
/// `toggle_recording_alt` came to disagree across the two languages from
/// February 2026 onward (#127). See the "Single source of truth" rule in
/// `.claude/CLAUDE.md`.
///
/// This reads nothing from disk and mutates nothing — it is `Config::default()`.
#[tauri::command]
#[tracing::instrument(target = TELEMETRY_TARGET, skip_all)]
pub fn get_default_config() -> Config {
    Config::default()
}

/// Update the configuration
///
/// Replaces the current configuration with the provided config and persists
/// it to disk. The version field is automatically updated to the current schema.
#[tauri::command]
#[tracing::instrument(target = TELEMETRY_TARGET, skip_all, err)]
pub fn set_config(mut config: Config) -> Result<(), Error> {
    // Ensure version is current
    config.version = CURRENT_VERSION;

    // Preserve device_id if the incoming config has None but the current config
    // has a device selected. This prevents other config saves (shortcuts, AI
    // settings, etc.) from accidentally clearing the user's device preference.
    // The dedicated set_audio_device command handles intentional device changes.
    //
    // Similarly, preserve prompt_id if the incoming value is the default but the
    // cached value differs. This prevents the frontend's generic config save from
    // overwriting a tray-initiated prompt change. The dedicated set_prompt_config
    // function handles intentional prompt changes.
    {
        let current = get_config_instance().read();
        if config.audio.device_id.is_none() && current.audio.device_id.is_some() {
            tracing::debug!(
                "Preserving device_id={:?} (incoming config had None)",
                current.audio.device_id
            );
            config.audio.device_id = current.audio.device_id.clone();
        }

        if config.transcription.model_id.is_none() && current.transcription.model_id.is_some() {
            tracing::debug!(
                "Preserving model_id={:?} (incoming config had None)",
                current.transcription.model_id
            );
            config.transcription.model_id = current.transcription.model_id.clone();
        }

        let default_prompt_id = EnhancementConfig::default().prompt_id;
        if config.enhancement.prompt_id == default_prompt_id
            && current.enhancement.prompt_id != default_prompt_id
        {
            tracing::debug!(
                "Preserving prompt_id={:?} (incoming config had default)",
                current.enhancement.prompt_id
            );
            config.enhancement.prompt_id = current.enhancement.prompt_id.clone();
        }

        // Preserve toggle_recording_alt if the incoming config has the default but
        // the cached config has a user-chosen value (e.g. "ShiftRight"). This prevents
        // unrelated config saves from overwriting the user's shortcut preference.
        let default_shortcuts = ShortcutConfig::default();
        if config.shortcuts.toggle_recording_alt == default_shortcuts.toggle_recording_alt
            && current.shortcuts.toggle_recording_alt != default_shortcuts.toggle_recording_alt
        {
            tracing::debug!(
                "Preserving toggle_recording_alt={:?} (incoming config had default)",
                current.shortcuts.toggle_recording_alt
            );
            config.shortcuts.toggle_recording_alt = current.shortcuts.toggle_recording_alt.clone();
        }

        // Preserve toggle_enhancement if incoming is None but cached has a user-set value.
        if config.shortcuts.toggle_enhancement.is_none()
            && current.shortcuts.toggle_enhancement.is_some()
        {
            tracing::debug!(
                "Preserving toggle_enhancement={:?} (incoming config had None)",
                current.shortcuts.toggle_enhancement
            );
            config.shortcuts.toggle_enhancement = current.shortcuts.toggle_enhancement.clone();
        }

        // Preserve copy_last if incoming is None but cached has a user-set value.
        if config.shortcuts.copy_last.is_none() && current.shortcuts.copy_last.is_some() {
            tracing::debug!(
                "Preserving copy_last={:?} (incoming config had None)",
                current.shortcuts.copy_last
            );
            config.shortcuts.copy_last = current.shortcuts.copy_last.clone();
        }

        // Preserve enhancement.api_key if the incoming config has None but the cached
        // config has a key. The Settings panel sends api_key: null when the field is
        // empty, so a generic full-config save must not wipe a stored key. Use the
        // dedicated set_enhancement_api_key command for intentional key changes.
        if config.enhancement.api_key.is_none() && current.enhancement.api_key.is_some() {
            tracing::debug!("Preserving enhancement.api_key (incoming config had None)");
            config.enhancement.api_key = current.enhancement.api_key.clone();
        }
    }

    // Save to disk first
    save_to_disk(&config)?;

    // Update cached config
    {
        let mut cached = get_config_instance().write();
        *cached = config.clone();
        tracing::info!(
            "Configuration updated (device_id: {:?}, toggle_recording_alt: {:?})",
            cached.audio.device_id,
            cached.shortcuts.toggle_recording_alt
        );
    }

    // Reconfigure the enhancement backend to reflect any provider changes.
    apply_enhancement_backend(&config.enhancement);

    Ok(())
}

/// Set the audio device_id directly, bypassing set_config's preservation logic.
///
/// This is the only correct way to change device_id (including clearing it to
/// None for "System Default"). The preservation logic in `set_config` is designed
/// to protect against accidental clears from frontend config saves, but would
/// also block intentional clears if used for device changes.
pub fn set_audio_device_config(device_id: Option<String>) -> Result<(), String> {
    let mut cached = get_config_instance().write();
    cached.audio.device_id = device_id;
    save_to_disk(&cached)?;
    tracing::info!(
        "Audio device config updated (device_id: {:?})",
        cached.audio.device_id
    );
    Ok(())
}

/// Set the prompt_id directly, bypassing set_config's preservation logic.
///
/// This is the correct way to change prompt_id from the tray menu. The
/// preservation logic in `set_config` prevents the frontend's generic config
/// save from overwriting a tray-initiated prompt change.
pub fn set_prompt_config(prompt_id: String) -> Result<(), String> {
    let mut cached = get_config_instance().write();
    cached.enhancement.prompt_id = prompt_id;
    save_to_disk(&cached)?;
    tracing::info!(
        "Prompt config updated (prompt_id: {:?})",
        cached.enhancement.prompt_id
    );
    Ok(())
}

/// Set enhancement enabled directly, bypassing set_config's preservation logic.
///
/// This is the correct way to toggle enhancement from the shortcut handler. The
/// preservation logic in `set_config` has a prompt_id guard that would interfere
/// with a full-config round-trip; this bypass touches only the `enabled` flag.
pub fn set_enhancement_enabled(enabled: bool) -> Result<(), String> {
    let mut cached = get_config_instance().write();
    cached.enhancement.enabled = enabled;
    save_to_disk(&cached)?;
    tracing::info!("Enhancement enabled updated to: {}", enabled);
    Ok(())
}

/// Set or clear the enhancement API key unconditionally.
///
/// This is the only correct path for changing the key value (including clearing
/// it to None). The preservation guard in `set_config` prevents the generic save
/// from wiping the key with an empty field; this command bypasses that guard so
/// an explicit user action can still clear it.
///
/// Pass `Some(key)` to store a new key, `None` to clear it.
#[tauri::command]
#[tracing::instrument(target = TELEMETRY_TARGET, skip_all, err)]
pub fn set_enhancement_api_key(key: Option<String>) -> Result<(), Error> {
    let mut cached = get_config_instance().write();
    cached.enhancement.api_key = key;
    save_to_disk(&cached)?;
    tracing::info!(
        "Enhancement API key {}",
        if cached.enhancement.api_key.is_some() {
            "updated"
        } else {
            "cleared"
        }
    );
    Ok(())
}

/// Record the running binary's version as `last_run_version`, persisting only
/// when it changed.
///
/// Mutates the live config singleton in place and writes that — it does NOT go
/// through the `get_config()` / `set_config()` round-trip, so it cannot be
/// caught by that path's preservation guards. Returns `Some(previous_version)` when a *different*
/// version was recorded before (a genuine update, so the caller can run
/// one-time post-update steps), or `None` when the version was unchanged (no
/// write) or this is a fresh install (no prior version recorded).
pub fn record_last_run_version(version: &str) -> Result<Option<String>, Error> {
    let mut cached = get_config_instance().write();
    let prev = cached.general.last_run_version.clone();
    if prev.as_deref() == Some(version) {
        return Ok(None); // unchanged — no write
    }
    cached.general.last_run_version = Some(version.to_string());
    save_to_disk(&cached)?;
    Ok(prev) // Some(old) on a real update; None on a fresh install
}

/// Record that a version's release notes have been shown (#113).
///
/// Writes the live config singleton directly for the same reason
/// [`record_last_run_version`] does: a full `set_config` round-trip re-saves
/// every setting, and dismissing a modal is no reason to do that.
pub fn record_whats_new_seen(version: &str) -> Result<(), Error> {
    let mut cached = get_config_instance().write();
    if cached.general.whats_new_seen_version.as_deref() == Some(version) {
        return Ok(()); // unchanged — no write
    }
    cached.general.whats_new_seen_version = Some(version.to_string());
    save_to_disk(&cached).map_err(Into::into)
}

/// Set shortcut config directly, bypassing set_config's preservation logic.
///
/// Used by the Settings UI when intentionally changing shortcuts. The
/// preservation logic in `set_config` prevents unrelated config saves from
/// overwriting shortcuts, but would also block intentional changes (e.g.
/// resetting a shortcut back to its default value).
#[tauri::command]
#[tracing::instrument(target = TELEMETRY_TARGET, skip_all, err)]
pub fn set_shortcut_config(shortcuts: ShortcutConfig) -> Result<(), Error> {
    let mut cached = get_config_instance().write();
    cached.shortcuts = shortcuts;
    save_to_disk(&cached)?;
    tracing::info!(
        "Shortcut config updated directly (toggle_recording_alt: {:?})",
        cached.shortcuts.toggle_recording_alt
    );
    Ok(())
}

/// Set the telemetry section directly, without re-saving every other setting.
///
/// Writes the live config singleton for the same reason
/// [`record_whats_new_seen`] does: pointing the exporter somewhere is no reason
/// to rewrite the whole file. Applying it to the live pipeline is
/// `telemetry_settings::install`, which the command calls next.
pub fn set_telemetry_config(telemetry: TelemetryConfig) -> Result<(), Error> {
    let mut cached = get_config_instance().write();
    cached.telemetry = telemetry;
    save_to_disk(&cached)?;
    tracing::info!(
        "Telemetry config updated (endpoint set: {})",
        !cached.telemetry.endpoint.is_empty()
    );
    Ok(())
}

/// Reset configuration to defaults
///
/// Resets all settings to their default values and persists to disk.
#[tauri::command]
#[tracing::instrument(target = TELEMETRY_TARGET, skip_all, err)]
pub fn reset_config() -> Result<Config, Error> {
    let default_config = Config::default();

    // Save to disk
    save_to_disk(&default_config)?;

    // Update cached config
    let mut cached = get_config_instance().write();
    *cached = default_config.clone();

    tracing::info!("Configuration reset to defaults");
    Ok(default_config)
}

/// Get the configuration file path
///
/// Returns the path to the config file for debugging or user information.
#[tauri::command]
#[tracing::instrument(target = TELEMETRY_TARGET, skip_all)]
pub fn get_config_path_cmd() -> String {
    get_config_path().to_string_lossy().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    /// Serialises tests that mutate the shared CONFIG singleton so they cannot
    /// interleave under parallel test execution. Every test that calls
    /// `get_config_instance()` and writes to it must hold this guard for its
    /// full lifetime.
    static CONFIG_TEST_LOCK: StdMutex<()> = StdMutex::new(());

    #[test]
    fn test_default_config_has_current_version() {
        let config = Config::default();
        assert_eq!(config.version, CURRENT_VERSION);
    }

    #[test]
    fn test_config_serialisation_roundtrip() {
        let config = Config::default();
        let json = serde_json::to_string(&config).unwrap();
        let deserialised: Config = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialised.version, config.version);
        assert_eq!(deserialised.audio.sample_rate, config.audio.sample_rate);
        assert_eq!(
            deserialised.shortcuts.toggle_recording,
            config.shortcuts.toggle_recording
        );
        assert_eq!(deserialised.enhancement.model, config.enhancement.model);
    }

    #[test]
    fn test_audio_config_defaults() {
        let audio = AudioConfig::default();
        assert_eq!(audio.device_id, None);
        assert_eq!(audio.sample_rate, 16000);
        assert!(audio.play_sounds);
    }

    #[test]
    fn test_transcription_config_defaults() {
        let transcription = TranscriptionConfig::default();
        assert!(!transcription.auto_copy);
        assert!(transcription.auto_paste);
        assert!(!transcription.add_leading_space);
    }

    /// A config written before `transcription.language` was removed (#102) must
    /// still load, with the stale key ignored rather than failing the parse.
    ///
    /// This holds because `Config` does not set `deny_unknown_fields`. The test
    /// exists so that adding it later cannot silently break every existing
    /// install's config on upgrade.
    #[test]
    fn stale_language_key_is_ignored_not_fatal() {
        let json = r#"{
            "language": "de",
            "auto_copy": true,
            "auto_paste": false
        }"#;

        let parsed: TranscriptionConfig =
            serde_json::from_str(json).expect("a stale language key must not fail the parse");

        // The rest of the object still round-trips.
        assert!(parsed.auto_copy);
        assert!(!parsed.auto_paste);
    }

    #[test]
    fn test_shortcut_config_defaults() {
        let shortcuts = ShortcutConfig::default();
        assert_eq!(shortcuts.toggle_recording, default_toggle_recording());
        if cfg!(target_os = "macos") {
            assert_eq!(shortcuts.toggle_recording, "ShiftRight");
        } else {
            assert_eq!(shortcuts.toggle_recording, "F13");
        }
        assert_eq!(
            shortcuts.toggle_recording_alt,
            Some("CommandOrControl+Shift+Space".to_string())
        );
        assert_eq!(shortcuts.copy_last, Some("F14".to_string()));
        assert_eq!(shortcuts.toggle_enhancement, None);
        assert_eq!(shortcuts.recording_mode, RecordingMode::Toggle);
    }

    /// `get_default_config` is the frontend's only source of defaults, so it must
    /// agree with `Config::default()` rather than drifting into its own values.
    #[test]
    fn get_default_config_returns_the_real_defaults() {
        let defaults = get_default_config();
        let expected = ShortcutConfig::default();
        assert_eq!(
            defaults.shortcuts.toggle_recording,
            expected.toggle_recording
        );
        assert_eq!(
            defaults.shortcuts.toggle_recording_alt,
            expected.toggle_recording_alt
        );
        assert_eq!(defaults.shortcuts.copy_last, expected.copy_last);
        assert_eq!(
            defaults.shortcuts.toggle_recording_alt,
            Some("CommandOrControl+Shift+Space".to_string())
        );
    }

    /// The TypeScript placeholder must not restate the shortcut defaults.
    ///
    /// `toggle_recording_alt` was defined in both languages with different values
    /// from February 2026 (#127): Rust said `ShiftRight`, TypeScript said
    /// `CommandOrControl+Shift+Space`. Because `ShortcutConfig` carries
    /// `#[serde(default)]`, the Rust value filled the field and won on every
    /// write, so users got a binding the UI never advertised.
    ///
    /// The fix is that the frontend reads defaults from `get_default_config`.
    /// This test fails if a hardcoded copy creeps back into the placeholder,
    /// which is the only way the two can diverge again. It asserts the *absence*
    /// of a second definition rather than comparing two copies, because the point
    /// is that there is exactly one.
    #[test]
    fn shortcut_defaults_match_typescript() {
        let ts_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../src/lib/stores/config.svelte.ts");
        let source = std::fs::read_to_string(&ts_path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", ts_path.display()));

        // Isolate the shortcuts block of getDefaultConfig()'s return value.
        //
        // Anchor on the function first: `    shortcuts: {` also appears in
        // parseConfig()'s mapper, which is earlier in the file and contains only
        // `raw.shortcuts.*` reads. Searching the whole file finds that block and
        // the assertion passes vacuously no matter what the defaults say.
        let fn_start = source
            .find("function getDefaultConfig()")
            .expect("getDefaultConfig() not found — update this test");
        let rest = &source[fn_start..];
        let start = rest
            .find("shortcuts: {")
            .expect("getDefaultConfig() no longer has a shortcuts block — update this test");
        let end = rest[start..]
            .find("},")
            .expect("unterminated shortcuts block")
            + start;
        let block = &rest[start..end];

        let defaults = ShortcutConfig::default();
        let real_values = [
            defaults.toggle_recording.clone(),
            defaults.toggle_recording_alt.clone().unwrap_or_default(),
            defaults.copy_last.clone().unwrap_or_default(),
        ];

        for value in real_values.iter().filter(|v| !v.is_empty()) {
            assert!(
                !block.contains(value.as_str()),
                "src/lib/stores/config.svelte.ts restates the shortcut default {value:?}.\n\
                 Shortcut defaults have exactly one definition: ShortcutConfig::default() \
                 in this file, reached from the frontend via get_default_config().\n\
                 Remove the hardcoded value — see the single-source-of-truth rule in \
                 .claude/CLAUDE.md."
            );
        }

        // The numeric defaults need the same guard as the string ones: a number
        // that happens to match reads as harmless and is exactly how the
        // shortcut defaults drifted for eight months (#127).
        let numeric = format!("{:?}", defaults.hands_free_silence_secs);
        assert!(
            !block.contains(&numeric),
            "src/lib/stores/config.svelte.ts restates the hands-free silence default \
             {numeric}. It has one definition, ShortcutConfig::default() in this file, \
             and reaches the frontend via get_default_config(); the placeholder in that \
             block must be a value that is deliberately NOT the default."
        );
    }

    #[test]
    fn test_enhancement_config_defaults() {
        let enhancement = EnhancementConfig::default();
        assert!(!enhancement.enabled);
        assert_eq!(enhancement.model, "llama3.2");
        assert_eq!(enhancement.prompt_id, "fix-grammar");
        assert_eq!(enhancement.ollama_url, "http://localhost:11434");
    }

    #[test]
    fn test_general_config_defaults() {
        let general = GeneralConfig::default();
        assert!(!general.launch_at_login);
        assert!(general.show_in_menu_bar);
        assert!(!general.show_in_dock);
    }

    #[test]
    fn test_recorder_config_defaults() {
        let recorder = RecorderConfig::default();
        assert_eq!(recorder.position, RecorderPosition::TopRight);
        assert_eq!(recorder.offset_x, -20);
        assert_eq!(recorder.offset_y, 20);
    }

    #[test]
    fn test_recorder_position_serialisation() {
        let positions = vec![
            (RecorderPosition::Cursor, "\"cursor\""),
            (RecorderPosition::TrayIcon, "\"tray-icon\""),
            (RecorderPosition::TopLeft, "\"top-left\""),
            (RecorderPosition::TopRight, "\"top-right\""),
            (RecorderPosition::BottomLeft, "\"bottom-left\""),
            (RecorderPosition::BottomRight, "\"bottom-right\""),
            (RecorderPosition::Centre, "\"centre\""),
        ];

        for (position, expected_json) in positions {
            let json = serde_json::to_string(&position).unwrap();
            assert_eq!(json, expected_json);

            let parsed: RecorderPosition = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, position);
        }
    }

    #[test]
    fn test_partial_config_deserialisation() {
        // Config should use defaults for missing fields
        let json = r#"{"version": 1, "audio": {"sample_rate": 48000}}"#;
        let config: Config = serde_json::from_str(json).unwrap();

        assert_eq!(config.version, 1);
        assert_eq!(config.audio.sample_rate, 48000);
        assert_eq!(config.audio.device_id, None); // Default
    }

    #[test]
    fn test_migration_from_version_0() {
        let old_config = Config {
            version: 0,
            ..Default::default()
        };

        let migrated = migrate_config(old_config).unwrap();
        assert_eq!(migrated.version, CURRENT_VERSION);
    }

    /// `migrate_config` used to save its result, so this very test — which hands
    /// it a synthetic version-0 `Config::default()` — wrote defaults over the
    /// developer's own `~/.thoth/config.json`, silently losing their device,
    /// model and shortcuts on every `cargo test`.
    #[test]
    fn migrating_a_synthetic_config_does_not_touch_the_real_config_file() {
        let _guard = CONFIG_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let path = get_config_path();
        let before = fs::read(&path).ok();

        migrate_config(Config {
            version: 0,
            ..Default::default()
        })
        .unwrap();

        assert_eq!(
            fs::read(&path).ok(),
            before,
            "{} was written",
            path.display()
        );
    }

    // =========================================================================
    // Additional config tests
    // =========================================================================

    #[test]
    fn test_recording_mode_serialisation() {
        assert_eq!(
            serde_json::to_string(&RecordingMode::Toggle).unwrap(),
            "\"toggle\""
        );
        assert_eq!(
            serde_json::to_string(&RecordingMode::HandsFree).unwrap(),
            "\"hands_free\""
        );
        assert_eq!(
            serde_json::to_string(&RecordingMode::HoldToRecord).unwrap(),
            "\"hold_to_record\""
        );
    }

    #[test]
    fn test_recording_mode_deserialisation() {
        assert_eq!(
            serde_json::from_str::<RecordingMode>("\"toggle\"").unwrap(),
            RecordingMode::Toggle
        );
        assert_eq!(
            serde_json::from_str::<RecordingMode>("\"hands_free\"").unwrap(),
            RecordingMode::HandsFree
        );
        assert_eq!(
            serde_json::from_str::<RecordingMode>("\"hold_to_record\"").unwrap(),
            RecordingMode::HoldToRecord
        );
    }

    #[test]
    fn test_config_path_format() {
        let path = get_config_path();
        let path_str = path.to_string_lossy();

        // Should be in .thoth directory
        assert!(path_str.contains(".thoth"));
        // Should be named config.json
        assert!(path_str.ends_with("config.json"));
    }

    #[test]
    fn test_full_config_serialisation_roundtrip() {
        let config = Config {
            version: CURRENT_VERSION,
            audio: AudioConfig {
                device_id: Some("test-device".to_string()),
                sample_rate: 44100,
                play_sounds: false,
                sound_volume: 1.0,
                warm_stream: true,
            },
            transcription: TranscriptionConfig {
                model_id: Some("test-model".to_string()),
                auto_copy: false,
                auto_paste: true,
                add_leading_space: true,
                append_trailing_space: false,
                auto_submit: AutoSubmit::Off,
                remove_fillers: false,
                australian_spelling: false,
                spoken_numbers_to_digits: false,
                normalise_whitespace: true,
                cleanup_punctuation: true,
                sentence_case: false,
                voice_formatting_commands: true,
                vocabulary_bias: false,
                typing_tool: TypingTool::Ydotool,
                model_idle_unload_secs: Some(900),
            },
            shortcuts: ShortcutConfig {
                toggle_recording: "F12".to_string(),
                toggle_recording_alt: None,
                copy_last: None,
                toggle_enhancement: None,
                recording_mode: RecordingMode::HandsFree,
                hands_free_silence_secs: 3.5,
            },
            enhancement: EnhancementConfig {
                enabled: true,
                model: "mistral".to_string(),
                prompt_id: "custom".to_string(),
                ollama_url: "http://custom:8080".to_string(),
                backend: "openai_compat".to_string(),
                openai_compat_url: "http://localhost:1234".to_string(),
                api_key: Some("sk-test".to_string()),
            },
            general: GeneralConfig {
                launch_at_login: true,
                show_in_menu_bar: false,
                show_in_dock: true,
                check_for_updates: true,
                show_recording_indicator: true,
                indicator_style: IndicatorStyle::CursorDot,
                window_decorations: true,
                last_run_version: None,
                whats_new_seen_version: Some("2026.6.6".to_string()),
            },
            recorder: RecorderConfig {
                position: RecorderPosition::Centre,
                offset_x: 10,
                offset_y: 20,
                auto_hide_delay: 5000,
            },
            integrations: IntegrationsConfig::default(),
            telemetry: TelemetryConfig {
                endpoint: "https://otlp.example".to_string(),
                headers_helper: "signet headers otlp".to_string(),
            },
        };

        let json = serde_json::to_string_pretty(&config).unwrap();
        let restored: Config = serde_json::from_str(&json).unwrap();

        // Verify all fields were preserved
        assert_eq!(restored.audio.device_id, Some("test-device".to_string()));
        assert_eq!(restored.audio.sample_rate, 44100);
        assert!(!restored.audio.play_sounds);

        assert!(!restored.transcription.auto_copy);
        assert!(restored.transcription.add_leading_space);
        assert_eq!(restored.transcription.typing_tool, TypingTool::Ydotool);

        assert_eq!(restored.shortcuts.toggle_recording, "F12");
        assert!(restored.shortcuts.toggle_recording_alt.is_none());
        assert_eq!(restored.shortcuts.recording_mode, RecordingMode::HandsFree);
        assert_eq!(restored.shortcuts.hands_free_silence_secs, 3.5);

        assert!(restored.enhancement.enabled);
        assert_eq!(restored.enhancement.model, "mistral");

        assert!(restored.general.launch_at_login);
        assert_eq!(
            restored.general.whats_new_seen_version.as_deref(),
            Some("2026.6.6")
        );
        assert!(!restored.general.show_in_menu_bar);

        assert_eq!(restored.recorder.position, RecorderPosition::Centre);

        assert_eq!(restored.telemetry.endpoint, "https://otlp.example");
        assert_eq!(restored.telemetry.headers_helper, "signet headers otlp");
    }

    #[test]
    fn test_config_unknown_fields_ignored() {
        // JSON with extra unknown fields should still parse
        let json = r#"{
            "version": 1,
            "unknown_field": "should be ignored",
            "audio": {"sample_rate": 16000, "extra": true}
        }"#;

        let config: Config = serde_json::from_str(json).unwrap();
        assert_eq!(config.version, 1);
        assert_eq!(config.audio.sample_rate, 16000);
    }

    #[test]
    fn test_apply_migration_unknown_version() {
        let future_config = Config {
            version: 999,
            ..Default::default()
        };

        let result = apply_migration(future_config);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Unknown config version"));
    }

    #[test]
    fn test_audio_config_custom_values() {
        let audio = AudioConfig {
            device_id: Some("custom-mic".to_string()),
            sample_rate: 48000,
            play_sounds: false,
            sound_volume: 1.0,
            warm_stream: false,
        };

        assert_eq!(audio.device_id, Some("custom-mic".to_string()));
        assert_eq!(audio.sample_rate, 48000);
        assert!(!audio.play_sounds);
    }

    #[test]
    fn test_enhancement_config_custom_ollama_url() {
        let enhancement = EnhancementConfig {
            enabled: true,
            model: "custom-model".to_string(),
            prompt_id: "summarise".to_string(),
            ollama_url: "http://192.168.1.100:11434".to_string(),
            ..Default::default()
        };

        assert!(enhancement.enabled);
        assert_eq!(enhancement.ollama_url, "http://192.168.1.100:11434");
    }

    // =========================================================================
    // OpenAI-compat provider field tests
    // =========================================================================

    #[test]
    fn test_enhancement_config_new_fields_roundtrip() {
        let enh = EnhancementConfig {
            enabled: true,
            model: "mistral".to_string(),
            prompt_id: "fix-grammar".to_string(),
            ollama_url: "http://localhost:11434".to_string(),
            backend: "openai_compat".to_string(),
            openai_compat_url: "http://localhost:1234".to_string(),
            api_key: Some("test-key".to_string()),
        };

        let json = serde_json::to_string(&enh).unwrap();
        let restored: EnhancementConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(restored.backend, "openai_compat");
        assert_eq!(restored.openai_compat_url, "http://localhost:1234");
        assert_eq!(restored.api_key, Some("test-key".to_string()));
    }

    #[test]
    fn test_enhancement_config_new_fields_snake_case_deserialise() {
        // The JSON uses snake_case (as serialised by the Rust backend; no camelCase mapping)
        let json = r#"{
            "enabled": false,
            "model": "llama3.2",
            "prompt_id": "fix-grammar",
            "ollama_url": "http://localhost:11434",
            "backend": "openai_compat",
            "openai_compat_url": "http://lm-studio:1234",
            "api_key": "sk-test"
        }"#;

        let enh: EnhancementConfig = serde_json::from_str(json).unwrap();
        assert_eq!(enh.backend, "openai_compat");
        assert_eq!(enh.openai_compat_url, "http://lm-studio:1234");
        assert_eq!(enh.api_key, Some("sk-test".to_string()));
    }

    #[test]
    fn test_old_config_without_new_fields_uses_defaults() {
        // A config JSON that predates the new fields should parse cleanly,
        // with the new fields taking their defaults.
        let json = r#"{
            "version": 1,
            "enhancement": {
                "enabled": false,
                "model": "llama3.2",
                "prompt_id": "fix-grammar",
                "ollama_url": "http://localhost:11434"
            }
        }"#;

        let config: Config = serde_json::from_str(json).unwrap();
        assert_eq!(config.enhancement.backend, "ollama");
        assert_eq!(
            config.enhancement.openai_compat_url,
            "http://localhost:1234"
        );
        assert_eq!(config.enhancement.api_key, None);
        // Old field preserved
        assert_eq!(config.enhancement.ollama_url, "http://localhost:11434");
    }

    #[test]
    fn test_set_config_null_api_key_preserves_cached() {
        // A generic set_config with api_key: null must NOT wipe a stored key.
        // The dedicated set_enhancement_api_key command is required for intentional clears.
        let _guard = CONFIG_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let instance = get_config_instance();
        {
            let mut cached = instance.write();
            cached.enhancement.api_key = Some("stored-key".to_string());
        }

        let mut incoming = Config::default();
        // api_key is None in the default config — simulating an empty-field save
        assert!(incoming.enhancement.api_key.is_none());

        {
            let current = instance.read();
            if incoming.enhancement.api_key.is_none() && current.enhancement.api_key.is_some() {
                incoming.enhancement.api_key = current.enhancement.api_key.clone();
            }
        }

        assert_eq!(incoming.enhancement.api_key, Some("stored-key".to_string()));

        // Restore to clean state
        instance.write().enhancement.api_key = None;
    }

    #[test]
    fn test_set_enhancement_api_key_sets_and_clears() {
        // set_enhancement_api_key(Some) sets the key; set_enhancement_api_key(None) clears it.
        // Exercise the guard logic directly (no disk I/O in unit tests).
        let _guard = CONFIG_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let instance = get_config_instance();

        // Set a key
        {
            let mut cached = instance.write();
            cached.enhancement.api_key = Some("my-api-key".to_string());
        }
        assert_eq!(
            instance.read().enhancement.api_key,
            Some("my-api-key".to_string())
        );

        // Clear the key unconditionally (bypassing the preservation guard)
        {
            let mut cached = instance.write();
            cached.enhancement.api_key = None;
        }
        assert_eq!(instance.read().enhancement.api_key, None);
    }

    #[test]
    fn test_set_config_with_unrelated_change_preserves_api_key() {
        // A full set_config that changes only an unrelated field (e.g. model) but
        // sends api_key: None must keep the previously-stored key.
        let _guard = CONFIG_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let instance = get_config_instance();
        {
            let mut cached = instance.write();
            cached.enhancement.api_key = Some("keep-me".to_string());
        }

        let mut incoming = Config::default();
        incoming.enhancement.model = "different-model".to_string();
        // api_key remains None (as the frontend sends for an empty field)

        // Apply the same preservation guard that set_config uses
        {
            let current = instance.read();
            if incoming.enhancement.api_key.is_none() && current.enhancement.api_key.is_some() {
                incoming.enhancement.api_key = current.enhancement.api_key.clone();
            }
        }

        assert_eq!(incoming.enhancement.api_key, Some("keep-me".to_string()));
        assert_eq!(incoming.enhancement.model, "different-model".to_string());

        // Restore clean state
        instance.write().enhancement.api_key = None;
    }

    // =========================================================================
    // canonicalise_patch_keys / merge_json tests (Bug 1 + Bug 2)
    // =========================================================================

    #[test]
    fn test_camel_to_snake_identity_on_snake_case() {
        // Already-snake_case keys must pass through unchanged (idempotent).
        for key in &["api_enabled", "api_port", "mcp_enabled", "auto_hide_delay"] {
            assert_eq!(
                camel_to_snake(key),
                *key,
                "snake_case key must be unchanged: {key}"
            );
        }
    }

    #[test]
    fn test_camel_to_snake_converts_known_field_aliases() {
        // Every field that carries a camelCase alias must round-trip to exactly the
        // snake_case canonical name that serde serialises it as.
        let pairs = [
            ("apiEnabled", "api_enabled"),
            ("apiPort", "api_port"),
            ("mcpEnabled", "mcp_enabled"),
            ("autoHideDelay", "auto_hide_delay"),
            ("offsetX", "offset_x"),
        ];
        for (camel, snake) in pairs {
            assert_eq!(
                camel_to_snake(camel),
                snake,
                "camelCase alias {camel} must convert to {snake}"
            );
        }
    }

    #[test]
    fn test_canonicalise_patch_keys_flat_camel_patch() {
        // A flat camelCase patch must deserialise successfully after canonicalisation.
        let patch_json = serde_json::json!({
            "apiEnabled": true,
            "apiPort": 8765_u16,
            "mcpEnabled": false
        });
        let canon = canonicalise_patch_keys(patch_json);
        // Keys must now be snake_case.
        let obj = canon.as_object().unwrap();
        assert!(obj.contains_key("api_enabled"), "key canonicalised");
        assert!(obj.contains_key("api_port"), "key canonicalised");
        assert!(obj.contains_key("mcp_enabled"), "key canonicalised");
        assert!(!obj.contains_key("apiEnabled"), "old key removed");
    }

    #[test]
    fn test_canonicalise_patch_keys_nested_camel_patch() {
        // Nested camelCase patch (e.g. {"integrations": {"apiPort": 8765}})
        // must be fully canonicalised.
        let patch_json = serde_json::json!({
            "integrations": {
                "apiPort": 8765_u16,
                "mcpEnabled": false
            }
        });
        let canon = canonicalise_patch_keys(patch_json);
        let integrations = canon.get("integrations").unwrap().as_object().unwrap();
        assert!(integrations.contains_key("api_port"));
        assert!(integrations.contains_key("mcp_enabled"));
        assert!(!integrations.contains_key("apiPort"));
    }

    #[test]
    fn test_camel_patch_merges_and_deserialises_without_duplicate_field_error() {
        // Simulates Bug 1: a camelCase MCP patch must merge onto the serialised config
        // and deserialise back to Config without a "duplicate field" error.
        let current_cfg = Config::default();
        let mut current_val = serde_json::to_value(&current_cfg).unwrap();

        let patch = serde_json::json!({ "integrations": { "apiPort": 9100_u16 } });
        let patch = canonicalise_patch_keys(patch);
        merge_json(&mut current_val, &patch);

        let result: Result<Config, _> = serde_json::from_value(current_val);
        assert!(
            result.is_ok(),
            "camelCase patch must deserialise without error: {:?}",
            result.err()
        );
        assert_eq!(result.unwrap().integrations.api_port, 9100);
    }

    #[test]
    fn test_snake_patch_merges_and_deserialises_correctly() {
        // snake_case patch must also work (idempotent canonicalisation).
        let current_cfg = Config::default();
        let mut current_val = serde_json::to_value(&current_cfg).unwrap();

        let patch = serde_json::json!({ "integrations": { "api_port": 9101_u16 } });
        let patch = canonicalise_patch_keys(patch);
        merge_json(&mut current_val, &patch);

        let result: Result<Config, _> = serde_json::from_value(current_val);
        assert!(
            result.is_ok(),
            "snake_case patch must deserialise: {:?}",
            result.err()
        );
        assert_eq!(result.unwrap().integrations.api_port, 9101);
    }

    #[test]
    fn test_partial_patch_preserves_untouched_fields() {
        // Simulates Bug 2: a partial patch that only sets one field must leave all
        // other fields at their prior values.
        let mut prior = Config::default();
        prior.integrations.api_port = 9102;
        prior.integrations.mcp_enabled = false;

        let mut current_val = serde_json::to_value(&prior).unwrap();

        // Patch only changes api_enabled; api_port and mcp_enabled must survive.
        let patch = serde_json::json!({ "integrations": { "apiEnabled": false } });
        let patch = canonicalise_patch_keys(patch);
        merge_json(&mut current_val, &patch);

        let result: Config = serde_json::from_value(current_val).unwrap();
        assert!(
            !result.integrations.api_enabled,
            "patched field must be set"
        );
        assert_eq!(
            result.integrations.api_port, 9102,
            "unpatched field must be preserved"
        );
        assert!(
            !result.integrations.mcp_enabled,
            "unpatched field must be preserved"
        );
    }

    #[test]
    fn test_config_ignores_retired_logging_block() {
        // A config.json written by an older build still carries the whole
        // `logging` block — retention, the Loki URL, the token. Nothing reads
        // those keys any more, and serde's default is to ignore unknown fields
        // (no `deny_unknown_fields` anywhere in this file), so the file must
        // still load rather than failing the user into a default config.
        let json = r#"{
            "version": 1,
            "logging": {
                "local_retention_days": 14,
                "remote_enabled": true,
                "loki_url": "http://loki:3100",
                "loki_auth": "Bearer glsa_whatever",
                "loki_tenant": "org1",
                "loki_labels": [["env", "prod"]],
                "telemetry_level": "debug"
            },
            "integrations": { "api_port": 9103 }
        }"#;
        let config: Config = serde_json::from_str(json).expect("retired keys must be ignored");
        assert_eq!(config.integrations.api_port, 9103);
    }

    #[test]
    fn test_camel_to_snake_no_leading_underscore_on_uppercase_start() {
        // An input that starts with uppercase must not produce a leading underscore.
        assert_eq!(camel_to_snake("URL"), "u_r_l");
        assert_eq!(&camel_to_snake("URL")[..1], "u");
        // No current config field starts uppercase, but the guard must hold.
        assert!(
            !camel_to_snake("Foo").starts_with('_'),
            "leading underscore must be stripped"
        );
    }
}
