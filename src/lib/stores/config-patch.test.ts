import { describe, expect, it } from 'vitest';
import { buildPatch } from './config-patch';

const base = {
  version: 1,
  audio: { device_id: 'mic', sample_rate: 16000 },
  shortcuts: { toggle_recording: 'F13', copy_last: null },
  tags: ['a', 'b'],
};

describe('buildPatch', () => {
  it('names a changed leaf only', () => {
    const next = structuredClone(base);
    next.audio.sample_rate = 48000;
    expect(buildPatch(base, next)).toEqual({ audio: { sample_rate: 48000 } });
  });

  it('is null for an unchanged document', () => {
    expect(buildPatch(base, structuredClone(base))).toBeNull();
  });

  it('nests changes across sections', () => {
    const next = structuredClone(base);
    next.audio.device_id = 'other';
    next.shortcuts.toggle_recording = 'F14';
    expect(buildPatch(base, next)).toEqual({
      audio: { device_id: 'other' },
      shortcuts: { toggle_recording: 'F14' },
    });
  });

  it('sends a changed array whole', () => {
    const next = structuredClone(base);
    next.tags = ['a', 'c'];
    expect(buildPatch(base, next)).toEqual({ tags: ['a', 'c'] });
  });

  it('sends null to clear a value, and a value over null', () => {
    const cleared = structuredClone(base) as Record<string, unknown>;
    (cleared.audio as Record<string, unknown>).device_id = null;
    expect(buildPatch(base, cleared)).toEqual({ audio: { device_id: null } });

    const set = structuredClone(base);
    set.shortcuts.copy_last = 'F15' as unknown as null;
    expect(buildPatch(base, set)).toEqual({ shortcuts: { copy_last: 'F15' } });
  });

  it('leaves out skipped paths', () => {
    const next = structuredClone(base);
    next.audio.device_id = 'other';
    next.audio.sample_rate = 8000;
    expect(buildPatch(base, next, ['audio.device_id'])).toEqual({ audio: { sample_rate: 8000 } });
  });
});
