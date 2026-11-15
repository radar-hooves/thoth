<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import { save } from '@tauri-apps/plugin-dialog';
  import * as Dialog from '@poodle64/ui/dialog';
  import { Button } from '@poodle64/ui/button';
  import { Checkbox } from '@poodle64/ui/checkbox';
  import { Input } from '@poodle64/ui/input';
  import { Label } from '@poodle64/ui/label';
  import { Alert, AlertDescription } from '@poodle64/ui/alert';
  import * as RadioGroup from '#lib/components/ui/radio-group/index.js';
  import AlertCircleIcon from '@lucide/svelte/icons/alert-circle';

  interface Props {
    /** Whether the dialog is visible */
    open: boolean;
    /** Selected transcription IDs to export (empty for all) */
    selectedIds?: string[];
    /** Callback when dialog is closed */
    onclose?: () => void;
  }

  type ExportFormat = 'json' | 'csv' | 'txt';

  let { open = $bindable(), selectedIds = [], onclose }: Props = $props();

  let format = $state<ExportFormat>('json');
  let useSearchFilter = $state(false);
  let searchQuery = $state('');
  let fromDate = $state('');
  let toDate = $state('');
  let enhancedOnly = $state(false);
  let isExporting = $state(false);
  let exportError = $state<string | null>(null);
  let exportSuccess = $state<string | null>(null);

  let exportDescription = $derived.by(() => {
    if (selectedIds.length > 0) {
      return `${selectedIds.length} selected transcription${selectedIds.length === 1 ? '' : 's'}`;
    }
    if (useSearchFilter) {
      const filters: string[] = [];
      if (searchQuery) filters.push(`matching "${searchQuery}"`);
      if (fromDate) filters.push(`from ${formatDateForDisplay(fromDate)}`);
      if (toDate) filters.push(`until ${formatDateForDisplay(toDate)}`);
      if (enhancedOnly) filters.push('enhanced only');
      return filters.length > 0 ? `All transcriptions ${filters.join(', ')}` : 'All transcriptions';
    }
    return 'All transcriptions';
  });

  function formatDateForDisplay(dateStr: string): string {
    if (!dateStr) return '';
    const date = new Date(dateStr);
    return date.toLocaleDateString('en-AU', { day: '2-digit', month: '2-digit', year: 'numeric' });
  }

  function dateToTimestamp(dateStr: string, isEndOfDay: boolean = false): number | null {
    if (!dateStr) return null;
    const date = new Date(dateStr);
    if (isEndOfDay) {
      date.setHours(23, 59, 59, 999);
    } else {
      date.setHours(0, 0, 0, 0);
    }
    return Math.floor(date.getTime() / 1000);
  }

  function getFileExtension(fmt: ExportFormat): string {
    switch (fmt) {
      case 'json':
        return 'json';
      case 'csv':
        return 'csv';
      case 'txt':
        return 'txt';
    }
  }

  function getFormatName(fmt: ExportFormat): string {
    switch (fmt) {
      case 'json':
        return 'JSON';
      case 'csv':
        return 'CSV';
      case 'txt':
        return 'Plain Text';
    }
  }

  async function handleExport() {
    exportError = null;
    exportSuccess = null;
    isExporting = true;

    try {
      const extension = getFileExtension(format);
      const filePath = await save({
        defaultPath: `thoth-export.${extension}`,
        filters: [{ name: getFormatName(format), extensions: [extension] }],
      });

      if (!filePath) {
        isExporting = false;
        return;
      }

      const searchParams =
        useSearchFilter && selectedIds.length === 0
          ? {
              query: searchQuery || null,
              from_date: dateToTimestamp(fromDate, false),
              to_date: dateToTimestamp(toDate, true),
              enhanced_only: enhancedOnly || null,
              limit: 10000,
              offset: 0,
            }
          : null;

      let exportedCount: number;
      switch (format) {
        case 'json':
          exportedCount = await invoke<number>('export_to_json', {
            ids: selectedIds,
            path: filePath,
            searchParams,
          });
          break;
        case 'csv':
          exportedCount = await invoke<number>('export_to_csv', {
            ids: selectedIds,
            path: filePath,
            searchParams,
          });
          break;
        case 'txt':
          exportedCount = await invoke<number>('export_to_txt', {
            ids: selectedIds,
            path: filePath,
            searchParams,
          });
          break;
      }

      exportSuccess = `Successfully exported ${exportedCount} transcription${exportedCount === 1 ? '' : 's'}`;

      setTimeout(() => {
        handleClose();
      }, 1500);
    } catch (error) {
      exportError = error instanceof Error ? error.message : String(error);
    } finally {
      isExporting = false;
    }
  }

  function handleClose() {
    open = false;
    exportError = null;
    exportSuccess = null;
    onclose?.();
  }
