<script lang="ts">
  /**
   * Overview pane - landing page for Settings showing stats and status at a glance.
   *
   * Displays summary cards, model performance, and system status.
   * Data refreshes automatically on each pane visit (component remounts).
   */
  import { onMount, onDestroy } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import { listen, type UnlistenFn } from '@tauri-apps/api/event';
  import { getVersion } from '@tauri-apps/api/app';
  import { enable, disable, isEnabled } from '@tauri-apps/plugin-autostart';
  import { configStore } from '../stores/config.svelte';
  import { settingsStore } from '../stores/settings.svelte';
  import { formatDuration, formatTotalDuration } from '../utils/format';
  import { getUpdaterState, checkForUpdate } from '@poodle64/ui/updater';
  import { Button } from '@poodle64/ui/button';
  import { Switch } from '@poodle64/ui/switch';
  import { Badge } from '@poodle64/ui/badge';
  import * as Card from '@poodle64/ui/card';
  import { Alert, AlertDescription } from '@poodle64/ui/alert';
  import { LoadingState } from '$components/common';

  interface ModelStats {
    name: string;
    count: number;
    avgAudioDuration: number;
    avgProcessingTime: number;
    speedFactor: number;
  }

  interface TranscriptionStats {
    totalCount: number;
    analysableCount: number;
    enhancedCount: number;
    totalAudioDuration: number;
    transcriptionModels: ModelStats[];
    enhancementModels: ModelStats[];
  }

  interface DetectedGpu {
    backend: string;
    name: string;
    vram_mb: number | null;
  }

  interface GpuInfo {
    compiled_backend: string;
    gpu_available: boolean;
    gpu_name: string | null;
    vram_mb: number | null;
    detected_gpus: DetectedGpu[];
  }

  interface Permissions {
    microphone: boolean;
    accessibility: boolean;
  }

  interface Props {
    /** Callback to navigate to another Settings pane */
    onNavigate: (paneId: string) => void;
  }

  let { onNavigate }: Props = $props();

  let stats = $state<TranscriptionStats | null>(null);
  let transcriptionReady = $state(false);
  let modelDownloaded = $state(false);
  let isLoading = $state(true);
  let ollamaStatus = $state<'checking' | 'connected' | 'unavailable' | 'not-configured'>(
    'checking'
  );
  let gpuInfo = $state<GpuInfo | null>(null);

  const updaterState = getUpdaterState();
  let currentVersion = $state('');

  /** Average recording duration */
  let avgRecordingDuration = $derived(
    stats && stats.analysableCount > 0 ? stats.totalAudioDuration / stats.analysableCount : 0
  );

  /** Selected device display name */
  let deviceName = $derived.by(() => {
    const deviceId = settingsStore.selectedDeviceId;
    if (!deviceId) {
      const defaultDevice = settingsStore.audioDevices.find((d) => d.is_default);
      return defaultDevice?.name ?? 'System Default';
    }
    const device = settingsStore.audioDevices.find((d) => d.id === deviceId);
    return device?.name ?? 'Unknown Device';
  });

  /** Autostart (launch at login) state */
  let autostartEnabled = $state(false);
  let autostartLoading = $state(false);
  let autostartError = $state<string | null>(null);

  /** Show in Dock state (macOS) */
  let showInDock = $state(false);
  let dockLoading = $state(false);

  /** Window decorations (Linux): native titlebar vs. custom close button */
  const isLinux = /Linux/.test(navigator.userAgent);
  let windowDecorations = $state(configStore.general.windowDecorations ?? true);

  /** What the OS has granted Thoth, read live; null until the first read. */
  let permissions = $state<Permissions | null>(null);
  let permissionsGranted = $derived(
    permissions !== null && permissions.microphone && permissions.accessibility
  );
  /** "Microphone and Accessibility", or whichever one is still missing. */
  let missingPermissions = $derived(
    [!permissions?.microphone && 'Microphone', !permissions?.accessibility && 'Accessibility']
      .filter(Boolean)
      .join(' and ')
  );

  async function handleAutostartToggle(checked: boolean) {
    autostartLoading = true;
    autostartError = null;

    try {
      if (checked) {
        await enable();
        autostartEnabled = true;
      } else {
        await disable();
        autostartEnabled = false;
      }
    } catch (error) {
      autostartError =
        error instanceof Error ? error.message : 'Failed to update autostart setting';
      console.error('Autostart toggle failed:', error);
    } finally {
      autostartLoading = false;
    }
  }

  async function loadAutostartState() {
    try {
      autostartEnabled = await isEnabled();
    } catch (error) {
      console.error('Failed to check autostart status:', error);
      autostartError = 'Failed to check autostart status';
    }
  }

  async function loadDockState() {
    try {
      showInDock = await invoke<boolean>('get_show_in_dock');
    } catch (error) {
      console.error('Failed to check dock state:', error);
    }
  }

  async function handleDockToggle(checked: boolean) {
    dockLoading = true;
    try {
      await invoke('set_show_in_dock', { show: checked });
      showInDock = checked;
    } catch (error) {
      console.error('Failed to toggle dock visibility:', error);
    } finally {
      dockLoading = false;
    }
  }

  async function handleDecorationToggle(checked: boolean) {
    windowDecorations = checked;
    try {
      await getCurrentWindow().setDecorations(checked);
      configStore.general.windowDecorations = checked;
      await configStore.save();
    } catch (error) {
      console.error('Failed to toggle window decorations:', error);
      windowDecorations = !checked; // revert on failure
    }
  }

  let permissionUnlisten: UnlistenFn | null = null;

  async function checkPermissions() {
    try {
      permissions = await invoke<Permissions>('get_permissions');
    } catch (error) {
      console.error('Failed to check permissions:', error);
    }
  }

  /**
   * Ask for everything missing: the microphone dialog, then Accessibility's.
   * The backend's watcher reports every change as `permission-changed`.
   */
  function grantPermissions() {
    invoke('request_permissions').catch((error) => {
      console.error('Failed to request permissions:', error);
    });
  }

  /** Model download state for setup card */
  type SetupState = 'needed' | 'downloading' | 'initialising' | 'ready' | 'error';
  let setupState = $state<SetupState>('ready');
  let downloadProgress = $state(0);
  let downloadError = $state<string | null>(null);
  let downloadUnlisteners: UnlistenFn[] = [];

  /** Setup step status tracking */
  let modelStepDone = $derived(setupState === 'ready');
  let allRequiredDone = $derived(modelStepDone && permissionsGranted);

  /** Celebration animation trigger */
  let showCelebration = $state(false);
  let hasShownCelebration = $state(false);

  async function downloadRecommendedModel() {
    setupState = 'downloading';
    downloadProgress = 0;
    downloadError = null;

    try {
      // Listen for progress events
      const progressUn = await listen<{ percentage: number }>(
        'model-download-progress',
        (event) => {
          downloadProgress = event.payload.percentage;
        }
      );
      downloadUnlisteners.push(progressUn);

      const completeUn = await listen<string>('model-download-complete', async () => {
        setupState = 'initialising';
        try {
          const modelDir = await invoke<string>('get_model_directory');
          await invoke('init_transcription', { modelPath: modelDir });
          transcriptionReady = true;
          setupState = 'ready';
        } catch (e) {
          downloadError = e instanceof Error ? e.message : String(e);
          setupState = 'error';
        }
        cleanupDownloadListeners();
      });
      downloadUnlisteners.push(completeUn);

      const errorUn = await listen<string>('model-download-error', (event) => {
        downloadError = event.payload;
        setupState = 'error';
        cleanupDownloadListeners();
      });
      downloadUnlisteners.push(errorUn);

      // Start the download (null = recommended model)
      await invoke('download_model');
    } catch (e) {
      downloadError = e instanceof Error ? e.message : String(e);
      setupState = 'error';
      cleanupDownloadListeners();
    }
  }

  function cleanupDownloadListeners() {
    for (const unlisten of downloadUnlisteners) {
      unlisten();
    }
    downloadUnlisteners = [];
  }

  function retryDownload() {
    invoke('reset_download_state').catch(() => {});
    downloadRecommendedModel();
  }

  $effect(() => {
    if (allRequiredDone && !hasShownCelebration && !isLoading) {
      showCelebration = true;
      hasShownCelebration = true;
      setTimeout(() => {
        showCelebration = false;
      }, 3000);
    }
  });

  onDestroy(() => {
    permissionUnlisten?.();
    cleanupDownloadListeners();
  });

  onMount(async () => {
    loadAutostartState();
    loadDockState();
    getVersion()
      .then((v) => {
        currentVersion = v;
      })
      .catch(() => {});

    listen('permission-changed', checkPermissions).then((u) => (permissionUnlisten = u));

    await checkPermissions();

    const [statsResult, readyResult, downloadedResult, gpuResult] = await Promise.allSettled([
      invoke<TranscriptionStats>('get_transcription_stats_cmd'),
      invoke<boolean>('is_transcription_ready'),
      invoke<boolean>('check_model_downloaded', { modelId: null }),
      invoke<GpuInfo>('get_gpu_info'),
    ]);

    if (statsResult.status === 'fulfilled') {
      stats = statsResult.value;
    }

    if (readyResult.status === 'fulfilled') {
      transcriptionReady = readyResult.value;
    }

    if (downloadedResult.status === 'fulfilled') {
      modelDownloaded = downloadedResult.value;
    }

    if (gpuResult.status === 'fulfilled') {
      gpuInfo = gpuResult.value;
    }

    setupState = transcriptionReady ? 'ready' : 'needed';
    isLoading = false;

    // Poll for transcription readiness if the model is downloaded but not yet
    // loaded (the backend warm-up thread may still be initialising). Without
    // this the status sticks on "Loading…" until the next pane visit.
    if (modelDownloaded && !transcriptionReady) {
      const pollInterval = setInterval(async () => {
        try {
          const ready = await invoke<boolean>('is_transcription_ready');
          if (ready) {
            transcriptionReady = true;
            setupState = 'ready';
            clearInterval(pollInterval);
          }
        } catch {
          clearInterval(pollInterval);
        }
      }, 1000);
      // Stop polling after 30 seconds regardless.
      setTimeout(() => clearInterval(pollInterval), 30000);
    }

    // Ollama check runs separately to avoid blocking (30s timeout)
    if (!configStore.enhancement.enabled) {
      ollamaStatus = 'not-configured';
    } else {
      invoke<boolean>('check_ollama_available')
        .then((available) => {
          ollamaStatus = available ? 'connected' : 'unavailable';
        })
        .catch(() => {
          ollamaStatus = 'unavailable';
        });
    }
  });
