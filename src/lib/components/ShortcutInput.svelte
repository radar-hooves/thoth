<script lang="ts">
  /**
   * ShortcutInput - Key capture component for configuring keyboard shortcuts
   *
   * Uses the unified keyboard_service in the Rust backend, which manages a
   * single polling thread with modal capture/monitoring states. Entering
   * capture mode is a single IPC call that atomically switches the mode,
   * unregisters all shortcuts, and starts reporting key presses as capture
   * events. Exiting capture mode re-registers everything from config.
   *
   * On Wayland, falls back to webview keyboard events since device_query
   * doesn't work there (X11-only).
   */

  import { onDestroy } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { listen, type UnlistenFn } from '@tauri-apps/api/event';
  import {
    formatForDisplay,
    reportShortcutFailures,
    validateShortcut,
    type ShortcutConflict,
  } from '../stores/shortcuts.svelte';
  import { Button } from '@poodle64/ui/button';
  import X from '@lucide/svelte/icons/x';
  import RotateCcw from '@lucide/svelte/icons/rotate-ccw';

  /** Debug logging — only active in development builds */
  const debug = import.meta.env.DEV
    ? (...args: unknown[]) => console.log('[ShortcutInput]', ...args)
    : () => {};

  interface Props {
    /** Current shortcut accelerator string */
    value: string;
    /** Unique identifier for this shortcut (used to skip re-registration) */
    shortcutId?: string;
    /** Callback when shortcut changes (can be async) */
    onchange?: (accelerator: string) => void | Promise<void>;
    /** Callback when shortcut is cleared */
    onclear?: () => void;
    /** Placeholder text when no shortcut is set */
    placeholder?: string;
    /** Whether the input is disabled */
    disabled?: boolean;
    /** Default value for reset functionality */
    defaultValue?: string;
    /** Callback when reset to default is clicked */
    onreset?: () => void;
  }

  let {
    value = '',
    shortcutId,
    onchange,
    onclear,
    placeholder = 'Click to record shortcut',
    disabled = false,
    defaultValue,
    onreset,
  }: Props = $props();

  let isCapturing = $state(false);
  let validationError = $state<string | null>(null);
  let pendingKeys = $state<string[]>([]);
  let buttonRef = $state<HTMLButtonElement | null>(null);
  /** Event listeners to clean up */
  let unlisteners: UnlistenFn[] = [];
  /** Whether using webview mode (Wayland) */
  let webviewMode = $state(false);

  /** Formatted display value for the current shortcut */
  let displayValue = $derived(formatForDisplay(value));

  /** Whether the current value differs from the default */
  let isModified = $derived(defaultValue !== undefined && value !== defaultValue);

  /** Display string for currently pressed keys */
  let pendingDisplay = $derived(pendingKeys.join(' + '));

  // Clean up on destroy
  onDestroy(async () => {
    await cleanup();
  });

  /**
   * Clean up event listeners and exit capture mode if active
   */
  async function cleanup(): Promise<void> {
    for (const unlisten of unlisteners) {
      unlisten();
    }
    unlisteners = [];

    if (isCapturing) {
      try {
        await invoke('exit_capture_mode');
      } catch {
        // Ignore errors during cleanup
      }
    }
  }

  /**
   * Start capturing keyboard input
   *
   * A single IPC call to enter_capture_mode handles everything:
   * switching the mode, unregistering shortcuts, and starting capture.
   */
  async function startCapture(): Promise<void> {
    if (disabled || isCapturing) return;

    debug('Starting capture...');

    isCapturing = true;
    validationError = null;
    pendingKeys = [];

    try {
      // Set up event listeners for capture events
      const updateUnlisten = await listen<{
        keys: string[];
        accelerator: string;
        isValid: boolean;
      }>('key-capture-update', (event) => {
        debug('Key update:', event.payload);
        pendingKeys = event.payload.keys;

        // Show validation error only if we have a non-modifier key and it's invalid
        if (event.payload.accelerator && !event.payload.isValid) {
          const hasNonModifier = event.payload.keys.some(
            (k) => !['Cmd', 'Ctrl', 'Alt', 'Shift', 'Super', 'Meta'].includes(k)
          );
          if (hasNonModifier) {
            validationError = 'Invalid shortcut combination';
          } else {
            validationError = null;
          }
        } else {
          validationError = null;
        }
      });
      unlisteners.push(updateUnlisten);

      const completeUnlisten = await listen<{
        accelerator: string;
        keys: string[];
        isValid: boolean;
      }>('key-capture-complete', async (event) => {
        debug('Capture complete:', event.payload);

        if (event.payload.isValid) {
          const error = validateShortcut(event.payload.accelerator);
          if (error) {
            validationError = error;
          } else {
            // onchange saves to config; exit_capture_mode re-registers from config
            await onchange?.(event.payload.accelerator);
            validationError = null;
          }
        }

        await stopCapture();
      });
      unlisteners.push(completeUnlisten);

      // Enter capture mode — returns 'native' or 'webview'
      const mode = await invoke<string>('enter_capture_mode');
      webviewMode = mode === 'webview';
      debug(`Capture started in ${mode} mode`);

      // Focus button for visual feedback
      buttonRef?.focus();
    } catch (e) {
      console.error('Failed to start capture:', e);
      const errorMsg = String(e);

      validationError = errorMsg.includes('Accessibility permission required')
        ? 'Accessibility permission required: press Grant on the Overview page'
        : `Failed to start capture: ${e}`;
      await stopCapture();
    }
  }

  /**
   * Stop capturing and exit capture mode
   *
   * A single IPC call to exit_capture_mode handles everything:
   * switching mode back, and re-registering all shortcuts from config.
   */
  async function stopCapture(): Promise<void> {
    if (!isCapturing) return;

    isCapturing = false;
    pendingKeys = [];
    webviewMode = false;

    // Clean up listeners
    for (const unlisten of unlisteners) {
      unlisten();
    }
    unlisteners = [];

    try {
      reportShortcutFailures(await invoke<ShortcutConflict[]>('exit_capture_mode'));
    } catch (e) {
      console.error('Failed to exit capture mode:', e);
    }
  }

  /**
   * Cancel capture without applying changes
   */
  async function cancelCapture(): Promise<void> {
    validationError = null;
    await stopCapture();
  }

  /**
   * Handle keyboard events
   * - In webview mode (Wayland): report keys to backend
   * - In native mode: only handle Escape to cancel
   */
  async function handleKeyDown(event: KeyboardEvent): Promise<void> {
    if (!isCapturing) return;

    // Escape cancels capture
    if (event.key === 'Escape') {
      event.preventDefault();
      await cancelCapture();
      return;
    }

    // In webview mode, report key events to backend
    if (webviewMode) {
      event.preventDefault();

      // Ignore pure modifier keydowns (they're tracked separately)
      const isModifier = ['Control', 'Shift', 'Alt', 'Meta'].includes(event.key);
      if (isModifier) {
        const modNames: string[] = [];
        if (event.ctrlKey) modNames.push('Ctrl');
        if (event.shiftKey) modNames.push('Shift');
        if (event.altKey) modNames.push('Alt');
        if (event.metaKey) modNames.push('Super');
        pendingKeys = modNames;
        return;
      }

      try {
        await invoke('report_key_event', {
          key: event.key,
          code: event.code,
          ctrl: event.ctrlKey,
          shift: event.shiftKey,
          alt: event.altKey,
          meta: event.metaKey,
          eventType: 'keydown',
        });
      } catch (e) {
        console.error('Failed to report key event:', e);
      }
    }
  }

  /**
   * Handle blur event - stop capture when focus is lost
   */
  async function handleBlur(): Promise<void> {
    if (isCapturing) {
      debug('Blur - stopping capture');
      await stopCapture();
    }
  }

  /**
   * Clear the current shortcut
   */
  function handleClear(event: MouseEvent): void {
    event.stopPropagation();
    onclear?.();
    validationError = null;
  }

  /**
   * Reset to default value
   */
  function handleReset(event: MouseEvent): void {
    event.stopPropagation();
    onreset?.();
    validationError = null;
  }
