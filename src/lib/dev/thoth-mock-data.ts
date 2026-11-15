/**
 * Thoth-specific Tauri command mock map for browser-only dev mode.
 *
 * Casing notes (verified against store source files):
 * - get_config returns snake_case (ConfigRaw in config.svelte.ts)
 * - list_all_transcriptions returns camelCase (TranscriptionRecordRaw in history.svelte.ts)
 * - list_audio_devices returns snake_case id/name/is_default (AudioDevice in settings.svelte.ts)
 * - get_dictionary_entries returns DictionaryEntry with camelCase caseSensitive
 * - list_registered_shortcuts / get_default_shortcuts return ShortcutInfo with
 *   is_enabled (snake_case from Rust); shortcuts.svelte.ts remaps it to isEnabled
 * - fetch_model_manifest returns ModelInfo with all snake_case fields (ModelManager.svelte)
 * - get_all_prompts returns PromptTemplate with isBuiltin camelCase (AIEnhancementSettings.svelte)
 * - get_transcription_stats_cmd returns camelCase (OverviewPane.svelte)
 * - get_gpu_info returns mixed snake/camel (OverviewPane.svelte)
 * - get_storage_usage returns camelCase (StoragePane.svelte)
 */

import type { CommandMap } from './tauri-mock';

// ---------------------------------------------------------------------------
// Seed data
// ---------------------------------------------------------------------------

const MOCK_CONFIG = {
  version: 1,
  audio: { device_id: null, sample_rate: 16000, play_sounds: true, sound_volume: 1 },
  transcription: {
    auto_copy: false,
    auto_paste: true,
    add_leading_space: false,
    append_trailing_space: false,
    auto_submit: 'off' as const,
    remove_fillers: true,
    australian_spelling: true,
    spoken_numbers_to_digits: false,
    voice_formatting_commands: true,
    vocabulary_bias: true,
    typing_tool: 'auto' as const,
    model_idle_unload_secs: null,
  },
  shortcuts: {
    toggle_recording: 'F13',
    toggle_recording_alt: 'CommandOrControl+Shift+Space',
    copy_last: 'F14',
    recording_mode: 'toggle' as const,
    hands_free_silence_secs: 2.0,
  },
  enhancement: {
    enabled: false,
    model: 'llama3.2',
    prompt_id: 'fix-grammar',
    ollama_url: 'http://localhost:11434',
  },
  general: {
    launch_at_login: false,
    show_in_menu_bar: true,
    show_in_dock: false,
    check_for_updates: true,
    show_recording_indicator: true,
    indicator_style: 'cursor-dot' as const,
  },
  recorder: { position: 'top-right' as const, offset_x: -20, offset_y: 20, auto_hide_delay: 3000 },
  integrations: {
    api_enabled: false,
    api_port: 8765,
    mcp_enabled: false,
  },
};

const MOCK_MODELS = [
  {
    id: 'whisper-base-en',
    name: 'Whisper Base (English)',
    description: 'Fast, lightweight English-only model',
    version: '1.0.0',
    size_mb: 148,
    downloaded: true,
    path: '/Users/dev/.thoth/models/whisper-base.en',
    disk_size: 155000000,
    recommended: false,
    languages: ['en'],
    update_available: false,
    selected: true,
    model_type: 'whisper',
    backend_available: true,
    accelerator: 'whisper.cpp (Metal GPU)',
  },
  {
    id: 'whisper-small-en',
    name: 'Whisper Small (English)',
    description: 'Recommended balance of speed and accuracy',
    version: '1.0.0',
    size_mb: 461,
    downloaded: false,
    path: null,
    disk_size: null,
    recommended: true,
    languages: ['en'],
    update_available: false,
    selected: false,
    model_type: 'whisper',
    backend_available: true,
    accelerator: 'whisper.cpp (Metal GPU)',
  },
];

