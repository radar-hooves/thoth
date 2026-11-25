import { describe, expect, it } from 'vitest';
import { bannerText, friendlyNameFromId, type InputResolution } from './audio-device';
import type { AudioDevice } from './stores/settings.svelte';

const PLANTRONICS = 'coreaudio:AppleUSBAudioEngine:Plantronics:Plantronics Blackwire 3220 Series:D0D3AA88166AA942A9AD34FF6FEA0410:1';
const dji: AudioDevice = { id: 'coreaudio:DJI', name: 'DJI MIC MINI', is_default: true };
const pods: AudioDevice = { id: 'coreaudio:pods', name: 'AirPods Pro', is_default: true };
const mac: AudioDevice = { id: 'coreaudio:mac', name: 'MacBook Microphone', is_default: false };

const res = (r: Partial<InputResolution>): InputResolution => ({
  saved_missing: false,
  active: dji,
  system_default: dji,
  reason: 'system_default',
  ...r,
});

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

describe('bannerText', () => {
  it('system default', () => {
    expect(bannerText(res({}), null)).toEqual({
      ok: true,
      title: 'Active Device',
      detail: 'DJI MIC MINI (System Default)',
    });
  });
  it('saved device connected', () => {
    const b = bannerText(res({ reason: 'saved', active: dji }), PLANTRONICS);
    expect(b).toMatchObject({ ok: true, detail: 'DJI MIC MINI' });
  });
  it('saved device absent: names it and what is used', () => {
    const b = bannerText(res({ saved_missing: true }), PLANTRONICS);
    expect(b.ok).toBe(false);
    expect(b.detail).toContain("Plantronics Blackwire 3220 Series isn't connected, so Thoth is using DJI MIC MINI (system default)");
    expect(b.detail).toContain('uses Plantronics Blackwire 3220 Series again once it is reconnected');
  });
  it('default is Bluetooth: names the device actually recorded from', () => {
    const b = bannerText(res({ reason: 'bluetooth_avoided', active: mac, system_default: pods }), null);
    expect(b.ok).toBe(true);
    expect(b.detail).toContain('MacBook Microphone (System Default)');
    expect(b.detail).toContain('AirPods Pro is Bluetooth, so Thoth records from MacBook Microphone');
  });
  it('default is Bluetooth and the saved device is absent: both reasons', () => {
    const b = bannerText(res({ saved_missing: true, reason: 'bluetooth_avoided', active: mac, system_default: pods }), PLANTRONICS);
    expect(b.detail).toContain('so Thoth is using MacBook Microphone');
    expect(b.detail).toContain('AirPods Pro is Bluetooth');
  });
  it('only a Bluetooth mic exists: warns', () => {
    const b = bannerText(res({ reason: 'bluetooth_only', active: pods, system_default: pods }), null);
    expect(b.ok).toBe(false);
    expect(b.detail).toContain('call quality');
  });
  it('no input at all', () => {
    expect(bannerText(res({ active: null, system_default: null }), null).title).toBe('No Microphone');
    expect(bannerText(res({ active: null, saved_missing: true, system_default: null }), PLANTRONICS).detail).toContain("isn't connected and no other input device");
  });
});
