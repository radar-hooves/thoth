// The Input Device pane tells one coherent story per state: the banner, the
// dropdown's shown value and the refresh control agree, and no raw device id
// or "Select device…" shows while a device is in effect.
import { describe, expect, it, vi, beforeEach } from 'vitest';
import { render, screen, waitFor, fireEvent } from '@testing-library/svelte';

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock('@tauri-apps/api/core', () => ({ invoke }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}) }));
vi.mock('svelte-sonner', () => ({ toast: { error: vi.fn(), success: vi.fn() } }));

import AudioDeviceSelector from './AudioDeviceSelector.svelte';
import { configStore } from '../stores/config.svelte';
import { settingsStore } from '../stores/settings.svelte';
import { toast } from 'svelte-sonner';

const PLANTRONICS = 'coreaudio:AppleUSBAudioEngine:Plantronics:Plantronics Blackwire 3220 Series:D0D3AA88166AA942A9AD34FF6FEA0410:1';
const DJI = { id: 'coreaudio:DJI', name: 'DJI MIC MINI', is_default: true };
const BLACKWIRE = { id: PLANTRONICS, name: 'Plantronics Blackwire 3220 Series', is_default: false };

function mount(saved: string | null, devices: unknown[]) {
  invoke.mockImplementation(async (cmd: string) => (cmd === 'list_audio_devices' ? devices : undefined));
  configStore.updateAudio('deviceId', saved);
  return render(AudioDeviceSelector);
}

describe('AudioDeviceSelector states', () => {
  beforeEach(() => vi.clearAllMocks());

  it('saved device absent: names it, says what is used, shows it in the dropdown', async () => {
    mount(PLANTRONICS, [DJI]);
    await screen.findByText(/isn't connected, so Thoth is using DJI MIC MINI \(system default\)/);
    expect(screen.getByText('Using System Default')).toBeTruthy();
    const trigger = document.getElementById('audio-device-select')!;
    expect(trigger.textContent).toContain('Plantronics Blackwire 3220 Series (not connected)');
    expect(trigger.textContent).not.toContain('coreaudio:');
    expect(screen.queryByText('Unknown Device')).toBeNull();
    expect(screen.queryByRole('button', { name: 'Refresh' })).toBeNull();
    expect(screen.getAllByTitle('Refresh device list')).toHaveLength(1);
  });

  it('saved device connected: active, dropdown shows its name', async () => {
    mount(PLANTRONICS, [DJI, BLACKWIRE]);
    await screen.findByText('Active Device');
    expect(document.getElementById('audio-device-select')!.textContent).toContain('Plantronics Blackwire 3220 Series');
  });

  it('system default: active, dropdown never reads "Select device…"', async () => {
    mount(null, [DJI]);
    await screen.findByText('DJI MIC MINI (System Default)');
    const text = document.getElementById('audio-device-select')!.textContent ?? '';
    expect(text).toContain('System Default (DJI MIC MINI)');
    expect(text).not.toContain('Select device');
  });

  it('device returns: the pane follows the store back to the saved device', async () => {
    mount(PLANTRONICS, [DJI]);
    await screen.findByText(/isn't connected/);
    invoke.mockImplementation(async () => [DJI, BLACKWIRE]);
    await settingsStore.loadAudioDevices();
    await waitFor(() => expect(screen.getByText('Active Device')).toBeTruthy());
    expect(screen.queryByText(/isn't connected/)).toBeNull();
  });

  it('no input at all: says so', async () => {
    mount(PLANTRONICS, []);
    await screen.findByText(/no other input device is available/);
  });

  it('refresh re-enumerates and confirms', async () => {
    mount(PLANTRONICS, [DJI]);
    await screen.findByText(/isn't connected/);
    await fireEvent.click(screen.getByTitle('Refresh device list'));
    await waitFor(() => expect(toast.success).toHaveBeenCalledWith('Devices refreshed: 1 input device found'));
    expect(invoke.mock.calls.filter(([c]) => c === 'list_audio_devices').length).toBeGreaterThan(1);
  });
});