</script>

<Dialog.Root
  bind:open
  onOpenChange={(v) => {
    if (!v) handleClose();
  }}
>
  <Dialog.Content class="max-w-[480px]" showCloseButton={false}>
    <Dialog.Header>
      <Dialog.Title>Export Transcriptions</Dialog.Title>
    </Dialog.Header>

    <div class="flex flex-col gap-5 py-2">
      <!-- Export summary -->
      <div class="bg-muted rounded-md px-4 py-3 flex gap-2 text-sm">
        <span class="text-muted-foreground">Exporting:</span>
        <span class="font-medium">{exportDescription}</span>
      </div>

      <!-- Format selection -->
      <div class="space-y-2">
        <Label class="text-sm font-medium">Export Format</Label>
        <RadioGroup.Root
          value={format}
          onValueChange={(v) => (format = v as ExportFormat)}
          class="flex flex-col gap-2"
        >
          {#each [{ value: 'json', label: 'JSON', hint: 'Full data, machine-readable' }, { value: 'csv', label: 'CSV', hint: 'Spreadsheet compatible' }, { value: 'txt', label: 'Plain Text', hint: 'Human readable' }] as opt (opt.value)}
            <label
              class="flex cursor-pointer items-start gap-3 rounded-md px-3 py-2 transition-colors hover:bg-muted"
            >
              <RadioGroup.Item value={opt.value} id="format-{opt.value}" class="mt-0.5" />
              <span class="flex flex-col gap-0.5">
                <span class="text-sm font-medium">{opt.label}</span>
                <span class="text-xs text-muted-foreground">{opt.hint}</span>
              </span>
            </label>
          {/each}
        </RadioGroup.Root>
      </div>

      <!-- Filter options (only if no specific selection) -->
      {#if selectedIds.length === 0}
        <div class="border rounded-lg p-4 space-y-3">
          <div class="flex items-center gap-2">
            <Checkbox id="use-filter" bind:checked={useSearchFilter} />
            <Label for="use-filter" class="cursor-pointer">Filter Results</Label>
          </div>

          {#if useSearchFilter}
            <div class="flex flex-col gap-3 pt-1">
              <div class="flex flex-col gap-1.5">
                <Label for="search-query">Search text</Label>
                <Input
                  type="text"
                  id="search-query"
                  placeholder="Search in transcriptions..."
                  bind:value={searchQuery}
                />
              </div>

              <div class="grid grid-cols-2 gap-3">
                <div class="flex flex-col gap-1.5">
                  <Label for="from-date">From date</Label>
                  <Input type="date" id="from-date" bind:value={fromDate} />
                </div>
                <div class="flex flex-col gap-1.5">
                  <Label for="to-date">To date</Label>
                  <Input type="date" id="to-date" bind:value={toDate} />
                </div>
              </div>

              <div class="flex items-center gap-2">
                <Checkbox id="enhanced-only" bind:checked={enhancedOnly} />
                <Label for="enhanced-only" class="cursor-pointer"
                  >Enhanced transcriptions only</Label
                >
              </div>
            </div>
          {/if}
        </div>
      {/if}

      <!-- Status messages -->
      {#if exportError}
        <Alert variant="destructive">
          <AlertCircleIcon />
          <AlertDescription>{exportError}</AlertDescription>
        </Alert>
      {/if}

      {#if exportSuccess}
        <Alert>
          <AlertDescription>{exportSuccess}</AlertDescription>
        </Alert>
      {/if}
    </div>

    <Dialog.Footer>
      <Button variant="secondary" onclick={handleClose} disabled={isExporting}>Cancel</Button>
      <Button onclick={handleExport} disabled={isExporting}>
        {isExporting ? 'Exporting...' : 'Export'}
      </Button>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>
