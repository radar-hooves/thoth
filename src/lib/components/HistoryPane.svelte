<script lang="ts">
  /**
   * History pane - 3-panel layout for managing transcription history.
   *
   * Reusable component that can be embedded in the Settings window or
   * used standalone in the History window. Contains the full history UI
   * without window chrome (title bar, drag region).
   *
   * Layout:
   * - Left pane: Scrollable list of transcriptions
   * - Main pane: Selected transcription details
   * - Right pane (collapsible): Search and filter controls
   */

  import { onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import type { TranscriptionRecord } from '../stores/history.svelte';
  import { historyStore } from '../stores/history.svelte';
  import { NO_SPEECH_SENTINEL } from '../stores/pipeline.svelte';
  import { toast } from 'svelte-sonner';
  import { Button } from '@poodle64/ui/button';
  import { Input } from '@poodle64/ui/input';
  import { Badge } from '@poodle64/ui/badge';
  import { Checkbox } from '@poodle64/ui/checkbox';
  import * as AlertDialog from '@poodle64/ui/alert-dialog';
  import { EmptyState } from '#lib/components/common/index.js';
  import HistoryList from './HistoryList.svelte';
  import HistoryFilterPanel, { type FilterState } from './HistoryFilterPanel.svelte';
  import ExportDialog from './ExportDialog.svelte';
  import PerformanceDialog from './PerformanceDialog.svelte';
  import AudioPlayer from './AudioPlayer.svelte';
  import Search from '@lucide/svelte/icons/search';
  import Mic from '@lucide/svelte/icons/mic';
  import Filter from '@lucide/svelte/icons/filter';
  import Download from '@lucide/svelte/icons/download';
  import BarChart2 from '@lucide/svelte/icons/bar-chart-2';
  import Trash2 from '@lucide/svelte/icons/trash-2';
  import Copy from '@lucide/svelte/icons/copy';
  import RotateCcw from '@lucide/svelte/icons/rotate-ccw';
  import Info from '@lucide/svelte/icons/info';
  import X from '@lucide/svelte/icons/x';

  const defaultFilters: FilterState = {
    searchQuery: '',
    fromDate: '',
    toDate: '',
    minDuration: null,
    maxDuration: null,
    showEnhancedOnly: false,
    showUnenhancedOnly: false,
  };

  let deleteConfirm = $state<TranscriptionRecord | null>(null);
  let showExportDialog = $state(false);
  let showPerformanceDialog = $state(false);
  let showFilterPanel = $state(false);
  let showMetadata = $state(false);
  let filters = $state<FilterState>({ ...defaultFilters });
  let bulkSelectedIds = $state(new Set<string>());
  let bulkDeleteConfirm = $state(false);
  let clearAllConfirm = $state(false);
  let bulkExportIds = $state<string[]>([]);
  let retranscribingId = $state<string | null>(null);

  const bulkMode = $derived(bulkSelectedIds.size > 0);

  const filteredRecords = $derived.by(() => {
    let records = historyStore.records;

    if (filters.searchQuery.trim()) {
      const query = filters.searchQuery.toLowerCase();
      records = records.filter(
        (record) =>
          record.text.toLowerCase().includes(query) ||
          historyStore.formatDate(record.timestamp).toLowerCase().includes(query)
      );
    }

    if (filters.fromDate) {
      const fromDate = new Date(filters.fromDate);
      fromDate.setHours(0, 0, 0, 0);
      records = records.filter((record) => record.timestamp >= fromDate);
    }

    if (filters.toDate) {
      const toDate = new Date(filters.toDate);
      toDate.setHours(23, 59, 59, 999);
      records = records.filter((record) => record.timestamp <= toDate);
    }

    if (filters.minDuration !== null) {
      records = records.filter((record) => record.duration >= filters.minDuration!);
    }

    if (filters.maxDuration !== null) {
      records = records.filter((record) => record.duration <= filters.maxDuration!);
    }

    if (filters.showEnhancedOnly) {
      records = records.filter((record) => record.enhanced === true);
    } else if (filters.showUnenhancedOnly) {
      records = records.filter((record) => !record.enhanced);
    }

    return records;
  });

  const hasActiveFilters = $derived(
    filters.searchQuery !== '' ||
      filters.fromDate !== '' ||
      filters.toDate !== '' ||
      filters.minDuration !== null ||
      filters.maxDuration !== null ||
      filters.showEnhancedOnly ||
      filters.showUnenhancedOnly
  );

  const allSelected = $derived(
    filteredRecords.length > 0 && bulkSelectedIds.size === filteredRecords.length
  );

  const someSelected = $derived(bulkSelectedIds.size > 0 && !allSelected);

  onMount(() => {
    historyStore.loadRecords();
  });

  $effect(() => {
    if (historyStore.error) {
      toast.error(historyStore.error);
      historyStore.clearError();
    }
  });

  function handleFilterChange(newFilters: FilterState) {
    filters = { ...newFilters };
  }

  function toggleFilterPanel() {
    showFilterPanel = !showFilterPanel;
  }

  function clearFilters() {
    filters = { ...defaultFilters };
  }

  let searchDebounceTimer: ReturnType<typeof setTimeout> | null = null;
  function handleSearchInput(event: Event) {
    const target = event.target as HTMLInputElement;
    const value = target.value;

    if (searchDebounceTimer) {
      clearTimeout(searchDebounceTimer);
    }

    searchDebounceTimer = setTimeout(() => {
      filters = { ...filters, searchQuery: value };
      searchDebounceTimer = null;
    }, 300);
  }

  function handleSelect(item: TranscriptionRecord) {
    historyStore.selectRecord(item.id);
  }

  async function handleCopy(item: TranscriptionRecord) {
    const success = await historyStore.copyToClipboard(item.text);
    if (success) {
      toast.success('Copied to clipboard');
    }
  }

  async function handleCopySelected() {
    if (historyStore.selectedRecord) {
      await handleCopy(historyStore.selectedRecord);
    }
  }

  function handleDeleteRequest(item: TranscriptionRecord) {
    deleteConfirm = item;
  }

  async function confirmDelete() {
    if (deleteConfirm) {
      await historyStore.deleteRecord(deleteConfirm.id);
      deleteConfirm = null;
    }
  }

  function cancelDelete() {
    deleteConfirm = null;
  }

  function handleDeleteSelected() {
    if (historyStore.selectedRecord) {
      handleDeleteRequest(historyStore.selectedRecord);
    }
  }

  async function handleRetranscribe() {
    const selected = historyStore.selectedRecord;
    if (!selected?.audioPath || retranscribingId) return;

    const id = selected.id;
    retranscribingId = id;

    try {
      const result = await invoke<{
        success: boolean;
        text: string;
        rawText: string;
        isEnhanced: boolean;
        transcriptionModelName: string | null;
        transcriptionDurationSeconds: number | null;
        enhancementModelName: string | null;
        enhancementDurationSeconds: number | null;
        error: string | null;
      }>('pipeline_retranscribe', { transcriptionId: id });

      if (result.success) {
        historyStore.updateRecord(id, {
          text: result.text,
          rawText: result.rawText || undefined,
          enhanced: result.isEnhanced,
          transcriptionModelName: result.transcriptionModelName ?? undefined,
          transcriptionDurationSeconds: result.transcriptionDurationSeconds ?? undefined,
          enhancementModelName: result.enhancementModelName ?? undefined,
          enhancementDurationSeconds: result.enhancementDurationSeconds ?? undefined,
        });
        toast.success('Retranscription complete');
      } else {
        toast.error(result.error ?? 'Retranscription failed');
      }
    } catch (e) {
      const msg = `${e}`;
      if (msg.includes(NO_SPEECH_SENTINEL)) {
        toast.info('No speech detected');
      } else {
        toast.error(msg);
      }
    } finally {
      retranscribingId = null;
    }
  }

  function handleLoadMore() {
    historyStore.loadMore();
  }

  function handleGlobalKeydown(event: KeyboardEvent) {
    if ((event.metaKey || event.ctrlKey) && event.key === 'a') {
      const target = event.target as HTMLElement;
      if (target.tagName !== 'INPUT' && target.tagName !== 'TEXTAREA') {
        event.preventDefault();
        selectAll();
      }
    }
    if (event.key === 'Escape' && bulkSelectedIds.size > 0) {
      deselectAll();
    }
    if ((event.key === 'Backspace' || event.key === 'Delete') && bulkSelectedIds.size > 0) {
      const target = event.target as HTMLElement;
      if (target.tagName !== 'INPUT' && target.tagName !== 'TEXTAREA') {
        event.preventDefault();
        handleBulkDeleteRequest();
      }
    }
  }

  function formatProcessingTime(seconds: number): string {
    return `${seconds.toFixed(1)}s`;
  }

  function hasMetadata(record: TranscriptionRecord): boolean {
    return !!(
      record.transcriptionModelName ||
      record.transcriptionDurationSeconds ||
      record.enhancementModelName ||
      record.enhancementDurationSeconds ||
      record.audioPath ||
      record.enhancementPrompt
    );
  }

  function handleBulkToggle(item: TranscriptionRecord) {
    const next = new Set(bulkSelectedIds);
    if (next.has(item.id)) {
      next.delete(item.id);
    } else {
      next.add(item.id);
    }
    bulkSelectedIds = next;
  }

  function toggleSelectAll() {
    if (allSelected) {
      deselectAll();
    } else {
      selectAll();
    }
  }

  function selectAll() {
    bulkSelectedIds = new Set(filteredRecords.map((r) => r.id));
  }

  function deselectAll() {
    bulkSelectedIds = new Set();
  }

  function handleBulkDeleteRequest() {
    if (bulkSelectedIds.size > 0) {
      bulkDeleteConfirm = true;
    }
  }

  async function confirmBulkDelete() {
    const ids = [...bulkSelectedIds];
    const count = ids.length;
    const success = await historyStore.deleteRecords(ids);
    if (success) {
      bulkSelectedIds = new Set();
      toast.success(`Deleted ${count} transcription${count === 1 ? '' : 's'}`);
    }
    bulkDeleteConfirm = false;
  }

  function cancelBulkDelete() {
    bulkDeleteConfirm = false;
  }

  function handleClearAllRequest() {
    clearAllConfirm = true;
  }

  async function confirmClearAll() {
    const success = await historyStore.deleteAll();
    if (success) {
      bulkSelectedIds = new Set();
      toast.success('All history cleared');
    }
    clearAllConfirm = false;
  }

  function handleBulkExport() {
    bulkExportIds = [...bulkSelectedIds];
    showExportDialog = true;
  }

  const hasActiveModal = $derived(deleteConfirm !== null || bulkDeleteConfirm || clearAllConfirm);
</script>

<svelte:window onkeydown={hasActiveModal ? undefined : handleGlobalKeydown} />

<div class="relative flex h-full w-full flex-col bg-background">
  <!-- Toolbar -->
  <div
    class="flex min-h-[44px] items-center justify-between gap-2 border-b bg-muted/50 px-3 py-1.5"
  >
    {#if bulkMode}
      <!-- Bulk selection toolbar -->
      <div class="flex flex-1 items-center gap-2">
        <Button
          variant="ghost"
          size="icon"
          onclick={toggleSelectAll}
          type="button"
          title={allSelected ? 'Deselect all' : 'Select all'}
          aria-label={allSelected ? 'Deselect all' : 'Select all'}
          class="h-7 w-7"
        >
          <Checkbox
            checked={allSelected}
            indeterminate={someSelected}
            class="pointer-events-none"
          />
        </Button>
        <span class="text-sm font-medium text-primary">{bulkSelectedIds.size} selected</span>
      </div>
      <div class="flex items-center gap-1">
        <Button
          variant="outline"
          size="sm"
          onclick={handleBulkExport}
          type="button"
          class="h-7 gap-1.5 text-xs"
        >
          <Download class="size-3.5" />
          Export
        </Button>
        <Button
          variant="destructive"
          size="sm"
          onclick={handleBulkDeleteRequest}
          type="button"
          class="h-7 gap-1.5 text-xs"
        >
          <Trash2 class="size-3.5" />
          Delete
        </Button>
        <Button
          variant="ghost"
          size="icon"
          onclick={deselectAll}
          type="button"
          title="Cancel selection"
          class="h-7 w-7"
        >
          <X class="size-4" />
        </Button>
      </div>
    {:else}
      <!-- Default toolbar -->
      <div class="flex min-w-0 flex-1 items-center gap-2">
        <Button
          variant="ghost"
          size="icon"
          onclick={toggleSelectAll}
          type="button"
          title="Select all"
          aria-label="Select all"
          disabled={filteredRecords.length === 0}
          class="h-7 w-7 shrink-0"
        >
          <Checkbox class="pointer-events-none" />
        </Button>
        <div class="relative flex min-w-0 flex-1 items-center" style="max-width: 240px;">
          <Search class="absolute left-2.5 size-3.5 text-muted-foreground pointer-events-none" />
          <Input
            type="search"
            placeholder="Search..."
            value={filters.searchQuery}
            oninput={handleSearchInput}
            class="h-7 pl-8 text-xs"
          />
        </div>
        <span class="shrink-0 whitespace-nowrap text-xs text-muted-foreground">
          {#if hasActiveFilters}
            {filteredRecords.length} of {historyStore.records.length}
          {:else}
            {filteredRecords.length}
          {/if}
        </span>
      </div>

      <div class="flex shrink-0 items-center gap-1">
        {#if hasActiveFilters && !showFilterPanel}
          <Button
            variant="ghost"
            size="sm"
            onclick={clearFilters}
            type="button"
            class="h-7 text-xs"
          >
            Clear
          </Button>
        {/if}
        <Button
          variant={showFilterPanel ? 'default' : 'outline'}
          size="icon"
          onclick={toggleFilterPanel}
          aria-expanded={showFilterPanel}
          aria-label="Toggle filter panel"
          type="button"
          class="relative h-7 w-7"
        >
          <Filter class="size-3.5" />
          {#if hasActiveFilters}
            <span
              class="absolute -right-0.5 -top-0.5 h-2 w-2 rounded-full bg-primary border-2 border-background"
            ></span>
          {/if}
        </Button>
        <Button
          variant="outline"
          size="icon"
          onclick={() => (showPerformanceDialog = true)}
          title="Performance analysis"
          type="button"
          class="h-7 w-7"
        >
          <BarChart2 class="size-3.5" />
        </Button>
        <Button
          variant="outline"
          size="icon"
          onclick={() => (showExportDialog = true)}
          title="Export transcriptions"
          type="button"
          class="h-7 w-7"
        >
          <Download class="size-3.5" />
        </Button>
      </div>
    {/if}
  </div>

  <!-- Content area -->
  <div class="flex flex-1 overflow-hidden">
    <!-- List panel -->
    <aside class="flex w-[320px] min-w-[280px] flex-col border-r">
      <div class="min-h-0 flex-1 overflow-hidden">
        <HistoryList
          items={filteredRecords}
          selectedId={historyStore.selectedId}
          {bulkSelectedIds}
          onSelect={handleSelect}
          onBulkToggle={handleBulkToggle}
          onCopy={handleCopy}
          onDelete={handleDeleteRequest}
          onLoadMore={handleLoadMore}
          isLoading={historyStore.pagination.isLoading}
          hasMore={historyStore.pagination.hasMore && !hasActiveFilters}
        >
          {#snippet emptyState()}
            {#if hasActiveFilters}
              <EmptyState
                icon={Search}
                title="No matches"
                description="Try adjusting your search or filters."
                class="h-full"
              >
                {#snippet action()}
                  <Button variant="outline" size="sm" onclick={clearFilters} type="button">
                    Clear filters
                  </Button>
                {/snippet}
              </EmptyState>
            {:else}
              <EmptyState
                icon={Mic}
                title="No transcriptions yet"
                description="Record or import audio to get started."
                class="h-full"
              />
            {/if}
          {/snippet}
        </HistoryList>
      </div>
      {#if historyStore.records.length > 0}
        <div class="shrink-0 border-t bg-muted/30 px-3 py-2">
          <Button
            variant="ghost"
            size="sm"
            onclick={handleClearAllRequest}
            type="button"
            class="w-full h-7 text-xs text-muted-foreground hover:text-destructive hover:bg-destructive/10"
          >
            Clear All History
          </Button>
        </div>
      {/if}
    </aside>

    <!-- Detail panel -->
    <main class="flex min-w-0 flex-1 flex-col overflow-hidden">
      {#if historyStore.selectedRecord}
        {@const selected = historyStore.selectedRecord}
        <div class="flex flex-col gap-1 border-b bg-muted/30 px-4 py-3">
          <div class="flex items-center justify-between gap-2">
            <div class="flex items-center gap-3">
              <span class="text-sm font-medium">{historyStore.formatDate(selected.timestamp)}</span>
              {#if selected.duration > 0}
                <span class="text-sm text-muted-foreground"
                  >{historyStore.formatDuration(selected.duration)}</span
                >
              {/if}
              {#if selected.enhanced}
                <Badge class="h-5 px-2 text-xs">Enhanced</Badge>
              {/if}
            </div>
            <div class="flex shrink-0 items-center gap-1">
              <Button
                variant="default"
                size="icon"
                onclick={handleCopySelected}
                type="button"
                title="Copy to clipboard"
                class="h-7 w-7"
              >
                <Copy class="size-3.5" />
              </Button>
              {#if hasMetadata(selected)}
                <Button
                  variant={showMetadata ? 'default' : 'outline'}
                  size="sm"
                  onclick={() => (showMetadata = !showMetadata)}
                  title="Toggle metadata"
                  type="button"
                  class="h-7 gap-1.5 text-xs"
                >
                  <Info class="size-3.5" />
                  Info
                </Button>
              {/if}
              {#if selected.audioPath}
                <Button
                  variant="outline"
                  size="sm"
                  onclick={handleRetranscribe}
                  disabled={retranscribingId !== null}
                  title="Re-run transcription with current model"
                  type="button"
                  class="h-7 gap-1.5 text-xs"
                >
                  <RotateCcw class="size-3.5" />
                  {retranscribingId ? 'Redo...' : 'Redo'}
                </Button>
              {/if}
              <Button
                variant="outline"
                size="icon"
                onclick={handleDeleteSelected}
                type="button"
                title="Delete transcription"
                class="h-7 w-7 border-destructive/40 text-destructive hover:bg-destructive/10"
              >
                <Trash2 class="size-3.5" />
              </Button>
            </div>
          </div>
          <div class="text-xs text-muted-foreground">
            {selected.timestamp.toLocaleString('en-AU', {
              weekday: 'long',
              year: 'numeric',
              month: 'long',
              day: 'numeric',
              hour: '2-digit',
              minute: '2-digit',
            })}
          </div>
        </div>

        <div class="flex-1 overflow-y-auto p-4">
          {#if selected.enhanced && selected.rawText}
            <div class="flex flex-col gap-4">
              <div class="max-w-[90%] self-start rounded-xl border bg-muted/50 p-3">
                <div class="mb-2 flex items-center justify-between">
                  <span class="text-xs font-semibold uppercase tracking-wide text-muted-foreground"
                    >Original</span
                  >
                  <Button
                    variant="ghost"
                    size="icon"
                    onclick={() => handleCopy({ ...selected, text: selected.rawText! })}
                    type="button"
                    title="Copy original text"
                    class="h-6 w-6 opacity-0 hover:opacity-100 group-hover:opacity-100 [.bubble:hover_&]:opacity-100"
                  >
                    <Copy class="size-3.5" />
                  </Button>
                </div>
                <p class="whitespace-pre-wrap text-sm leading-relaxed text-foreground">
                  {selected.rawText}
                </p>
              </div>
              <div
                class="max-w-[90%] self-end rounded-xl border border-primary/25 bg-primary/10 p-3"
              >
                <div class="mb-2 flex items-center justify-between">
                  <span class="text-xs font-semibold uppercase tracking-wide text-primary"
                    >Enhanced</span
                  >
                  <Button
                    variant="ghost"
                    size="icon"
                    onclick={() => handleCopy(selected)}
                    type="button"
                    title="Copy enhanced text"
                    class="h-6 w-6 opacity-0 hover:opacity-100"
                  >
                    <Copy class="size-3.5" />
                  </Button>
                </div>
                <p class="whitespace-pre-wrap text-sm leading-relaxed text-foreground">
                  {selected.text}
                </p>
              </div>
            </div>
          {:else}
            <p class="whitespace-pre-wrap leading-relaxed text-foreground">{selected.text}</p>
          {/if}
        </div>

        {#if selected.audioPath}
          <div class="border-t px-4 py-2">
            <AudioPlayer audioPath={selected.audioPath} />
          </div>
        {/if}

        {#if showMetadata && hasMetadata(selected)}
          <div class="border-t bg-muted/30 px-4 py-3">
            <div class="flex flex-col gap-1.5">
              {#if selected.transcriptionModelName}
                <div class="flex items-baseline gap-4">
                  <span class="min-w-[100px] shrink-0 text-xs text-muted-foreground">Model</span>
                  <span class="min-w-0 font-mono text-xs text-foreground"
                    >{selected.transcriptionModelName}</span
                  >
                </div>
              {/if}
              {#if selected.transcriptionDurationSeconds}
                <div class="flex items-baseline gap-4">
                  <span class="min-w-[100px] shrink-0 text-xs text-muted-foreground"
                    >Transcription time</span
                  >
                  <span class="text-xs text-foreground"
                    >{formatProcessingTime(selected.transcriptionDurationSeconds)}</span
                  >
                </div>
              {/if}
              {#if selected.duration > 0 && selected.transcriptionDurationSeconds}
                <div class="flex items-baseline gap-4">
                  <span class="min-w-[100px] shrink-0 text-xs text-muted-foreground"
                    >Speed (RTFX)</span
                  >
                  <span class="text-xs text-foreground"
                    >{(selected.duration / selected.transcriptionDurationSeconds).toFixed(1)}x</span
                  >
                </div>
              {/if}
              {#if selected.enhancementModelName}
                <div class="flex items-baseline gap-4">
                  <span class="min-w-[100px] shrink-0 text-xs text-muted-foreground"
                    >Enhancement model</span
                  >
                  <span
                    class="min-w-0 truncate font-mono text-xs text-foreground"
                    title={selected.enhancementModelName}>{selected.enhancementModelName}</span
                  >
                </div>
              {/if}
              {#if selected.enhancementDurationSeconds}
                <div class="flex items-baseline gap-4">
                  <span class="min-w-[100px] shrink-0 text-xs text-muted-foreground"
                    >Enhancement time</span
                  >
                  <span class="text-xs text-foreground"
                    >{formatProcessingTime(selected.enhancementDurationSeconds)}</span
                  >
                </div>
              {/if}
              {#if selected.audioPath}
                <div class="flex items-baseline gap-4">
                  <span class="min-w-[100px] shrink-0 text-xs text-muted-foreground"
                    >Audio file</span
                  >
                  <span
                    class="min-w-0 truncate font-mono text-xs text-foreground"
                    title={selected.audioPath}>{selected.audioPath.split('/').pop()}</span
                  >
                </div>
              {/if}
              {#if selected.enhancementPrompt}
                <div class="flex items-baseline gap-4">
                  <span class="min-w-[100px] shrink-0 text-xs text-muted-foreground">Prompt</span>
                  <span
                    class="min-w-0 truncate text-xs text-foreground"
                    title={selected.enhancementPrompt}
                    >{selected.enhancementPrompt.slice(0, 60)}{selected.enhancementPrompt.length >
                    60
                      ? '...'
                      : ''}</span
                  >
                </div>
              {/if}
            </div>
          </div>
        {/if}
      {:else}
        <div
          class="flex h-full flex-col items-center justify-center gap-3 p-8 text-center text-muted-foreground"
        >
          <svg
            class="size-12"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="1.5"
          >
            <path
              d="M9 12h6m-6 4h6m2 5H7a2 2 0 01-2-2V5a2 2 0 012-2h5.586a1 1 0 01.707.293l5.414 5.414a1 1 0 01.293.707V19a2 2 0 01-2 2z"
            />
          </svg>
          <p class="text-sm">Select a transcription to view details</p>
        </div>
      {/if}
    </main>

    {#if showFilterPanel}
      <HistoryFilterPanel
        bind:filters
        onchange={handleFilterChange}
        onclose={() => (showFilterPanel = false)}
        matchCount={filteredRecords.length}
        totalCount={historyStore.records.length}
      />
    {/if}
  </div>

  <!-- Delete single record confirmation -->
  <AlertDialog.Root
    open={deleteConfirm !== null}
    onOpenChange={(open) => {
      if (!open) cancelDelete();
    }}
  >
    <AlertDialog.Content>
      <AlertDialog.Header>
        <AlertDialog.Title>Delete Transcription</AlertDialog.Title>
        <AlertDialog.Description>
          Are you sure you want to delete this transcription? This action cannot be undone.
        </AlertDialog.Description>
      </AlertDialog.Header>
      {#if deleteConfirm}
        <p class="rounded-md bg-muted px-3 py-2 text-sm italic text-muted-foreground">
          {deleteConfirm.text.slice(0, 100)}{deleteConfirm.text.length > 100 ? '...' : ''}
        </p>
      {/if}
      <AlertDialog.Footer>
        <AlertDialog.Cancel onclick={cancelDelete}>Cancel</AlertDialog.Cancel>
        <AlertDialog.Action onclick={confirmDelete} variant="destructive">
          Delete
        </AlertDialog.Action>
      </AlertDialog.Footer>
    </AlertDialog.Content>
  </AlertDialog.Root>

  <!-- Bulk delete confirmation -->
  <AlertDialog.Root
    open={bulkDeleteConfirm}
    onOpenChange={(open) => {
      if (!open) cancelBulkDelete();
    }}
  >
    <AlertDialog.Content>
      <AlertDialog.Header>
        <AlertDialog.Title>Delete {bulkSelectedIds.size} Transcriptions</AlertDialog.Title>
        <AlertDialog.Description>
          Are you sure you want to delete {bulkSelectedIds.size} transcription{bulkSelectedIds.size ===
          1
            ? ''
            : 's'}? This action cannot be undone.
        </AlertDialog.Description>
      </AlertDialog.Header>
      <AlertDialog.Footer>
        <AlertDialog.Cancel onclick={cancelBulkDelete}>Cancel</AlertDialog.Cancel>
        <AlertDialog.Action onclick={confirmBulkDelete} variant="destructive">
          Delete {bulkSelectedIds.size}
        </AlertDialog.Action>
      </AlertDialog.Footer>
    </AlertDialog.Content>
  </AlertDialog.Root>

  <!-- Clear all confirmation -->
  <AlertDialog.Root
    open={clearAllConfirm}
    onOpenChange={(open) => {
      if (!open) clearAllConfirm = false;
    }}
  >
    <AlertDialog.Content>
      <AlertDialog.Header>
        <AlertDialog.Title>Clear All History</AlertDialog.Title>
        <AlertDialog.Description>
          This will permanently delete all {historyStore.records.length} transcription{historyStore
            .records.length === 1
            ? ''
            : 's'}. This action cannot be undone.
        </AlertDialog.Description>
      </AlertDialog.Header>
      <AlertDialog.Footer>
        <AlertDialog.Cancel onclick={() => (clearAllConfirm = false)}>Cancel</AlertDialog.Cancel>
        <AlertDialog.Action onclick={confirmClearAll} variant="destructive">
          Clear All
        </AlertDialog.Action>
      </AlertDialog.Footer>
    </AlertDialog.Content>
  </AlertDialog.Root>

  <ExportDialog bind:open={showExportDialog} selectedIds={bulkExportIds} />
  <PerformanceDialog bind:open={showPerformanceDialog} />
</div>
