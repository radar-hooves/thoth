/**
 * History state management store using Svelte 5 runes.
 * Manages transcription records with search, filtering, and CRUD operations.
 */

import { invoke } from '@tauri-apps/api/core';
import { formatDuration } from '#lib/utils/format.js';

/** Transcription record structure matching SQLite backend */
export interface TranscriptionRecord {
  id: string;
  text: string;
  timestamp: Date;
  duration: number;
  audioPath?: string;
  enhanced?: boolean;
  rawText?: string;
  enhancementPrompt?: string;
  transcriptionModelName?: string;
  transcriptionDurationSeconds?: number;
  enhancementModelName?: string;
  enhancementDurationSeconds?: number;
}

/** Serialised form from backend (dates as ISO strings, camelCase field names) */
interface TranscriptionRecordRaw {
  id: string;
  text: string;
  createdAt: string;
  durationSeconds: number | null;
  audioPath: string | null;
  isEnhanced: boolean;
  rawText: string | null;
  enhancementPrompt: string | null;
  transcriptionModelName: string | null;
  transcriptionDurationSeconds: number | null;
  enhancementModelName: string | null;
  enhancementDurationSeconds: number | null;
}

/** Pagination state for infinite scroll */
interface PaginationState {
  offset: number;
  limit: number;
  hasMore: boolean;
  isLoading: boolean;
}

/** Convert raw record from backend to typed record */
function parseRecord(raw: TranscriptionRecordRaw): TranscriptionRecord {
  return {
    id: raw.id,
    text: raw.text,
    timestamp: new Date(raw.createdAt),
    duration: raw.durationSeconds ?? 0,
    audioPath: raw.audioPath ?? undefined,
    enhanced: raw.isEnhanced,
    rawText: raw.rawText ?? undefined,
    enhancementPrompt: raw.enhancementPrompt ?? undefined,
    transcriptionModelName: raw.transcriptionModelName ?? undefined,
    transcriptionDurationSeconds: raw.transcriptionDurationSeconds ?? undefined,
    enhancementModelName: raw.enhancementModelName ?? undefined,
    enhancementDurationSeconds: raw.enhancementDurationSeconds ?? undefined,
  };
}

