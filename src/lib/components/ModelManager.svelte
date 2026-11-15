<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import { toast } from 'svelte-sonner';
  import { listen, type UnlistenFn } from '@tauri-apps/api/event';
  import { onMount } from 'svelte';
  import { Button } from '@poodle64/ui/button';
  import { Badge } from '@poodle64/ui/badge';
  import { Skeleton } from '@poodle64/ui/skeleton';
  import * as Card from '@poodle64/ui/card';
  import * as AlertDialog from '@poodle64/ui/alert-dialog';
  import { Alert, AlertDescription } from '@poodle64/ui/alert';
  import { formatBytes } from '$lib/utils/format';

  interface ModelInfo {
    id: string;
    name: string;
    description: string;
    version: string;
    size_mb: number;
    downloaded: boolean;
    path: string;
    disk_size: number | null;
    recommended: boolean;
    languages: string[];
    update_available: boolean;
    selected: boolean;
    model_type: string;
    backend_available: boolean;
    /**
     * Accelerator for this build and platform, derived by the backend
     * (`accelerator_label` in manifest.rs) — e.g. "whisper.cpp (Vulkan GPU)".
     * Optional so an older backend without the field degrades to model_type.
     */
    accelerator?: string;
  }

  interface DownloadProgress {
    current_file: string;
    bytes_downloaded: number;
    total_bytes: number | null;
    percentage: number;
    status: string;
  }

  type DownloadState = 'Idle' | 'Downloading' | 'Extracting' | 'Completed' | { Failed: string };

  let models = $state<ModelInfo[]>([]);
  let downloadState = $state<DownloadState>('Idle');
  let progress = $state<DownloadProgress | null>(null);
  let showDeleteConfirm = $state(false);
  let modelToDelete = $state<ModelInfo | null>(null);
  let error = $state<string | null>(null);
  let loading = $state(true);
  let checking = $state(false);
  let lastChecked = $state<string | null>(null);
  let downloadingModelId = $state<string | null>(null);
  /** Model currently being initialised (loading into memory / compiling) */
  let initialisingModelId = $state<string | null>(null);

  let unlistenProgress: UnlistenFn | null = null;
  let unlistenComplete: UnlistenFn | null = null;
  let unlistenError: UnlistenFn | null = null;
  let unlistenModelChanged: UnlistenFn | null = null;
  let unlistenModelReady: UnlistenFn | null = null;
  let unlistenModelFailed: UnlistenFn | null = null;

  onMount(() => {
    loadModels(false);
    loadLastChecked();
    setupEventListeners();

    return () => {
      if (unlistenProgress) unlistenProgress();
      if (unlistenComplete) unlistenComplete();
      if (unlistenError) unlistenError();
      if (unlistenModelChanged) unlistenModelChanged();
      if (unlistenModelReady) unlistenModelReady();
      if (unlistenModelFailed) unlistenModelFailed();
    };
  });

  async function setupEventListeners() {
    unlistenProgress = await listen<DownloadProgress>('model-download-progress', (event) => {
      progress = event.payload;
    });

    unlistenComplete = await listen<string>('model-download-complete', async (event) => {
      downloadState = 'Completed';
      progress = null;
      const completedModelId = downloadingModelId;
      downloadingModelId = null;
      await loadModels(false);

      // Auto-initialise transcription engine with the downloaded model
      if (completedModelId) {
        try {
          const completedModel = models.find((m) => m.id === completedModelId);
          if (completedModel?.model_type === 'fluidaudio_coreml') {
            // FluidAudio already initialised during download; just set selected
            await invoke('set_selected_model_id', { modelId: completedModelId });
          } else {
            const modelDir = await invoke<string>('get_model_directory');
            await invoke('init_transcription', { modelPath: modelDir });
          }
        } catch (e) {
          toast.error('Model downloaded but failed to load', {
            description: e instanceof Error ? e.message : String(e),
          });
        }
      }
    });

    unlistenError = await listen<string>('model-download-error', (event) => {
      downloadState = { Failed: event.payload };
      error = event.payload;
      progress = null;
      downloadingModelId = null;
    });

    // Keep the model cards honest when the active model changes or finishes
    // loading — e.g. when switched from the tray menu, or when a background
    // warmup succeeds or fails. Without this the "Active" badge can drift out of
    // sync with what the backend has actually loaded.
    unlistenModelChanged = await listen<string>('model-changed', () => loadModels(false));
    unlistenModelReady = await listen<string>('model-ready', () => loadModels(false));
    unlistenModelFailed = await listen<string>('model-init-failed', async (event) => {
      await loadModels(false);
      const failed = models.find((m) => m.id === event.payload);
      error = `${failed?.name ?? event.payload} failed to load. Try re-downloading it from this page.`;
    });
  }

  async function loadModels(forceRefresh: boolean) {
    loading = true;
    error = null;
    try {
      models = await invoke<ModelInfo[]>('fetch_model_manifest', { forceRefresh });
      downloadState = await invoke<DownloadState>('get_download_progress');
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      loading = false;
    }
  }

  async function loadLastChecked() {
    try {
      const time = await invoke<string | null>('get_manifest_update_time');
      if (time) {
        lastChecked = formatLastChecked(time);
      }
    } catch {
      // Ignore errors for last checked time
    }
  }

  function formatLastChecked(isoTime: string): string {
    const date = new Date(isoTime);
    const now = new Date();
    const diffMs = now.getTime() - date.getTime();
    const diffHours = Math.floor(diffMs / (1000 * 60 * 60));

    if (diffHours < 1) {
      return 'Just now';
    } else if (diffHours < 24) {
      return `${diffHours} hour${diffHours === 1 ? '' : 's'} ago`;
    } else {
      const diffDays = Math.floor(diffHours / 24);
      return `${diffDays} day${diffDays === 1 ? '' : 's'} ago`;
    }
  }

  async function checkForUpdates() {
    checking = true;
    error = null;
    try {
      await loadModels(true);
      await loadLastChecked();
    } finally {
      checking = false;
    }
  }

  async function downloadModel(model: ModelInfo) {
    error = null;
    try {
      downloadState = 'Downloading';
      downloadingModelId = model.id;
      await invoke('download_model', { modelId: model.id });
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
      downloadState = { Failed: error };
      downloadingModelId = null;
    }
  }

  async function confirmDelete(model: ModelInfo) {
    modelToDelete = model;
    showDeleteConfirm = true;
  }

  async function deleteModel() {
    if (!modelToDelete) return;

    error = null;
    try {
      await invoke('delete_model', { modelId: modelToDelete.id });
      showDeleteConfirm = false;
      modelToDelete = null;
      await loadModels(false);
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    }
  }

  async function resetState() {
    error = null;
    try {
      await invoke('reset_download_state');
      downloadState = 'Idle';
      progress = null;
      downloadingModelId = null;
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    }
  }

  async function selectModel(model: ModelInfo) {
    error = null;
    initialisingModelId = model.id;
    try {
      await invoke('set_selected_model_id', { modelId: model.id });
      // Re-initialise transcription with the newly selected model
      if (model.model_type === 'fluidaudio_coreml') {
        await invoke('init_fluidaudio_transcription');
      } else {
        const modelDir = await invoke<string>('get_model_directory');
        await invoke('init_transcription', { modelPath: modelDir });
      }
      await loadModels(false);
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      initialisingModelId = null;
    }
  }

  function isDownloading(modelId?: string): boolean {
    if (modelId && downloadingModelId !== modelId) return false;
    return downloadState === 'Downloading' || downloadState === 'Extracting';
  }

  function isFailed(): boolean {
    return typeof downloadState === 'object' && 'Failed' in downloadState;
  }

  function getFailedMessage(): string {
    if (typeof downloadState === 'object' && 'Failed' in downloadState) {
      return downloadState.Failed;
    }
    return '';
  }

  function formatLanguages(languages: string[]): string {
    if (languages.length === 0) return 'All languages';
    if (languages.length <= 3) return languages.map((l) => l.toUpperCase()).join(', ');
    return `${languages
      .slice(0, 3)
      .map((l) => l.toUpperCase())
      .join(', ')} +${languages.length - 3} more`;
  }

  /** Sort models: selected first, then recommended, then downloaded, then by name */
  function sortedModels(list: ModelInfo[]): ModelInfo[] {
    return [...list].sort((a, b) => {
      // Selected always first
      if (a.selected && !b.selected) return -1;
      if (!a.selected && b.selected) return 1;
      // Recommended next
      if (a.recommended && !b.recommended) return -1;
      if (!a.recommended && b.recommended) return 1;
      // Downloaded before not-downloaded
      if (a.downloaded && !b.downloaded) return -1;
      if (!a.downloaded && b.downloaded) return 1;
      // Alphabetical within the same tier
      return a.name.localeCompare(b.name);
    });
  }

  /**
   * Friendly backend label for the detail row.
   *
   * The accelerator is computed by the backend (`accelerator_label` in
   * manifest.rs), which is the only place that knows the host platform and the
   * compiled feature set. These strings were previously hardcoded for macOS here
   * and shown verbatim everywhere (#129): a Linux CUDA+Vulkan build claimed
   * "whisper.cpp (Metal GPU)" — impossible on that platform — and "Sherpa-ONNX
   * (CPU)" for a build that links CUDA.
   *
   * Do not reintroduce a switch on model_type. `model_type` remains the fallback
   * only for a backend too old to send the field.
   */
  function backendLabel(model: ModelInfo): string {
    return model.accelerator || model.model_type;
  }
