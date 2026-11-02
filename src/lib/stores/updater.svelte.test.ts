import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const { check, invoke, toast } = vi.hoisted(() => ({
  check: vi.fn(),
  invoke: vi.fn(() => Promise.resolve()),
  toast: { info: vi.fn(), error: vi.fn(), loading: vi.fn(), success: vi.fn() },
}));

vi.mock('@tauri-apps/plugin-updater', () => ({ check }));
vi.mock('@tauri-apps/plugin-process', () => ({ relaunch: vi.fn() }));
vi.mock('@tauri-apps/api/core', () => ({ invoke }));
vi.mock('svelte-sonner', () => ({ toast }));

import { UPDATE_RECHECK_MS, getUpdaterState, startPeriodicUpdateChecks } from './updater.svelte';

describe('periodic update checks', () => {
  let stop: () => void = () => {};

  beforeEach(() => {
    vi.useFakeTimers();
    check.mockReset();
    toast.info.mockReset();
    toast.error.mockReset();
    getUpdaterState().state = 'idle';
  });

  afterEach(() => {
    stop();
    vi.useRealTimers();
  });

  it('offers a release published after launch', async () => {
    check.mockResolvedValue({ version: '2026.11.1' });
    stop = startPeriodicUpdateChecks(() => true);

    await vi.advanceTimersByTimeAsync(UPDATE_RECHECK_MS);

    expect(check).toHaveBeenCalledTimes(1);
    expect(getUpdaterState().updateVersion).toBe('2026.11.1');
    expect(toast.info).toHaveBeenCalledTimes(1);
  });

  it('does not offer the same update again once it is on offer', async () => {
    check.mockResolvedValue({ version: '2026.11.1' });
    stop = startPeriodicUpdateChecks(() => true);

    await vi.advanceTimersByTimeAsync(3 * UPDATE_RECHECK_MS);

    expect(check).toHaveBeenCalledTimes(1);
    expect(toast.info).toHaveBeenCalledTimes(1);
  });

  it('fails quietly while offline', async () => {
    check.mockRejectedValue(new Error('network unreachable'));
    stop = startPeriodicUpdateChecks(() => true);

    await vi.advanceTimersByTimeAsync(2 * UPDATE_RECHECK_MS);

    expect(check).toHaveBeenCalledTimes(2);
    expect(toast.error).not.toHaveBeenCalled();
  });

  it('does nothing while update checks are switched off', async () => {
    stop = startPeriodicUpdateChecks(() => false);

    await vi.advanceTimersByTimeAsync(2 * UPDATE_RECHECK_MS);

    expect(check).not.toHaveBeenCalled();
  });
});
