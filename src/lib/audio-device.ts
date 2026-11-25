/** What the Input Device pane shows, resolved from the saved choice and the devices present. */

import type { AudioDevice } from './stores/settings.svelte';

/** The backend's resolution (`resolve_input_device`): the device recording uses, and why. */
export interface InputResolution {
  saved_missing: boolean;
  active: AudioDevice | null;
  system_default: AudioDevice | null;
  reason: 'saved' | 'system_default' | 'bluetooth_avoided' | 'bluetooth_only';
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

/** What the pane says. Names come from the backend's resolution; only a missing device's name is read from its id. */
export function bannerText(r: InputResolution, savedId: string | null): { title: string; detail: string; ok: boolean } {
  if (!r.active) {
    return {
      ok: false,
      title: 'No Microphone',
      detail: savedId ? `${savedName(savedId)} isn't connected and no other input device is available.` : 'No input device is available.',
    };
  }
  const using = r.active.name;
  const bluetooth =
    r.reason === 'bluetooth_avoided'
      ? ` ${r.system_default?.name} is Bluetooth, so Thoth records from ${using} to keep headphone audio in high quality.`
      : r.reason === 'bluetooth_only'
        ? ' It is a Bluetooth mic, so headphone audio drops to call quality while recording.'
        : '';
  if (r.saved_missing && savedId) {
    const saved = savedName(savedId);
    return {
      ok: false,
      title: 'Using System Default',
      detail: `${saved} isn't connected, so Thoth is using ${using} (system default).${bluetooth} Thoth uses ${saved} again once it is reconnected.`,
    };
  }
  if (r.reason === 'saved') return { ok: true, title: 'Active Device', detail: using };
  return {
    ok: r.reason !== 'bluetooth_only',
    title: 'Active Device',
    detail: `${using} (System Default)${bluetooth}`,
  };
}

function savedName(id: string): string {
  return friendlyNameFromId(id) ?? 'Your saved microphone';
}
