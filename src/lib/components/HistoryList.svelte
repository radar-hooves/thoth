<script lang="ts">
  import type { Snippet } from 'svelte';
  import type { TranscriptionRecord } from '../stores/history.svelte';
  import HistoryItem from './HistoryItem.svelte';
  import { Skeleton } from '@poodle64/ui/skeleton';
  import Clock from '@lucide/svelte/icons/clock';
  import { EmptyState } from '#lib/components/common/index.js';

  interface Props {
    items: TranscriptionRecord[];
    selectedId?: string | null;
    bulkSelectedIds?: Set<string>;
    onSelect?: (item: TranscriptionRecord) => void;
    onBulkToggle?: (item: TranscriptionRecord) => void;
    onCopy?: (item: TranscriptionRecord) => void;
    onDelete?: (item: TranscriptionRecord) => void;
    onLoadMore?: () => void;
    isLoading?: boolean;
    hasMore?: boolean;
    emptyState?: Snippet;
  }

  let {
    items,
    selectedId = null,
    bulkSelectedIds = new Set(),
    onSelect,
    onBulkToggle,
    onCopy,
    onDelete,
    onLoadMore,
    isLoading = false,
    hasMore = false,
    emptyState,
  }: Props = $props();

  const bulkMode = $derived(bulkSelectedIds.size > 0);

  let listContainer: HTMLDivElement;
  let visibleItems = $state<TranscriptionRecord[]>([]);
  let startIndex = $state(0);
  let endIndex = $state(0);

  // Virtual scrolling configuration
  const ITEM_HEIGHT = 72;
  const BUFFER_SIZE = 5;
  const SCROLL_THRESHOLD = 200;

  function calculateVisibleItems() {
    if (!listContainer) return;

    const scrollTop = listContainer.scrollTop;
    const containerHeight = listContainer.clientHeight;

    const newStartIndex = Math.max(0, Math.floor(scrollTop / ITEM_HEIGHT) - BUFFER_SIZE);
    const visibleCount = Math.ceil(containerHeight / ITEM_HEIGHT) + BUFFER_SIZE * 2;
    const newEndIndex = Math.min(items.length, newStartIndex + visibleCount);

    startIndex = newStartIndex;
    endIndex = newEndIndex;
    visibleItems = items.slice(startIndex, endIndex);

    const scrollBottom = listContainer.scrollHeight - scrollTop - containerHeight;
    if (scrollBottom < SCROLL_THRESHOLD && hasMore && !isLoading) {
      onLoadMore?.();
    }
  }

  let scrollRAF: number | null = null;
  function handleScroll() {
    if (scrollRAF) return;
    scrollRAF = requestAnimationFrame(() => {
      calculateVisibleItems();
      scrollRAF = null;
    });
  }

  $effect(() => {
    if (items.length > 0) {
      calculateVisibleItems();
    }
  });

  const totalHeight = $derived(items.length * ITEM_HEIGHT);
  const offsetY = $derived(startIndex * ITEM_HEIGHT);

  function handleKeydown(event: KeyboardEvent) {
    if (!items.length) return;

    const currentIndex = selectedId ? items.findIndex((i) => i.id === selectedId) : -1;

    if (event.key === 'ArrowDown') {
      event.preventDefault();
      const nextIndex = Math.min(currentIndex + 1, items.length - 1);
      onSelect?.(items[nextIndex]!);
      scrollToIndex(nextIndex);
    } else if (event.key === 'ArrowUp') {
      event.preventDefault();
      const prevIndex = Math.max(currentIndex - 1, 0);
      onSelect?.(items[prevIndex]!);
      scrollToIndex(prevIndex);
    } else if (event.key === 'Home') {
      event.preventDefault();
      onSelect?.(items[0]!);
      scrollToIndex(0);
    } else if (event.key === 'End') {
      event.preventDefault();
      onSelect?.(items[items.length - 1]!);
      scrollToIndex(items.length - 1);
    }
  }

  function scrollToIndex(index: number) {
    if (!listContainer) return;

    const itemTop = index * ITEM_HEIGHT;
    const itemBottom = itemTop + ITEM_HEIGHT;
    const containerTop = listContainer.scrollTop;
    const containerBottom = containerTop + listContainer.clientHeight;

    if (itemTop < containerTop) {
      listContainer.scrollTop = itemTop;
    } else if (itemBottom > containerBottom) {
      listContainer.scrollTop = itemBottom - listContainer.clientHeight;
    }
  }
</script>

<div
  class="flex h-full flex-col overflow-y-auto bg-muted/20 focus-visible:outline-none"
  bind:this={listContainer}
  onscroll={handleScroll}
  onkeydown={handleKeydown}
  role="listbox"
  aria-label="Transcription history"
  tabindex="0"
>
  {#if items.length === 0}
    {#if emptyState}
      {@render emptyState()}
    {:else}
      <EmptyState
        icon={Clock}
        title="No transcriptions yet"
        description="Start recording to create your first transcription."
        class="h-full"
      />
    {/if}
  {:else}
    <!-- Virtual scroller: keep exact layout for scroll math to work -->
    <div class="relative" style:height="{totalHeight}px">
      <div class="absolute left-0 right-0 top-0" style:transform="translateY({offsetY}px)">
        {#each visibleItems as item (item.id)}
          <HistoryItem
            {item}
            selected={selectedId === item.id}
            bulkSelected={bulkSelectedIds.has(item.id)}
            {bulkMode}
            {onSelect}
            {onBulkToggle}
            {onCopy}
            {onDelete}
          />
        {/each}
      </div>
    </div>

    {#if isLoading}
      <div class="flex flex-col gap-2 p-3">
        <Skeleton class="h-14 w-full" />
        <Skeleton class="h-14 w-full" />
      </div>
    {/if}
  {/if}
</div>
