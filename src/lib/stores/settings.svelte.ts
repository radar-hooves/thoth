/**
 * Settings state management for Thoth
 *
 * Manages application settings including audio device enumeration
 * and provides convenience methods for updating config.
 * Persistence is handled by the configStore (backed by ~/.thoth/config.json).
 */

import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { configStore } from './config.svelte';

/** Audio device information from the Tauri backend */
export interface AudioDevice {
  /** Unique identifier for the device */
  id: string;
  /** Human-readable device name */
  name: string;
  /** Whether this is the system default input device */
  is_default: boolean;
}

/**
 * Create a settings store for managing application settings
 *
 * This store wraps the configStore and adds audio device enumeration
 * and convenience methods for common settings operations.
 */
export function createSettingsStore() {
  let audioDevices = $state<AudioDevice[]>([]);
  let isLoadingDevices = $state<boolean>(false);
  let error = $state<string | null>(null);
  let isInitialised = $state<boolean>(false);
  let deviceChangedUnlisten: UnlistenFn | null = null;
  let promptChangedUnlisten: UnlistenFn | null = null;
  let enhancementToggledUnlisten: UnlistenFn | null = null;
  let trayRebuildUnlisten: UnlistenFn | null = null;

  /**
   * Load available audio input devices from the backend
   */
  async function loadAudioDevices(): Promise<void> {
    isLoadingDevices = true;
    error = null;
    try {
      const devices = await invoke<AudioDevice[]>('list_audio_devices');
      audioDevices = devices;

      // Check if stored selection is currently available
      const currentDeviceId = configStore.audio.deviceId;
      if (currentDeviceId && !devices.some((d) => d.id === currentDeviceId)) {
        // Device not currently enumerated, but keep the preference intact.
        // The Rust backend falls back to the system default gracefully,
        // and the device may reappear (e.g. USB reconnect, sleep/wake).
        console.warn(
          `Configured audio device '${currentDeviceId}' not currently available. ` +
            'Preference preserved; backend will use system default as fallback.'
        );
      }
    } catch (e) {
      const errorMsg = `${e}`;
      console.error('Failed to load audio devices:', errorMsg);
      error = `Failed to load audio devices: ${errorMsg}`;
    } finally {
      isLoadingDevices = false;
    }
  }

  /**
   * Select an audio input device
   *
   * Uses a dedicated backend command to persist the device_id directly,
   * preventing it from being accidentally overwritten by other config saves.
   *
   * @param deviceId - The device ID to select, or null for system default
   */
  async function selectAudioDevice(deviceId: string | null): Promise<void> {
    await invoke('set_audio_device', { deviceId });
    // Update the in-memory config so the UI reflects the change immediately
    configStore.updateAudio('deviceId', deviceId);
  }

  /**
   * Get the currently selected device, or the default device if none selected
   */
  function getSelectedDevice(): AudioDevice | null {
    const deviceId = configStore.audio.deviceId;
    if (deviceId) {
      return audioDevices.find((d) => d.id === deviceId) ?? null;
    }
    return audioDevices.find((d) => d.is_default) ?? null;
  }

  /**
   * Reset audio device selection to system default
   */
  async function resetAudioDevice(): Promise<void> {
    await selectAudioDevice(null);
  }

  /**
   * Set whether to auto-paste transcription to active app
   */
  async function setAutoPaste(enabled: boolean): Promise<void> {
    configStore.updateTranscription('autoPaste', enabled);
    await configStore.save();
  }

  /**
   * Set whether to auto-copy transcription to clipboard
   */
  async function setAutoCopy(enabled: boolean): Promise<void> {
    configStore.updateTranscription('autoCopy', enabled);
    await configStore.save();
  }

  /**
   * Reset all settings to defaults
   */
  async function resetToDefaults(): Promise<void> {
    await configStore.reset();
  }

  /**
   * Initialise the store by loading config, devices, and syncing with backend
   */
  async function initialise(): Promise<void> {
    if (isInitialised) return;

    // App initialises config before this shared store. Recorder windows can mount
    // independently, so load only when this webview has not done that yet.
    if (!configStore.isInitialised) {
      await configStore.load();
    }
    await loadAudioDevices();
    isInitialised = true;

    // Listen for device changes from the tray menu so the UI stays in sync
    deviceChangedUnlisten = await listen<string | null>('audio-device-changed', (event) => {
      configStore.updateAudio('deviceId', event.payload);
    });

    // Listen for prompt changes from the tray menu so the UI stays in sync
    promptChangedUnlisten = await listen<string>('prompt-changed', (event) => {
      configStore.updateEnhancement('promptId', event.payload);
    });

    // Listen for enhancement toggle from the tray menu so the UI stays in sync
    enhancementToggledUnlisten = await listen<boolean>('enhancement-toggled', (event) => {
      configStore.updateEnhancement('enabled', event.payload);
    });

    // Rebuild the tray when a shortcut handler requests it (e.g. enhancement toggle shortcut)
    trayRebuildUnlisten = await listen('tray-rebuild-needed', async () => {
      try {
        await invoke('refresh_tray_menu');
      } catch (e) {
        console.warn('Failed to refresh tray menu:', e);
      }
    });
  }

  /**
   * Clean up event listeners
   */
  function cleanup(): void {
    if (deviceChangedUnlisten) {
      deviceChangedUnlisten();
      deviceChangedUnlisten = null;
    }
    if (promptChangedUnlisten) {
      promptChangedUnlisten();
      promptChangedUnlisten = null;
    }
    if (enhancementToggledUnlisten) {
      enhancementToggledUnlisten();
      enhancementToggledUnlisten = null;
    }
    if (trayRebuildUnlisten) {
      trayRebuildUnlisten();
      trayRebuildUnlisten = null;
    }
  }

  return {
    get audioDevices() {
      return audioDevices;
    },
    get selectedDeviceId() {
      return configStore.audio.deviceId;
    },
    get isLoadingDevices() {
      return isLoadingDevices;
    },
    get error() {
      return error;
    },
    get autoCopy() {
      return configStore.transcription.autoCopy;
    },
    get autoPaste() {
      return configStore.transcription.autoPaste;
    },
    get isInitialised() {
      return isInitialised;
    },
    loadAudioDevices,
    selectAudioDevice,
    getSelectedDevice,
    resetAudioDevice,
    setAutoCopy,
    setAutoPaste,
    resetToDefaults,
    initialise,
    cleanup,
  };
}

/** Singleton settings store instance */
export const settingsStore = createSettingsStore();
