/** What the Input Device pane shows, resolved from the saved choice and the devices present. */

import type { AudioDevice } from './stores/settings.svelte';

export type InputState =
  /** Nothing saved; recording from the system default. */
  | 'default'
  /** The saved device is connected and in use. */
  | 'saved'
  /** The saved device is absent; recording from the system default. */
  | 'fallback'
  /** The saved device is absent and there is no input at all. */
  | 'none';

export interface InputResolution {
  state: InputState;
  /** The saved device's friendly name, when one is saved. */
  savedName: string | null;
  /** The device recording uses now. */
  active: AudioDevice | null;
}

const SERIAL = /^([0-9A-F]{8,}|\d+)$/i;

/**
 * The friendly name inside a saved cpal device id, for a device that is not
 * connected and so has no entry to read it from. CoreAudio ids end in the
 * device UID (`AppleUSBAudioEngine:<vendor>:<name>:<serial>:<n>`); ALSA ids
 * carry `CARD=<name>`.
 */
export function friendlyNameFromId(id: string): string | null {
  const card = /CARD=([^,:]+)/.exec(id);
  if (card) return (card[1] ?? '').replace(/_/g, ' ');
  const [, ...parts] = id.split(':');
  const names = parts.filter((p) => p && !SERIAL.test(p) && !/Engine$/.test(p));
  return names.at(-1) ?? null;
}

export function resolveInput(savedId: string | null, devices: AudioDevice[]): InputResolution {
  const systemDefault = devices.find((d) => d.is_default) ?? null;
  if (!savedId) {
    return { state: systemDefault ? 'default' : 'none', savedName: null, active: systemDefault };
  }
  const saved = devices.find((d) => d.id === savedId);
  if (saved) return { state: 'saved', savedName: saved.name, active: saved };
  return {
    state: systemDefault ? 'fallback' : 'none',
    savedName: friendlyNameFromId(savedId) ?? 'Your saved microphone',
    active: systemDefault,
  };
}

export function bannerText(r: InputResolution): { title: string; detail: string; ok: boolean } {
  switch (r.state) {
    case 'default':
      return { ok: true, title: 'Active Device', detail: `${r.active?.name} (System Default)` };
    case 'saved':
      return { ok: true, title: 'Active Device', detail: r.active?.name ?? '' };
    case 'fallback':
      return {
        ok: false,
        title: 'Using System Default',
        detail: `${r.savedName} isn't connected, so Thoth is using ${r.active?.name} (system default). Thoth uses ${r.savedName} again once it is reconnected.`,
      };
    case 'none':
      return {
        ok: false,
        title: 'No Microphone',
        detail: r.savedName ? `${r.savedName} isn't connected and no other input device is available.` : 'No input device is available.',
      };
  }
}
