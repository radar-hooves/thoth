/**
 * Configuration state management store using Svelte 5 runes.
 *
 * Manages application settings with persistence via the Tauri backend.
 * Settings are stored in ~/.thoth/config.json and support schema migrations.
 */

import { invoke } from '@tauri-apps/api/core';
import { buildPatch } from './config-patch';

/** Audio recording configuration */
export interface AudioConfig {
  /** Selected audio input device ID (null for system default) */
  deviceId: string | null;
  /** Sample rate in Hz */
  sampleRate: number;
  /** Whether to play audio feedback sounds */
  playSounds: boolean;
  /** Recording cue volume, 0.0 (silent) to 1.0 (full) */
  soundVolume: number;
}

/** Transcription engine configuration */
export interface TranscriptionConfig {
  /** Whether to automatically copy transcription to clipboard */
  autoCopy: boolean;
  /** Whether to automatically paste transcription at cursor */
  autoPaste: boolean;
  /** Whether to add space before pasted text */
  addLeadingSpace: boolean;
  /** Whether to append a single space after the inserted text */
  appendTrailingSpace: boolean;
  /** Key combination sent after a successful insertion */
  autoSubmit: AutoSubmit;
  /** Whether to remove hesitation sounds (um, uh, er, ah) from transcription */
  removeFillers: boolean;
  /** Whether to convert US spellings to Australian/British equivalents */
  australianSpelling: boolean;
  /** Whether to convert spoken number words to digits */
  spokenNumbersToDigits: boolean;
  /** Whether to collapse runs of whitespace and trim leading/trailing spaces */
  normaliseWhitespace: boolean;
  /** Whether to fix spacing around punctuation marks */
  cleanupPunctuation: boolean;
  /** Whether to capitalise the first word of each sentence */
  sentenceCase: boolean;
  /** Whether to convert spoken formatting commands ("new paragraph" / "new line") into line breaks */
  voiceFormattingCommands: boolean;
  /** Whether to feed dictionary and canonical terms to the decoder as an initial prompt */
  vocabularyBias: boolean;
  /** Which Linux tool types the text; 'auto' orders them by the session */
  typingTool: TypingTool;
  /** Unload the transcription model after this many idle seconds; null means never */
  modelIdleUnloadSecs: number | null;
}

/** Recording mode options */
export type RecordingMode = 'toggle' | 'hands_free' | 'hold_to_record' | 'tap_or_hold';

/**
 * Which Linux tool synthesises keystrokes. Linux only; ignored on macOS.
 *
 * Mirrors the Rust `TypingTool` enum; serialised snake_case over IPC.
 */
export type TypingTool = 'auto' | 'wtype' | 'kwtype' | 'dotool' | 'ydotool' | 'xdotool' | 'enigo';

/**
 * Key combination sent after a successful insertion.
 *
 * Mirrors the Rust `AutoSubmit` enum; serialised snake_case over IPC.
 */
export type AutoSubmit = 'off' | 'enter' | 'ctrl_enter' | 'cmd_enter';

/** Keyboard shortcut configuration */
export interface ShortcutConfig {
  /** Toggle recording shortcut (e.g., "F13") */
  toggleRecording: string;
  /** Alternative toggle recording shortcut */
  toggleRecordingAlt: string | null;
  /** Copy last transcription shortcut */
  copyLast: string | null;
  /** Toggle AI enhancement on/off shortcut (null = unbound) */
  toggleEnhancement: string | null;
  /** How a recording ends: on a second press, or by itself on silence */
  recordingMode: RecordingMode;
  /** Seconds of silence that end a hands-free recording */
  handsFreeSilenceSecs: number;
}

/** Integrations configuration */
export interface IntegrationsConfig {
  /** Whether the loopback HTTP control API is enabled */
  apiEnabled: boolean;
  /** Port for the loopback HTTP control API */
  apiPort: number;
  /** Whether the MCP server is enabled */
  mcpEnabled: boolean;
}

