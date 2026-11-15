import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock('@tauri-apps/api/core', () => ({ invoke }));

const { listen } = vi.hoisted(() => ({ listen: vi.fn() }));
vi.mock('@tauri-apps/api/event', () => ({ listen }));

const RAW_CONFIG = {
  version: 1,
  audio: {
    deviceId: 'microphone-a',
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
  shortcuts: {
    toggleRecording: 'F13',
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
    showInDock: true,
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

describe('settingsStore.initialise', () => {
  beforeEach(() => {
    invoke.mockReset();
    invoke.mockImplementation(async (cmd: string) => {
      if (cmd === 'get_config') return RAW_CONFIG;
      if (cmd === 'list_audio_devices') return [];
      return undefined;
    });
    listen.mockReset();
    listen.mockResolvedValue(() => {});
    vi.resetModules();
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('loads config when the config store has not initialised yet', async () => {
    const { configStore } = await import('./config.svelte');
    const { createSettingsStore } = await import('./settings.svelte');
    const settings = createSettingsStore();

    expect(configStore.isInitialised).toBe(false);
    await settings.initialise();

    expect(configStore.isInitialised).toBe(true);
    expect(invoke).toHaveBeenCalledWith('get_config');
  });

  it('does not reload config when this webview has already loaded it', async () => {
    const { configStore } = await import('./config.svelte');
    const { createSettingsStore } = await import('./settings.svelte');

    // A recorder window can mount its own configStore.load() before this
    // store initialises; the guard must see that and skip the second load.
    await configStore.load();
    expect(configStore.isInitialised).toBe(true);
    invoke.mockClear();

    const settings = createSettingsStore();
    await settings.initialise();

    expect(invoke).not.toHaveBeenCalledWith('get_config');
    expect(settings.isInitialised).toBe(true);
  });
});
