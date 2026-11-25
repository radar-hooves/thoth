import { describe, expect, it } from 'vitest';
import { bannerText, friendlyNameFromId, resolveInput } from './audio-device';
import type { AudioDevice } from './stores/settings.svelte';

const PLANTRONICS = 'coreaudio:AppleUSBAudioEngine:Plantronics:Plantronics Blackwire 3220 Series:D0D3AA88166AA942A9AD34FF6FEA0410:1';
const dji: AudioDevice = { id: 'coreaudio:DJI', name: 'DJI MIC MINI', is_default: true };
const plantronics: AudioDevice = {
  id: PLANTRONICS,
  name: 'Plantronics Blackwire 3220 Series',
  is_default: false,
};

describe('friendlyNameFromId', () => {
  it('reads the device name out of a CoreAudio UID', () => {
    expect(friendlyNameFromId(PLANTRONICS)).toBe('Plantronics Blackwire 3220 Series');
  });
  it('reads the card out of an ALSA id', () => {
    expect(friendlyNameFromId('alsa:sysdefault:CARD=Blackwire_3220,DEV=0')).toBe('Blackwire 3220');
  });
  it('gives up on an id with no name in it', () => {
    expect(friendlyNameFromId('coreaudio:0A1B2C3D4E5F:1')).toBeNull();
  });
});

describe('resolveInput', () => {
  it('uses the system default when nothing is saved', () => {
    const r = resolveInput(null, [dji]);
    expect(r).toMatchObject({ state: 'default', active: dji });
    expect(bannerText(r)).toMatchObject({ ok: true, detail: 'DJI MIC MINI (System Default)' });
  });
  it('uses the saved device when connected', () => {
    const r = resolveInput(PLANTRONICS, [dji, plantronics]);
    expect(r).toMatchObject({ state: 'saved', active: plantronics });
    expect(bannerText(r)).toMatchObject({ ok: true, detail: plantronics.name });
  });
  it('falls back to the default, by name, when the saved device is absent', () => {
    const r = resolveInput(PLANTRONICS, [dji]);
    expect(r).toMatchObject({
      state: 'fallback',
      savedName: 'Plantronics Blackwire 3220 Series',
      active: dji,
    });
    const b = bannerText(r);
    expect(b.ok).toBe(false);
    expect(b.detail).toContain("Plantronics Blackwire 3220 Series isn't connected, so Thoth is using DJI MIC MINI (system default)");
  });
  it('returns to the saved device when it reappears', () => {
    expect(resolveInput(PLANTRONICS, [dji]).state).toBe('fallback');
    expect(resolveInput(PLANTRONICS, [dji, plantronics]).state).toBe('saved');
  });
  it('says so when there is no input at all', () => {
    expect(resolveInput(PLANTRONICS, []).state).toBe('none');
    expect(resolveInput(null, []).state).toBe('none');
  });
});