/** Telemetry exporter configuration */
export interface TelemetryConfig {
  /** The collector's OTLP/HTTP base; empty means unset */
  endpoint: string;
  /** A command printing a JSON object of headers; empty means none */
  headersHelper: string;
}

/** Word-list publishing (WebDAV) configuration. Off by default. */
export interface PublishConfig {
  /** Whether edits are published to the WebDAV file */
  enabled: boolean;
  /** The WebDAV URL of the shared word-list file; empty means unset */
  url: string;
  /** The WebDAV username (basic auth); empty for anonymous */
  username: string;
}

/** External dictionary (wordhoard) configuration. Off by default. */
export interface ExternalDictionaryConfig {
  enabled: boolean;
  /** The service's base URL; empty means unset */
  url: string;
  /** The device's login (basic auth) */
  username: string;
  /** Machine name sent with writes; empty uses the hostname */
  device: string;
  /** Command whose output is the app password (the broker); empty uses the typed password */
  credentialHelper: string;
}

/** What the external dictionary card shows. Never the password itself. */
export interface ExternalDictionaryStatus {
  hasPassword: boolean;
  lastSyncAt: string | null;
  lastError: string | null;
}

/** What the word-list publishing card shows. Never the password itself. */
export interface PublishStatus {
  hasPassword: boolean;
  /** RFC 3339 timestamp of the last successful publish, or null */
  lastPublishAt: string | null;
  /** The last publish's failure, or null once it succeeds again */
  lastError: string | null;
}

/**
 * What the Telemetry card shows: the live exporter, whether the environment
 * owns it, and the saved values underneath.
 */
export interface TelemetryStatus {
  /** The endpoint this process is exporting to; empty means local only */
  endpoint: string;
  /** The helper command behind it */
  headersHelper: string;
  /** Log and span providers are installed; not proof the collector is reachable */
  started: boolean;
  /** True when the environment set it, so the card shows it read-only */
  fromEnv: boolean;
  /** The saved endpoint, which the environment overrides where it is set */
  savedEndpoint: string;
  /** The saved helper command */
  savedHeadersHelper: string;
}

/** AI enhancement configuration */
export interface EnhancementConfig {
  /** Whether AI enhancement is enabled */
  enabled: boolean;
  /** Model name used by the active backend */
  model: string;
  /** Selected prompt template ID */
  promptId: string;
  /** Ollama server URL */
  ollamaUrl: string;
  /** Active backend: "ollama" (default) or "openai_compat" */
  backend: string;
  /** OpenAI-compatible server base URL */
  openaiCompatUrl: string;
  /** Optional API key for the OpenAI-compatible endpoint */
  apiKey: string | null;
}

/** Recording indicator visual style */
export type IndicatorStyle = 'cursor-dot' | 'fixed-float' | 'pill';

/** General application settings */
export interface GeneralConfig {
  /** Launch application on system startup */
  launchAtLogin: boolean;
  /** Show menu bar icon */
  showInMenuBar: boolean;
  /** Show dock icon (macOS) */
  showInDock: boolean;
  /** Automatically check for updates on launch */
  checkForUpdates: boolean;
  /** Show the floating recording indicator during recording */
  showRecordingIndicator: boolean;
  /** Visual style for the recording indicator */
  indicatorStyle: IndicatorStyle;
  /** Show native window decorations (Linux); custom close button when off */
  windowDecorations: boolean;
}

/** Recorder window position options */
export type RecorderPosition = 'cursor' | 'tray-icon' | 'top-left' | 'top-right' | 'bottom-left' | 'bottom-right' | 'centre';

/** Recorder window configuration */
export interface RecorderConfig {
  /** Window position preference */
  position: RecorderPosition;
  /** Horizontal offset from position anchor (in pixels) */
  offsetX: number;
  /** Vertical offset from position anchor (in pixels) */
  offsetY: number;
  /** Auto-hide delay in milliseconds after transcription completes (0 = no auto-hide) */
  autoHideDelay: number;
}