</script>

{#if isLoading}
  <LoadingState />
{:else if stats && stats.totalCount === 0}
  <!-- First-run setup: stepped checklist -->
  <div
    class="flex flex-col items-center px-6 pt-8 pb-4 text-center"
    class:celebrating={showCelebration}
  >
    <div class="w-12 h-12 text-muted-foreground mb-4 opacity-50">
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5">
        <path
          d="M12 1a3 3 0 0 0-3 3v8a3 3 0 0 0 6 0V4a3 3 0 0 0-3-3z"
          stroke-linecap="round"
          stroke-linejoin="round"
        />
        <path
          d="M19 10v2a7 7 0 0 1-14 0v-2M12 19v4M8 23h8"
          stroke-linecap="round"
          stroke-linejoin="round"
        />
      </svg>
    </div>
    {#if allRequiredDone}
      <p class="m-0 text-lg font-semibold text-foreground">Ready to go</p>
      <p class="mt-1.5 text-sm text-muted-foreground">
        Press your shortcut key to start recording.
      </p>
    {:else}
      <p class="m-0 text-lg font-semibold text-foreground">Set up Thoth</p>
      <p class="mt-1.5 text-sm text-muted-foreground">Two quick steps and you're recording.</p>
    {/if}
  </div>

  <!-- Setup steps -->
  <div class="flex flex-col gap-3">
    <!-- Step 1: Download speech model -->
    <div
      class={[
        'flex gap-3.5 p-4 bg-secondary border rounded-md transition-colors',
        modelStepDone && 'border-green-500/30',
      ]
        .filter(Boolean)
        .join(' ')}
    >
      <div
        class={[
          'w-7 h-7 rounded-full flex items-center justify-center flex-shrink-0 text-sm font-semibold',
          !modelStepDone && 'bg-muted text-muted-foreground',
          modelStepDone && 'bg-green-500/15 text-green-600',
        ]
          .filter(Boolean)
          .join(' ')}
      >
        {#if modelStepDone}
          <svg
            viewBox="0 0 16 16"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            class="w-3.5 h-3.5"
          >
            <path d="M3 8.5l3.5 3.5 6.5-7" stroke-linecap="round" stroke-linejoin="round" />
          </svg>
        {:else}
          1
        {/if}
      </div>
      <div class="flex-1 min-w-0">
        <p class="text-sm font-semibold text-foreground m-0 mb-1">Download speech model</p>
        {#if modelStepDone}
          <p class="text-sm text-muted-foreground m-0">Model ready</p>
        {:else}
          <p class="text-sm text-muted-foreground m-0 mb-3 leading-snug">
            {#if setupState === 'downloading'}
              Downloading... {Math.round(downloadProgress)}%
            {:else if setupState === 'initialising'}
              Preparing transcription engine...
            {:else if setupState === 'error'}
              {downloadError ?? 'Download failed.'}
            {:else}
              A ~1.5 GB model that runs entirely on your machine. Nothing is sent to the cloud.
            {/if}
          </p>
          {#if setupState === 'downloading'}
            <div class="progress-bar">
              <div
                class="progress-fill"
                style="transform: translateX(-{100 - Math.round(downloadProgress)}%)"
              ></div>
            </div>
          {:else if setupState === 'initialising'}
            <div class="progress-bar">
              <div class="progress-fill indeterminate"></div>
            </div>
          {/if}
          {#if setupState === 'needed'}
            <div class="flex gap-2.5 items-center mt-3">
              <Button onclick={downloadRecommendedModel}>Download Recommended Model</Button>
              <Button variant="ghost" onclick={() => onNavigate('models')}>
                Choose a different model
              </Button>
            </div>
          {:else if setupState === 'error'}
            <div class="flex gap-2.5 items-center mt-3">
              <Button onclick={retryDownload}>Retry Download</Button>
            </div>
          {/if}
        {/if}
      </div>
    </div>

    <!-- Step 2: Allow microphone and Accessibility -->
    <div
      class={[
        'flex gap-3.5 p-4 bg-secondary border rounded-md transition-colors',
        permissionsGranted && 'border-green-500/30',
      ]
        .filter(Boolean)
        .join(' ')}
    >
      <div
        class={[
          'w-7 h-7 rounded-full flex items-center justify-center flex-shrink-0 text-sm font-semibold',
          !permissionsGranted && 'bg-muted text-muted-foreground',
          permissionsGranted && 'bg-green-500/15 text-green-600',
        ]
          .filter(Boolean)
          .join(' ')}
      >
        {#if permissionsGranted}
          <svg
            viewBox="0 0 16 16"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            class="w-3.5 h-3.5"
          >
            <path d="M3 8.5l3.5 3.5 6.5-7" stroke-linecap="round" stroke-linejoin="round" />
          </svg>
        {:else}
          2
        {/if}
      </div>
      <div class="flex-1 min-w-0">
        <p class="text-sm font-semibold text-foreground m-0 mb-1">Allow microphone and keyboard</p>
        {#if permissionsGranted}
          <p class="text-sm text-muted-foreground m-0">Permissions granted</p>
        {:else}
          <p class="text-sm text-muted-foreground m-0 mb-3 leading-snug">
            Thoth hears you through the microphone, and Accessibility lets your shortcut key and
            paste work in any app. Allow each when macOS asks.
          </p>
          <div class="flex gap-2.5 items-center">
            <Button onclick={grantPermissions}>Grant</Button>
          </div>
        {/if}
      </div>
    </div>
  </div>

  <!-- Optional settings -->
  <details class="mt-2">
    <summary class="optional-summary">Optional settings</summary>
    <div class="mt-2 p-3.5 bg-secondary border rounded-md flex flex-col gap-0.5">
      <div class="flex items-center gap-2.5 py-1.5">
        <span
          class={[
            'w-2 h-2 rounded-full flex-shrink-0',
            ollamaStatus === 'connected' && 'bg-green-500',
            ollamaStatus === 'not-configured' && 'bg-muted-foreground',
            ollamaStatus === 'unavailable' && 'bg-yellow-500',
            ollamaStatus === 'checking' && 'animate-pulse bg-muted-foreground/50',
          ]
            .filter(Boolean)
            .join(' ')}
        ></span>
        <span class="text-sm text-muted-foreground w-[110px] flex-shrink-0">AI Enhancement</span>
        <span class="text-sm text-foreground font-medium">
          {#if ollamaStatus === 'checking'}
            Checking...
          {:else if ollamaStatus === 'connected'}
            Connected
          {:else if ollamaStatus === 'not-configured'}
            Not configured
          {:else}
            Unavailable
          {/if}
        </span>
      </div>
      <div class="flex items-center justify-between py-1.5">
        <span class="text-sm text-muted-foreground">Launch at Login</span>
        <Switch
          checked={autostartEnabled}
          disabled={autostartLoading}
          onCheckedChange={handleAutostartToggle}
        />
      </div>
      {#if autostartError}
        <Alert variant="destructive" class="mt-1">
          <AlertDescription>{autostartError}</AlertDescription>
        </Alert>
      {/if}
      <div class="flex items-center justify-between py-1.5">
        <span class="text-sm text-muted-foreground">Show in Dock</span>
        <Switch checked={showInDock} disabled={dockLoading} onCheckedChange={handleDockToggle} />
      </div>
      {#if isLinux}
        <div class="flex items-center justify-between py-1.5">
          <span class="text-sm text-muted-foreground">Window Decorations</span>
          <Switch checked={windowDecorations} onCheckedChange={handleDecorationToggle} />
        </div>
      {/if}
    </div>
  </details>
{:else if stats}
  <!-- Summary Cards -->
  <section class="flex flex-col">
    <div class="mb-3">
      <h2 class="text-base font-semibold text-foreground m-0">Summary</h2>
    </div>
    <div class="flex flex-col gap-2">
      <div class="grid grid-cols-2 gap-2.5">
        <Card.Root>
          <Card.Content class="p-4">
            <span
              class="text-[22px] font-semibold text-foreground tabular-nums leading-tight block"
            >
              {stats.totalCount}
            </span>
            <span class="text-xs text-muted-foreground mt-1 block">Transcriptions</span>
          </Card.Content>
        </Card.Root>
        <Card.Root>
          <Card.Content class="p-4">
            <span
              class="text-[22px] font-semibold text-foreground tabular-nums leading-tight block"
            >
              {formatTotalDuration(stats.totalAudioDuration)}
            </span>
            <span class="text-xs text-muted-foreground mt-1 block">Total audio</span>
          </Card.Content>
        </Card.Root>
        <Card.Root>
          <Card.Content class="p-4">
            <span
              class="text-[22px] font-semibold text-foreground tabular-nums leading-tight block"
            >
              {stats.enhancedCount}
            </span>
            <span class="text-xs text-muted-foreground mt-1 block">Enhanced</span>
          </Card.Content>
        </Card.Root>
        <Card.Root>
          <Card.Content class="p-4">
            <span
              class="text-[22px] font-semibold text-foreground tabular-nums leading-tight block"
            >
              {avgRecordingDuration > 0 ? formatDuration(avgRecordingDuration) : '--'}
            </span>
            <span class="text-xs text-muted-foreground mt-1 block">Avg recording</span>
          </Card.Content>
        </Card.Root>
      </div>
    </div>
  </section>

  <!-- Updates -->
  <section class="flex flex-col">
    <div class="mb-3">
      <h2 class="text-base font-semibold text-foreground m-0">Updates</h2>
      <p class="text-xs text-muted-foreground m-0">Application version and update preferences</p>
    </div>
    <div class="flex flex-col gap-2">
      <div class="status-list">
        <div class="status-row">
          <span class="status-label">Current Version</span>
          <span class="status-value">{currentVersion}</span>
        </div>
        <div class="status-row">
          <span class="status-label">Status</span>
          <span class="status-value">
            {#if updaterState.state === 'checking'}
              Checking...
            {:else if updaterState.state === 'available'}
              <Badge variant="default">Update available: {updaterState.updateVersion}</Badge>
            {:else if updaterState.state === 'up-to-date'}
              Up to date
            {:else if updaterState.state === 'error'}
              <Badge variant="destructive">{updaterState.error || 'Check failed'}</Badge>
            {:else}
              Not checked
            {/if}
          </span>
        </div>
        <div class="autostart-row">
          <span class="status-label">Check on Launch</span>
          <Switch
            bind:checked={configStore.general.checkForUpdates}
            onCheckedChange={async () => {
              await configStore.save();
            }}
          />
        </div>
        <div class="autostart-row">
          <span class="status-label">
            {#if updaterState.state === 'checking'}
              Checking...
            {:else}
              Check Now
            {/if}
          </span>
          <Button
            size="sm"
            variant="outline"
            onclick={() => checkForUpdate()}
            disabled={updaterState.state === 'checking'}
          >
            {updaterState.state === 'checking' ? 'Checking...' : 'Check for Updates'}
          </Button>
        </div>
      </div>
    </div>
  </section>

  <!-- System Status -->
  <section class="flex flex-col">
    <div class="mb-3">
      <h2 class="text-base font-semibold text-foreground m-0">System</h2>
      <p class="text-xs text-muted-foreground m-0">Services, permissions, and application preferences</p>
    </div>
    <div class="flex flex-col gap-2">
      <div class="status-list">
        <div class="status-row">
          <span
            class="status-dot"
            class:ready={transcriptionReady}
            class:not-configured={!modelDownloaded}
            class:checking={modelDownloaded && !transcriptionReady}
          ></span>
          <span class="status-label">Transcription</span>
          <span class="status-value">
            {#if transcriptionReady}
              Ready
            {:else if modelDownloaded}
              Loading...
            {:else}
              No model downloaded
            {/if}
          </span>
        </div>
        <div class="status-row">
          <span
            class="status-dot"
            class:ready={gpuInfo?.gpu_available}
            class:not-configured={!gpuInfo}
          ></span>
          <span class="status-label">GPU</span>
          <span class="status-value">
            {#if gpuInfo}
              {#if gpuInfo.gpu_available}
                <span class="gpu-info">
                  <span class="gpu-backend">{gpuInfo.compiled_backend}</span>
                  {#if gpuInfo.gpu_name}
                    <span class="gpu-name" title={gpuInfo.gpu_name}>{gpuInfo.gpu_name}</span>
                  {/if}
                  {#if gpuInfo.vram_mb}
                    <span class="gpu-vram">{gpuInfo.vram_mb} MB</span>
                  {/if}
                </span>
              {:else}
                <span class="gpu-cpu">CPU only</span>
              {/if}
            {:else}
              Checking...
            {/if}
          </span>
        </div>
        <div class="status-row">
          <span
            class="status-dot"
            class:ready={ollamaStatus === 'connected'}
            class:not-configured={ollamaStatus === 'not-configured'}
            class:warning={ollamaStatus === 'unavailable'}
            class:checking={ollamaStatus === 'checking'}
          ></span>
          <span class="status-label">Enhancement</span>
          <span class="status-value">
            {#if ollamaStatus === 'checking'}
              Checking...
            {:else if ollamaStatus === 'connected'}
              Connected
            {:else if ollamaStatus === 'not-configured'}
              Not configured
            {:else}
              Unavailable
            {/if}
          </span>
        </div>
        <div class="status-row">
          <span class="status-dot ready"></span>
          <span class="status-label">Microphone</span>
          <span class="status-value truncate">{deviceName}</span>
        </div>
        <div class="status-row">
          <span
            class="status-dot"
            class:ready={permissionsGranted}
            class:warning={permissions !== null && !permissionsGranted}
          ></span>
          <span class="status-label">Permissions</span>
          <span class="status-value">
            {#if permissions === null}
              Checking...
            {:else if permissionsGranted}
              All granted
            {:else}
              {missingPermissions} needed
              <Button size="sm" class="ml-2" onclick={grantPermissions}>Grant</Button>
            {/if}
          </span>
        </div>
        <div class="autostart-row">
          <span class="status-label">Launch at Login</span>
          <Switch
            checked={autostartEnabled}
            disabled={autostartLoading}
            onCheckedChange={handleAutostartToggle}
          />
        </div>
        {#if autostartError}
          <Alert variant="destructive" class="my-1">
            <AlertDescription>{autostartError}</AlertDescription>
          </Alert>
        {/if}
        <div class="autostart-row">
          <span class="status-label">Show in Dock</span>
          <Switch checked={showInDock} disabled={dockLoading} onCheckedChange={handleDockToggle} />
        </div>
        {#if isLinux}
          <div class="autostart-row">
            <span class="status-label">Window Decorations</span>
            <Switch checked={windowDecorations} onCheckedChange={handleDecorationToggle} />
          </div>
        {/if}
      </div>
    </div>
  </section>
{/if}

<style>
  /* Celebration animation */
  .celebrating :global(.empty-title) {
    animation: celebrateText 0.6s ease-out;
  }

  @keyframes celebrateText {
    0% {
      transform: scale(1);
    }
    50% {
      transform: scale(1.05);
    }
    100% {
      transform: scale(1);
    }
  }

  /* Optional settings summary toggle */
  .optional-summary {
    font-size: 0.75rem;
    font-weight: 600;
    color: var(--muted-foreground);
    text-transform: uppercase;
    letter-spacing: 0.5px;
    cursor: pointer;
    padding: 8px 0;
    list-style: none;
  }

  .optional-summary::before {
    content: '>';
    display: inline-block;
    margin-right: 6px;
    transition: transform 150ms ease;
  }

  details[open] > .optional-summary::before {
    transform: rotate(90deg);
  }

  /* Progress bar. Bespoke because @poodle64/ui's Progress models the determinate
     case only, and the indeterminate sweep below is the state this surface spends
     most of its time in. The determinate fill is driven by transform rather than
     width for the reason the craft gate enforces: animating a layout property
     re-lays-out the subtree every frame. */
  .progress-bar {
    height: 6px;
    background: var(--muted);
    border-radius: 9999px;
    overflow: hidden;
  }

  .progress-fill {
    width: 100%;
    height: 100%;
    background: var(--primary);
    border-radius: 9999px;
    transition: transform 0.3s ease;
  }

  .progress-fill.indeterminate {
    width: 40%;
    transition: none;
    animation: indeterminate 1.2s ease-in-out infinite;
  }

  @keyframes indeterminate {
    0% {
      transform: translateX(-100%);
    }
    100% {
      transform: translateX(350%);
    }
  }

  /* System status list */
  .status-list {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 8px 14px;
    background: var(--card);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
  }

  .status-row {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 6px 0;
  }

  .status-dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    flex-shrink: 0;
    background: var(--muted-foreground);
  }

  .status-dot.ready {
    background: var(--chart-2);
  }

  .status-dot.not-configured {
    background: var(--muted-foreground);
  }

  .status-dot.warning {
    background: var(--chart-3);
  }

  .status-dot.checking {
    background: var(--muted-foreground);
    animation: pulse 1s ease-in-out infinite;
  }

  @keyframes pulse {
    0%,
    100% {
      opacity: 1;
    }
    50% {
      opacity: 0.4;
    }
  }

  .status-label {
    font-size: 0.8125rem;
    color: var(--muted-foreground);
    width: 110px;
    flex-shrink: 0;
  }

  .status-value {
    font-size: 0.8125rem;
    color: var(--foreground);
    font-weight: 500;
    min-width: 0;
  }

  .autostart-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 6px 0;
  }

  /* Truncation */
  .truncate {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  /* GPU info display */
  .gpu-info {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .gpu-backend {
    font-weight: 600;
    color: var(--chart-2);
  }

  .gpu-name {
    font-size: 0.75rem;
    color: var(--muted-foreground);
    max-width: 180px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .gpu-vram {
    font-size: 0.75rem;
    color: var(--muted-foreground);
  }

  .gpu-cpu {
    color: var(--muted-foreground);
  }
</style>
