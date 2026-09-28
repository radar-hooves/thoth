/**
 * Configuration state management store using Svelte 5 runes.
 *
 * Manages application settings with persistence via the Tauri backend.
 * Settings are stored in ~/.thoth/config.json and support schema migrations.
 */

import { invoke } from '@tauri-apps/api/core';

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
export type RecordingMode = 'toggle' | 'hands_free' | 'hold_to_record';

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

/**
 * What the Telemetry card shows: the live exporter, whether the environment
 * owns it, and the saved values underneath.
 */
export interface TelemetryStatus {
  /** The endpoint this process is exporting to; empty means local only */
  endpoint: string;
  /** The helper command behind it */
  headersHelper: string;
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
}

/** Raw config from backend (snake_case fields) */
interface ConfigRaw {
  version: number;
  audio: {
    device_id: string | null;
    sample_rate: number;
    play_sounds: boolean;
    sound_volume: number;
  };
  transcription: {
    auto_copy: boolean;
    auto_paste: boolean;
    add_leading_space: boolean;
    append_trailing_space: boolean;
    auto_submit: AutoSubmit;
    remove_fillers: boolean;
    australian_spelling: boolean;
    spoken_numbers_to_digits: boolean;
    normalise_whitespace: boolean;
    cleanup_punctuation: boolean;
    sentence_case: boolean;
    voice_formatting_commands: boolean;
    vocabulary_bias: boolean;
    typing_tool: TypingTool;
    model_idle_unload_secs: number | null;
  };
  shortcuts: {
    toggle_recording: string;
    toggle_recording_alt: string | null;
    copy_last: string | null;
    toggle_enhancement: string | null;
    recording_mode: RecordingMode;
    hands_free_silence_secs: number;
  };
  enhancement: {
    enabled: boolean;
    model: string;
    prompt_id: string;
    ollama_url: string;
    backend: string;
    openai_compat_url: string;
    api_key: string | null;
  };
  general: {
    launch_at_login: boolean;
    show_in_menu_bar: boolean;
    show_in_dock: boolean;
    check_for_updates: boolean;
    show_recording_indicator: boolean;
    indicator_style: IndicatorStyle;
    window_decorations: boolean;
  };
  recorder: {
    position: RecorderPosition;
    offset_x: number;
    offset_y: number;
    auto_hide_delay: number;
  };
  integrations?: {
    api_enabled: boolean;
    api_port: number;
    mcp_enabled: boolean;
  };
  telemetry?: {
    endpoint: string;
    headers_helper: string;
  };
}

/** Convert raw backend config to frontend format (snake_case to camelCase) */
function parseConfig(raw: ConfigRaw): Config {
  return {
    version: raw.version,
    audio: {
      deviceId: raw.audio.device_id,
      sampleRate: raw.audio.sample_rate,
      playSounds: raw.audio.play_sounds,
      soundVolume: raw.audio.sound_volume ?? 1,
    },
    transcription: {
      autoCopy: raw.transcription.auto_copy,
      autoPaste: raw.transcription.auto_paste,
      addLeadingSpace: raw.transcription.add_leading_space,
      appendTrailingSpace: raw.transcription.append_trailing_space ?? false,
      autoSubmit: raw.transcription.auto_submit ?? 'off',
      removeFillers: raw.transcription.remove_fillers,
      australianSpelling: raw.transcription.australian_spelling,
      spokenNumbersToDigits: raw.transcription.spoken_numbers_to_digits,
      normaliseWhitespace: raw.transcription.normalise_whitespace ?? true,
      cleanupPunctuation: raw.transcription.cleanup_punctuation ?? true,
      sentenceCase: raw.transcription.sentence_case ?? false,
      voiceFormattingCommands: raw.transcription.voice_formatting_commands ?? true,
      vocabularyBias: raw.transcription.vocabulary_bias ?? true,
      typingTool: raw.transcription.typing_tool ?? 'auto',
      modelIdleUnloadSecs: raw.transcription.model_idle_unload_secs ?? null,
    },
    shortcuts: {
      toggleRecording: raw.shortcuts.toggle_recording,
      toggleRecordingAlt: raw.shortcuts.toggle_recording_alt,
      copyLast: raw.shortcuts.copy_last,
      toggleEnhancement: raw.shortcuts.toggle_enhancement,
      recordingMode: raw.shortcuts.recording_mode,
      handsFreeSilenceSecs: raw.shortcuts.hands_free_silence_secs,
    },
    enhancement: {
      enabled: raw.enhancement.enabled,
      model: raw.enhancement.model,
      promptId: raw.enhancement.prompt_id,
      ollamaUrl: raw.enhancement.ollama_url,
      backend: raw.enhancement.backend,
      openaiCompatUrl: raw.enhancement.openai_compat_url,
      apiKey: raw.enhancement.api_key,
    },
    general: {
      launchAtLogin: raw.general.launch_at_login,
      showInMenuBar: raw.general.show_in_menu_bar,
      showInDock: raw.general.show_in_dock,
      checkForUpdates: raw.general.check_for_updates,
      showRecordingIndicator: raw.general.show_recording_indicator,
      indicatorStyle: raw.general.indicator_style,
      windowDecorations: raw.general.window_decorations ?? true,
    },
    recorder: {
      position: raw.recorder.position,
      offsetX: raw.recorder.offset_x,
      offsetY: raw.recorder.offset_y,
      autoHideDelay: raw.recorder.auto_hide_delay,
    },
    integrations: {
      apiEnabled: raw.integrations?.api_enabled ?? false,
      apiPort: raw.integrations?.api_port ?? 8765,
      mcpEnabled: raw.integrations?.mcp_enabled ?? false,
    },
    telemetry: {
      endpoint: raw.telemetry?.endpoint ?? '',
      headersHelper: raw.telemetry?.headers_helper ?? '',
    },
  };
}

