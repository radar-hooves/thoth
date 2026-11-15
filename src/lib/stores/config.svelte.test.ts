import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));

vi.mock('@tauri-apps/api/core', () => ({ invoke }));

const BACKEND_CONFIG = {
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

describe('configStore.load', () => {
  beforeEach(() => {
    invoke.mockReset();
    vi.resetModules();
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('retains the last verified configuration when a refresh fails', async () => {
    invoke.mockResolvedValueOnce(BACKEND_CONFIG);
    const { configStore } = await import('./config.svelte');

    await configStore.load();
    expect(configStore.general.showInDock).toBe(true);
    expect(configStore.isInitialised).toBe(true);

    const error = vi.spyOn(console, 'error').mockImplementation(() => {});
    invoke.mockRejectedValueOnce(new Error('temporary IPC failure'));
    await configStore.load();

    expect(configStore.general.showInDock).toBe(true);
    expect(configStore.isInitialised).toBe(true);
    expect(invoke).toHaveBeenCalledTimes(2);
    expect(error).toHaveBeenCalledOnce();
  });

  it('uses backend defaults only when no configuration has loaded', async () => {
    invoke.mockRejectedValueOnce(new Error('temporary IPC failure'));
    invoke.mockResolvedValueOnce(BACKEND_CONFIG);
    const { configStore } = await import('./config.svelte');
    vi.spyOn(console, 'error').mockImplementation(() => {});

    await configStore.load();

    expect(configStore.general.showInDock).toBe(true);
    expect(configStore.isInitialised).toBe(false);
    expect(invoke).toHaveBeenNthCalledWith(1, 'get_config');
    expect(invoke).toHaveBeenNthCalledWith(2, 'get_default_config');
  });
});

describe('configStore.save', () => {
  beforeEach(() => {
    invoke.mockReset();
    vi.resetModules();
  });

  it('sends only the changed leaf, then nothing until the next edit', async () => {
    invoke.mockResolvedValueOnce(BACKEND_CONFIG);
    const { configStore } = await import('./config.svelte');
    await configStore.load();

    configStore.updateGeneral('showInDock', !configStore.general.showInDock);
    // The backend's copy carries a change made over MCP that this window never saw.
    invoke.mockResolvedValueOnce({
      ...BACKEND_CONFIG,
      general: { ...BACKEND_CONFIG.general, showInDock: !BACKEND_CONFIG.general.showInDock },
      audio: { ...BACKEND_CONFIG.audio, playSounds: !BACKEND_CONFIG.audio.playSounds },
    });
    expect(await configStore.save()).toBe(true);
    expect(invoke).toHaveBeenLastCalledWith('patch_config', {
      patch: { general: { showInDock: !BACKEND_CONFIG.general.showInDock } },
    });

    // Nothing edited since: nothing is sent, so the MCP change is not written back.
    expect(await configStore.save()).toBe(true);
    expect(invoke).toHaveBeenCalledTimes(2);
  });

  it('never sends the API key', async () => {
    invoke.mockResolvedValueOnce(BACKEND_CONFIG);
    const { configStore } = await import('./config.svelte');
    await configStore.load();

    configStore.updateEnhancement('apiKey', 'typed-but-unsaved');
    await configStore.save();
    expect(invoke).toHaveBeenCalledTimes(1);
  });
});
