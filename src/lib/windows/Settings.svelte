<script lang="ts">
  /**
   * Settings window - configuration interface for Thoth
   *
   * Settings window with 7 panes:
   * Overview, Recording, Models, AI, History, Dictionary, Transcribe
   */

  import type { Component } from 'svelte';
  import { onMount, onDestroy } from 'svelte';
  import { listen, type UnlistenFn } from '@tauri-apps/api/event';
  import { invoke } from '@tauri-apps/api/core';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import LayoutDashboard from '@lucide/svelte/icons/layout-dashboard';
  import Mic from '@lucide/svelte/icons/mic';
  import Cpu from '@lucide/svelte/icons/cpu';
  import Sparkles from '@lucide/svelte/icons/sparkles';
  import History from '@lucide/svelte/icons/history';
  import BookOpen from '@lucide/svelte/icons/book-open';
  import FileText from '@lucide/svelte/icons/file-text';
  import HardDrive from '@lucide/svelte/icons/hard-drive';
  import Plug from '@lucide/svelte/icons/plug';
  import BarChart3 from '@lucide/svelte/icons/bar-chart-3';
  import Info from '@lucide/svelte/icons/info';
  import AIEnhancementSettings from '../components/AIEnhancementSettings.svelte';
  import AudioDeviceSelector from '../components/AudioDeviceSelector.svelte';
  import DictionaryEditor from '../components/DictionaryEditor.svelte';
  import FilterSettings from '../components/FilterSettings.svelte';
  import StoragePane from '../components/StoragePane.svelte';
  import IntegrationsSettings from '../components/IntegrationsSettings.svelte';
  import HistoryPane from '../components/HistoryPane.svelte';
  import InsightsPane from '../components/InsightsPane.svelte';
  import ModelManager from '../components/ModelManager.svelte';
  import TranscribePane from '../components/TranscribePane.svelte';
  import OverviewPane from '../components/OverviewPane.svelte';
  import AboutDialog from '../components/AboutDialog.svelte';
  import WhatsNewDialog from '../components/WhatsNewDialog.svelte';
  import ShortcutInput from '../components/ShortcutInput.svelte';
  import {
    configStore,
    type IndicatorStyle,
    type AutoSubmit,
    type RecordingMode,
    type TypingTool,
  } from '../stores/config.svelte';
  import { pipelineStore } from '../stores/pipeline.svelte';
  import { shortcutsStore, type ShortcutInfo } from '../stores/shortcuts.svelte';
  import { soundStore } from '../stores/sound.svelte';
  import { Button } from '@poodle64/ui/button';
  import { Switch } from '@poodle64/ui/switch';
  import * as RadioGroup from '../components/ui/radio-group';
  import WindowControls from '../components/WindowControls.svelte';

  /** Settings pane definition */
  interface SettingsPane {
    id: string;
    title: string;
    icon: Component;
  }

  /** Filter options type matching Rust FilterOptions */
  interface FilterOptions {
    remove_fillers: boolean;
    normalise_whitespace: boolean;
    cleanup_punctuation: boolean;
    sentence_case: boolean;
    australian_spelling: boolean;
    spoken_numbers_to_digits: boolean;
    voice_formatting_commands: boolean;
  }

  /** Available settings panes matching Swift app */
  const panes: SettingsPane[] = [
    { id: 'overview', title: 'Overview', icon: LayoutDashboard },
    { id: 'insights', title: 'Insights', icon: BarChart3 },
    { id: 'recording', title: 'Recording', icon: Mic },
    { id: 'models', title: 'Models', icon: Cpu },
    { id: 'ai', title: 'AI Enhancement', icon: Sparkles },
    { id: 'history', title: 'History', icon: History },
    { id: 'dictionary', title: 'Dictionary', icon: BookOpen },
    { id: 'transcribe', title: 'Transcribe', icon: FileText },
    { id: 'storage', title: 'Storage', icon: HardDrive },
    { id: 'integrations', title: 'Integrations', icon: Plug },
  ];

  let activePane = $state('overview');

  /** About dialog visibility */
  let showAbout = $state(false);

  /** Linux only — show the custom close button when decorations are disabled. */
  const isLinux = /Linux/.test(navigator.userAgent);
  const showWindowControls = $derived(isLinux && !configStore.general.windowDecorations);

  /** Map of shortcut IDs to their pending (unsaved) accelerators */
  const pendingChanges = $state<Map<string, string>>(new Map());

  /** Combined list of all shortcuts (registered + defaults not yet registered) */
  const allShortcuts = $derived.by(() => {
    const registered = new Map(shortcutsStore.shortcuts.map((s) => [s.id, s]));
    const combined: ShortcutInfo[] = [];

    for (const def of shortcutsStore.defaults) {
      const reg = registered.get(def.id);
      if (reg) {
        combined.push(reg);
      } else {
        combined.push({ ...def, isEnabled: false });
      }
    }

    for (const reg of shortcutsStore.shortcuts) {
      if (!shortcutsStore.defaults.some((d) => d.id === reg.id)) {
        combined.push(reg);
      }
    }

    return combined;
  });

  function getCurrentAccelerator(shortcut: ShortcutInfo): string {
    return pendingChanges.get(shortcut.id) ?? shortcut.accelerator;
  }

  function getDefaultAccelerator(id: string): string | undefined {
    return shortcutsStore.defaults.find((d) => d.id === id)?.accelerator;
  }

  async function handleShortcutChange(shortcut: ShortcutInfo, newAccelerator: string) {
    // Update in-memory config, then save directly via set_shortcut_config
    // which bypasses the preservation logic in set_config. This ensures
    // the shortcut value is saved even when it matches the default.
    // exit_capture_mode (called by ShortcutInput.stopCapture) will then
    // re-register all shortcuts from the saved config.
    updateShortcutConfig(shortcut.id, newAccelerator);
    await saveShortcutConfig();

    // Reload registered shortcuts after the capture cycle completes.
    // onchange fires before stopCapture/exit_capture_mode, so we defer
    // the reload to run after the full async chain finishes.
    setTimeout(() => shortcutsStore.loadRegistered(), 100);
  }

  async function handleShortcutClear(shortcut: ShortcutInfo) {
    // Clear from config using bypass, then re-register all shortcuts
    updateShortcutConfig(shortcut.id, null);
    await saveShortcutConfig();
    await reRegisterShortcuts();
  }

  async function handleShortcutReset(shortcut: ShortcutInfo) {
    // Reset to default value using bypass (important: preservation logic in
    // set_config would block this since the incoming value matches the default)
    const defaultAcc = getDefaultAccelerator(shortcut.id);
    if (defaultAcc) {
      updateShortcutConfig(shortcut.id, defaultAcc);
      await saveShortcutConfig();
      await reRegisterShortcuts();
    }
  }

  /** Update shortcut in config based on shortcut ID */
  function updateShortcutConfig(id: string, accelerator: string | null): void {
    switch (id) {
      case 'toggle_recording':
        configStore.updateShortcuts('toggleRecording', accelerator ?? 'F13');
        break;
      case 'toggle_recording_alt':
        configStore.updateShortcuts('toggleRecordingAlt', accelerator);
        break;
      case 'copy_last_transcription':
        configStore.updateShortcuts('copyLast', accelerator);
        break;
      case 'toggle_enhancement':
        configStore.updateShortcuts('toggleEnhancement', accelerator);
        break;
    }
  }

  /**
   * Save shortcut config directly via the bypass IPC command.
   * This avoids the preservation logic in set_config that would prevent
   * resetting shortcuts back to their default values.
   */
  async function saveShortcutConfig(): Promise<void> {
    try {
      await invoke('set_shortcut_config', {
        shortcuts: {
          toggle_recording: configStore.shortcuts.toggleRecording,
          toggle_recording_alt: configStore.shortcuts.toggleRecordingAlt,
          copy_last: configStore.shortcuts.copyLast,
          toggle_enhancement: configStore.shortcuts.toggleEnhancement,
          recording_mode: configStore.shortcuts.recordingMode,
          hands_free_silence_secs: configStore.shortcuts.handsFreeSilenceSecs,
        },
      });
    } catch (e) {
      console.error('Failed to save shortcut config:', e);
    }
  }

  /**
   * Unregister all shortcuts and re-register from saved config.
   * Used after clear/reset operations that change config outside of capture mode.
   */
  async function reRegisterShortcuts(): Promise<void> {
    try {
      await invoke('reregister_shortcuts');
      await shortcutsStore.loadRegistered();
    } catch (e) {
      console.error('Failed to re-register shortcuts:', e);
    }
  }

  async function handleFilterChange(options: FilterOptions) {
    configStore.updateTranscription('removeFillers', options.remove_fillers);
    configStore.updateTranscription('australianSpelling', options.australian_spelling);
    configStore.updateTranscription('spokenNumbersToDigits', options.spoken_numbers_to_digits);
    configStore.updateTranscription('normaliseWhitespace', options.normalise_whitespace);
    configStore.updateTranscription('cleanupPunctuation', options.cleanup_punctuation);
    configStore.updateTranscription('sentenceCase', options.sentence_case);
    configStore.updateTranscription('voiceFormattingCommands', options.voice_formatting_commands);
    await configStore.save();
  }

  /**
   * Recording-mode options.
   *
   * The hints say what the user does, not what the VAD does — the mode is a
   * choice about the hotkey, and describing it in signal-processing terms is
   * how a settings pane stops being readable.
   */
  const RECORDING_MODES: { value: RecordingMode; label: string; hint: string }[] = [
    { value: 'toggle', label: 'Toggle', hint: 'Press once to start, again to stop' },
    { value: 'hands_free', label: 'Hands-free', hint: 'Press once to start; a pause ends it' },
    {
      value: 'hold_to_record',
      label: 'Hold to record',
      hint: 'Records for exactly as long as you hold the key',
    },
    {
      value: 'tap_or_hold',
      label: 'Tap or hold',
      hint: 'Tap to start and tap again to stop, or hold to talk and let go to stop',
    },
  ];

  /**
   * Bounds on the hands-free timeout.
   *
   * These mirror HANDS_FREE_SILENCE_RANGE in src-tauri/src/config.rs, which
   * clamps whatever arrives — so the slider cannot offer a value the backend
   * would refuse, and a drifted copy here narrows the slider rather than
   * changing behaviour.
   */
  const HANDS_FREE_SILENCE_MIN = 0.8;
  const HANDS_FREE_SILENCE_MAX = 10;

  /**
   * Linux typing tools, in the order the auto chain considers them.
   *
   * The hints say what each one needs, because that is the whole decision: a
   * user on KDE or GNOME cannot make `wtype` work however much they want to.
   */
  const TYPING_TOOLS: { value: TypingTool; label: string; hint: string }[] = [
    { value: 'auto', label: 'Automatic', hint: 'Order the tools by what this desktop is' },
    { value: 'wtype', label: 'wtype', hint: 'Wayland virtual keyboard — not KDE or GNOME' },
    { value: 'kwtype', label: 'kwtype', hint: "KDE's own Fake Input protocol" },
    { value: 'dotool', label: 'dotool', hint: 'Any compositor, via /dev/uinput' },
    { value: 'ydotool', label: 'ydotool', hint: 'Any compositor, via its own daemon' },
    { value: 'xdotool', label: 'xdotool', hint: 'X11 sessions only' },
    { value: 'enigo', label: 'Built-in', hint: 'No external tool; XWayland on Wayland' },
  ];

  async function handleTypingToolChange(tool: TypingTool) {
    configStore.updateTranscription('typingTool', tool);
    await configStore.save();
  }

  async function handleRecordingModeChange(mode: RecordingMode) {
    configStore.updateShortcuts('recordingMode', mode);
    await configStore.save();
  }

  async function handleSilenceTimeoutChange(secs: number) {
    configStore.updateShortcuts('handsFreeSilenceSecs', secs);
    await configStore.save();
  }

  async function handleIndicatorStyleChange(style: IndicatorStyle) {
    configStore.updateGeneral('indicatorStyle', style);
    await configStore.save();

    // If currently recording, re-show the indicator to apply the new style
    try {
      if (pipelineStore.isRecording && configStore.general.showRecordingIndicator) {
        await invoke('hide_recording_indicator');
        await invoke('show_recording_indicator');
      }
    } catch (e) {
      console.error('Failed to update indicator style:', e);
    }
  }

  /** Handle window dragging from title bar */
  function handleDragRegionMouseDown(event: MouseEvent): void {
    if (event.buttons !== 1) return;
    const target = event.target as HTMLElement;
    const interactive = target.closest('button, input, a, [role="button"]');
    if (!interactive) {
      getCurrentWindow()
        .startDragging()
        .catch(() => {});
    }
  }

  let navigateUnlisten: UnlistenFn | null = null;

  onMount(async () => {
    // Listen for tray menu navigation events (e.g. "History...", "Settings...")
    navigateUnlisten = await listen<string>('navigate', (event) => {
      if (panes.some((p) => p.id === event.payload)) {
        activePane = event.payload;
      }
    });
  });

  onDestroy(() => {
    navigateUnlisten?.();
  });
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="settings-window">
  <!-- Title bar with drag region -->
  <header class="title-bar" onmousedown={handleDragRegionMouseDown}>
    <span class="title-text">Thoth</span>
    {#if pipelineStore.isRecording}
      <span class="recording-indicator">
        <span class="recording-dot"></span>
        Recording {pipelineStore.formattedDuration}
      </span>
    {:else if pipelineStore.isProcessing}
      <span class="processing-indicator">Processing...</span>
    {/if}
    {#if showWindowControls}
      <div class="title-bar-controls">
        <WindowControls />
      </div>
    {/if}
  </header>

  <div class="main-area">
    <!-- Sidebar -->
    <nav class="sidebar">
      <div class="sidebar-items">
        {#each panes as pane}
          {@const Icon = pane.icon}
          <Button
            variant="ghost"
            class="sidebar-item {activePane === pane.id ? 'active' : ''}"
            onclick={() => (activePane = pane.id)}
          >
            <span class="sidebar-icon">
              <Icon size={16} />
            </span>
            <span class="sidebar-label">{pane.title}</span>
          </Button>
        {/each}
      </div>
      <div class="sidebar-footer">
        <Button variant="ghost" class="sidebar-item" onclick={() => (showAbout = true)}>
          <span class="sidebar-icon">
            <Info size={16} />
          </span>
          <span class="sidebar-label">About Thoth</span>
        </Button>
      </div>
    </nav>

    <!-- Content -->
    <main class="content">
      {#if activePane === 'overview'}
        <div class="pane">
          <OverviewPane onNavigate={(paneId) => (activePane = paneId)} />
        </div>
      {:else if activePane === 'insights'}
        <div class="pane">
          <InsightsPane />
        </div>
      {:else if activePane === 'recording'}
        <div class="pane">
          <!-- Audio Input Section -->
          <section class="flex flex-col">
            <div class="mb-3">
              <h2 class="text-base font-semibold text-foreground m-0">Audio Input</h2>
              <p class="text-xs text-muted-foreground m-0">
                Choose how Thoth selects your audio input device
              </p>
            </div>
            <div class="flex flex-col gap-2">
              <AudioDeviceSelector />
            </div>
          </section>

          <!-- Shortcuts Section -->
          <section class="flex flex-col">
            <div class="mb-3">
              <h2 class="text-base font-semibold text-foreground m-0">Shortcuts</h2>
              <p class="text-xs text-muted-foreground m-0">
                Tap to start recording, tap again to stop.
              </p>
            </div>
            <div class="flex flex-col gap-2">
              {#if !shortcutsStore.isLoading}
                <div class="shortcuts-list">
                  {#each allShortcuts as shortcut, i (shortcut.id)}
                    <div
                      class="flex items-center justify-between gap-4 rounded-md border border-border bg-card p-3"
                    >
                      <div class="flex flex-1 flex-col gap-1">
                        <span class="text-sm font-medium text-foreground"
                          >{shortcut.description}</span
                        >
                        <span class="text-xs shortcut-status" class:enabled={shortcut.isEnabled}>
                          {shortcut.isEnabled ? 'Active' : 'Inactive'}
                        </span>
                      </div>
                      <ShortcutInput
                        value={getCurrentAccelerator(shortcut)}
                        shortcutId={shortcut.id}
                        defaultValue={getDefaultAccelerator(shortcut.id)}
                        onchange={(acc) => handleShortcutChange(shortcut, acc)}
                        onclear={() => handleShortcutClear(shortcut)}
                        onreset={() => handleShortcutReset(shortcut)}
                        placeholder="Click to set"
                      />
                    </div>
                  {/each}
                </div>
              {/if}
            </div>
          </section>

          <!-- Recording Behaviour Section -->
          <section class="flex flex-col">
            <div class="mb-3">
              <h2 class="text-base font-semibold text-foreground m-0">Recording Behaviour</h2>
              <p class="text-xs text-muted-foreground m-0">
                Audio and clipboard handling during transcription
              </p>
            </div>
            <div class="flex flex-col gap-2">
              <div class="flex flex-col gap-3 rounded-md border border-border bg-card p-3">
                <div class="flex flex-col gap-1">
                  <span class="text-sm font-medium text-foreground">Recording Mode</span>
                  <span class="text-xs text-muted-foreground">How a recording ends</span>
                </div>
                <RadioGroup.Root
                  value={configStore.shortcuts.recordingMode}
                  onValueChange={(v) => handleRecordingModeChange(v as RecordingMode)}
                  class="flex flex-col gap-1"
                >
                  {#each RECORDING_MODES as opt (opt.value)}
                    <label
                      class="flex cursor-pointer items-start gap-3 rounded-md px-2 py-1.5 transition-colors hover:bg-muted"
                    >
                      <RadioGroup.Item value={opt.value} id="rec-mode-{opt.value}" class="mt-0.5" />
                      <span class="flex flex-col gap-0.5">
                        <span class="text-sm font-medium text-foreground">{opt.label}</span>
                        <span class="text-xs text-muted-foreground">{opt.hint}</span>
                      </span>
                    </label>
                  {/each}
                </RadioGroup.Root>
              </div>
              {#if configStore.shortcuts.recordingMode === 'hands_free'}
                <div
                  class="flex items-center justify-between gap-4 rounded-md border border-border bg-card p-3"
                >
                  <div class="flex flex-1 flex-col gap-1">
                    <span class="text-sm font-medium text-foreground">Stop After Silence</span>
                    <span class="text-xs text-muted-foreground"
                      >How long a pause has to run before recording ends</span
                    >
                  </div>
                  <div class="flex items-center gap-3">
                    <input
                      type="range"
                      min={HANDS_FREE_SILENCE_MIN}
                      max={HANDS_FREE_SILENCE_MAX}
                      step="0.1"
                      class="accent-primary h-1 w-32 cursor-pointer"
                      aria-label="Seconds of silence before recording stops"
                      value={configStore.shortcuts.handsFreeSilenceSecs}
                      oninput={(e) => handleSilenceTimeoutChange(Number(e.currentTarget.value))}
                    />
                    <span class="text-muted-foreground w-9 text-right text-xs tabular-nums">
                      {configStore.shortcuts.handsFreeSilenceSecs.toFixed(1)}s
                    </span>
                  </div>
                </div>
              {/if}
              {#if isLinux}
                <div class="flex flex-col gap-3 rounded-md border border-border bg-card p-3">
                  <div class="flex flex-col gap-1">
                    <span class="text-sm font-medium text-foreground">Typing Tool</span>
                    <span class="text-xs text-muted-foreground">
                      Which tool sends the keystrokes. A pinned tool is tried first; the others
                      still follow it, so a missing one never stops you dictating.
                    </span>
                  </div>
                  <RadioGroup.Root
                    value={configStore.transcription.typingTool}
                    onValueChange={(v) => handleTypingToolChange(v as TypingTool)}
                    class="flex flex-col gap-1"
                  >
                    {#each TYPING_TOOLS as opt (opt.value)}
                      <label
                        class="flex cursor-pointer items-start gap-3 rounded-md px-2 py-1.5 transition-colors hover:bg-muted"
                      >
                        <RadioGroup.Item
                          value={opt.value}
                          id="typing-tool-{opt.value}"
                          class="mt-0.5"
                        />
                        <span class="flex flex-col gap-0.5">
                          <span class="text-sm font-medium text-foreground">{opt.label}</span>
                          <span class="text-xs text-muted-foreground">{opt.hint}</span>
                        </span>
                      </label>
                    {/each}
                  </RadioGroup.Root>
                </div>
              {/if}
              <div class="row-separator"></div>
              <div
                class="flex items-center justify-between gap-4 rounded-md border border-border bg-card p-3"
              >
                <div class="flex flex-1 flex-col gap-1">
                  <span class="text-sm font-medium text-foreground">Sound Feedback</span>
                  <span class="text-xs text-muted-foreground"
                    >Play sounds when recording starts and stops</span
                  >
                </div>
                <Switch
                  checked={soundStore.isEnabled}
                  disabled={soundStore.isLoading}
                  onCheckedChange={() => soundStore.toggle()}
                />
              </div>
              {#if soundStore.isEnabled}
                <div
                  class="flex items-center justify-between gap-4 rounded-md border border-border bg-card p-3"
                >
                  <div class="flex flex-1 flex-col gap-1">
                    <span class="text-sm font-medium text-foreground">Sound Volume</span>
                    <span class="text-xs text-muted-foreground"
                      >How loud the start and stop cues are</span
                    >
                  </div>
                  <div class="flex items-center gap-3">
                    <input
                      type="range"
                      min="0"
                      max="100"
                      step="5"
                      class="accent-primary h-1 w-32 cursor-pointer"
                      aria-label="Sound volume"
                      value={Math.round(soundStore.volume * 100)}
                      oninput={(e) =>
                        soundStore.setVolume(Number(e.currentTarget.value) / 100)}
                    />
                    <span class="text-muted-foreground w-9 text-right text-xs tabular-nums">
                      {Math.round(soundStore.volume * 100)}%
                    </span>
                  </div>
                </div>
              {/if}
              <div class="row-separator"></div>
              <div
                class="flex items-center justify-between gap-4 rounded-md border border-border bg-card p-3"
              >
                <div class="flex flex-1 flex-col gap-1">
                  <span class="text-sm font-medium text-foreground">Recording Indicator</span>
                  <span class="text-xs text-muted-foreground"
                    >Show floating indicator during recording</span
                  >
                </div>
                <Switch
                  checked={configStore.general.showRecordingIndicator}
                  onCheckedChange={async (checked) => {
                    configStore.updateGeneral('showRecordingIndicator', checked);
                    await configStore.save();

                    try {
                      if (checked) {
                        if (pipelineStore.isRecording) {
                          await invoke('show_recording_indicator');
                        }
                      } else {
                        await invoke('hide_recording_indicator');
                      }
                    } catch (e) {
                      console.error('Failed to update indicator visibility:', e);
                    }
                  }}
                />
              </div>
              {#if configStore.general.showRecordingIndicator}
                <div class="row-separator"></div>
                <div
                  class="flex items-center justify-between gap-4 rounded-md border border-border bg-card p-3"
                >
                  <div class="flex flex-1 flex-col gap-1">
                    <span class="text-sm font-medium text-foreground">Indicator Style</span>
                    <span class="text-xs text-muted-foreground"
                      >Visual appearance of the recording indicator</span
                    >
                  </div>
                </div>
                <div class="indicator-style-selector">
                  <button
                    class="mode-option"
                    class:active={configStore.general.indicatorStyle === 'cursor-dot'}
                    onclick={() => handleIndicatorStyleChange('cursor-dot')}
                  >
                    <div class="mode-preview">
                      <svg viewBox="0 0 80 52" fill="none" xmlns="http://www.w3.org/2000/svg">
                        <!-- Cursor arrow -->
                        <path
                          d="M20 8 L20 36 L26 30 L32 40 L36 38 L30 28 L38 28 Z"
                          fill="var(--muted-foreground)"
                          stroke="var(--background)"
                          stroke-width="1.5"
                        />
                        <!-- Glowing dot -->
                        <circle cx="52" cy="22" r="10" fill="var(--primary)" opacity="0.2" />
                        <circle cx="52" cy="22" r="7" fill="var(--primary)" opacity="0.4" />
                        <circle cx="52" cy="22" r="4.5" fill="var(--primary)" />
                        <!-- Mic icon inside dot -->
                        <rect x="50.5" y="18" width="3" height="5" rx="1.5" fill="white" />
                        <path
                          d="M49.5 22.5 C49.5 24 50.5 25 52 25 C53.5 25 54.5 24 54.5 22.5"
                          stroke="white"
                          stroke-width="0.8"
                          fill="none"
                        />
                        <line x1="52" y1="25" x2="52" y2="26.5" stroke="white" stroke-width="0.8" />
                      </svg>
                    </div>
                    <span class="mode-title">Cursor Dot</span>
                    <span class="mode-description">Follows your mouse cursor</span>
                  </button>
                  <button
                    class="mode-option"
                    class:active={configStore.general.indicatorStyle === 'fixed-float'}
                    onclick={() => handleIndicatorStyleChange('fixed-float')}
                  >
                    <div class="mode-preview">
                      <svg viewBox="0 0 80 52" fill="none" xmlns="http://www.w3.org/2000/svg">
                        <!-- Screen outline -->
                        <rect
                          x="10"
                          y="6"
                          width="60"
                          height="36"
                          rx="3"
                          stroke="var(--muted-foreground)"
                          stroke-width="1.5"
                          fill="none"
                        />
                        <!-- Screen stand -->
                        <line
                          x1="32"
                          y1="42"
                          x2="48"
                          y2="42"
                          stroke="var(--muted-foreground)"
                          stroke-width="1.5"
                          stroke-linecap="round"
                        />
                        <line
                          x1="40"
                          y1="42"
                          x2="40"
                          y2="46"
                          stroke="var(--muted-foreground)"
                          stroke-width="1.5"
                        />
                        <line
                          x1="34"
                          y1="46"
                          x2="46"
                          y2="46"
                          stroke="var(--muted-foreground)"
                          stroke-width="1.5"
                          stroke-linecap="round"
                        />
                        <!-- Fixed dot in top-right corner of screen -->
                        <circle cx="59" cy="16" r="6" fill="var(--primary)" opacity="0.3" />
                        <circle cx="59" cy="16" r="4" fill="var(--primary)" />
                        <!-- Mic icon inside dot -->
                        <rect x="58" y="13.5" width="2" height="3.5" rx="1" fill="white" />
                        <path
                          d="M57.2 16.5 C57.2 17.5 58 18.2 59 18.2 C60 18.2 60.8 17.5 60.8 16.5"
                          stroke="white"
                          stroke-width="0.6"
                          fill="none"
                        />
                        <line x1="59" y1="18.2" x2="59" y2="19" stroke="white" stroke-width="0.6" />
                      </svg>
                    </div>
                    <span class="mode-title">Fixed Float</span>
                    <span class="mode-description">Stays at a fixed screen position</span>
                  </button>
                  <button
                    class="mode-option"
                    class:active={configStore.general.indicatorStyle === 'pill'}
                    onclick={() => handleIndicatorStyleChange('pill')}
                  >
                    <div class="mode-preview">
                      <svg viewBox="0 0 80 52" fill="none" xmlns="http://www.w3.org/2000/svg">
                        <!-- Pill shape -->
                        <rect x="8" y="16" width="64" height="20" rx="10" fill="var(--primary)" />
                        <!-- Mic icon on left side -->
                        <rect x="17" y="22" width="4" height="6" rx="2" fill="white" />
                        <path
                          d="M15.5 27 C15.5 29 17 30.5 19 30.5 C21 30.5 22.5 29 22.5 27"
                          stroke="white"
                          stroke-width="1"
                          fill="none"
                        />
                        <line x1="19" y1="30.5" x2="19" y2="32" stroke="white" stroke-width="1" />
                        <!-- Waveform bars -->
                        <rect
                          x="28"
                          y="24"
                          width="2.5"
                          height="4"
                          rx="1"
                          fill="white"
                          opacity="0.45"
                        />
                        <rect
                          x="33"
                          y="22"
                          width="2.5"
                          height="8"
                          rx="1"
                          fill="white"
                          opacity="0.5"
                        />
                        <rect
                          x="38"
                          y="20"
                          width="2.5"
                          height="12"
                          rx="1"
                          fill="white"
                          opacity="0.55"
                        />
                        <rect
                          x="43"
                          y="23"
                          width="2.5"
                          height="6"
                          rx="1"
                          fill="white"
                          opacity="0.65"
                        />
                        <rect
                          x="48"
                          y="19"
                          width="2.5"
                          height="14"
                          rx="1"
                          fill="white"
                          opacity="0.75"
                        />
                        <rect
                          x="53"
                          y="21"
                          width="2.5"
                          height="10"
                          rx="1"
                          fill="white"
                          opacity="0.85"
                        />
                        <rect
                          x="58"
                          y="23"
                          width="2.5"
                          height="6"
                          rx="1"
                          fill="white"
                          opacity="0.95"
                        />
                        <rect x="63" y="24" width="2.5" height="4" rx="1" fill="white" />
                      </svg>
                    </div>
                    <span class="mode-title">Pill Bar</span>
                    <span class="mode-description">Waveform bar at top of screen</span>
                  </button>
                </div>
              {/if}
            </div>
          </section>

          <!-- Text Insertion Section -->
          <section class="flex flex-col">
            <div class="mb-3">
              <h2 class="text-base font-semibold text-foreground m-0">Text Insertion</h2>
              <p class="text-xs text-muted-foreground m-0">
                What happens after a transcription reaches your cursor
              </p>
            </div>
            <div class="flex flex-col gap-2">
              <div
                class="flex items-center justify-between gap-4 rounded-md border border-border bg-card p-3"
              >
                <div class="flex flex-1 flex-col gap-1">
                  <span class="text-sm font-medium text-foreground">Append Trailing Space</span>
                  <span class="text-xs text-muted-foreground"
                    >Add a space after the text, so consecutive dictations into the same field stay
                    word-spaced</span
                  >
                </div>
                <Switch
                  checked={configStore.transcription.appendTrailingSpace}
                  onCheckedChange={async (checked) => {
                    configStore.updateTranscription('appendTrailingSpace', checked);
                    await configStore.save();
                  }}
                />
              </div>
              <div class="row-separator"></div>
              <div
                class="flex items-center justify-between gap-4 rounded-md border border-border bg-card p-3"
              >
                <div class="flex flex-1 flex-col gap-1">
                  <span class="text-sm font-medium text-foreground">Auto-Submit</span>
                  <span class="text-xs text-muted-foreground"
                    >Send a key combination after inserting, to submit the message. Only fires when
                    the insertion succeeded.</span
                  >
                </div>
                <select
                  class="border-border bg-card text-foreground rounded-md border px-2 py-1 text-sm"
                  aria-label="Auto-submit key combination"
                  value={configStore.transcription.autoSubmit}
                  onchange={async (e) => {
                    configStore.updateTranscription(
                      'autoSubmit',
                      e.currentTarget.value as AutoSubmit
                    );
                    await configStore.save();
                  }}
                >
                  <option value="off">Off</option>
                  <option value="enter">Enter</option>
                  <option value="ctrl_enter">Ctrl+Enter</option>
                  <option value="cmd_enter">Cmd+Enter</option>
                </select>
              </div>
            </div>
          </section>
        </div>
      {:else if activePane === 'models'}
        <div class="pane">
          <section class="flex flex-col">
            <div class="mb-3">
              <h2 class="text-base font-semibold text-foreground m-0">Speech Recognition</h2>
              <p class="text-xs text-muted-foreground m-0">
                Local models for transcribing speech to text
              </p>
            </div>
            <div class="flex flex-col gap-2">
              <ModelManager />
            </div>
          </section>
          <section class="flex flex-col">
            <div class="mb-3">
              <h2 class="text-base font-semibold text-foreground m-0">Memory</h2>
              <p class="text-xs text-muted-foreground m-0">
                What the loaded model does while you are not dictating
              </p>
            </div>
            <div
              class="flex items-center justify-between gap-4 rounded-md border border-border bg-card p-3"
            >
              <div class="flex flex-1 flex-col gap-1">
                <span class="text-sm font-medium text-foreground">Unload when idle</span>
                <span class="text-xs text-muted-foreground"
                  >Free the model's memory after a spell without dictation. The next dictation
                  reloads it, which costs a few seconds.</span
                >
              </div>
              <select
                class="border-border bg-card text-foreground rounded-md border px-2 py-1 text-sm"
                aria-label="Unload the model when idle"
                value={String(configStore.transcription.modelIdleUnloadSecs ?? 'never')}
                onchange={async (e) => {
                  const chosen = e.currentTarget.value;
                  configStore.updateTranscription(
                    'modelIdleUnloadSecs',
                    chosen === 'never' ? null : Number(chosen)
                  );
                  await configStore.save();
                }}
              >
                <option value="never">Never</option>
                <option value="300">After 5 minutes</option>
                <option value="900">After 15 minutes</option>
                <option value="1800">After 30 minutes</option>
                <option value="3600">After 1 hour</option>
              </select>
            </div>
          </section>
        </div>
      {:else if activePane === 'ai'}
        <div class="pane">
          <section class="flex flex-col">
            <div class="mb-3">
              <h2 class="text-base font-semibold text-foreground m-0">AI Enhancement</h2>
              <p class="text-xs text-muted-foreground m-0">
                Configure AI-powered text enhancement using Ollama for grammar correction,
                formatting, and more.
              </p>
            </div>
            <div class="flex flex-col gap-2">
              <AIEnhancementSettings />
            </div>
          </section>
        </div>
      {:else if activePane === 'history'}
        <div class="pane history-pane">
          <HistoryPane />
        </div>
      {:else if activePane === 'dictionary'}
        <div class="pane dictionary-pane">
          <section class="flex min-h-0 flex-1 flex-col">
            <div class="mb-3">
              <h2 class="text-base font-semibold text-foreground m-0">Dictionary</h2>
              <p class="text-xs text-muted-foreground m-0">
                Add words to help recognition and set up automatic replacements
              </p>
            </div>
            <div
              class="mb-3 flex items-center justify-between gap-4 rounded-md border border-border bg-card p-3"
            >
              <div class="flex flex-1 flex-col gap-1">
                <span class="text-sm font-medium text-foreground">Use these words while listening</span>
                <span class="text-xs text-muted-foreground"
                  >Give your words to the speech model as it decodes, so an unusual name is heard
                  correctly rather than corrected afterwards. Whisper models only.</span
                >
              </div>
              <Switch
                checked={configStore.transcription.vocabularyBias}
                onCheckedChange={async (checked) => {
                  configStore.updateTranscription('vocabularyBias', checked);
                  await configStore.save();
                }}
              />
            </div>
            <DictionaryEditor />
          </section>
        </div>
      {:else if activePane === 'transcribe'}
        <div class="pane">
          <section class="flex flex-col">
            <div class="mb-3">
              <h2 class="text-base font-semibold text-foreground m-0">Import Audio Files</h2>
              <p class="text-xs text-muted-foreground m-0">Transcribe existing audio files</p>
            </div>
            <div class="flex flex-col gap-2">
              <TranscribePane />
            </div>
          </section>
          <section class="flex flex-col">
            <div class="mb-3">
              <h2 class="text-base font-semibold text-foreground m-0">Output Filtering</h2>
              <p class="text-xs text-muted-foreground m-0">
                Configure how transcription output is processed
              </p>
            </div>
            <div class="flex flex-col gap-2">
              <FilterSettings
                initialOptions={{
                  remove_fillers: configStore.transcription.removeFillers,
                  normalise_whitespace: configStore.transcription.normaliseWhitespace,
                  cleanup_punctuation: configStore.transcription.cleanupPunctuation,
                  sentence_case: configStore.transcription.sentenceCase,
                  australian_spelling: configStore.transcription.australianSpelling,
                  spoken_numbers_to_digits: configStore.transcription.spokenNumbersToDigits,
                  voice_formatting_commands: configStore.transcription.voiceFormattingCommands,
                }}
                onchange={handleFilterChange}
                onOpenDictionary={() => (activePane = 'dictionary')}
              />
            </div>
          </section>
        </div>
      {:else if activePane === 'storage'}
        <div class="pane">
          <StoragePane />
        </div>
      {:else if activePane === 'integrations'}
        <div class="pane">
          <IntegrationsSettings />
        </div>
      {/if}
    </main>
  </div>
</div>

<AboutDialog open={showAbout} onclose={() => (showAbout = false)} />

<!--
  Decides for itself whether to open: it asks the backend, which answers only
  when the running version's notes have not been seen. Mounted here rather than
  in the recorder or indicator windows so it appears once, in the window a user
  actually reads, and never over a recording.
-->
<WhatsNewDialog />

<style>
  /* Settings window layout - component-specific styles only */
  .settings-window {
    display: flex;
    flex-direction: column;
    width: 100%;
    height: 100vh;
    background: var(--background);
  }

  /* Title bar */
  .title-bar {
    position: relative;
    display: flex;
    align-items: center;
    justify-content: center;
    height: 54px;
    background-color: var(--background);
    border-bottom: 1px solid var(--border);
    flex-shrink: 0;
    -webkit-app-region: drag;
    app-region: drag;
    -webkit-user-select: none;
    user-select: none;
  }

  .title-text {
    font-size: 0.8125rem;
    font-weight: 500;
    color: var(--foreground);
    user-select: none;
  }

  /* Pinned to the right edge so the centred title stays centred. */
  .title-bar-controls {
    position: absolute;
    right: 8px;
    top: 50%;
    transform: translateY(-50%);
    display: flex;
    align-items: center;
  }

  .main-area {
    display: flex;
    flex: 1;
    min-height: 0;
  }

  /* Sidebar */
  .sidebar {
    width: 220px;
    flex-shrink: 0;
    display: flex;
    flex-direction: column;
    padding-top: 12px;
    border-right: 1px solid var(--border);
  }

  .sidebar-items {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 0 12px;
  }

  /* Override Button ghost to match sidebar item appearance */
  :global(.sidebar-item) {
    display: flex !important;
    align-items: center !important;
    gap: 10px !important;
    width: 100% !important;
    padding: 8px 12px !important;
    border-radius: 6px !important;
    color: var(--muted-foreground) !important;
    font-size: 0.8125rem !important;
    font-weight: 400 !important;
    text-align: left !important;
    justify-content: flex-start !important;
    height: auto !important;
  }

  :global(.sidebar-item:hover) {
    background-color: rgba(255, 255, 255, 0.07) !important;
    color: var(--foreground) !important;
  }

  :global(.sidebar-item.active) {
    background-color: rgba(255, 255, 255, 0.1) !important;
    color: var(--foreground) !important;
    font-weight: 500 !important;
  }

  .sidebar-icon {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 18px;
    flex-shrink: 0;
    color: inherit;
  }

  .sidebar-label {
    color: inherit;
  }

  .sidebar-footer {
    margin-top: auto;
    padding: 12px;
    border-top: 1px solid var(--border);
  }

  /* Content area */
  .content {
    flex: 1;
    min-width: 0;
    overflow-y: auto;
    padding: 24px 32px 48px 32px;
  }

  .pane {
    display: flex;
    flex-direction: column;
    gap: 24px;
  }

  .indicator-style-selector {
    display: flex;
    gap: 12px;
    margin-bottom: 16px;
    margin-top: 8px;
  }

  .mode-option {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 16px;
    background: var(--card);
    border: 2px solid var(--border);
    border-radius: var(--radius-md);
    cursor: pointer;
    text-align: left;
    transition:
      border-color 150ms ease,
      background 150ms ease;
  }

  .indicator-style-selector .mode-option {
    align-items: center;
    text-align: center;
    gap: 6px;
    padding: 12px 12px 14px;
  }

  .mode-preview {
    width: 100%;
    max-width: 80px;
    aspect-ratio: 80 / 52;
  }

  .mode-preview svg {
    width: 100%;
    height: 100%;
  }

  .mode-option:hover {
    border-color: var(--muted-foreground);
  }

  .mode-option.active {
    border-color: var(--primary);
    background: color-mix(in srgb, var(--primary) 5%, transparent);
  }

  .mode-title {
    font-size: 0.8125rem;
    font-weight: 600;
    color: var(--foreground);
  }

  .mode-option.active .mode-title {
    color: var(--primary);
  }

  .mode-description {
    font-size: 0.75rem;
    color: var(--muted-foreground);
    line-height: 1.4;
  }

  /* Shortcuts */
  .shortcuts-list {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .shortcut-status {
    font-size: 0.75rem;
  }

  .shortcut-status.enabled {
    color: var(--chart-2);
  }

  /* History pane: remove content padding so the 3-panel layout fills the space */
  .content:has(.history-pane) {
    padding: 0;
    overflow: hidden;
  }

  .history-pane {
    height: 100%;
  }

  /* Dictionary pane fills the content height; the table scrolls internally
     while the page keeps a consistent 32px gap on the sides and bottom. */
  .content:has(.dictionary-pane) {
    overflow: hidden;
    padding-bottom: 32px;
  }

  .dictionary-pane {
    height: 100%;
    min-height: 0;
  }

  /* Recording indicator */
  .recording-indicator {
    display: flex;
    align-items: center;
    gap: 6px;
    position: absolute;
    right: 16px;
    font-size: 0.75rem;
    font-weight: 500;
    color: var(--destructive);
  }

  .recording-dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--destructive);
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

  .processing-indicator {
    position: absolute;
    right: 16px;
    font-size: 0.75rem;
    font-weight: 500;
    color: var(--primary);
  }
</style>