const MOCK_TRANSCRIPTIONS = [
  {
    id: '1',
    text: "Can you push the latest changes to the staging branch before standup? I'll review the pull request straight after.",
    createdAt: new Date(Date.now() - 1800 * 1000).toISOString(),
    durationSeconds: 6.2,
    audioPath: null,
    isEnhanced: false,
    rawText:
      "Can you push the latest changes to the staging branch before standup? I'll review the pull request straight after.",
    enhancementPrompt: null,
    transcriptionModelName: 'Parakeet TDT v3',
    transcriptionDurationSeconds: 0.4,
    enhancementModelName: null,
    enhancementDurationSeconds: null,
  },
  {
    id: '2',
    text: "Let's grab coffee at 3 and go over the Q3 numbers.",
    createdAt: new Date(Date.now() - 3600 * 1000).toISOString(),
    durationSeconds: 3.4,
    audioPath: null,
    isEnhanced: true,
    rawText: 'lets grab coffee at 3 and go over the q3 numbers',
    enhancementPrompt: 'Fix grammar and punctuation.',
    transcriptionModelName: 'Parakeet TDT v3',
    transcriptionDurationSeconds: 0.3,
    enhancementModelName: 'llama3.2',
    enhancementDurationSeconds: 0.7,
  },
  {
    id: '3',
    text: 'Thanks for the quick turnaround on the design mockups. Let us lock in the colour palette on Thursday and ship the landing page by Friday.',
    createdAt: new Date(Date.now() - 7200 * 1000).toISOString(),
    durationSeconds: 7.8,
    audioPath: null,
    isEnhanced: false,
    rawText:
      'Thanks for the quick turnaround on the design mockups. Let us lock in the colour palette on Thursday and ship the landing page by Friday.',
    enhancementPrompt: null,
    transcriptionModelName: 'Parakeet TDT v3',
    transcriptionDurationSeconds: 0.5,
    enhancementModelName: null,
    enhancementDurationSeconds: null,
  },
  {
    id: '4',
    text: 'Note to self: cache the model manifest so the settings window opens instantly next time.',
    createdAt: new Date(Date.now() - 10800 * 1000).toISOString(),
    durationSeconds: 4.6,
    audioPath: null,
    isEnhanced: false,
    rawText:
      'Note to self: cache the model manifest so the settings window opens instantly next time.',
    enhancementPrompt: null,
    transcriptionModelName: 'Parakeet TDT v3',
    transcriptionDurationSeconds: 0.3,
    enhancementModelName: null,
    enhancementDurationSeconds: null,
  },
];

const MOCK_AUDIO_DEVICES = [
  { id: 'default', name: 'MacBook Pro Microphone', is_default: true },
  { id: 'usb-1', name: 'DJI MIC MINI', is_default: false },
];

const MOCK_DICTIONARY_ENTRIES = [
  { from: 'github', to: 'GitHub', caseSensitive: false },
  { from: 'postgres', to: 'PostgreSQL', caseSensitive: false },
  { from: 'kubernetes', to: 'Kubernetes', caseSensitive: false },
  { from: 'im', to: "I'm", caseSensitive: false },
];

const MOCK_SHORTCUTS = [
  { id: 'toggle_recording', accelerator: 'F13', description: 'Toggle recording', is_enabled: true },
  {
    id: 'copy_last',
    accelerator: 'F14',
    description: 'Copy last transcription',
    is_enabled: true,
  },
];

const MOCK_PROMPTS = [
  {
    id: 'fix-grammar',
    name: 'Fix Grammar',
    template: 'Fix grammar and punctuation in the following text.\n\nText: {text}',
    isBuiltin: true,
  },
  {
    id: 'summarise',
    name: 'Summarise',
    template: 'Summarise the following text in one sentence.\n\nText: {text}',
    isBuiltin: true,
  },
];

const MOCK_GPU_INFO = {
  compiled_backend: 'metal',
  gpu_available: true,
  gpu_name: 'Apple M-series',
  vram_mb: null,
  detected_gpus: [{ backend: 'metal', name: 'Apple M-series', vram_mb: null }],
};

const MOCK_STORAGE_USAGE = {
  modelsBytes: 155000000,
  recordingsBytes: 0,
  databaseBytes: 32768,
  configBytes: 1024,
  fluidaudioBytes: 0,
  totalBytes: 155033792,
  recordingCount: 0,
};

const MOCK_TRANSCRIPTION_STATS = {
  totalCount: 4,
  analysableCount: 4,
  enhancedCount: 1,
  totalAudioDuration: 22.0,
  transcriptionModels: [{ name: 'Parakeet TDT v3', count: 4 }],
  enhancementModels: [{ name: 'llama3.2', count: 1 }],
};

// ---------------------------------------------------------------------------
// Insights mock data
// ---------------------------------------------------------------------------