/** Main configuration structure */
export interface Config {
  /** Schema version for migrations */
  version: number;
  /** Audio recording settings */
  audio: AudioConfig;
  /** Transcription settings */
  transcription: TranscriptionConfig;
  /** Keyboard shortcut settings */
  shortcuts: ShortcutConfig;
  /** AI enhancement settings */
  enhancement: EnhancementConfig;
  /** General application settings */
  general: GeneralConfig;
  /** Recorder window settings */
  recorder: RecorderConfig;
  /** Integrations settings */
  integrations: IntegrationsConfig;
  /** Telemetry exporter settings */
  telemetry: TelemetryConfig;
  /** Word-list publishing settings */
  publish: PublishConfig;
  /** External dictionary service settings */
  externalDictionary: ExternalDictionaryConfig;
}

/** Default configuration values */
function getDefaultConfig(): Config {
  return {
    version: 1,
    audio: {
      deviceId: null,
      sampleRate: 16000,
      playSounds: true,
      soundVolume: 1,
    },
    // Transcription defaults are a guarded copy of TranscriptionConfig::default()
    // in src-tauri/src/config.rs: the placeholder must be type-complete and a
    // boolean has no inert form, so unlike the shortcut defaults below this
    // block carries the real values. transcription_defaults_match_typescript in
    // config.rs fails the build if it drifts from the Rust defaults.
    transcription: {
      autoCopy: false,
      autoPaste: true,
      addLeadingSpace: false,
      appendTrailingSpace: false,
      autoSubmit: 'off',
      removeFillers: true,
      australianSpelling: true,
      spokenNumbersToDigits: false,
      normaliseWhitespace: true,
      cleanupPunctuation: true,
      sentenceCase: false,
      voiceFormattingCommands: true,
      vocabularyBias: true,
      typingTool: 'auto',
      modelIdleUnloadSecs: null,
    },
    // Shortcut defaults are NOT restated here. They live in ShortcutConfig::default()
    // in src-tauri/src/config.rs and arrive via get_default_config(); this placeholder
    // is only what renders in the instant before that resolves. Retyping them here is
    // what let toggle_recording_alt disagree with the backend from February 2026
    // onward (#127) — the Rust value silently won on every write while the UI
    // advertised a binding that was never bound.
    //
    // shortcut_defaults_match_typescript in config.rs fails the build if real values
    // reappear in this block.
    shortcuts: {
      toggleRecording: '',
      toggleRecordingAlt: null,
      copyLast: null,
      toggleEnhancement: null,
      recordingMode: 'toggle',
      handsFreeSilenceSecs: 0,
    },
    enhancement: {
      enabled: false,
      model: 'llama3.2',
      promptId: 'fix-grammar',
      ollamaUrl: 'http://localhost:11434',
      backend: 'ollama',
      openaiCompatUrl: 'http://localhost:1234',
      apiKey: null,
    },
    general: {
      launchAtLogin: false,
      showInMenuBar: true,
      showInDock: false,
      checkForUpdates: true,
      showRecordingIndicator: true,
      indicatorStyle: 'cursor-dot',
      windowDecorations: true,
    },
    recorder: {
      position: 'top-right',
      offsetX: -20,
      offsetY: 20,
      autoHideDelay: 3000,
    },
    integrations: {
      apiEnabled: false,
      apiPort: 8765,
      mcpEnabled: false,
    },
    telemetry: {
      endpoint: '',
      headersHelper: '',
    },
    // Publish defaults are a guarded copy of PublishConfig::default() in
    // src-tauri/src/config.rs (off, empty URL and username): the strings are
    // inert empty, and publish_defaults_match_typescript in config.rs guards
    // the block against drift from the Rust defaults.
    publish: {
      enabled: false,
      url: '',
      username: '',
    },
    externalDictionary: {
      enabled: false,
      url: '',
      username: '',
      device: '',
      credentialHelper: '',
    },
  };
}

