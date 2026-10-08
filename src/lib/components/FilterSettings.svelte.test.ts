// The reset button must reset to the defaults the backend owns
// (TranscriptionConfig::default(), via get_default_config), not a copy this
// component carries. The copy said Australian spelling was off while Rust
// defaults it on, so pressing reset silently turned it off.
import { describe, expect, it, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/svelte';

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));

vi.mock('@tauri-apps/api/core', () => ({ invoke }));

import FilterSettings from './FilterSettings.svelte';

/** Filter options matching the Rust FilterOptions struct */
interface FilterOptions {
  remove_fillers: boolean;
  normalise_whitespace: boolean;
  cleanup_punctuation: boolean;
  sentence_case: boolean;
  australian_spelling: boolean;
  spoken_numbers_to_digits: boolean;
  voice_formatting_commands: boolean;
}

/** The filter defaults the mocked backend serves: Rust's own. */
const FILTER_DEFAULTS: FilterOptions = {
  remove_fillers: true,
  normalise_whitespace: true,
  cleanup_punctuation: true,
  sentence_case: false,
  australian_spelling: true,
  spoken_numbers_to_digits: false,
  voice_formatting_commands: true,
};

/** The raw config `get_default_config` returns, in the backend's snake_case. */
function rawDefaultConfig() {
  return {
    version: 1,
    audio: { device_id: null, sample_rate: 16000, play_sounds: true, sound_volume: 1 },
    transcription: {
      auto_copy: false,
      auto_paste: true,
      add_leading_space: false,
      append_trailing_space: false,
      auto_submit: 'off',
      ...FILTER_DEFAULTS,
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
    recorder: { position: 'top-right', offset_x: -20, offset_y: 20, auto_hide_delay: 3000 },
    integrations: { api_enabled: false, api_port: 8765, mcp_enabled: false },
    telemetry: { endpoint: '', headers_helper: '' },
  };
}

describe('FilterSettings reset to defaults', () => {
  beforeEach(() => {
    invoke.mockReset();
    invoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_default_config') return Promise.resolve(rawDefaultConfig());
      if (cmd === 'filter_transcription') return Promise.resolve('preview text');
      return Promise.reject(new Error(`unexpected command: ${cmd}`));
    });
  });

  it('reset leaves Australian spelling on when the change being reset is elsewhere', async () => {
    const onchange = vi.fn<(options: FilterOptions) => void>();
    render(FilterSettings, {
      props: {
        // Australian spelling sits at its default (on); sentence case is the
        // user's one change — resetting it must not touch spelling.
        initialOptions: { ...FILTER_DEFAULTS, sentence_case: true },
        onchange,
      },
    });

    const reset = await screen.findByRole('button', { name: 'Reset to defaults' });
    await fireEvent.click(reset);

    const emitted = onchange.mock.lastCall?.[0];
    expect(emitted).toBeDefined();
    expect(emitted).toEqual(FILTER_DEFAULTS);
    expect(emitted?.australian_spelling).toBe(true);
  });

  it('reset turns Australian spelling back on when the user turned it off', async () => {
    const onchange = vi.fn<(options: FilterOptions) => void>();
    render(FilterSettings, {
      props: { initialOptions: { ...FILTER_DEFAULTS, australian_spelling: false }, onchange },
    });

    const reset = await screen.findByRole('button', { name: 'Reset to defaults' });
    await fireEvent.click(reset);

    const emitted = onchange.mock.lastCall?.[0];
    expect(emitted).toBeDefined();
    expect(emitted).toEqual(FILTER_DEFAULTS);
    expect(emitted?.australian_spelling).toBe(true);
  });

  it('shows no reset button when the options already match the backend defaults', async () => {
    render(FilterSettings, {
      props: { initialOptions: { ...FILTER_DEFAULTS }, onchange: vi.fn() },
    });

    // Wait for the defaults fetch and the preview effect to have flushed, then
    // assert the button never appeared.
    await screen.findByText('preview text');
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(
      screen.queryByRole('button', { name: 'Reset to defaults' })
    ).not.toBeInTheDocument();
  });
});
