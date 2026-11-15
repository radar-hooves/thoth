<script lang="ts">
  /**
   * Insights pane — rich stats dashboard for the heavy dictator.
   *
   * Sections:
   *  1. Hero stat cards (words, audio time, typing saved, streak)
   *  2. Activity heatmap (GitHub-style, CSS grid)
   *  3. Throughput bar chart (CSS, speedFactor per backend)
   *  4. Model usage (backend counts + enhanced %)
   *  5. Recording length histogram (CSS bars)
   *  6. Time of day (CSS bars, 24 buckets)
   *  7. Cruft finder (multi-select, quarantine to reversible Trash)
   *  8. Storage breakdown (segmented bar)
   *
   * LayerChart (installed as layerchart/d3-scale) was trialled but its
   * SvelteComponentTyped-based Chart slot API is not compatible with Svelte 5
   * strict-typed snippets. CSS flex bars provide equivalent readability
   * for these simple distributions without the type friction.
   */

  import { onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { toast } from 'svelte-sonner';
  import * as Card from '@poodle64/ui/card';
  import * as AlertDialog from '@poodle64/ui/alert-dialog';
  import { Button } from '@poodle64/ui/button';
  import { Checkbox } from '@poodle64/ui/checkbox';
  import { formatBytes, formatTotalDuration } from '../utils/format';
  import { friendlyModelName } from '../utils/model-name';
  import Flame from '@lucide/svelte/icons/flame';
  import Trash2 from '@lucide/svelte/icons/trash-2';
  import RotateCcw from '@lucide/svelte/icons/rotate-ccw';
  import ChevronDown from '@lucide/svelte/icons/chevron-down';
  import ChevronRight from '@lucide/svelte/icons/chevron-right';
  import X from '@lucide/svelte/icons/x';

  // ---------------------------------------------------------------------------
  // Types
  // ---------------------------------------------------------------------------

  type Range = 'allTime' | 'year' | 'month' | 'week';

  interface ActivityDay {
    day: string;
    count: number;
    words: number;
  }

  interface ThroughputRow {
    name: string;
    count: number;
    avgAudioDuration: number;
    avgProcessingTime: number;
    speedFactor: number;
  }

  interface ModelUsage {
    backendCounts: Array<{ name: string; count: number }>;
    enhancementPrompts: Array<{ prompt: string; count: number }>;
    enhancedPct: number;
  }

  interface StorageInfo {
    recordingsBytes: number;
    modelsBytes: number;
    dbBytes: number;
    totalBytes: number;
    oldestRecordingAt: string | null;
  }

  interface InsightsData {
    totals: {
      totalCount: number;
      totalAudioSeconds: number;
      totalWords: number;
      enhancedCount: number;
      typingTimeSavedSeconds: number;
      firstRecordingAt: string | null;
    };
    activity: ActivityDay[];
    currentStreak: number;
    longestStreak: number;
    throughput: ThroughputRow[];
    modelUsage: ModelUsage;
    lengthHistogram: Array<{ bucketLabel: string; count: number }>;
    timeOfDay: number[];
    storage: StorageInfo;
  }

  interface CruftCandidate {
    id: string;
    createdAt: string;
    textPreview: string;
    durationSeconds: number;
    density: number;
    audioPath: string | null;
    fileBytes: number;
    rms: number | null;
  }

  interface TrashEntry {
    id: string;
    textPreview: string;
    createdAt: string;
    deletedAt: string;
    durationSeconds: number | null;
    fileBytes: number;
    audioMoved: boolean;
  }

  // ---------------------------------------------------------------------------
  // State
  // ---------------------------------------------------------------------------

  let range = $state<Range>('allTime');
  let data = $state<InsightsData | null>(null);
  let isLoading = $state(true);
  let loadError = $state<string | null>(null);

  let cruftCandidates = $state<CruftCandidate[]>([]);
  let cruftLoading = $state(false);
  let cruftLoaded = $state(false);
  let cruftSelectedIds = $state(new Set<string>());
  let quarantineConfirm = $state(false);
  /** Trash entry awaiting confirmation of permanent deletion. */
  let purgeConfirmId = $state<string | null>(null);

  let trashEntries = $state<TrashEntry[]>([]);
  let trashOpen = $state(false);
  let trashLoading = $state(false);

  // Track whether we've done the initial load so $effect doesn't double-fire.
  // Plain let (not $state) so reading it inside $effect doesn't create a tracked dep.
  let initialLoadDone = false;

  const RANGES: Array<{ value: Range; label: string }> = [
    { value: 'allTime', label: 'All time' },
    { value: 'year', label: 'This year' },
    { value: 'month', label: 'This month' },
    { value: 'week', label: 'This week' },
  ];

  // ---------------------------------------------------------------------------
  // Derived
  // ---------------------------------------------------------------------------

  const cruftBulkMode = $derived(cruftSelectedIds.size > 0);
  const cruftAllSelected = $derived(
    cruftCandidates.length > 0 && cruftSelectedIds.size === cruftCandidates.length
  );
  const cruftSomeSelected = $derived(cruftSelectedIds.size > 0 && !cruftAllSelected);

  const MONTH_ABBREVS = [
    'Jan',
    'Feb',
    'Mar',
    'Apr',
    'May',
    'Jun',
    'Jul',
    'Aug',
    'Sep',
    'Oct',
    'Nov',
    'Dec',
  ];

  const heatmapData = $derived.by(() => {
    if (!data?.activity?.length)
      return {
        weeks: [] as Array<Array<{ day: string; count: number; words: number } | null>>,
        max: 1,
        monthLabels: [] as Array<{ label: string; weekIndex: number }>,
      };
    const byDay = new Map(data.activity.map((d) => [d.day, d]));

    const today = new Date();
    const start = new Date(today);
    start.setDate(start.getDate() - 364);
    start.setDate(start.getDate() - start.getDay()); // align to Sunday

    const weeks: Array<Array<{ day: string; count: number; words: number } | null>> = [];
    const cursor = new Date(start);
    let weekIndex = 0;
    // Track which months we've already placed a label for
    const seenMonths = new Set<number>();
    const monthLabels: Array<{ label: string; weekIndex: number }> = [];

    while (cursor <= today) {
      const week: Array<{ day: string; count: number; words: number } | null> = [];
      // Check the Sunday (first day of week) for month boundary
      const sundayMonth = cursor.getMonth();
      const sundayYear = cursor.getFullYear();
      const monthKey = sundayYear * 12 + sundayMonth;
      if (!seenMonths.has(monthKey)) {
        seenMonths.add(monthKey);
        // Only label if not the very first partial week (to avoid clipping)
        if (weekIndex > 0) {
          monthLabels.push({ label: MONTH_ABBREVS[sundayMonth], weekIndex });
        }
      }

      for (let d = 0; d < 7; d++) {
        if (cursor > today) {
          week.push(null);
        } else {
          const key = [
            cursor.getFullYear(),
            String(cursor.getMonth() + 1).padStart(2, '0'),
            String(cursor.getDate()).padStart(2, '0'),
          ].join('-');
          week.push(byDay.get(key) ?? { day: key, count: 0, words: 0 });
        }
        cursor.setDate(cursor.getDate() + 1);
      }
      weeks.push(week);
      weekIndex++;
    }

    const max = Math.max(1, ...data.activity.map((d) => d.count));
    return { weeks, max, monthLabels };
  });

  const peakHour = $derived.by(() => {
    if (!data?.timeOfDay?.length) return 0;
    let peak = 0;
    let peakIdx = 0;
    data.timeOfDay.forEach((v, i) => {
      if (v > peak) {
        peak = v;
        peakIdx = i;
      }
    });
    return peakIdx;
  });

  const maxTimeOfDay = $derived(data?.timeOfDay ? Math.max(1, ...data.timeOfDay) : 1);

  const maxThroughput = $derived(
    data?.throughput ? Math.max(1, ...data.throughput.map((r) => r.speedFactor)) : 1
  );

  const maxHistogram = $derived(
    data?.lengthHistogram ? Math.max(1, ...data.lengthHistogram.map((r) => r.count)) : 1
  );

  const maxBackendCount = $derived(
    data?.modelUsage ? Math.max(1, ...data.modelUsage.backendCounts.map((b) => b.count)) : 1
  );

  const storageSegments = $derived.by(() => {
    if (!data?.storage)
      return [] as Array<{ label: string; bytes: number; pct: number; color: string }>;
    const { recordingsBytes, modelsBytes, dbBytes } = data.storage;
    const total = recordingsBytes + modelsBytes + dbBytes || 1;
    return [
      {
        label: 'Recordings',
        bytes: recordingsBytes,
        pct: (recordingsBytes / total) * 100,
        color: 'var(--chart-1)',
      },
      {
        label: 'Models',
        bytes: modelsBytes,
        pct: (modelsBytes / total) * 100,
        color: 'var(--chart-2)',
      },
      { label: 'Database', bytes: dbBytes, pct: (dbBytes / total) * 100, color: 'var(--chart-5)' },
    ];
  });

  // ---------------------------------------------------------------------------
  // Load
  // ---------------------------------------------------------------------------

  async function loadInsights() {
    isLoading = true;
    loadError = null;
    try {
      data = await invoke<InsightsData>('get_insights', { range });
    } catch (e) {
      loadError = e instanceof Error ? e.message : String(e);
    } finally {
      isLoading = false;
    }
  }

  async function loadCruft() {
    cruftLoading = true;
    try {
      cruftCandidates = await invoke<CruftCandidate[]>('get_cruft_candidates');
      cruftLoaded = true;
    } catch (e) {
      toast.error(`Cruft scan failed: ${e instanceof Error ? e.message : String(e)}`);
    } finally {
      cruftLoading = false;
    }
  }

  async function loadTrash() {
    trashLoading = true;
    try {
      trashEntries = await invoke<TrashEntry[]>('list_trash');
    } catch (e) {
      toast.error(`Failed to load trash: ${e instanceof Error ? e.message : String(e)}`);
    } finally {
      trashLoading = false;
    }
  }

  async function confirmQuarantine() {
    const ids = [...cruftSelectedIds];
    try {
      const count = await invoke<number>('quarantine_recordings', { ids });
      toast.success(`Moved ${count} recording${count === 1 ? '' : 's'} to Trash (reversible)`);
      cruftSelectedIds = new Set();
      quarantineConfirm = false;
      await loadCruft();
      if (trashOpen) await loadTrash();
    } catch (e) {
      toast.error(`Failed to quarantine: ${e instanceof Error ? e.message : String(e)}`);
      quarantineConfirm = false;
    }
  }

  async function handleRestore(id: string) {
    try {
      await invoke('restore_recordings', { ids: [id] });
      toast.success('Restored');
      await loadTrash();
    } catch (e) {
      toast.error(`Restore failed: ${e instanceof Error ? e.message : String(e)}`);
    }
  }

  async function handlePurge(id: string) {
    try {
      await invoke('purge_trash', { ids: [id] });
      toast.success('Permanently deleted');
      await loadTrash();
    } catch (e) {
      toast.error(`Purge failed: ${e instanceof Error ? e.message : String(e)}`);
    }
  }

  // Cruft bulk-select helpers (matches HistoryPane pattern: new Set() reassign for reactivity)
  function toggleCruftItem(id: string) {
    const next = new Set(cruftSelectedIds);
    if (next.has(id)) {
      next.delete(id);
    } else {
      next.add(id);
    }
    cruftSelectedIds = next;
  }

  function selectAllCruft() {
    cruftSelectedIds = new Set(cruftCandidates.map((c) => c.id));
  }

  function deselectAllCruft() {
    cruftSelectedIds = new Set();
  }

  function formatHour(h: number): string {
    if (h === 0) return '12am';
    if (h < 12) return `${h}am`;
    if (h === 12) return '12pm';
    return `${h - 12}pm`;
  }

  function formatDate(iso: string): string {
    return new Date(iso).toLocaleDateString('en-AU', {
      day: 'numeric',
      month: 'short',
      year: 'numeric',
    });
  }

  onMount(() => {
    loadInsights().then(() => {
      initialLoadDone = true;
    });
  });

  $effect(() => {
    // Re-fetch when range changes, but not on initial mount (onMount handles that)
    if (!initialLoadDone) return;
    void range; // intentional reactive read — makes the effect re-run when range changes
    loadInsights();
  });
</script>

<!-- Page header with range selector -->
<div class="flex items-center justify-between mb-6">
  <h2 class="text-base font-semibold text-foreground m-0">Insights</h2>
  <div class="flex items-center gap-1 rounded-md border border-border bg-card p-1">
    {#each RANGES as r}
      <button
        class="px-3 py-1 rounded text-xs font-medium transition-colors {range === r.value
          ? 'bg-primary text-primary-foreground'
          : 'text-muted-foreground hover:text-foreground'}"
        onclick={() => (range = r.value)}
      >
        {r.label}
      </button>
    {/each}
  </div>
</div>

{#if isLoading}
  <div class="flex items-center justify-center py-16 text-muted-foreground text-sm">Loading…</div>
{:else if loadError}
  <div
    class="rounded-md border border-destructive/30 bg-destructive/10 px-4 py-3 text-sm text-destructive"
  >
    {loadError}
  </div>
{:else if data}
  <!-- 1. Hero stat cards -->
  <section class="mb-6">
    <div class="grid grid-cols-2 gap-2.5">
      <Card.Root>
        <Card.Content class="p-4">
          <span class="text-[22px] font-semibold text-foreground tabular-nums leading-tight block">
            ≈{data.totals.totalWords.toLocaleString()}
          </span>
          <span class="text-xs text-muted-foreground mt-1 block">
            Words dictated
            <span class="opacity-60">({data.totals.totalCount.toLocaleString()} recordings)</span>
          </span>
        </Card.Content>
      </Card.Root>
      <Card.Root>
        <Card.Content class="p-4">
          <span class="text-[22px] font-semibold text-foreground tabular-nums leading-tight block">
            {formatTotalDuration(data.totals.totalAudioSeconds)}
          </span>
          <span class="text-xs text-muted-foreground mt-1 block">Audio recorded</span>
        </Card.Content>
      </Card.Root>
      <Card.Root>
        <Card.Content class="p-4">
          <span class="text-[22px] font-semibold text-foreground tabular-nums leading-tight block">
            {formatTotalDuration(data.totals.typingTimeSavedSeconds)}
          </span>
          <span class="text-xs text-muted-foreground mt-1 block">
            Typing saved <span class="opacity-60">@ 40 wpm</span>
          </span>
        </Card.Content>
      </Card.Root>
      <Card.Root>
        <Card.Content class="p-4">
          <span
            class="text-[22px] font-semibold text-foreground tabular-nums flex items-center gap-1.5 leading-tight"
          >
            <Flame class="size-5 text-chart-1 shrink-0" />
            {data.currentStreak}d
          </span>
          <span class="text-xs text-muted-foreground mt-1 block">
            Streak <span class="opacity-60">longest {data.longestStreak}d</span>
          </span>
        </Card.Content>
      </Card.Root>
    </div>
  </section>

  <!-- 2. Activity heatmap -->
  <section class="mb-6">
    <h3 class="text-sm font-semibold text-foreground mb-3">Activity</h3>
    <div class="rounded-md border border-border bg-card p-4 overflow-x-auto">
      <!-- Month labels row + weekday gutter side-by-side -->
      <div class="heatmap-outer">
        <!-- Left gutter: weekday labels aligned to Mon/Wed/Fri rows -->
        <div class="heatmap-day-labels" aria-hidden="true">
          <!-- spacer for month-label row -->
          <div class="heatmap-month-spacer"></div>
          <span class="heatmap-day-label">Mon</span>
          <span class="heatmap-day-label heatmap-day-label--hidden"></span>
          <span class="heatmap-day-label">Wed</span>
          <span class="heatmap-day-label heatmap-day-label--hidden"></span>
          <span class="heatmap-day-label">Fri</span>
          <span class="heatmap-day-label heatmap-day-label--hidden"></span>
          <span class="heatmap-day-label heatmap-day-label--hidden"></span>
        </div>

        <!-- Right side: month labels + grid -->
        <div class="heatmap-right">
          <!-- Month label row — absolutely positioned over the grid -->
          <div class="heatmap-month-row" aria-hidden="true">
            {#each heatmapData.monthLabels as ml}
              <span class="heatmap-month-label" style="left: calc({ml.weekIndex} * 12px)"
                >{ml.label}</span
              >
            {/each}
          </div>

          <!-- Cell grid -->
          <div class="heatmap-grid">
            {#each heatmapData.weeks as week}
              <div class="heatmap-col">
                {#each week as cell}
                  {#if cell === null}
                    <div class="heatmap-cell heatmap-cell--empty"></div>
                  {:else}
                    {@const intensity = heatmapData.max > 0 ? cell.count / heatmapData.max : 0}
                    <div
                      class="heatmap-cell"
                      style="--intensity: {intensity}"
                      title="{cell.day}: {cell.count} recording{cell.count === 1
                        ? ''
                        : 's'}{cell.words > 0 ? `, ≈${cell.words.toLocaleString()} words` : ''}"
                    ></div>
                  {/if}
                {/each}
              </div>
            {/each}
          </div>
        </div>
      </div>

      <div class="flex items-center gap-1.5 mt-3 justify-end">
        <span class="text-[10px] text-muted-foreground">Less</span>
        <div class="heatmap-legend-cell" style="--intensity: 0"></div>
        <div class="heatmap-legend-cell" style="--intensity: 0.25"></div>
        <div class="heatmap-legend-cell" style="--intensity: 0.5"></div>
        <div class="heatmap-legend-cell" style="--intensity: 0.75"></div>
        <div class="heatmap-legend-cell" style="--intensity: 1"></div>
        <span class="text-[10px] text-muted-foreground">More</span>
      </div>
    </div>
  </section>

  <!-- 3. Throughput -->
  {#if data.throughput.length > 0}
    <section class="mb-6">
      <h3 class="text-sm font-semibold text-foreground mb-3">Transcription speed</h3>
      <div class="rounded-md border border-border bg-card p-4">
        <p class="text-xs text-muted-foreground mb-4">×realtime — higher is faster</p>
        <div class="flex flex-col gap-3">
          {#each data.throughput as row}
            {@const displayName = friendlyModelName(row.name)}
            <div class="flex items-center gap-3">
              <span
                class="text-xs text-muted-foreground w-36 shrink-0 truncate"
                title={displayName}
              >
                {displayName}
              </span>
              <div class="flex-1 h-5">
                <div
                  class="h-full rounded-sm"
                  style="width: {(row.speedFactor / maxThroughput) *
                    100}%; background: var(--chart-1); min-width: 4px"
                ></div>
              </div>
              <span
                class="text-xs font-semibold text-foreground tabular-nums w-12 text-right shrink-0"
              >
                {row.speedFactor >= 10 ? row.speedFactor.toFixed(0) : row.speedFactor.toFixed(1)}×
              </span>
            </div>
          {/each}
        </div>
      </div>
    </section>
  {/if}

  <!-- 4. Model usage -->
  {#if data.modelUsage.backendCounts.length > 0}
    <section class="mb-6">
      <h3 class="text-sm font-semibold text-foreground mb-3">Model usage</h3>
      <div class="rounded-md border border-border bg-card p-4">
        <div class="flex items-baseline gap-2 mb-4">
          <span class="text-[28px] font-semibold text-foreground tabular-nums leading-none">
            {data.modelUsage.enhancedPct.toFixed(0)}%
          </span>
          <span class="text-xs text-muted-foreground">enhanced with AI</span>
        </div>
        <div class="flex flex-col gap-2.5">
          {#each data.modelUsage.backendCounts as bc}
            {@const displayName = friendlyModelName(bc.name)}
            <div class="flex items-center gap-3">
              <span
                class="text-xs text-muted-foreground w-36 shrink-0 truncate"
                title={displayName}
              >
                {displayName}
              </span>
              <div class="flex-1 h-2 rounded-full bg-muted overflow-hidden">
                <div
                  class="h-full rounded-full"
                  style="width: {(bc.count / maxBackendCount) * 100}%; background: var(--chart-1)"
                ></div>
              </div>
              <span class="text-xs text-foreground tabular-nums w-14 text-right shrink-0">
                {bc.count.toLocaleString()}
              </span>
            </div>
          {/each}
        </div>
      </div>
    </section>
  {/if}

  <!-- 5. Recording length histogram -->
  {#if data.lengthHistogram.length > 0}
    <section class="mb-6">
      <h3 class="text-sm font-semibold text-foreground mb-3">Recording length</h3>
      <div class="rounded-md border border-border bg-card p-4">
        <div class="flex items-end gap-1.5 h-28">
          {#each data.lengthHistogram as bucket}
            <div class="flex flex-col items-center gap-1 flex-1">
              <div
                class="w-full rounded-t-sm"
                style="height: {(bucket.count / maxHistogram) *
                  88}px; background: var(--chart-2); min-height: {bucket.count > 0 ? '3' : '0'}px"
                title="{bucket.bucketLabel}: {bucket.count}"
              ></div>
              <span class="text-[9px] text-muted-foreground text-center leading-tight">
                {bucket.bucketLabel}
              </span>
            </div>
          {/each}
        </div>
      </div>
    </section>
  {/if}

  <!-- 6. Time of day -->
  {#if data.timeOfDay.length === 24}
    <section class="mb-6">
      <h3 class="text-sm font-semibold text-foreground mb-3">Time of day</h3>
      <div class="rounded-md border border-border bg-card p-4">
        <p class="text-xs text-muted-foreground mb-3">Peak: {formatHour(peakHour)}</p>
        <div class="flex items-end gap-px h-20">
          {#each data.timeOfDay as count, i}
            <div
              class="flex-1 rounded-t-sm transition-colors"
              style="height: {(count / maxTimeOfDay) * 64}px; background: {i === peakHour
                ? 'var(--chart-1)'
                : 'var(--chart-5)'}; min-height: {count > 0 ? '2' : '0'}px"
              title="{formatHour(i)}: {count}"
            ></div>
          {/each}
        </div>
        <!-- Labels share the bars' 24-cell flex layout so each tick sits
             directly under its hour bar (a 4-up justify-between row drifts
             out of alignment with the 24 bars). -->
        <div class="flex gap-px mt-1">
          {#each data.timeOfDay as _, i}
            <span class="flex-1 text-center text-[10px] text-muted-foreground">
              {i % 6 === 0 ? formatHour(i) : ''}
            </span>
          {/each}
        </div>
      </div>
    </section>
  {/if}

  <!-- 7. Cruft finder -->
  <section class="mb-6">
    <h3 class="text-sm font-semibold text-foreground mb-3">Cruft finder</h3>
    <div class="rounded-md border border-border bg-card p-4">
      <p class="text-xs text-muted-foreground mb-4 leading-snug">
        Flags recordings by <strong class="text-foreground">density</strong> (characters per second)
        — short, low-content dictations. Moving to Trash is
        <strong class="text-foreground">reversible</strong>; restore or permanently delete from the
        Trash section below.
      </p>

      {#if !cruftLoaded}
        <Button
          variant="outline"
          size="sm"
          onclick={loadCruft}
          disabled={cruftLoading}
          class="mb-2"
        >
          {cruftLoading ? 'Scanning…' : 'Scan for cruft'}
        </Button>
      {:else}
        <!-- Cruft toolbar -->
        {#if cruftBulkMode}
          <div class="flex items-center gap-2 mb-3 px-1">
            <Button
              variant="ghost"
              size="icon"
              onclick={cruftAllSelected ? deselectAllCruft : selectAllCruft}
              class="h-7 w-7"
              title={cruftAllSelected ? 'Deselect all' : 'Select all'}
            >
              <Checkbox
                checked={cruftAllSelected}
                indeterminate={cruftSomeSelected}
                class="pointer-events-none"
              />
            </Button>
            <span class="text-sm font-medium text-primary">{cruftSelectedIds.size} selected</span>
            <div class="flex-1"></div>
            <Button
              variant="destructive"
              size="sm"
              onclick={() => (quarantineConfirm = true)}
              class="h-7 gap-1.5 text-xs"
            >
              <Trash2 class="size-3.5" />
              Move {cruftSelectedIds.size} to Trash
            </Button>
            <Button
              variant="ghost"
              size="icon"
              onclick={deselectAllCruft}
              class="h-7 w-7"
              title="Cancel selection"
            >
              <X class="size-4" />
            </Button>
          </div>
        {:else}
          <div class="flex items-center gap-2 mb-3 px-1">
            <span class="text-xs text-muted-foreground">
              {cruftCandidates.length === 0
                ? 'No cruft found'
                : `${cruftCandidates.length} candidate${cruftCandidates.length === 1 ? '' : 's'} found`}
            </span>
            <div class="flex-1"></div>
            <Button
              variant="ghost"
              size="sm"
              onclick={loadCruft}
              disabled={cruftLoading}
              class="h-7 text-xs"
            >
              Rescan
            </Button>
          </div>
        {/if}

        {#if cruftCandidates.length > 0}
          <div class="flex flex-col gap-1">
            {#each cruftCandidates as candidate}
              {@const selected = cruftSelectedIds.has(candidate.id)}
              <!-- svelte-ignore a11y_click_events_have_key_events -->
              <!-- svelte-ignore a11y_no_static_element_interactions -->
              <div
                class="flex items-start gap-3 rounded-md border px-3 py-2.5 cursor-pointer transition-colors {selected
                  ? 'border-primary/50 bg-primary/5'
                  : 'border-border hover:bg-muted/40'}"
                onclick={() => toggleCruftItem(candidate.id)}
              >
                <Checkbox checked={selected} class="mt-0.5 pointer-events-none shrink-0" />
                <div class="flex-1 min-w-0">
                  {#if candidate.textPreview.trim()}
                    <p class="text-sm text-foreground truncate mb-1">{candidate.textPreview}</p>
                  {:else}
                    <p class="text-sm text-muted-foreground italic truncate mb-1">(no speech)</p>
                  {/if}
                  <div class="flex items-center gap-3 flex-wrap">
                    <span class="text-[11px] text-muted-foreground">
                      {formatDate(candidate.createdAt)}
                    </span>
                    <span class="text-[11px] font-mono text-chart-3">
                      {candidate.density.toFixed(1)} c/s
                    </span>
                    {#if candidate.rms !== null}
                      <span class="text-[11px] text-muted-foreground">
                        rms {(candidate.rms * 100).toFixed(0)}%
                      </span>
                    {/if}
                    <span class="text-[11px] text-muted-foreground">
                      {formatTotalDuration(candidate.durationSeconds)}
                    </span>
                    {#if candidate.fileBytes > 0}
                      <span class="text-[11px] text-muted-foreground">
                        {formatBytes(candidate.fileBytes)}
                      </span>
                    {/if}
                  </div>
                </div>
              </div>
            {/each}
          </div>
        {/if}
      {/if}

      <!-- Trash disclosure -->
      <div class="mt-4 border-t border-border pt-3">
        <button
          class="flex items-center gap-1.5 text-xs text-muted-foreground hover:text-foreground transition-colors"
          onclick={() => {
            trashOpen = !trashOpen;
            if (trashOpen && trashEntries.length === 0) loadTrash();
          }}
        >
          {#if trashOpen}
            <ChevronDown class="size-3.5" />
          {:else}
            <ChevronRight class="size-3.5" />
          {/if}
          Trash (reversible)
        </button>
        {#if trashOpen}
          <div class="mt-3">
            {#if trashLoading}
              <p class="text-xs text-muted-foreground">Loading…</p>
            {:else if trashEntries.length === 0}
              <p class="text-xs text-muted-foreground">Trash is empty.</p>
            {:else}
              <div class="flex flex-col gap-1">
                {#each trashEntries as entry}
                  <div class="flex items-center gap-3 rounded-md border border-border px-3 py-2">
                    <div class="flex-1 min-w-0">
                      <p class="text-xs text-foreground truncate">{entry.textPreview}</p>
                      <p class="text-[10px] text-muted-foreground mt-0.5">
                        {formatDate(entry.createdAt)} · deleted {formatDate(entry.deletedAt)} · {formatBytes(
                          entry.fileBytes
                        )}
                      </p>
                    </div>
                    <div class="flex items-center gap-1 shrink-0">
                      <Button
                        variant="ghost"
                        size="sm"
                        onclick={() => handleRestore(entry.id)}
                        class="h-6 text-[11px] gap-1"
                        title="Restore"
                      >
                        <RotateCcw class="size-3" />
                        Restore
                      </Button>
                      <Button
                        variant="ghost"
                        size="sm"
                        onclick={() => (purgeConfirmId = entry.id)}
                        class="h-6 text-[11px] text-destructive hover:text-destructive hover:bg-destructive/10"
                        title="Delete permanently"
                      >
                        <Trash2 class="size-3" />
                      </Button>
                    </div>
                  </div>
                {/each}
              </div>
            {/if}
          </div>
        {/if}
      </div>
    </div>
  </section>

  <!-- 8. Storage -->
  <section class="mb-6">
    <h3 class="text-sm font-semibold text-foreground mb-3">Storage</h3>
    <div class="rounded-md border border-border bg-card p-4">
      <div class="flex items-baseline gap-2 mb-3">
        <span class="text-[22px] font-semibold text-foreground tabular-nums">
          {formatBytes(data.storage.totalBytes)}
        </span>
        <span class="text-xs text-muted-foreground">total</span>
      </div>
      <!-- Segmented bar -->
      <div class="flex h-3 rounded-full overflow-hidden gap-0.5 mb-3">
        {#each storageSegments as seg}
          {#if seg.pct > 0.5}
            <div
              class="h-full rounded-full"
              style="width: {seg.pct}%; background: {seg.color}; min-width: 4px"
              title="{seg.label}: {formatBytes(seg.bytes)}"
            ></div>
          {/if}
        {/each}
      </div>
      <div class="flex flex-col gap-1.5">
        {#each storageSegments as seg}
          <div class="flex items-center gap-2">
            <span class="w-2 h-2 rounded-full shrink-0" style="background: {seg.color}"></span>
            <span class="text-xs text-muted-foreground w-20 shrink-0">{seg.label}</span>
            <span class="text-xs text-foreground tabular-nums">{formatBytes(seg.bytes)}</span>
          </div>
        {/each}
      </div>
      {#if data.storage.oldestRecordingAt}
        <p class="text-[11px] text-muted-foreground mt-3">
          Oldest recording: {formatDate(data.storage.oldestRecordingAt)}
        </p>
      {/if}
    </div>
  </section>
{/if}

<!-- Quarantine confirm dialog -->
<AlertDialog.Root
  open={quarantineConfirm}
  onOpenChange={(open) => {
    if (!open) quarantineConfirm = false;
  }}
>
  <AlertDialog.Content>
    <AlertDialog.Header>
      <AlertDialog.Title>
        Move {cruftSelectedIds.size} recording{cruftSelectedIds.size === 1 ? '' : 's'} to Trash?
      </AlertDialog.Title>
      <AlertDialog.Description>
        Flagged by density, not length. This is <strong>reversible</strong> — restore from the Trash section
        at any time. Audio files are moved, not deleted.
      </AlertDialog.Description>
    </AlertDialog.Header>
    <AlertDialog.Footer>
      <AlertDialog.Cancel>Cancel</AlertDialog.Cancel>
      <AlertDialog.Action onclick={confirmQuarantine} variant="destructive">
        Move to Trash
      </AlertDialog.Action>
    </AlertDialog.Footer>
  </AlertDialog.Content>
</AlertDialog.Root>

<!-- Permanent delete confirm dialog -->
<AlertDialog.Root
  open={purgeConfirmId !== null}
  onOpenChange={(open) => {
    if (!open) purgeConfirmId = null;
  }}
>
  <AlertDialog.Content>
    <AlertDialog.Header>
      <AlertDialog.Title>Delete this recording permanently?</AlertDialog.Title>
      <AlertDialog.Description>
        Its audio and transcription are removed for good; this cannot be undone.
      </AlertDialog.Description>
    </AlertDialog.Header>
    <AlertDialog.Footer>
      <AlertDialog.Cancel>Cancel</AlertDialog.Cancel>
      <AlertDialog.Action
        variant="destructive"
        onclick={() => {
          const id = purgeConfirmId;
          purgeConfirmId = null;
          if (id) handlePurge(id);
        }}
      >
        Delete permanently
      </AlertDialog.Action>
    </AlertDialog.Footer>
  </AlertDialog.Content>
</AlertDialog.Root>

<style>
  /* GitHub-style contribution heatmap */

  /* Outer wrapper: day-label gutter beside the grid+month-labels block */
  .heatmap-outer {
    display: flex;
    align-items: flex-start;
    gap: 4px;
    min-width: max-content;
  }

  /* Left column: weekday labels */
  .heatmap-day-labels {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding-top: 0; /* aligns with month-row + grid */
  }

  /* Spacer that matches the month-label row height */
  .heatmap-month-spacer {
    height: 14px;
    display: block;
  }

  .heatmap-day-label {
    height: 10px;
    line-height: 10px;
    font-size: 9px;
    color: var(--muted-foreground);
    white-space: nowrap;
    display: block;
  }

  .heatmap-day-label--hidden {
    visibility: hidden;
  }

  /* Right block: month labels above the grid */
  .heatmap-right {
    position: relative;
  }

  .heatmap-month-row {
    position: relative;
    height: 14px;
    margin-bottom: 2px;
  }

  .heatmap-month-label {
    position: absolute;
    top: 0;
    font-size: 9px;
    color: var(--muted-foreground);
    line-height: 1;
    white-space: nowrap;
    /* each week column is 12px (10px cell + 2px gap) */
  }

  .heatmap-grid {
    display: flex;
    gap: 2px;
  }

  .heatmap-col {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .heatmap-cell {
    width: 10px;
    height: 10px;
    border-radius: 2px;
    /* intensity 0 → muted tone; intensity 1 → full chart-1 amber */
    background: color-mix(
      in srgb,
      var(--chart-1) calc(var(--intensity, 0) * 75% + 5%),
      var(--muted)
    );
    transition: opacity 100ms;
  }

  .heatmap-cell--empty {
    background: transparent;
  }

  .heatmap-cell:not(.heatmap-cell--empty):hover {
    outline: 1px solid var(--primary);
    outline-offset: 1px;
  }

  .heatmap-legend-cell {
    width: 10px;
    height: 10px;
    border-radius: 2px;
    background: color-mix(
      in srgb,
      var(--chart-1) calc(var(--intensity, 0) * 75% + 5%),
      var(--muted)
    );
  }
</style>