/** Convert frontend config to backend format (camelCase to snake_case) */
function serialiseConfig(config: Config): ConfigRaw {
  return {
    version: config.version,
    audio: {
      device_id: config.audio.deviceId,
      sample_rate: config.audio.sampleRate,
      play_sounds: config.audio.playSounds,
      sound_volume: config.audio.soundVolume,
    },
    transcription: {
      auto_copy: config.transcription.autoCopy,
      auto_paste: config.transcription.autoPaste,
      add_leading_space: config.transcription.addLeadingSpace,
      append_trailing_space: config.transcription.appendTrailingSpace,
      auto_submit: config.transcription.autoSubmit,
      remove_fillers: config.transcription.removeFillers,
      australian_spelling: config.transcription.australianSpelling,
      spoken_numbers_to_digits: config.transcription.spokenNumbersToDigits,
      normalise_whitespace: config.transcription.normaliseWhitespace,
      cleanup_punctuation: config.transcription.cleanupPunctuation,
      sentence_case: config.transcription.sentenceCase,
      voice_formatting_commands: config.transcription.voiceFormattingCommands,
      vocabulary_bias: config.transcription.vocabularyBias,
      typing_tool: config.transcription.typingTool,
      model_idle_unload_secs: config.transcription.modelIdleUnloadSecs,
    },
    shortcuts: {
      toggle_recording: config.shortcuts.toggleRecording,
      toggle_recording_alt: config.shortcuts.toggleRecordingAlt,
      copy_last: config.shortcuts.copyLast,
      toggle_enhancement: config.shortcuts.toggleEnhancement,
      recording_mode: config.shortcuts.recordingMode,
      hands_free_silence_secs: config.shortcuts.handsFreeSilenceSecs,
    },
    enhancement: {
      enabled: config.enhancement.enabled,
      model: config.enhancement.model,
      prompt_id: config.enhancement.promptId,
      ollama_url: config.enhancement.ollamaUrl,
      backend: config.enhancement.backend,
      openai_compat_url: config.enhancement.openaiCompatUrl,
      api_key: config.enhancement.apiKey,
    },
    general: {
      launch_at_login: config.general.launchAtLogin,
      show_in_menu_bar: config.general.showInMenuBar,
      show_in_dock: config.general.showInDock,
      check_for_updates: config.general.checkForUpdates,
      show_recording_indicator: config.general.showRecordingIndicator,
      indicator_style: config.general.indicatorStyle,
      window_decorations: config.general.windowDecorations,
    },
    recorder: {
      position: config.recorder.position,
      offset_x: config.recorder.offsetX,
      offset_y: config.recorder.offsetY,
      auto_hide_delay: config.recorder.autoHideDelay,
    },
    integrations: {
      api_enabled: config.integrations.apiEnabled,
      api_port: config.integrations.apiPort,
      mcp_enabled: config.integrations.mcpEnabled,
    },
    telemetry: {
      endpoint: config.telemetry.endpoint,
      headers_helper: config.telemetry.headersHelper,
    },
  };
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
    transcription: {
      autoCopy: false,
      autoPaste: true,
      addLeadingSpace: false,
      appendTrailingSpace: false,
      autoSubmit: 'off',
      removeFillers: true,
      australianSpelling: false,
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
  };
}

/** Create the configuration store with reactive state */
function createConfigStore() {
  let config = $state<Config>(getDefaultConfig());
  let isLoading = $state<boolean>(false);
  let isSaving = $state<boolean>(false);
  let error = $state<string | null>(null);
  let isInitialised = $state<boolean>(false);

  /**
   * Load configuration from the backend
   */
  async function load(): Promise<void> {
    isLoading = true;
    error = null;
    const hadLoadedConfig = isInitialised;

    try {
      const rawConfig = await invoke<ConfigRaw>('get_config');
      config = parseConfig(rawConfig);
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
          config = parseConfig(await invoke<ConfigRaw>('get_default_config'));
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
    return parseConfig(await invoke<ConfigRaw>('get_default_config'));
  }

  /**
   * Save configuration to the backend
   *
   * Refuses to save if the config hasn't been loaded from the backend yet,
   * preventing accidental overwrite of persisted settings with in-memory defaults.
   */
  async function save(): Promise<boolean> {
    if (!isInitialised) {
      console.warn('[ConfigStore] save() called before config was loaded — ignoring to prevent overwriting persisted settings with defaults');
      return false;
    }

    isSaving = true;
    error = null;

    try {
      const rawConfig = serialiseConfig(config);
      await invoke('set_config', { config: rawConfig });
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
      const rawConfig = await invoke<ConfigRaw>('reset_config');
      config = parseConfig(rawConfig);
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

  /**
   * Set or clear the enhancement API key via the dedicated backend command.
   *
   * This is the only correct way to change the API key. The generic save()
   * path cannot clear the key (the backend preservation guard blocks it);
   * this command bypasses that guard so an explicit user action takes effect.
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
   * Updating the local copy afterwards is what stops the next generic save()
   * from writing the stale section back over it.
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
    setEnhancementApiKey,
    setTelemetry,
    clearError,
  };
}

/** Singleton configuration store instance */
export const configStore = createConfigStore();
