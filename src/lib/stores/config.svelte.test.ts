import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));

vi.mock('@tauri-apps/api/core', () => ({ invoke }));

const RAW_CONFIG = {
  version: 1,
  audio: {
    device_id: 'microphone-a',
    sample_rate: 16000,
    play_sounds: true,
    sound_volume: 1,
  },
  transcription: {
    auto_copy: false,
    auto_paste: true,
    add_leading_space: false,
    append_trailing_space: false,
    auto_submit: 'off',
    remove_fillers: true,
    australian_spelling: false,
    spoken_numbers_to_digits: false,
    normalise_whitespace: true,
    cleanup_punctuation: true,
    sentence_case: false,
    voice_formatting_commands: true,
    vocabulary_bias: true,
    typing_tool: 'auto',
    model_idle_unload_secs: null,
  },
  shortcuts: {
    toggle_recording: 'F13',
    toggle_recording_alt: null,
    copy_last: null,
    toggle_enhancement: null,
    recording_mode: 'toggle',
    hands_free_silence_secs: 0,
  },
  enhancement: {
    enabled: false,
    model: 'llama3.2',
    prompt_id: 'fix-grammar',
    ollama_url: 'http://localhost:11434',
    backend: 'ollama',
    openai_compat_url: 'http://localhost:1234',
    api_key: null,
  },
  general: {
    launch_at_login: false,
    show_in_menu_bar: true,
    show_in_dock: true,
    check_for_updates: true,
    show_recording_indicator: true,
    indicator_style: 'cursor-dot',
    window_decorations: true,
  },
  recorder: {
    position: 'top-right',
    offset_x: -20,
    offset_y: 20,
    auto_hide_delay: 3000,
  },
  integrations: {
    api_enabled: false,
    api_port: 8765,
    mcp_enabled: false,
  },
  telemetry: {
    endpoint: '',
    headers_helper: '',
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
    invoke.mockResolvedValueOnce(RAW_CONFIG);
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
    invoke.mockResolvedValueOnce(RAW_CONFIG);
    const { configStore } = await import('./config.svelte');
    vi.spyOn(console, 'error').mockImplementation(() => {});

    await configStore.load();

    expect(configStore.general.showInDock).toBe(true);
    expect(configStore.isInitialised).toBe(false);
    expect(invoke).toHaveBeenNthCalledWith(1, 'get_config');
    expect(invoke).toHaveBeenNthCalledWith(2, 'get_default_config');
  });
});