/** Create the history store with reactive state */
function createHistoryStore() {
  // Core state
  let records = $state<TranscriptionRecord[]>([]);
  let searchQuery = $state<string>('');
  let selectedId = $state<string | null>(null);
  let pagination = $state<PaginationState>({
    offset: 0,
    limit: 50,
    hasMore: true,
    isLoading: false,
  });
  let error = $state<string | null>(null);

  // Debounce timer for search
  let searchDebounceTimer: ReturnType<typeof setTimeout> | null = null;
  const SEARCH_DEBOUNCE_MS = 300;

  // Derived state
  const filteredRecords = $derived.by(() => {
    if (!searchQuery.trim()) {
      return records;
    }
    const query = searchQuery.toLowerCase();
    return records.filter(
      (record) =>
        record.text.toLowerCase().includes(query) ||
        formatDate(record.timestamp).toLowerCase().includes(query)
    );
  });

  const selectedRecord = $derived(
    selectedId ? (records.find((r) => r.id === selectedId) ?? null) : null
  );

  const isEmpty = $derived(records.length === 0);
  const hasResults = $derived(filteredRecords.length > 0);

  /** Format date for display */
  function formatDate(date: Date): string {
    const now = new Date();
    const diff = now.getTime() - date.getTime();
    const days = Math.floor(diff / (1000 * 60 * 60 * 24));

    if (days === 0) {
      return date.toLocaleTimeString('en-AU', {
        hour: '2-digit',
        minute: '2-digit',
      });
    } else if (days === 1) {
      return 'Yesterday';
    } else if (days < 7) {
      return date.toLocaleDateString('en-AU', { weekday: 'long' });
    } else {
      return date.toLocaleDateString('en-AU', {
        day: '2-digit',
        month: '2-digit',
        year: 'numeric',
      });
    }
  }

  /** Load initial records from backend */
  async function loadRecords(): Promise<void> {
    pagination.isLoading = true;
    error = null;

    try {
      const rawRecords = await invoke<TranscriptionRecordRaw[]>('list_all_transcriptions', {
        offset: 0,
        limit: pagination.limit,
      });

      records = rawRecords.map(parseRecord);
      pagination.offset = records.length;
      pagination.hasMore = rawRecords.length === pagination.limit;
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to load transcriptions';
      console.error('Failed to load transcriptions:', e);
    } finally {
      pagination.isLoading = false;
    }
  }

  /** Load more records for infinite scroll */
  async function loadMore(): Promise<void> {
    if (pagination.isLoading || !pagination.hasMore) {
      return;
    }

    pagination.isLoading = true;
    error = null;

    try {
      const rawRecords = await invoke<TranscriptionRecordRaw[]>('list_all_transcriptions', {
        offset: pagination.offset,
        limit: pagination.limit,
      });

      const newRecords = rawRecords.map(parseRecord);
      records = [...records, ...newRecords];
      pagination.offset += newRecords.length;
      pagination.hasMore = rawRecords.length === pagination.limit;
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to load more transcriptions';
      console.error('Failed to load more transcriptions:', e);
    } finally {
      pagination.isLoading = false;
    }
  }

  /** Set search query with debouncing */
  function setSearchQuery(query: string): void {
    if (searchDebounceTimer) {
      clearTimeout(searchDebounceTimer);
    }

    searchDebounceTimer = setTimeout(() => {
      searchQuery = query;
      searchDebounceTimer = null;
    }, SEARCH_DEBOUNCE_MS);
  }

  /** Set search query immediately (no debounce) */
  function setSearchQueryImmediate(query: string): void {
    if (searchDebounceTimer) {
      clearTimeout(searchDebounceTimer);
      searchDebounceTimer = null;
    }
    searchQuery = query;
  }

  /** Select a record by ID */
  function selectRecord(id: string | null): void {
    selectedId = id;
  }

  /** Delete a record */
  async function deleteRecord(id: string): Promise<boolean> {
    try {
      await invoke('delete_transcription_by_id', { id });

      records = records.filter((r) => r.id !== id);

      if (selectedId === id) {
        selectedId = null;
      }

      return true;
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to delete transcription';
      console.error('Failed to delete transcription:', e);
      return false;
    }
  }

  /** Delete multiple records */
  async function deleteRecords(ids: string[]): Promise<boolean> {
    try {
      for (const id of ids) {
        await invoke('delete_transcription_by_id', { id });
      }

      records = records.filter((r) => !ids.includes(r.id));

      if (selectedId && ids.includes(selectedId)) {
        selectedId = null;
      }

      return true;
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to delete transcriptions';
      console.error('Failed to delete transcriptions:', e);
      return false;
    }
  }

  /** Delete all transcriptions */
  async function deleteAll(): Promise<boolean> {
    try {
      await invoke('delete_all_transcriptions_cmd');

      records = [];
      selectedId = null;
      pagination = {
        offset: 0,
        limit: 50,
        hasMore: false,
        isLoading: false,
      };

      return true;
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to delete all transcriptions';
      console.error('Failed to delete all transcriptions:', e);
      return false;
    }
  }

  /** Copy text to clipboard */
  async function copyToClipboard(text: string): Promise<boolean> {
    try {
      await navigator.clipboard.writeText(text);
      return true;
    } catch (e) {
      console.error('Failed to copy to clipboard:', e);
      return false;
    }
  }

  /** Add a new record (typically from recording completion) */
  function addRecord(record: TranscriptionRecord): void {
    // Add to beginning of list (most recent first)
    records = [record, ...records];
    pagination.offset += 1;
  }

  /** Update an existing record */
  function updateRecord(id: string, updates: Partial<TranscriptionRecord>): void {
    records = records.map((r) => (r.id === id ? { ...r, ...updates } : r));
  }

  /** Clear error state */
  function clearError(): void {
    error = null;
  }

  /** Reset store to initial state */
  function reset(): void {
    records = [];
    searchQuery = '';
    selectedId = null;
    pagination = {
      offset: 0,
      limit: 50,
      hasMore: true,
      isLoading: false,
    };
    error = null;
  }

  return {
    // State (getters for reactive access)
    get records() {
      return records;
    },
    get searchQuery() {
      return searchQuery;
    },
    get selectedId() {
      return selectedId;
    },
    get pagination() {
      return pagination;
    },
    get error() {
      return error;
    },

    // Derived state
    get filteredRecords() {
      return filteredRecords;
    },
    get selectedRecord() {
      return selectedRecord;
    },
    get isEmpty() {
      return isEmpty;
    },
    get hasResults() {
      return hasResults;
    },

    // Utility functions
    formatDate,
    formatDuration,

    // Actions
    loadRecords,
    loadMore,
    setSearchQuery,
    setSearchQueryImmediate,
    selectRecord,
    deleteRecord,
    deleteRecords,
    deleteAll,
    copyToClipboard,
    addRecord,
    updateRecord,
    clearError,
    reset,
  };
}

// Export singleton instance
export const historyStore = createHistoryStore();
