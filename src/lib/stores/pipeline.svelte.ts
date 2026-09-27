/**
 * Pipeline state management for the recording to transcription to output flow.
 *
 * This store orchestrates the complete transcription pipeline:
 * 1. Recording audio
 * 2. Transcribing audio to text
 * 3. Filtering and dictionary replacements
 * 4. Optional AI enhancement
 * 5. Output (clipboard copy and/or paste at cursor)
 * 6. Saving to history
 */

import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { toast } from 'svelte-sonner';
import { configStore, type AutoSubmit } from './config.svelte';
import { settingsStore } from './settings.svelte';
import { soundStore } from './sound.svelte';

/** Debug logging — only active in development builds */
const debug = import.meta.env.DEV ? (...args: unknown[]) => console.log('[Pipeline]', ...args) : () => {};

/** Pipeline execution states */
export type PipelineState = 'idle' | 'recording' | 'converting' | 'transcribing' | 'filtering' | 'enhancing' | 'outputting' | 'completed' | 'failed';

/** Pipeline configuration for execution */
export interface PipelineConfig {
  /** Whether to apply dictionary replacements */
  applyDictionary: boolean;
  /** Whether to apply output filtering (formatting, whitespace) */
  applyFiltering: boolean;
  /** Whether to remove hesitation sounds (um, uh, er, ah) */
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
  /** Whether AI enhancement is enabled */
  enhancementEnabled: boolean;
  /** Ollama model for enhancement */
  enhancementModel: string;
  /** Enhancement prompt template */
  enhancementPrompt: string;
  /** Whether to auto-copy to clipboard */
  autoCopy: boolean;
  /** Whether to auto-paste at cursor */
  autoPaste: boolean;
  /** Insertion method: "typing" or "paste" */
  insertionMethod: string;
  /** Whether to append a single space after the inserted text */
  appendTrailingSpace: boolean;
  /** Key combination sent after a successful insertion */
  autoSubmit: AutoSubmit;
}

/** Pipeline execution result */
export interface PipelineResult {
  /** Whether the pipeline completed successfully */
  success: boolean;
  /** Final transcribed text (after all processing) */
  text: string;
  /** Raw transcription text (before filtering/enhancement) */
  rawText: string;
  /** Whether the text was enhanced by AI */
  isEnhanced: boolean;
  /** Duration of the audio in seconds */
  durationSeconds: number | null;
  /** Path to the audio file */
  audioPath: string | null;
  /** Error message if the pipeline failed */
  error: string | null;
  /** ID of the saved transcription record */
  transcriptionId: string | null;
}

/** Progress event from the backend */
interface PipelineProgress {
  state: PipelineState;
  message: string;
}

/**
 * Sentinel error message emitted by the Rust backend when transcription produces no
 * text (silent recording). Must match pipeline.rs `NO_SPEECH_ERROR` exactly.
 * Used to suppress error UI on both the import path and the retranscribe path.
 */
export const NO_SPEECH_SENTINEL = 'Transcription produced no text';

/** Default enhancement prompt */
const DEFAULT_ENHANCEMENT_PROMPT = `Fix grammar and punctuation in the following text.
Keep the original meaning and tone. Output only the corrected text, nothing else.

Text: {text}`;

/** Resolve the configured prompt ID to its template text */
async function resolveEnhancementPrompt(promptId: string): Promise<string> {
  try {
    const prompt = await invoke<{ template: string } | null>('get_prompt_by_id', {
      promptId,
    });
    if (prompt?.template) {
      return prompt.template;
    }
  } catch (e) {
    console.warn('[Pipeline] Failed to resolve prompt by ID, using default:', e);
  }
  return DEFAULT_ENHANCEMENT_PROMPT;
}

/** Create the default pipeline configuration based on app settings */
async function getDefaultConfig(): Promise<PipelineConfig> {
  const config = configStore.config;
  const enhancementPrompt = config.enhancement.enabled ? await resolveEnhancementPrompt(config.enhancement.promptId) : DEFAULT_ENHANCEMENT_PROMPT;

  return {
    applyDictionary: true,
    applyFiltering: true,
    removeFillers: config.transcription.removeFillers,
    australianSpelling: config.transcription.australianSpelling,
    spokenNumbersToDigits: config.transcription.spokenNumbersToDigits,
    normaliseWhitespace: config.transcription.normaliseWhitespace,
    cleanupPunctuation: config.transcription.cleanupPunctuation,
    sentenceCase: config.transcription.sentenceCase,
    voiceFormattingCommands: config.transcription.voiceFormattingCommands,
    enhancementEnabled: config.enhancement.enabled,
    enhancementModel: config.enhancement.model,
    enhancementPrompt,
    autoCopy: config.transcription.autoCopy,
    autoPaste: config.transcription.autoPaste && settingsStore.autoPaste,
    insertionMethod: 'paste',
    appendTrailingSpace: config.transcription.appendTrailingSpace,
    autoSubmit: config.transcription.autoSubmit,
  };
}

