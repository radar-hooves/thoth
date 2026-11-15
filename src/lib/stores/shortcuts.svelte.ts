/**
 * Shortcut state management for Thoth
 *
 * Manages global keyboard shortcuts configuration and synchronisation
 * with the Tauri backend.
 */

import { invoke } from '@tauri-apps/api/core';
import { toast } from 'svelte-sonner';

/** Information about a keyboard shortcut */
export interface ShortcutInfo {
  /** Unique identifier for the shortcut */
  id: string;
  /** Keyboard accelerator string (e.g., "F13", "CommandOrControl+Shift+Space") */
  accelerator: string;
  /** Human-readable description of what the shortcut does */
  description: string;
  /** Whether the shortcut is currently enabled/registered */
  isEnabled: boolean;
}

/** A shortcut the system refused to register */
export interface ShortcutConflict {
  /** The shortcut that failed to register */
  shortcut: string;
  /** The ID of the shortcut that was being registered */
  shortcutId: string;
  /** Human-readable reason for the failure */
  reason: string;
}

/** Tell the user which shortcuts the system refused, and why. */
export function reportShortcutFailures(failed: ShortcutConflict[] | null | undefined): void {
  for (const f of failed ?? []) {
    toast.error(`Shortcut ${formatForDisplay(f.shortcut)} could not be registered`, {
      description: f.reason,
    });
  }
}

/**
 * Format an accelerator string for human-readable display
 *
 * @param accelerator - The Tauri accelerator string
 * @returns Human-readable display string
 */
export function formatForDisplay(accelerator: string): string {
  if (!accelerator) return '';

  return (
    accelerator
      // Handle right-side modifier codes first (before generic replacements)
      .replace(/ShiftRight/g, 'Right ⇧')
      .replace(/ShiftLeft/g, 'Left ⇧')
      .replace(/AltRight/g, navigator.platform.includes('Mac') ? 'Right ⌥' : 'Right Alt')
      .replace(/AltLeft/g, navigator.platform.includes('Mac') ? 'Left ⌥' : 'Left Alt')
      .replace(/ControlRight/g, 'Right ⌃')
      .replace(/ControlLeft/g, 'Left ⌃')
      .replace(/MetaRight/g, 'Right ⌘')
      .replace(/MetaLeft/g, 'Left ⌘')
      // Then generic modifiers
      .replace(/CommandOrControl/g, navigator.platform.includes('Mac') ? '⌘' : 'Ctrl')
      .replace(/Command/g, '⌘')
      .replace(/Control/g, '⌃')
      .replace(/Alt/g, navigator.platform.includes('Mac') ? '⌥' : 'Alt')
      .replace(/Shift/g, '⇧')
      .replace(/\+/g, ' ')
  );
}

/**
 * Validate a shortcut accelerator string
 *
 * @param accelerator - The accelerator string to validate
 * @returns An error message if invalid, or null if valid
 */
export function validateShortcut(accelerator: string): string | null {
  if (!accelerator || accelerator.trim() === '') {
    return 'Shortcut cannot be empty';
  }

  // Check for valid format (no leading/trailing +, no empty parts)
  if (accelerator.startsWith('+') || accelerator.endsWith('+')) {
    return 'Invalid shortcut format';
  }
  if (accelerator.includes('++')) {
    return 'Invalid shortcut format';
  }

  const parts = accelerator.split('+');

  if (parts.length === 0) {
    return 'Invalid shortcut format';
  }

  // Check for at least one non-modifier key
  // Note: Right-side modifier codes (ShiftRight, AltRight, etc.) count as main keys
  const modifiers = new Set(['CommandOrControl', 'Command', 'Control', 'Alt', 'Shift', 'Meta']);
  const rightModifierCodes = new Set([
    'ShiftRight',
    'ShiftLeft',
    'ControlRight',
    'ControlLeft',
    'AltRight',
    'AltLeft',
    'MetaRight',
    'MetaLeft',
  ]);
  const hasNonModifier = parts.some((part) => !modifiers.has(part) || rightModifierCodes.has(part));

  if (!hasNonModifier) {
    return 'Shortcut must include a non-modifier key';
  }

  // Any key or key combination is valid - no requirement for modifiers
  return null;
}

/**
 * Create a shortcuts store for managing shortcut state
 */
export function createShortcutsStore() {
  let shortcuts = $state<ShortcutInfo[]>([]);
  let defaults = $state<ShortcutInfo[]>([]);
  let isLoading = $state<boolean>(false);

  /** The backend sends `is_enabled`; the store exposes `isEnabled`. */
  function fromBackend(list: ShortcutInfo[]): ShortcutInfo[] {
    return list.map((s) => ({
      id: s.id,
      accelerator: s.accelerator,
      description: s.description,
      isEnabled: (s as unknown as { is_enabled?: boolean }).is_enabled ?? s.isEnabled ?? false,
    }));
  }

  async function loadDefaults(): Promise<void> {
    try {
      defaults = fromBackend(await invoke<ShortcutInfo[]>('get_default_shortcuts'));
    } catch (e) {
      console.error('Failed to load default shortcuts:', e);
    }
  }

  async function loadRegistered(): Promise<void> {
    isLoading = true;
    try {
      shortcuts = fromBackend(await invoke<ShortcutInfo[]>('list_registered_shortcuts'));
    } catch (e) {
      console.error('Failed to load registered shortcuts:', e);
    } finally {
      isLoading = false;
    }
  }

  async function initialise(): Promise<void> {
    await Promise.all([loadDefaults(), loadRegistered()]);
  }

  return {
    get shortcuts() {
      return shortcuts;
    },
    get defaults() {
      return defaults;
    },
    get isLoading() {
      return isLoading;
    },
    loadDefaults,
    loadRegistered,
    initialise,
  };
}

/** Singleton shortcuts store instance */
export const shortcutsStore = createShortcutsStore();
