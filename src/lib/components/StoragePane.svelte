<script lang="ts">
  /**
   * Storage pane - disk usage overview and cleanup tools.
   *
   * Shows storage breakdown by category (models, recordings, database, config,
   * FluidAudio cache) with selective cleanup actions and a full reset.
   */

  import { onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { toast } from 'svelte-sonner';
  import { Button } from '@poodle64/ui/button';
  import { Alert, AlertDescription } from '@poodle64/ui/alert';
  import * as AlertDialog from '@poodle64/ui/alert-dialog';
  import { LoadingState } from '$components/common';
  import { formatBytes } from '$lib/utils/format';

  interface StorageUsage {
    modelsBytes: number;
    recordingsBytes: number;
    databaseBytes: number;
    configBytes: number;
    fluidaudioBytes: number;
    totalBytes: number;
    recordingCount: number;
  }

  let usage = $state<StorageUsage | null>(null);
  let isLoading = $state(true);
  let error = $state<string | null>(null);

  /** Which destructive action is pending confirmation */
  let confirmAction = $state<'recordings' | 'fluidaudio' | 'all' | null>(null);
  let actionInProgress = $state<string | null>(null);

  async function loadUsage() {
    isLoading = true;
    error = null;
    try {
      usage = await invoke<StorageUsage>('get_storage_usage');
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      isLoading = false;
    }
  }

  /** Calculate percentage of total for the bar chart */
  function pct(bytes: number): number {
    if (!usage || usage.totalBytes === 0) return 0;
    return (bytes / usage.totalBytes) * 100;
  }

  function actionFailed(title: string, e: unknown) {
    toast.error(title, { description: e instanceof Error ? e.message : String(e) });
  }

  async function executeDeleteRecordings() {
    confirmAction = null;
    actionInProgress = 'recordings';
    try {
      const deleted = await invoke<number>('delete_all_recordings');
      toast.success(`Deleted ${deleted} recording${deleted === 1 ? '' : 's'}`);
      await loadUsage();
    } catch (e) {
      actionFailed('Failed to delete recordings', e);
    } finally {
      actionInProgress = null;
    }
  }

  async function executeDeleteFluidaudio() {
    confirmAction = null;
    actionInProgress = 'fluidaudio';
    try {
      await invoke('delete_fluidaudio_cache');
      toast.success('Neural Engine cache cleared');
      await loadUsage();
    } catch (e) {
      actionFailed('Failed to clear the Neural Engine cache', e);
    } finally {
      actionInProgress = null;
    }
  }

  async function executeDeleteAll() {
    confirmAction = null;
    actionInProgress = 'all';
    try {
      await invoke('delete_all_data');
      toast.success('All data deleted');
      await loadUsage();
    } catch (e) {
      actionFailed('Failed to delete all data', e);
    } finally {
      actionInProgress = null;
    }
  }

  onMount(() => {
    loadUsage();
  });
</script>

{#if isLoading}
  <LoadingState message="Calculating storage usage..." />
{:else if error}
  <Alert variant="destructive">
    <AlertDescription class="flex items-center justify-between gap-3">
      <span>{error}</span>
      <Button variant="ghost" size="sm" onclick={loadUsage}>Retry</Button>
    </AlertDescription>
  </Alert>
{:else if usage}
  <!-- Storage overview -->
  <section class="flex flex-col gap-4">
    <div>
      <h2 class="text-sm font-semibold">Disk Usage</h2>
      <p class="text-muted-foreground text-xs">
        Thoth is using {formatBytes(usage.totalBytes)} of disk space
      </p>
    </div>

    <!-- Storage bar chart (bespoke: custom proportional segments with per-category colours) -->
    <div
      class="bg-muted flex h-3 w-full gap-px overflow-hidden rounded-full"
      role="img"
      aria-label="Storage breakdown chart"
    >
      {#if usage.modelsBytes + usage.fluidaudioBytes > 0}
        <div
          class="bg-primary min-w-[3px] transition-[width]"
          style:width="{Math.max(pct(usage.modelsBytes + usage.fluidaudioBytes), 1)}%"
          title="Models: {formatBytes(usage.modelsBytes + usage.fluidaudioBytes)}"
        ></div>
      {/if}
      {#if usage.recordingsBytes > 0}
        <div
          class="min-w-[3px] bg-emerald-500 transition-[width]"
          style:width="{Math.max(pct(usage.recordingsBytes), 1)}%"
          title="Recordings: {formatBytes(usage.recordingsBytes)}"
        ></div>
      {/if}
      {#if usage.databaseBytes > 0}
        <div
          class="min-w-[3px] bg-amber-500 transition-[width]"
          style:width="{Math.max(pct(usage.databaseBytes), 1)}%"
          title="Database: {formatBytes(usage.databaseBytes)}"
        ></div>
      {/if}
      {#if usage.configBytes > 0}
        <div
          class="min-w-[3px] bg-slate-400 transition-[width]"
          style:width="{Math.max(pct(usage.configBytes), 1)}%"
          title="Config: {formatBytes(usage.configBytes)}"
        ></div>
      {/if}
    </div>

    <!-- Legend (bespoke: matches bar chart colours) -->
    <div class="flex flex-col gap-1.5">
      <div class="flex items-center gap-2.5 text-sm">
        <span class="bg-primary h-2.5 w-2.5 flex-shrink-0 rounded-sm"></span>
        <span class="text-muted-foreground flex-1">Models</span>
        <span class="tabular-nums font-medium">
          {formatBytes(usage.modelsBytes + usage.fluidaudioBytes)}
        </span>
      </div>
      <div class="flex items-center gap-2.5 text-sm">
        <span class="h-2.5 w-2.5 flex-shrink-0 rounded-sm bg-emerald-500"></span>
        <span class="text-muted-foreground flex-1">Recordings</span>
        <span class="tabular-nums font-medium">
          {formatBytes(usage.recordingsBytes)}
          {#if usage.recordingCount > 0}
            <span class="text-muted-foreground ml-1 text-xs font-normal">
              ({usage.recordingCount} files)
            </span>
          {/if}
        </span>
      </div>
      <div class="flex items-center gap-2.5 text-sm">
        <span class="h-2.5 w-2.5 flex-shrink-0 rounded-sm bg-amber-500"></span>
        <span class="text-muted-foreground flex-1">Database</span>
        <span class="tabular-nums font-medium">{formatBytes(usage.databaseBytes)}</span>
      </div>
      <div class="flex items-center gap-2.5 text-sm">
        <span class="h-2.5 w-2.5 flex-shrink-0 rounded-sm bg-slate-400"></span>
        <span class="text-muted-foreground flex-1">Config</span>
        <span class="tabular-nums font-medium">{formatBytes(usage.configBytes)}</span>
      </div>
    </div>
  </section>

  <!-- Selective cleanup -->
  <section class="mt-6 flex flex-col gap-3">
    <div>
      <h2 class="text-sm font-semibold">Cleanup</h2>
      <p class="text-muted-foreground text-xs">Selectively remove data to free up disk space</p>
    </div>

    <div class="flex flex-col gap-0.5">
      <!-- Recordings -->
      <div class="flex items-center justify-between rounded-md border px-3.5 py-2.5">
        <div class="flex flex-col gap-0.5">
          <span class="text-sm font-medium">Recordings</span>
          <span class="text-muted-foreground text-xs">
            {usage.recordingCount} audio files ({formatBytes(usage.recordingsBytes)})
          </span>
        </div>
        <Button
          variant="destructive"
          size="sm"
          disabled={usage.recordingsBytes === 0 || actionInProgress !== null}
          onclick={() => (confirmAction = 'recordings')}
        >
          {actionInProgress === 'recordings' ? 'Deleting...' : 'Delete'}
        </Button>
      </div>

      <!-- FluidAudio cache -->
      {#if usage.fluidaudioBytes > 0}
        <div class="flex items-center justify-between rounded-md border px-3.5 py-2.5">
          <div class="flex flex-col gap-0.5">
            <span class="text-sm font-medium">Neural Engine cache</span>
            <span class="text-muted-foreground text-xs">
              CoreML compiled models ({formatBytes(usage.fluidaudioBytes)})
            </span>
          </div>
          <Button
            variant="destructive"
            size="sm"
            disabled={actionInProgress !== null}
            onclick={() => (confirmAction = 'fluidaudio')}
          >
            {actionInProgress === 'fluidaudio' ? 'Deleting...' : 'Delete'}
          </Button>
        </div>
      {/if}
    </div>
  </section>

  <!-- Full reset -->
  <section class="mt-6 flex flex-col gap-3">
    <div>
      <h2 class="text-sm font-semibold">Reset</h2>
      <p class="text-muted-foreground text-xs">
        Remove all Thoth data from this machine. This cannot be undone.
      </p>
    </div>
    <div>
      <Button
        variant="destructive"
        disabled={actionInProgress !== null}
        onclick={() => (confirmAction = 'all')}
      >
        {actionInProgress === 'all' ? 'Deleting...' : 'Delete All Thoth Data'}
      </Button>
    </div>
  </section>
{/if}

<!-- Confirmation dialogs for destructive cleanup actions -->
<AlertDialog.Root
  open={confirmAction === 'recordings'}
  onOpenChange={(v) => {
    if (!v) confirmAction = null;
  }}
>
  <AlertDialog.Content>
    <AlertDialog.Header>
      <AlertDialog.Title>Delete all recordings?</AlertDialog.Title>
      <AlertDialog.Description>
        This will permanently delete all audio recording files. This cannot be undone.
      </AlertDialog.Description>
    </AlertDialog.Header>
    <AlertDialog.Footer>
      <AlertDialog.Cancel>Cancel</AlertDialog.Cancel>
      <AlertDialog.Action variant="destructive" onclick={executeDeleteRecordings}>
        Delete
      </AlertDialog.Action>
    </AlertDialog.Footer>
  </AlertDialog.Content>
</AlertDialog.Root>

<AlertDialog.Root
  open={confirmAction === 'fluidaudio'}
  onOpenChange={(v) => {
    if (!v) confirmAction = null;
  }}
>
  <AlertDialog.Content>
    <AlertDialog.Header>
      <AlertDialog.Title>Delete Neural Engine cache?</AlertDialog.Title>
      <AlertDialog.Description>
        The CoreML cache will be deleted. It will recompile automatically on next use, which may
        take a minute.
      </AlertDialog.Description>
    </AlertDialog.Header>
    <AlertDialog.Footer>
      <AlertDialog.Cancel>Cancel</AlertDialog.Cancel>
      <AlertDialog.Action variant="destructive" onclick={executeDeleteFluidaudio}>
        Delete
      </AlertDialog.Action>
    </AlertDialog.Footer>
  </AlertDialog.Content>
</AlertDialog.Root>

<AlertDialog.Root
  open={confirmAction === 'all'}
  onOpenChange={(v) => {
    if (!v) confirmAction = null;
  }}
>
  <AlertDialog.Content>
    <AlertDialog.Header>
      <AlertDialog.Title>Delete all Thoth data?</AlertDialog.Title>
      <AlertDialog.Description>
        This will permanently delete all Thoth data including models, recordings, transcription
        history, settings, and the Neural Engine cache. This cannot be undone.
      </AlertDialog.Description>
    </AlertDialog.Header>
    <AlertDialog.Footer>
      <AlertDialog.Cancel>Cancel</AlertDialog.Cancel>
      <AlertDialog.Action variant="destructive" onclick={executeDeleteAll}>
        Delete Everything
      </AlertDialog.Action>
    </AlertDialog.Footer>
  </AlertDialog.Content>
</AlertDialog.Root>