/** Create the pipeline store */
function createPipelineStore() {
  // Reactive state
  let state = $state<PipelineState>('idle');
  let message = $state<string>('');
  let isRunning = $state<boolean>(false);
  let lastResult = $state<PipelineResult | null>(null);
  let error = $state<string | null>(null);
  let audioPath = $state<string | null>(null);
  let recordingStartTime = $state<number | null>(null);
  let recordingDuration = $state<number>(0);

  // Event listeners
  let unlisteners: UnlistenFn[] = [];
  let isInitialised = false;

  // Toggle cooldown: after a state change (start/stop), ignore further toggles
  // for this duration. Prevents accidental double-toggles from immediately reversing the action.
  // Key bounce is handled by the Rust shortcut debounce (50ms); this is a higher-level guard.
  /**
   * What a toggle call intends. Mirrors the Rust `ToggleIntent`; serialised
   * snake_case over IPC.
   */
  type ToggleIntent = 'toggle' | 'stop_only';

  const TOGGLE_COOLDOWN_MS = 300;
  let lastToggleTime = 0;
  let isToggling = false;

  /**
   * Initialise the pipeline store and set up event listeners
   */
  async function initialise(): Promise<void> {
    debug('initialise() called, isInitialised:', isInitialised, 'unlisteners:', unlisteners.length);
    // Always clean up existing listeners first (handles HMR where state may be stale)
    if (unlisteners.length > 0) {
      debug(' Cleaning up existing listeners first');
      cleanup();
    }
    isInitialised = true;

    // Authoritative state mirror: the backend is the single source of truth.
    // This listener owns `state` and `isRunning`. Nothing else sets them.
    // The payload is always derived from get_pipeline_state() on the Rust side,
    // so a late completion from clip A cannot clobber an active clip-B recording
    // (because get_pipeline_state() prioritises is_recording()).
    const recordingStateUnlisten = await listen<PipelineState>('recording-state', (event) => {
      const incoming = event.payload;
      debug(' recording-state event received:', incoming);
      state = incoming;
      isRunning = state !== 'idle' && state !== 'completed' && state !== 'failed';
      if (state !== 'recording') {
        recordingStartTime = null;
      }
      debug(' State mirrored to:', state, 'isRunning:', isRunning);
    });
    unlisteners.push(recordingStateUnlisten);

    // Progress events: update the display message only.
    // State is owned by recording-state; pipeline-progress must not set it.
    // Exception: on a failed transition, surface the error to the user via
    // toast and error sound. This listener is the SINGLE owner of failure
    // feedback: every genuine pipeline failure emits a `failed` progress event
    // (including the detached recording path, which no imperative caller can
    // catch), so the imperative call sites deliberately no longer also play the
    // error sound. The no-speech sentinel is excluded; it is handled separately
    // (a gentle info toast) and must not produce an error noise on a
    // deliberately silent recording.
    const progressUnlisten = await listen<PipelineProgress>('pipeline-progress', (event) => {
      debug(' pipeline-progress message update:', event.payload.message);
      message = event.payload.message;
      if (event.payload.state === 'failed' && event.payload.message !== NO_SPEECH_SENTINEL) {
        toast.error(event.payload.message);
        soundStore.playError();
      }
    });
    unlisteners.push(progressUnlisten);

    // Completion events: store the result payload only.
    // State is owned by recording-state; pipeline-complete must not set it.
    // The Rust side emits recording-state after emitting pipeline-complete, so
    // the authoritative state update always follows the result delivery.
    const completeUnlisten = await listen<PipelineResult>('pipeline-complete', (event) => {
      debug(' pipeline-complete result received');
      lastResult = event.payload;
    });
    unlisteners.push(completeUnlisten);

    // Listen for cancellation events
    const cancelUnlisten = await listen('pipeline-cancelled', () => {
      state = 'idle';
      isRunning = false;
      recordingStartTime = null;
      recordingDuration = 0;
    });
    unlisteners.push(cancelUnlisten);

    // Show a toast when AI enhancement is toggled via the global shortcut
    const enhancementShortcutUnlisten = await listen<{
      enabled: boolean;
      promptName: string;
    }>('enhancement-toggled-shortcut', (event) => {
      const { enabled, promptName } = event.payload;
      if (enabled) {
        toast.info(`AI Enhancement: On — ${promptName}`);
      } else {
        toast.info('AI Enhancement: Off');
      }
    });
    unlisteners.push(enhancementShortcutUnlisten);

    // Surface the Wayland global-shortcut portal result. This event only fires
    // on Linux/Wayland; on macOS and Linux/X11 it never arrives, so listening
    // is harmless elsewhere. Without this, a compositor that cannot bind global
    // shortcuts would leave the user pressing a hotkey that silently does
    // nothing — the toast tells them why and what to do instead.
    const waylandShortcutsUnlisten = await listen<{
      available: boolean;
      message: string;
      bindings: [string, string][];
    }>('wayland-shortcuts-status', (event) => {
      const { available, message } = event.payload;
      if (available) {
        toast.success(message);
      } else {
        toast.warning(message, { duration: 10000 });
      }
    });
    unlisteners.push(waylandShortcutsUnlisten);

    // One-time advisory on Linux/Wayland when `wtype` is not installed, so the
    // user understands why text insertion may prompt for permission. Only fires
    // on Linux/Wayland without wtype; silent everywhere else.
    const typingAdvisoryUnlisten = await listen<string>('text-insertion-advisory', (event) => {
      toast.info(event.payload, { duration: 12000 });
    });
    unlisteners.push(typingAdvisoryUnlisten);

    // Auto-paste reached the end of the pipeline but the text never landed at
    // the cursor — most often a missing macOS Accessibility grant, which makes
    // the synthetic Cmd+V a silent no-op. The transcription is still in history
    // and (on the clipboard route) still on the clipboard, so this is a warning
    // rather than a pipeline failure.
    const insertionFailedUnlisten = await listen<string>('text-insertion-failed', (event) => {
      toast.warning(event.payload, { duration: 15000 });
    });
    unlisteners.push(insertionFailedUnlisten);

    // AI enhancement was skipped (a dictation over the length cap) or its
    // reply was discarded (implausibly short or long) — the unenhanced
    // transcript still pasted, so this is informational, not a failure.
    const enhancementSkippedUnlisten = await listen<string>('enhancement-skipped', (event) => {
      toast.info(event.payload, { duration: 8000 });
    });
    unlisteners.push(enhancementSkippedUnlisten);

    // Notify when the configured microphone is unavailable and recording fell
    // back to the system default (e.g. an unplugged USB mic). Deduped in Rust to
    // one toast per distinct missing device.
    const deviceFallbackUnlisten = await listen<string>('audio-device-fallback', (event) => {
      toast.warning(event.payload, { duration: 8000 });
    });
    unlisteners.push(deviceFallbackUnlisten);

    // Listen for shortcut events to trigger recording.
    // The start-vs-stop decision is made by the Rust pipeline_toggle_recording
    // command (which reads is_recording() — the single authority). The frontend
    // does NOT re-decide here; it only updates display state from the outcome.
    debug(' Setting up shortcut-triggered listener...');
    const shortcutUnlisten = await listen<string>('shortcut-triggered', async (event) => {
      const shortcutId = event.payload;
      const timestamp = new Date().toISOString();
      debug(`${timestamp} Shortcut event received:`, shortcutId, 'current state:', state, 'isRunning:', isRunning);

      if (shortcutId === 'toggle_recording' || shortcutId === 'toggle_recording_alt') {
        debug(`${timestamp} Calling pipeline_toggle_recording (Rust authority)...`);
        await toggleRecording();
        debug(`${timestamp} toggle completed, new state:`, state);
      }
    });
    debug(' shortcut-triggered listener registered');
    unlisteners.push(shortcutUnlisten);

    // Hold-to-record's key-up (#111). Rust emits this ONLY in that mode and
    // only for a recording shortcut, so there is no mode check here — and it
    // asks to stop rather than to toggle, so a release can never start a
    // recording the press failed to.
    const releaseUnlisten = await listen<string>('shortcut-released', async (event) => {
      debug('Shortcut released:', event.payload, '— stopping');
      await toggleRecording(undefined, 'stop_only');
    });
    unlisteners.push(releaseUnlisten);

    // Seed the mirror from the backend's current state so a reload/HMR shows
    // the true state rather than defaulting to idle.
    try {
      const backendState = await invoke<PipelineState>('get_pipeline_state');
      debug(' Seeding state from backend:', backendState);
      state = backendState;
      isRunning = state !== 'idle' && state !== 'completed' && state !== 'failed';
    } catch (e) {
      console.warn('[Pipeline] Failed to seed state from backend:', e);
    }

    debug(' Initialization complete, state:', state, 'isRunning:', isRunning);
  }

  /**
   * Clean up event listeners
   */
  function cleanup(): void {
    for (const unlisten of unlisteners) {
      unlisten();
    }
    unlisteners = [];
    isInitialised = false;
  }

  /**
   * Toggle recording — thin caller of the Rust authority command.
   *
   * Start-vs-stop is decided by `pipeline_toggle_recording` in Rust, which
   * reads `is_recording()` — the single source of truth.  The frontend does NOT
   * branch on `state` here; it updates display from the returned outcome.
   *
   * Sound is also owned by Rust:
   * - BING is played by the shortcut handler the instant the key is pressed.
   * - BONG is played by pipeline_toggle_recording before capture disarms.
   */
  async function toggleRecording(config?: Partial<PipelineConfig>, intent: ToggleIntent = 'toggle'): Promise<{ success: boolean; result?: PipelineResult; error?: string }> {
    // Cooldown: ignore toggles that arrive too soon after the last state change.
    // Prevents key bounce from immediately reversing a start or stop.
    //
    // A hold-to-record key-up is exempt (#111). It is a deliberate second
    // event, not bounce, and a hold shorter than the cooldown is completely
    // ordinary — applying it there would swallow the stop and leave the
    // recording running with the key already released. Rust still debounces
    // each edge at 50ms, so bounce protection is not lost.
    const now = Date.now();
    const elapsed = now - lastToggleTime;
    if (intent !== 'stop_only' && elapsed < TOGGLE_COOLDOWN_MS) {
      debug(`Toggle cooldown active (${elapsed}ms < ${TOGGLE_COOLDOWN_MS}ms), ignoring`);
      return { success: false, error: 'Toggle cooldown active' };
    }

    if (isToggling) {
      debug(' Toggle already in progress, ignoring');
      return { success: false, error: 'Toggle already in progress' };
    }
    isToggling = true;
    lastToggleTime = now;

    try {
      debug(' toggleRecording: calling pipeline_toggle_recording (Rust authority)');

      // Build config for the stop path (start path ignores it).
      const defaultConfig = await getDefaultConfig();
      const fullConfig: PipelineConfig = { ...defaultConfig, ...config };

      type ToggleOutcome = { action: 'started'; path: string } | { action: 'stopped' } | { action: 'ignored' };

      let outcome: ToggleOutcome;
      try {
        outcome = await invoke<ToggleOutcome>('pipeline_toggle_recording', {
          config: fullConfig,
          intent,
        });
      } catch (e) {
        const errorMsg = `${e}`;
        debug(' pipeline_toggle_recording error:', errorMsg);
        error = errorMsg;
        state = 'failed';
        invoke('hide_recording_indicator').catch(() => {});
        return { success: false, error: errorMsg };
      }

      if (outcome.action === 'ignored') {
        // A key-up with nothing running. Ordinary, not a failure.
        debug(' Rust decided: nothing to stop');
      } else if (outcome.action === 'started') {
        debug(' Rust decided: START, path:', outcome.path);
        // State is owned by the recording-state event emitted by Rust.
        // Set only local non-state fields here.
        error = null;
        lastResult = null;
        audioPath = outcome.path;
        recordingStartTime = Date.now();
        recordingDuration = 0;
        startDurationTimer();
      } else {
        debug(' Rust decided: STOP — recording-state event will update state');
        // BONG already played by Rust; indicator hidden by pipeline_stop_and_process.
        // State is owned by the recording-state Transcribing event emitted before
        // the detached task is spawned.
      }

      return { success: true };
    } finally {
      isToggling = false;
    }
  }

  /**
   * Cancel the current pipeline execution
   */
  async function cancel(): Promise<{ success: boolean; error?: string }> {
    if (!isRunning) {
      return { success: true }; // Nothing to cancel
    }

    try {
      // Hide recording indicator
      try {
        await invoke('hide_recording_indicator');
      } catch (e) {
        console.warn('[Pipeline] Failed to hide recording indicator:', e);
      }
      await invoke('pipeline_cancel');
      state = 'idle';
      isRunning = false;
      recordingStartTime = null;
      recordingDuration = 0;
      return { success: true };
    } catch (e) {
      const errorMsg = `${e}`;
      return { success: false, error: errorMsg };
    }
  }

  /**
   * Reset the pipeline state (after viewing result)
   */
  function reset(): void {
    if (!isRunning) {
      state = 'idle';
      message = '';
      error = null;
      recordingDuration = 0;
    }
  }

  /**
   * Transcribe an imported audio file (WAV, MP3, M4A, OGG, FLAC)
   */
  async function transcribeFile(filePath: string): Promise<{ success: boolean; result?: PipelineResult; error?: string }> {
    debug(' transcribeFile() called:', filePath);
    if (isRunning) {
      return { success: false, error: 'Pipeline is already running' };
    }

    error = null;
    lastResult = null;
    isRunning = true;

    try {
      const defaultConfig = await getDefaultConfig();
      const config: PipelineConfig = {
        ...defaultConfig,
        autoCopy: false,
        autoPaste: false,
        // File transcription writes nowhere, so auto-submit would fire a stray
        // Return into whatever happens to be focused (#112).
        autoSubmit: 'off',
      };

      const result = await invoke<PipelineResult>('pipeline_transcribe_file', {
        filePath,
        config,
      });

      lastResult = result;

      if (result.success) {
        state = 'completed';
        return { success: true, result };
      } else {
        state = 'failed';
        error = result.error ?? 'Unknown error';
        return { success: false, error: result.error ?? 'Unknown error' };
      }
    } catch (e) {
      const errorMsg = `${e}`;
      if (errorMsg.includes(NO_SPEECH_SENTINEL)) {
        // Silent import: not an error from the user's perspective.
        state = 'idle';
        toast.info('No speech detected');
        return { success: false, error: errorMsg };
      }
      console.error('[Pipeline] Exception in transcribeFile:', errorMsg);
      error = errorMsg;
      state = 'failed';
      return { success: false, error: errorMsg };
    } finally {
      isRunning = false;
    }
  }

  /**
   * Force reset the pipeline state (for recovery from stuck states)
   * This should only be called when the pipeline is known to be stuck
   */
  async function forceReset(): Promise<void> {
    debug(' Force reset called');
    try {
      // Cancel any running pipeline on the backend
      await invoke('pipeline_cancel');
    } catch (e) {
      console.warn('[Pipeline] Failed to cancel backend pipeline:', e);
    }
    // Reset all frontend state
    state = 'idle';
    message = '';
    error = null;
    isRunning = false;
    lastResult = null;
    audioPath = null;
    recordingStartTime = null;
    recordingDuration = 0;
    debug(' Force reset complete');
  }

  /**
   * Start the duration timer for recording
   */
  function startDurationTimer(): void {
    const interval = setInterval(() => {
      if (recordingStartTime && state === 'recording') {
        recordingDuration = Math.floor((Date.now() - recordingStartTime) / 1000);
      } else {
        clearInterval(interval);
      }
    }, 100);
  }

  /**
   * Format duration as MM:SS
   */
  function formatDuration(seconds: number): string {
    const mins = Math.floor(seconds / 60);
    const secs = seconds % 60;
    return `${mins}:${secs.toString().padStart(2, '0')}`;
  }

  return {
    // Reactive state (getters)
    get state() {
      return state;
    },
    get message() {
      return message;
    },
    get isRunning() {
      return isRunning;
    },
    get isRecording() {
      return state === 'recording';
    },
    get isProcessing() {
      return state === 'converting' || state === 'transcribing' || state === 'filtering' || state === 'enhancing' || state === 'outputting';
    },
    get lastResult() {
      return lastResult;
    },
    get error() {
      return error;
    },
    get audioPath() {
      return audioPath;
    },
    get recordingDuration() {
      return recordingDuration;
    },
    get formattedDuration() {
      return formatDuration(recordingDuration);
    },

    // Actions
    initialise,
    cleanup,
    toggleRecording,
    transcribeFile,
    cancel,
    reset,
    forceReset,
    getDefaultConfig,
  };
}

/** Singleton pipeline store instance */
export const pipelineStore = createPipelineStore();
