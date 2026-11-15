<script lang="ts">
  import type { TranscriptionRecord } from '../stores/history.svelte';
  import { historyStore } from '../stores/history.svelte';
  import { Checkbox } from '@poodle64/ui/checkbox';
  import { Badge } from '@poodle64/ui/badge';
  import * as ContextMenu from '#lib/components/ui/context-menu/index.js';
  import Copy from '@lucide/svelte/icons/copy';
  import Trash2 from '@lucide/svelte/icons/trash-2';

  interface Props {
    item: TranscriptionRecord;
    selected?: boolean;
    bulkSelected?: boolean;
    bulkMode?: boolean;
    onSelect?: (item: TranscriptionRecord) => void;
    onBulkToggle?: (item: TranscriptionRecord) => void;
    onCopy?: (item: TranscriptionRecord) => void;
    onDelete?: (item: TranscriptionRecord) => void;
  }

  let {
    item,
    selected = false,
    bulkSelected = false,
    bulkMode = false,
    onSelect,
    onBulkToggle,
    onCopy,
    onDelete,
  }: Props = $props();

  const previewText = $derived.by(() => {
    const text = item.text.trim();
    const maxLength = 80;
    if (text.length <= maxLength) {
      return text;
    }
    return text.slice(0, maxLength).trim() + '...';
  });

  function handleClick(event: MouseEvent) {
    if (event.metaKey || event.ctrlKey) {
      event.preventDefault();
      onBulkToggle?.(item);
      return;
    }
    onSelect?.(item);
  }

  function handleCheckboxChange() {
    onBulkToggle?.(item);
  }

  function handleKeydown(event: KeyboardEvent) {
    if (event.key === 'Enter' || event.key === ' ') {
      event.preventDefault();
      onSelect?.(item);
    }
  }

  function handleCopy() {
    onCopy?.(item);
  }

  function handleDelete() {
    onDelete?.(item);
  }
</script>

<ContextMenu.Root>
  <ContextMenu.Trigger>
    {#snippet child({ props })}
      <button
        {...props}
        class={[
          'flex h-[72px] w-full flex-row items-start overflow-hidden border-b px-3 py-2.5 text-left transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-inset',
          selected
            ? 'bg-primary text-primary-foreground hover:bg-primary'
            : 'hover:bg-accent/50',
          bulkSelected && !selected && 'bg-primary/10',
        ]
          .filter(Boolean)
          .join(' ')}
        onclick={handleClick}
        onkeydown={handleKeydown}
        aria-selected={selected}
        role="option"
      >
        <!-- Checkbox area -->
        <div
          class="mr-3 flex shrink-0 items-center justify-center transition-opacity"
          class:opacity-0={!bulkMode && !bulkSelected}
          class:opacity-100={bulkMode || bulkSelected}
        >
          <!-- svelte-ignore a11y_click_events_have_key_events -->
          <!-- svelte-ignore a11y_no_static_element_interactions -->
          <div
            onclick={(e) => {
              e.stopPropagation();
              handleCheckboxChange();
            }}
          >
            <Checkbox
              checked={bulkSelected}
              class={selected
                ? 'border-primary-foreground/50 data-[state=checked]:bg-primary-foreground data-[state=checked]:text-primary'
                : ''}
            />
          </div>
        </div>

        <!-- Content -->
        <div class="flex min-w-0 flex-1 flex-col gap-1">
          <span
            class="line-clamp-2 break-words text-sm leading-snug"
            class:text-foreground={!selected}
            class:text-primary-foreground={selected}>{previewText}</span
          >
          <div class="flex flex-wrap items-center gap-2">
            <span
              class={[
                'text-xs',
                selected ? 'text-primary-foreground/70' : 'text-muted-foreground',
              ].join(' ')}>{historyStore.formatDate(item.timestamp)}</span
            >
            {#if item.duration > 0}
              <span
                class={[
                  'text-xs',
                  selected ? 'text-primary-foreground/70' : 'text-muted-foreground',
                ].join(' ')}>{historyStore.formatDuration(item.duration)}</span
              >
            {/if}
            {#if item.enhanced}
              <Badge variant={selected ? 'secondary' : 'default'} class="h-4 px-1.5 text-[10px]"
                >Enhanced</Badge
              >
            {/if}
          </div>
        </div>
      </button>
    {/snippet}
  </ContextMenu.Trigger>

  <ContextMenu.Content>
    <ContextMenu.Item onclick={handleCopy}>
      <Copy class="size-4" />
      Copy
    </ContextMenu.Item>
    <ContextMenu.Item variant="destructive" onclick={handleDelete}>
      <Trash2 class="size-4" />
      Delete
    </ContextMenu.Item>
  </ContextMenu.Content>
</ContextMenu.Root>