</script>

<div class="flex flex-wrap items-center gap-2">
  <!-- Capture trigger button — bespoke keyboard-capture styling retained via class -->
  <button
    bind:this={buttonRef}
    type="button"
    class="shortcut-capture-btn"
    class:capturing={isCapturing}
    class:has-error={!!validationError}
    onclick={startCapture}
    onkeydown={handleKeyDown}
    onblur={handleBlur}
    {disabled}
  >
    {#if isCapturing}
      <span class="text-primary italic">
        {#if pendingKeys.length > 0}
          {pendingDisplay}
        {:else}
          Press keys...
        {/if}
      </span>
    {:else if value}
      <span class="font-mono tracking-wide">{displayValue}</span>
    {:else}
      <span class="text-muted-foreground">{placeholder}</span>
    {/if}
  </button>

  <div class="flex gap-1">
    {#if value && !disabled}
      <Button variant="ghost" size="icon" title="Clear shortcut" onclick={handleClear}>
        <X />
      </Button>
    {/if}
    {#if isModified && onreset && !disabled}
      <Button variant="ghost" size="icon" title="Reset to default" onclick={handleReset}>
        <RotateCcw />
      </Button>
    {/if}
  </div>

  {#if validationError}
    <span class="w-full text-xs text-destructive">{validationError}</span>
  {/if}
</div>

<style>
  .shortcut-capture-btn {
    min-width: 180px;
    padding: 8px 12px;
    border: 1px solid var(--border);
    border-radius: var(--radius-lg, 8px);
    background: color-mix(in srgb, var(--input) 30%, transparent);
    color: var(--foreground);
    font-family: inherit;
    font-size: 0.875rem;
    text-align: left;
    cursor: pointer;
    transition:
      border-color 0.15s ease,
      background 0.15s ease,
      box-shadow 0.15s ease;
    outline: none;
  }

  .shortcut-capture-btn:hover:not(:disabled) {
    border-color: var(--ring);
  }

  .shortcut-capture-btn:focus-visible {
    border-color: var(--ring);
    box-shadow: 0 0 0 3px color-mix(in srgb, var(--ring) 50%, transparent);
  }

  .shortcut-capture-btn.capturing {
    border-color: var(--ring);
    background: var(--muted);
    box-shadow: 0 0 0 3px color-mix(in srgb, var(--ring) 20%, transparent);
  }

  .shortcut-capture-btn.has-error {
    border-color: var(--destructive);
  }

  .shortcut-capture-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
</style>
