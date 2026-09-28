/**
 * Auto-update state management using Svelte 5 runes.
 *
 * Manages update checking, downloading, and installation via tauri-plugin-updater.
 * User-facing notifications are handled via sonner toasts.
 */

import { check, type Update } from '@tauri-apps/plugin-updater';
import { relaunch } from '@tauri-apps/plugin-process';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'svelte-sonner';

/** GitHub releases page for manual download fallback */
export const RELEASES_URL = 'https://github.com/radar-hooves/thoth/releases/latest';

/** Update state visible to the Overview pane */
export type UpdateState = 'idle' | 'checking' | 'available' | 'downloading' | 'up-to-date' | 'error';

interface UpdaterState {
  state: UpdateState;
  update: Update | null;
  updateVersion: string | null;
  error: string | null;
}

const updaterState = $state<UpdaterState>({
  state: 'idle',
  update: null,
  updateVersion: null,
  error: null,
});

function openReleasesPage() {
  invoke('open_url', { url: RELEASES_URL }).catch((err) => console.error('Failed to open releases page:', err));
}

/** Download and install the available update, then relaunch */
async function downloadAndInstall(): Promise<void> {
  if (!updaterState.update) return;

  const toastId = toast.loading('Downloading update...');

  try {
    updaterState.state = 'downloading';

    let downloaded = 0;
    await updaterState.update.downloadAndInstall((event) => {
      switch (event.event) {
        case 'Progress': {
          const chunk = event.data as { chunkLength: number; contentLength?: number };
          downloaded += chunk.chunkLength;
          if (chunk.contentLength && chunk.contentLength > 0) {
            const pct = Math.min(99, Math.round((downloaded / chunk.contentLength) * 100));
            toast.loading(`Downloading update... ${pct}%`, { id: toastId });
          }
          break;
        }
        case 'Finished':
          toast.success('Update installed. Restarting...', { id: toastId });
          break;
      }
    });

    await relaunch();
  } catch (err) {
    const message = describeUpdateError(err);
    updaterState.state = 'error';
    updaterState.error = message;
    toast.error('Update failed', {
      id: toastId,
      description: message,
      action: { label: 'Retry', onClick: () => void checkForUpdate() },
    });
  }
}

/** Translate raw update errors into actionable user-facing messages */
function describeUpdateError(err: unknown): string {
  const raw = err instanceof Error ? err.message : String(err);
  const lower = raw.toLowerCase();

  if (lower.includes('permission') || lower.includes('privilege') || lower.includes('cancel')) {
    return 'Update requires administrator access. Please try again and enter your password when prompted.';
  }
  if (lower.includes('network') || lower.includes('connect') || lower.includes('timed out') || lower.includes('fetch')) {
    return 'Download interrupted. Check your internet connection and try again.';
  }
  if (lower.includes('signature') || lower.includes('verify')) {
    return 'Update signature verification failed. The download may be corrupted. Please try again.';
  }

  return `Update failed: ${raw}`;
}

/**
 * Check for available updates.
 * Shows a toast when an update is found or when the check fails.
 */
export async function checkForUpdate(): Promise<void> {
  updaterState.state = 'checking';
  updaterState.error = null;
  updaterState.update = null;
  updaterState.updateVersion = null;

  try {
    const update = await check();

    if (update) {
      updaterState.state = 'available';
      updaterState.update = update;
      updaterState.updateVersion = update.version;

      void invoke('report_update_check', { available: true, version: update.version });

      // Persist until acted on, but always dismissible: "Update Now" installs,
      // "Later" (cancel) closes it. Without an explicit dismiss path an
      // Infinity-duration toast can only be cleared by updating, which is the
      // toast-era version of the old banner being impossible to get rid of.
      toast.info(`Update available: v${update.version}`, {
        description: `Version ${update.version} is ready to install`,
        duration: Infinity,
        action: { label: 'Update Now', onClick: () => void downloadAndInstall() },
        cancel: { label: 'Later', onClick: () => {} },
      });
    } else {
      updaterState.state = 'up-to-date';
      void invoke('report_update_check', { available: false, version: null });
    }
  } catch (err) {
    updaterState.state = 'error';
    updaterState.error = err instanceof Error ? err.message : 'Failed to check for updates';
    console.error('Update check failed:', err);

    // The host process is the one thing allowed to emit telemetry (the
    // webview ships nothing of its own) — this is the failure's own message
    // about the update mechanism, not caller-supplied content.
    void invoke('report_update_check', {
      available: false,
      version: null,
      error: updaterState.error,
    });

    toast.error('Failed to check for updates', {
      description: updaterState.error,
      action: { label: 'Download Manually', onClick: openReleasesPage },
    });
  }
}

/** Get the current updater state (reactive, for OverviewPane status display) */
export function getUpdaterState(): UpdaterState {
  return updaterState;
}