</script>

<div class="flex flex-col gap-6">
  <div class="flex items-center justify-between rounded-lg border p-4">
    <div class="flex flex-col gap-1">
      <span class="text-sm font-medium">Model Updates</span>
      <span class="text-muted-foreground text-xs">
        {#if lastChecked}
          Last checked: {lastChecked}
        {:else}
          Check for new or updated transcription models
        {/if}
      </span>
    </div>
    <Button
      variant="outline"
      size="sm"
      onclick={checkForUpdates}
      disabled={checking || isDownloading()}
    >
      {#if checking}
        Checking...
      {:else}
        Check for Updates
      {/if}
    </Button>
  </div>

  {#if loading}
    <div class="flex flex-col gap-3 p-1">
      <Skeleton class="h-[88px] w-full rounded-lg" />
      <Skeleton class="h-[88px] w-full rounded-lg" />
      <Skeleton class="h-[88px] w-full rounded-lg" />
    </div>
  {:else if error}
    <Alert variant="destructive">
      <AlertDescription class="flex items-center justify-between gap-3">
        <span>{error}</span>
        <Button variant="ghost" size="sm" onclick={() => (error = null)}>Dismiss</Button>
      </AlertDescription>
    </Alert>
  {/if}

  <div class="flex flex-col gap-3">
    {#each sortedModels(models) as model (model.id)}
      <Card.Root
        class={model.selected ? 'border-primary' : !model.backend_available ? 'opacity-55' : ''}
      >
        <Card.Content class="flex flex-col gap-2 p-4">
          <!-- Top row: name + primary badge -->
          <div class="flex items-center justify-between gap-2">
            <div class="flex min-w-0 items-center gap-2">
              <span class="truncate text-sm font-semibold">{model.name}</span>
              {#if model.selected && model.downloaded}
                <Badge>Active</Badge>
              {:else if model.selected}
                <Badge variant="destructive">Not installed</Badge>
              {:else if model.recommended}
                <Badge variant="secondary">Recommended</Badge>
              {/if}
            </div>
            {#if model.update_available}
              <Badge variant="outline">Update</Badge>
            {/if}
          </div>

          <!-- Description -->
          <p class="text-muted-foreground text-xs leading-relaxed">{model.description}</p>

          <!-- Metadata row -->
          <div class="text-muted-foreground flex flex-wrap items-center gap-0 text-xs">
            <span class="whitespace-nowrap">{backendLabel(model)}</span>
            <span class="px-1.5 opacity-40">&middot;</span>
            <span class="whitespace-nowrap">{formatLanguages(model.languages)}</span>
            <span class="px-1.5 opacity-40">&middot;</span>
            {#if model.downloaded && model.disk_size}
              <span class="whitespace-nowrap">{formatBytes(model.disk_size)}</span>
            {:else}
              <span class="whitespace-nowrap">~{model.size_mb} MB</span>
            {/if}
          </div>

          <!-- Backend warning -->
          {#if !model.backend_available}
            <p class="bg-warning/10 text-warning rounded-sm px-2.5 py-1.5 text-xs leading-snug">
              {#if model.model_type === 'fluidaudio_coreml'}
                Requires macOS with Apple Silicon (M1 or later).
              {:else if model.model_type === 'nemo_transducer'}
                Requires the Parakeet backend (not available in this build).
              {:else}
                Backend not available in this build.
              {/if}
            </p>
          {/if}

          <!-- Actions -->
          <div class="flex items-center gap-2 pt-2">
            {#if isDownloading(model.id)}
              <div class="flex flex-1 flex-col gap-2">
                {#if progress}
                  <!-- Custom progress bar: bespoke because it shows download % with animation -->
                  <div class="bg-muted h-1.5 w-full overflow-hidden rounded-full">
                    <div
                      class="bg-primary h-full rounded-full transition-[width] duration-300"
                      style:width="{Math.min(progress.percentage, 100)}%"
                    ></div>
                  </div>
                  <span class="text-muted-foreground text-xs">{progress.status}</span>
                {:else if downloadState === 'Extracting'}
                  <div class="bg-muted h-1.5 w-full overflow-hidden rounded-full">
                    <div class="bg-primary h-full w-full animate-pulse rounded-full"></div>
                  </div>
                  <span class="text-muted-foreground text-xs">Extracting model files...</span>
                {:else}
                  <span class="text-muted-foreground text-xs">Preparing download...</span>
                {/if}
              </div>
            {:else if isFailed() && downloadingModelId === model.id}
              <div class="flex flex-1 items-center gap-3">
                <span class="text-destructive flex-1 text-xs">
                  Download failed: {getFailedMessage()}
                </span>
                <Button variant="secondary" size="sm" onclick={() => resetState()}>Reset</Button>
              </div>
            {:else if initialisingModelId === model.id}
              <div class="flex flex-1 flex-col gap-2">
                <!-- Custom progress bar: bespoke indeterminate pulse for compile/load -->
                <div class="bg-muted h-1.5 w-full overflow-hidden rounded-full">
                  <div class="bg-primary h-full w-full animate-pulse rounded-full"></div>
                </div>
                <span class="text-muted-foreground text-xs">
                  {model.model_type === 'fluidaudio_coreml'
                    ? 'Compiling for Neural Engine… this may take a minute'
                    : 'Loading model…'}
                </span>
              </div>
            {:else if model.downloaded && model.selected}
              <Button
                variant="destructive"
                size="sm"
                onclick={() => confirmDelete(model)}
                disabled={isDownloading() || initialisingModelId !== null}
              >
                Delete
              </Button>
            {:else if model.downloaded}
              <Button
                size="sm"
                onclick={() => selectModel(model)}
                disabled={isDownloading() ||
                  initialisingModelId !== null ||
                  !model.backend_available}
              >
                {model.backend_available ? 'Use Model' : 'Unavailable'}
              </Button>
              <Button
                variant="destructive"
                size="sm"
                onclick={() => confirmDelete(model)}
                disabled={isDownloading() || initialisingModelId !== null}
              >
                Delete
              </Button>
            {:else}
              <Button
                size="sm"
                onclick={() => downloadModel(model)}
                disabled={isDownloading() || !model.backend_available}
              >
                {#if !model.backend_available}
                  Unavailable
                {:else if model.model_type === 'fluidaudio_coreml'}
                  Initialise
                {:else}
                  Download
                {/if}
              </Button>
            {/if}
          </div>
        </Card.Content>
      </Card.Root>
    {/each}
  </div>

  {#if models.length === 0 && !loading}
    <div class="text-muted-foreground p-8 text-center text-sm">
      <p>No models available. Click "Check for Updates" to refresh the list.</p>
    </div>
  {/if}
</div>

<!-- Delete confirmation dialog -->
<AlertDialog.Root bind:open={showDeleteConfirm}>
  <AlertDialog.Content>
    <AlertDialog.Header>
      <AlertDialog.Title>Delete Model?</AlertDialog.Title>
      <AlertDialog.Description>
        Are you sure you want to delete "{modelToDelete?.name}"? You will need to download it again
        to use offline transcription.
      </AlertDialog.Description>
    </AlertDialog.Header>
    <AlertDialog.Footer>
      <AlertDialog.Cancel>Cancel</AlertDialog.Cancel>
      <AlertDialog.Action
        variant="destructive"
        onclick={deleteModel}
      >
        Delete
      </AlertDialog.Action>
    </AlertDialog.Footer>
  </AlertDialog.Content>
</AlertDialog.Root>