/** Build ~14 months of realistic activity ending today, so heatmap month labels span multiple months */
function buildMockActivity(): Array<{ day: string; count: number; words: number }> {
  const activity = [];
  const today = new Date();
  // Seed the RNG deterministically so the heatmap looks the same each load
  let seed = 42;
  const rand = () => {
    seed = (seed * 1664525 + 1013904223) & 0xffffffff;
    return (seed >>> 0) / 0xffffffff;
  };

  // 365 days = one full year of data so all month labels appear
  for (let i = 364; i >= 0; i--) {
    const d = new Date(today);
    d.setDate(d.getDate() - i);
    const day = [
      d.getFullYear(),
      String(d.getMonth() + 1).padStart(2, '0'),
      String(d.getDate()).padStart(2, '0'),
    ].join('-');
    // Realistic pattern: weekdays heavier, occasional weekends, some gaps
    const dow = d.getDay();
    const isWeekend = dow === 0 || dow === 6;
    const base = isWeekend ? 2 : 8;
    const r = rand();
    if (r < 0.15) {
      // Day off
      continue;
    }
    const count = Math.round(base * r * 2 + 1);
    const words = count * Math.round(30 + rand() * 80);
    activity.push({ day, count, words });
  }
  return activity;
}

const MOCK_INSIGHTS_DATA = {
  totals: {
    totalCount: 1842,
    totalAudioSeconds: 28560,
    totalWords: 183400,
    enhancedCount: 312,
    typingTimeSavedSeconds: 275100, // 183400 words / 40 wpm * 60s
    firstRecordingAt: new Date(Date.now() - 90 * 24 * 3600 * 1000).toISOString(),
  },
  activity: buildMockActivity(),
  currentStreak: 14,
  longestStreak: 31,
  throughput: [
    {
      name: 'fluidaudio-parakeet-tdt-coreml',
      count: 1200,
      avgAudioDuration: 8.2,
      avgProcessingTime: 0.12,
      speedFactor: 67,
    },
    {
      name: 'ggml-large-v3-turbo',
      count: 580,
      avgAudioDuration: 11.4,
      avgProcessingTime: 0.67,
      speedFactor: 17,
    },
    {
      name: 'parakeet-tdt-1.1',
      count: 62,
      avgAudioDuration: 6.1,
      avgProcessingTime: 1.05,
      speedFactor: 5.8,
    },
  ],
  modelUsage: {
    backendCounts: [
      { name: 'fluidaudio-parakeet-tdt-coreml', count: 1200 },
      { name: 'ggml-large-v3-turbo', count: 580 },
      { name: 'parakeet-tdt-1.1', count: 62 },
    ],
    enhancementPrompts: [
      { prompt: 'Fix grammar and punctuation', count: 210 },
      { prompt: 'Summarise', count: 102 },
    ],
    enhancedPct: 17,
  },
  lengthHistogram: [
    { bucketLabel: '0–5s', count: 412 },
    { bucketLabel: '5–15s', count: 680 },
    { bucketLabel: '15–30s', count: 420 },
    { bucketLabel: '30–60s', count: 210 },
    { bucketLabel: '1–2m', count: 88 },
    { bucketLabel: '2m+', count: 32 },
  ],
  timeOfDay: [
    2, 0, 0, 0, 0, 1, 4, 18, 72, 95, 112, 98, 85, 76, 88, 102, 94, 68, 42, 28, 14, 8, 4, 2,
  ],
  storage: {
    recordingsBytes: 412 * 1024 * 1024,
    modelsBytes: 155 * 1024 * 1024,
    dbBytes: 2.4 * 1024 * 1024,
    totalBytes: (412 + 155 + 2.4) * 1024 * 1024,
    oldestRecordingAt: new Date(Date.now() - 90 * 24 * 3600 * 1000).toISOString(),
  },
};

// Realistic cruft: low-density recordings (chars/sec well below the ~1.0
// threshold) — a silent forgotten-toggle and two silence-hallucinations.
const MOCK_CRUFT_CANDIDATES = [
  {
    id: 'cruft-1',
    createdAt: new Date(Date.now() - 5 * 24 * 3600 * 1000).toISOString(),
    textPreview: '',
    durationSeconds: 47.2,
    density: 0.0,
    audioPath: '/Users/dev/.thoth/Recordings/thoth_recording_20240617_050312.wav',
    fileBytes: 1510400,
    rms: 0.015,
  },
  {
    id: 'cruft-2',
    createdAt: new Date(Date.now() - 3 * 24 * 3600 * 1000).toISOString(),
    textPreview: 'Thank you.',
    durationSeconds: 16.4,
    density: 0.6,
    audioPath: '/Users/dev/.thoth/Recordings/thoth_recording_20240619_091033.wav',
    fileBytes: 524800,
    rms: 0.041,
  },
  {
    id: 'cruft-3',
    createdAt: new Date(Date.now() - 1 * 24 * 3600 * 1000).toISOString(),
    textPreview: 'Okay.',
    durationSeconds: 23.1,
    density: 0.2,
    audioPath: '/Users/dev/.thoth/Recordings/thoth_recording_20240621_152244.wav',
    fileBytes: 739200,
    rms: null,
  },
];

