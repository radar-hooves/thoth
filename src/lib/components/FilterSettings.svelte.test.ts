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

/** The raw config `get_default_config` returns, as the webview receives it, camelCase. */
function rawDefaultConfig() {
  return {
    version: 1,
    audio: { deviceId: null, sampleRate: 16000, playSounds: true, soundVolume: 1 },
    transcription: {
      autoCopy: false,
      autoPaste: true,
      addLeadingSpace: false,
      appendTrailingSpace: false,
      autoSubmit: 'off',
      removeFillers: true,
      normaliseWhitespace: true,
      cleanupPunctuation: true,
      sentenceCase: false,
      australianSpelling: true,
      spokenNumbersToDigits: false,
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
    recorder: { position: 'top-right', offsetX: -20, offsetY: 20, autoHideDelay: 3000 },
    integrations: { apiEnabled: false, apiPort: 8765, mcpEnabled: false },
    telemetry: { endpoint: '', headersHelper: '' },
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
