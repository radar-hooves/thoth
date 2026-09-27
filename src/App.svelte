<script lang="ts">
  /**
   * Main application component - handles initialization and renders the Settings UI.
   *
   * This is the single entry point for the application, responsible for:
   * 1. Loading configuration
   * 2. Initializing database
   * 3. Initializing transcription service
   * 4. Setting up event listeners
   * 5. Rendering the main Settings window
   */
  import { invoke } from '@tauri-apps/api/core';
  import { listen, type UnlistenFn } from '@tauri-apps/api/event';
  import { onMount, onDestroy } from 'svelte';
  import Settings from './lib/windows/Settings.svelte';
  import { configStore } from './lib/stores/config.svelte';
  import { pipelineStore } from './lib/stores/pipeline.svelte';
  import { settingsStore } from './lib/stores/settings.svelte';
  import { shortcutsStore } from './lib/stores/shortcuts.svelte';
  import { soundStore } from './lib/stores/sound.svelte';
  import { checkForUpdate } from './lib/stores/updater.svelte';

  /** Debug logging — only active in development builds */
  const debug = import.meta.env.DEV
    ? (...args: unknown[]) => console.log('[App]', ...args)
    : () => {};

  let indicatorLogUnlisten: UnlistenFn | null = null;

  let isInitialising = $state(true);
  let initError = $state<string | null>(null);

  async function initialise() {
    try {
      debug('Starting initialisation...');

      // Load configuration from backend
      await configStore.load();
      debug('Config loaded');

      // Load sound settings
      soundStore.load();

      // Initialise settings store
      await settingsStore.initialise();
      debug('Settings store initialised');

      // Initialise database
      try {
        await invoke('init_database');
        debug('Database initialised');
      } catch (e) {
        console.warn('[App] Database initialisation failed (may already be initialised):', e);
      }

      // Initialise shortcuts store (loads registered shortcuts)
      await shortcutsStore.initialise();
      debug('Shortcuts store initialised');

      // Initialise pipeline store (sets up event listeners for recording)
      await pipelineStore.initialise();
      debug('Pipeline store initialised');

      isInitialising = false;
      debug('Initialisation complete');

      // Check for updates if enabled (delay slightly to avoid blocking UI)
      if (configStore.general.checkForUpdates) {
        setTimeout(() => {
          debug('Checking for updates...');
          checkForUpdate().catch((err) => {
            console.warn('[App] Update check failed:', err);
          });
        }, 2000);
      }
    } catch (e) {
      console.error('[App] Initialisation failed:', e);
      initError = e instanceof Error ? e.message : String(e);
      isInitialising = false;
    }
  }

  onMount(async () => {
    // Listen for logs from the indicator window (since it has a separate console)
    indicatorLogUnlisten = await listen<{ message: string }>('indicator-log', (event) => {
      debug(event.payload.message);
    });

    initialise();
  });

  onDestroy(() => {
    pipelineStore.cleanup();
    if (indicatorLogUnlisten) {
      indicatorLogUnlisten();
    }
  });
</script>

{#if isInitialising}
  <div class="loading-container">
    <div class="spinner"></div>
    <p class="loading-text">Initialising...</p>
  </div>
{:else if initError}
  <div class="error-container">
    <h2>Initialisation Error</h2>
    <p>{initError}</p>
  </div>
{:else}
  <Settings />
{/if}

<style>
  .loading-container {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    min-height: 100vh;
    background: var(--background);
  }

  .spinner {
    width: 40px;
    height: 40px;
    border: 3px solid var(--muted);
    border-top-color: var(--primary);
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }

  .loading-text {
    margin-top: 16px;
    color: var(--muted-foreground);
  }

  .error-container {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    min-height: 100vh;
    padding: 24px;
    background: var(--background);
    color: var(--destructive);
    text-align: center;
  }

  .error-container h2 {
    margin-bottom: 8px;
  }

  .error-container p {
    color: var(--muted-foreground);
  }
</style>