/** Create the configuration store with reactive state */
function createConfigStore() {
  let config = $state<Config>(getDefaultConfig());
  let isLoading = $state<boolean>(false);
  let isSaving = $state<boolean>(false);
  let error = $state<string | null>(null);
  let isInitialised = $state<boolean>(false);
  // A deep copy of what was last loaded or sent: save() sends only what differs
  // from it, so a field this window never touched is never overwritten.
  let snapshot: Config | null = null;

  /**
   * Load configuration from the backend
   */
  async function load(): Promise<void> {
    isLoading = true;
    error = null;
    const hadLoadedConfig = isInitialised;

    try {
      const loaded = await invoke<Config>('get_config');
      config = loaded;
      snapshot = structuredClone(loaded);
      isInitialised = true;
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to load configuration';
      console.error('Failed to load config:', e);

      // A failed refresh must retain the last verified configuration. Replacing
      // it with defaults while isInitialised stays true lets the next Settings
      // save overwrite the user's settings. On first load there is no good
      // state, so show the backend defaults but keep saves disabled.
      if (!hadLoadedConfig) {
        try {
          config = await invoke<Config>('get_default_config');
        } catch (defaultsError) {
          console.error('Failed to load default config:', defaultsError);
        }
      }
    } finally {
      isLoading = false;
    }
  }

  /**
   * Fetch the backend's default configuration.
   *
   * The single definition lives in Rust (`Config::default()`); this is the only
   * way the frontend should obtain defaults. Do not reintroduce a hardcoded copy.
   */
  async function loadDefaults(): Promise<Config> {
    return invoke<Config>('get_default_config');
  }

  /**
   * Save configuration to the backend
   *
   * Sends only the leaves that changed since the last load or save; nothing
   * changed sends nothing. The API key is never part of it (`setEnhancementApiKey`).
   * Refuses to save if the config hasn't been loaded from the backend yet.
   */
  async function save(): Promise<boolean> {
    if (!isInitialised || !snapshot) {
      console.warn('[ConfigStore] save() called before config was loaded — ignoring to prevent overwriting persisted settings with defaults');
      return false;
    }

    isSaving = true;
    error = null;

    try {
      // The snapshot advances to what this window sent, never to the backend's
      // copy: a change made elsewhere that this window never saw (MCP, HTTP)
      // must not read as this window's edit on the next save.
      const next = $state.snapshot(config) as Config;
      const patch = buildPatch(snapshot, next, ['enhancement.apiKey']);
      if (patch) {
        await invoke('patch_config', { patch });
      }
      snapshot = next;
      return true;
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to save configuration';
      console.error('Failed to save config:', e);
      return false;
    } finally {
      isSaving = false;
    }
  }

  /**
   * Reset configuration to defaults
   */
  async function reset(): Promise<boolean> {
    isLoading = true;
    error = null;

    try {
      const reset = await invoke<Config>('reset_config');
      config = reset;
      snapshot = structuredClone(reset);
      return true;
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to reset configuration';
      console.error('Failed to reset config:', e);
      return false;
    } finally {
      isLoading = false;
    }
  }

  /**
   * Get the configuration file path
   */
  async function getConfigPath(): Promise<string> {
    return invoke<string>('get_config_path_cmd');
  }

  /**
   * Update a specific audio config field
   */
  function updateAudio<K extends keyof AudioConfig>(key: K, value: AudioConfig[K]): void {
    config.audio[key] = value;
  }

  /**
   * Update a specific transcription config field
   */
  function updateTranscription<K extends keyof TranscriptionConfig>(key: K, value: TranscriptionConfig[K]): void {
    config.transcription[key] = value;
  }

  /**
   * Update a specific shortcut config field
   */
  function updateShortcuts<K extends keyof ShortcutConfig>(key: K, value: ShortcutConfig[K]): void {
    config.shortcuts[key] = value;
  }

  /**
   * Update a specific enhancement config field
   */
  function updateEnhancement<K extends keyof EnhancementConfig>(key: K, value: EnhancementConfig[K]): void {
    config.enhancement[key] = value;
  }

  /**
   * Update a specific general config field
   */
  function updateGeneral<K extends keyof GeneralConfig>(key: K, value: GeneralConfig[K]): void {
    config.general[key] = value;
  }

  /**
   * Update a specific recorder config field
   */
  function updateRecorder<K extends keyof RecorderConfig>(key: K, value: RecorderConfig[K]): void {
    config.recorder[key] = value;
  }

  /**
   * Update a specific integrations config field
   */
  function updateIntegrations<K extends keyof IntegrationsConfig>(key: K, value: IntegrationsConfig[K]): void {
    config.integrations[key] = value;
  }

  function updateExternalDictionary<K extends keyof ExternalDictionaryConfig>(key: K, value: ExternalDictionaryConfig[K]): void {
    config.externalDictionary[key] = value;
  }

  /**
   * Update a specific word-list publishing config field
   */
  function updatePublish<K extends keyof PublishConfig>(key: K, value: PublishConfig[K]): void {
    config.publish[key] = value;
  }

  /**
   * Set or clear the enhancement API key via the dedicated backend command.
   *
   * The only way the key is written: save() never sends it.
   */
  async function setEnhancementApiKey(key: string | null): Promise<boolean> {
    try {
      await invoke('set_enhancement_api_key', { key });
      config.enhancement.apiKey = key;
      return true;
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to save API key';
      console.error('Failed to set enhancement API key:', e);
      return false;
    }
  }

  /**
   * Save the telemetry endpoint and headers helper via the dedicated backend
   * command, which writes only that section and points the live exporter at it.
   *
   * The snapshot is updated with the local copy so the next save() sees no
   * change in the section.
   */
  async function setTelemetry(endpoint: string, headersHelper: string): Promise<TelemetryStatus | null> {
    try {
      const status = await invoke<TelemetryStatus>('plugin:telemetry|telemetry_set', {
        endpoint,
        headersHelper,
      });
      config.telemetry = {
        endpoint: status.savedEndpoint,
        headersHelper: status.savedHeadersHelper,
      };
      if (snapshot) {
        snapshot.telemetry = {
          endpoint: status.savedEndpoint,
          headersHelper: status.savedHeadersHelper,
        };
      }
      return status;
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to save telemetry settings';
      console.error('Failed to set telemetry config:', e);
      return null;
    }
  }

  /**
   * Clear error state
   */
  function clearError(): void {
    error = null;
  }

  return {
    // State (getters for reactive access)
    get config() {
      return config;
    },
    get isLoading() {
      return isLoading;
    },
    get isSaving() {
      return isSaving;
    },
    get error() {
      return error;
    },
    get isInitialised() {
      return isInitialised;
    },

    // Shorthand accessors for common config sections
    get audio() {
      return config.audio;
    },
    get transcription() {
      return config.transcription;
    },
    get shortcuts() {
      return config.shortcuts;
    },
    get enhancement() {
      return config.enhancement;
    },
    get general() {
      return config.general;
    },
    get recorder() {
      return config.recorder;
    },
    get integrations() {
      return config.integrations;
    },
    get publish() {
      return config.publish;
    },
    get externalDictionary() {
      return config.externalDictionary;
    },

    // Actions
    load,
    loadDefaults,
    save,
    reset,
    getConfigPath,
    updateAudio,
    updateTranscription,
    updateShortcuts,
    updateEnhancement,
    updateGeneral,
    updateRecorder,
    updateIntegrations,
    updatePublish,
    updateExternalDictionary,
    setEnhancementApiKey,
    setTelemetry,
    clearError,
  };
}

/** Singleton configuration store instance */
export const configStore = createConfigStore();