const MOCK_TRASH_ENTRIES: Array<{
  id: string;
  textPreview: string;
  createdAt: string;
  deletedAt: string;
  durationSeconds: number | null;
  fileBytes: number;
  audioMoved: boolean;
}> = [];

// ---------------------------------------------------------------------------
// Command map
// ---------------------------------------------------------------------------

export const thothMockCommands: CommandMap = {
  // -- Startup-critical (must resolve for initialise() to complete) --
  get_config: () => MOCK_CONFIG,
  // The frontend's only source of defaults (#127). Mocked with the same shape as
  // get_config so the dev harness exercises the real code path.
  get_default_config: () => MOCK_CONFIG,
  init_database: () => undefined,
  is_transcription_ready: () => true,
  get_model_directory: () => '/Users/dev/.thoth/models',
  check_model_downloaded: () => true,
  init_transcription: () => undefined,
  get_pipeline_state: () => 'idle',

  // -- What's new (#113). Null keeps the dev shell free of the modal; the
  //    real command answers only when the running version's notes are unseen.
  whats_new: () => null,
  mark_whats_new_seen: () => undefined,

  // -- Sounds (called during soundStore.load()) --
  are_sounds_enabled: () => true,
  get_sound_volume: () => 1,
  set_sound_volume: () => undefined,

  // -- Shortcuts store initialise() --
  get_default_shortcuts: () => MOCK_SHORTCUTS,
  list_registered_shortcuts: () => MOCK_SHORTCUTS,

  // -- Settings store initialise() (loads audio devices) --
  list_audio_devices: () => MOCK_AUDIO_DEVICES,

  // -- Model manager --
  fetch_model_manifest: () => MOCK_MODELS,
  get_download_progress: () => 'Idle',
  get_manifest_update_time: () => new Date(Date.now() - 3600 * 1000).toISOString(),

  // -- History pane --
  list_all_transcriptions: () => MOCK_TRANSCRIPTIONS,

  // -- Dictionary pane --
  get_dictionary_entries: () => MOCK_DICTIONARY_ENTRIES,

  // -- AI enhancement pane --
  get_all_prompts: () => MOCK_PROMPTS,
  check_ollama_available: () => false,
  list_ollama_models: () => [],

  // -- Overview / performance panes --
  get_transcription_stats_cmd: () => MOCK_TRANSCRIPTION_STATS,
  get_gpu_info: () => MOCK_GPU_INFO,

  // -- Storage pane --
  get_storage_usage: () => MOCK_STORAGE_USAGE,

  // -- Insights pane --
  get_insights: () => MOCK_INSIGHTS_DATA,
  get_cruft_candidates: () => MOCK_CRUFT_CANDIDATES,
  quarantine_recordings: (args) => {
    const ids = (args as { ids?: string[] } | undefined)?.ids ?? [];
    // Move matched candidates into the mock trash
    const moved = MOCK_CRUFT_CANDIDATES.filter((c) => ids.includes(c.id));
    for (const m of moved) {
      MOCK_TRASH_ENTRIES.push({
        id: m.id,
        textPreview: m.textPreview,
        createdAt: m.createdAt,
        deletedAt: new Date().toISOString(),
        durationSeconds: m.durationSeconds,
        fileBytes: m.fileBytes,
        audioMoved: true,
      });
      const idx = MOCK_CRUFT_CANDIDATES.findIndex((c) => c.id === m.id);
      if (idx !== -1) MOCK_CRUFT_CANDIDATES.splice(idx, 1);
    }
    return moved.length;
  },
  restore_recordings: (args) => {
    const ids = (args as { ids?: string[] } | undefined)?.ids ?? [];
    ids.forEach((id) => {
      const idx = MOCK_TRASH_ENTRIES.findIndex((e) => e.id === id);
      if (idx !== -1) MOCK_TRASH_ENTRIES.splice(idx, 1);
    });
    return ids.length;
  },
  purge_trash: (args) => {
    const ids = (args as { ids?: string[] } | undefined)?.ids;
    if (!ids || ids.length === 0) {
      MOCK_TRASH_ENTRIES.splice(0, MOCK_TRASH_ENTRIES.length);
    } else {
      ids.forEach((id) => {
        const idx = MOCK_TRASH_ENTRIES.findIndex((e) => e.id === id);
        if (idx !== -1) MOCK_TRASH_ENTRIES.splice(idx, 1);
      });
    }
    return undefined;
  },
  list_trash: () => [...MOCK_TRASH_ENTRIES],

  // -- Permissions pane --
  get_permissions: () => ({ microphone: true, accessibility: true }),
  request_permissions: () => undefined,

  // -- Config path (settings pane) --
  get_config_path_cmd: () => '/Users/dev/.thoth/config.json',
  get_show_in_dock: () => false,
  reset_config: () => MOCK_CONFIG,

  // -- No-ops: write commands (set_*, delete_*, play_*, etc.) --
  set_config: () => undefined,
  set_audio_device: () => undefined,
  set_sounds_enabled: () => undefined,
  set_selected_model_id: () => undefined,
  set_show_in_dock: () => undefined,
  set_shortcut_config: () => undefined,
  register_shortcut: () => undefined,
  unregister_shortcut: () => undefined,
  register_default_shortcuts: () => undefined,
  unregister_all_shortcuts: () => undefined,
  try_register_shortcut: () => ({
    type: 'Success',
    shortcut: 'F13',
    shortcut_id: 'toggle_recording',
  }),
  check_shortcut_available: () => true,
  get_shortcut_suggestions: () => [],
  reregister_shortcuts: () => undefined,
  add_dictionary_entry: () => undefined,
  update_dictionary_entry: () => undefined,
  remove_dictionary_entry: () => undefined,
  import_dictionary: () => 0,
  export_dictionary: () => '[]',
  apply_dictionary_to_text: (args) => (args as { text?: string } | undefined)?.text ?? '',
  save_custom_prompt_cmd: () => undefined,
  delete_custom_prompt_cmd: () => undefined,
  download_model: () => undefined,
  delete_model: () => undefined,
  reset_download_state: () => undefined,
  delete_fluidaudio_cache: () => undefined,
  init_fluidaudio_transcription: () => undefined,
  delete_transcription_by_id: () => undefined,
  delete_all_transcriptions_cmd: () => undefined,
  insert_text: () => undefined,
  insert_text_by_paste: () => undefined,
  insert_text_by_typing: () => undefined,
  play_recording_start_sound: () => undefined,
  play_recording_stop_sound: () => undefined,
  play_transcription_complete_sound: () => undefined,
  play_error_sound: () => undefined,
  pipeline_toggle_recording: () => ({ action: 'started', path: '/tmp/mock.wav' }),
  pipeline_cancel: () => undefined,
  pipeline_transcribe_file: () => ({
    success: true,
    text: 'Mock file transcription.',
    rawText: 'Mock file transcription.',
    isEnhanced: false,
    durationSeconds: 2.0,
    audioPath: null,
    error: null,
    transcriptionId: null,
  }),
  show_recording_indicator: () => undefined,
  hide_recording_indicator: () => undefined,
  show_window: () => undefined,
  refresh_tray_menu: () => undefined,
  open_url: () => undefined,
  delete_all_data: () => undefined,
  delete_all_recordings: () => 0,
  export_to_csv: () => 0,
  export_to_json: () => 0,
  export_to_txt: () => 0,
  filter_transcription: (args) => (args as { text?: string } | undefined)?.text ?? '',
  enter_capture_mode: () => 'F13',
  exit_capture_mode: () => undefined,
  report_key_event: () => undefined,
  start_audio_preview: () => undefined,
  stop_audio_preview: () => undefined,

  // -- Integrations (Local Control API + MCP server) --
  get_integrations_status: () => ({
    apiEnabled: false,
    apiRunning: false,
    apiPort: 8765,
    mcpEnabled: false,
    hasToken: true,
  }),
  get_api_token: () => 'thoth-dev-0000-1111-2222-3333-444455556666',
  set_api_enabled: () => undefined,
  set_mcp_enabled: () => undefined,
  rotate_api_token: () => 'thoth-dev-rotated-aaaa-bbbb-cccc-ddddeeeeffff',
};
