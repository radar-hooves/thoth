<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { toast } from 'svelte-sonner';
  import { listen, type UnlistenFn } from '@tauri-apps/api/event';
  import { settingsStore } from '../stores/settings.svelte';
  import { Button } from '@poodle64/ui/button';
  import * as Select from '@poodle64/ui/select';
  import { Alert, AlertDescription } from '@poodle64/ui/alert';
  import { Select as SelectPrimitive } from 'bits-ui';
  import RefreshCw from '@lucide/svelte/icons/refresh-cw';
  import Mic from '@lucide/svelte/icons/mic';
  import MicOff from '@lucide/svelte/icons/mic-off';
  import AlertCircle from '@lucide/svelte/icons/alert-circle';
  import { bannerText, friendlyNameFromId } from '../audio-device';

  interface AudioLevelEvent {
    rms: number;
    peak: number;
  }

  const MIN_DB = -60;
  const MAX_DB = 0;

  let isPreviewActive = $state(false);
  let currentLevel = $state(0);
  let peakLevel = $state(0);
  let unlisten: UnlistenFn | null = null;

  const DEFAULT_VALUE = '__system_default__';

  let resolution = $derived(settingsStore.resolution);
  let banner = $derived(
    resolution ? bannerText(resolution, settingsStore.selectedDeviceId) : null
  );

  function linearToMeterPosition(linear: number): number {
    if (linear <= 0) return 0;
    const db = 20 * Math.log10(linear);
    const normalized = (db - MIN_DB) / (MAX_DB - MIN_DB);
    return Math.max(0, Math.min(1, normalized));
  }

  let meterLevel = $derived(linearToMeterPosition(currentLevel));
  let meterPeak = $derived(linearToMeterPosition(peakLevel));

  /** A null device id is the system default; the select needs a non-empty value to show a label. */
  let selectValue = $derived(settingsStore.selectedDeviceId ?? DEFAULT_VALUE);

  /** The saved device when it is not connected, so the select can still name it. */
  let missingDevice = $derived(
    resolution?.saved_missing && settingsStore.selectedDeviceId
      ? {
          id: settingsStore.selectedDeviceId,
          label: `${friendlyNameFromId(settingsStore.selectedDeviceId) ?? 'Saved microphone'} (not connected)`,
        }
      : null
  );

  onMount(async () => {
    await settingsStore.loadAudioDevices();
  });

  onDestroy(async () => {
    await stopPreview();
  });

  async function handleDeviceChange(value: string | undefined): Promise<void> {
    const resolved = value === DEFAULT_VALUE || value === undefined ? null : value;
    await settingsStore.selectAudioDevice(resolved);
    if (isPreviewActive) {
      await restartPreview();
    }
  }

  async function startPreview(): Promise<void> {
    if (isPreviewActive) return;
    try {
      unlisten = await listen<AudioLevelEvent>('audio-level', (event) => {
        currentLevel = event.payload.rms;
        peakLevel = event.payload.peak;
      });
      await invoke('start_audio_preview', {
        deviceId: settingsStore.selectedDeviceId,
      });
      isPreviewActive = true;
    } catch (e) {
      toast.error('Microphone test failed', {
        description: e instanceof Error ? e.message : String(e),
      });
      await stopPreview();
    }
  }

  async function stopPreview(): Promise<void> {
    if (unlisten) {
      unlisten();
      unlisten = null;
    }
    try {
      await invoke('stop_audio_preview');
    } catch (e) {
      console.error('Failed to stop audio preview:', e);
    }
    isPreviewActive = false;
    currentLevel = 0;
    peakLevel = 0;
  }

  async function togglePreview(): Promise<void> {
    if (isPreviewActive) {
      await stopPreview();
    } else {
      await startPreview();
    }
  }

  async function restartPreview(): Promise<void> {
    await stopPreview();
    await startPreview();
  }

  async function refreshDevices(): Promise<void> {
    await settingsStore.loadAudioDevices();
    if (!settingsStore.error) {
      const n = settingsStore.audioDevices.length;
      toast.success(`Devices refreshed: ${n} input ${n === 1 ? 'device' : 'devices'} found`);
    }
  }

  /** Build the display label for the system default option */
  let defaultOptionLabel = $derived.by(() => {
    const def = settingsStore.audioDevices.find((d) => d.is_default);
    return def ? `System Default (${def.name})` : 'System Default';
  });
</script>

<div class="flex flex-col gap-5">
  <!-- Device status banner -->
  {#if banner}
  <div
    class="flex items-center gap-3 rounded-lg border px-4 py-3 {banner.ok
      ? 'border-green-500/40 bg-green-500/5'
      : 'border-yellow-500/40 bg-yellow-500/5'}"
  >
    <div class="shrink-0 size-5 {banner.ok ? 'text-green-500' : 'text-yellow-500'}">
      {#if banner.ok}
        <Mic class="size-5" />
      {:else}
        <MicOff class="size-5" />
      {/if}
    </div>
    <div class="flex flex-col gap-0.5 flex-1 min-w-0">
      <span
        class="text-xs font-semibold uppercase tracking-wide {banner.ok
          ? 'text-green-500'
          : 'text-yellow-500'}"
      >
        {banner.title}
      </span>
      <span class="text-sm font-medium text-foreground">{banner.detail}</span>
    </div>
  </div>
  {/if}

  <!-- Device selector -->
  <div class="flex flex-col gap-2">
    <label class="text-sm font-medium text-muted-foreground" for="audio-device-select">
      Input Device
    </label>
    <div class="flex gap-2 items-center">
      {#key `${settingsStore.audioDevices.length}:${missingDevice?.id}`}
        <Select.Root
          type="single"
          value={selectValue}
          onValueChange={handleDeviceChange}
          disabled={settingsStore.isLoadingDevices}
          items={[
            { value: DEFAULT_VALUE, label: defaultOptionLabel },
            ...(missingDevice ? [{ value: missingDevice.id, label: missingDevice.label }] : []),
            ...settingsStore.audioDevices.map((d) => ({
              value: d.id,
              label: `${d.name}${d.is_default ? ' (Default)' : ''}`,
            })),
          ]}
        >
          <Select.Trigger id="audio-device-select" class="flex-1">
            <SelectPrimitive.Value placeholder="Select device…" />
          </Select.Trigger>
          <Select.Content>
            <Select.Item value={DEFAULT_VALUE} label={defaultOptionLabel}>
              {defaultOptionLabel}
            </Select.Item>
            {#if missingDevice}
              <Select.Item value={missingDevice.id} label={missingDevice.label}>
                {missingDevice.label}
              </Select.Item>
            {/if}
            {#each settingsStore.audioDevices as device (device.id)}
              <Select.Item
                value={device.id}
                label="{device.name}{device.is_default ? ' (Default)' : ''}"
              >
                {device.name}{device.is_default ? ' (Default)' : ''}
              </Select.Item>
            {/each}
          </Select.Content>
        </Select.Root>
      {/key}
      <Button
        variant="ghost"
        size="icon"
        onclick={refreshDevices}
        disabled={settingsStore.isLoadingDevices}
        title="Refresh device list"
      >
        <RefreshCw class="size-4 {settingsStore.isLoadingDevices ? 'animate-spin' : ''}" />
      </Button>
    </div>
  </div>

  <!-- Audio preview section -->
  <div class="flex flex-col gap-3 rounded-lg bg-muted/40 p-4">
    <div class="flex items-center justify-between">
      <span class="text-sm font-medium text-muted-foreground">Audio Preview</span>
      <Button variant={isPreviewActive ? 'default' : 'secondary'} size="sm" onclick={togglePreview}>
        {isPreviewActive ? 'Stop' : 'Test'}
      </Button>
    </div>

    <!-- Level meter — kept bespoke (custom audio metering logic) -->
    <div class="level-meter flex flex-col gap-1">
      <div class="relative h-2 rounded-full bg-muted overflow-hidden">
        <div
          class="absolute inset-y-0 left-0 rounded-full transition-[width] duration-[50ms] {isPreviewActive
            ? 'meter-fill-active'
            : 'bg-muted-foreground/20'}"
          style:width="{meterLevel * 100}%"
        ></div>
        <div
          class="absolute top-0 w-0.5 h-full bg-foreground transition-[left,opacity] duration-[50ms] {isPreviewActive &&
          meterPeak > 0.01
            ? 'opacity-80'
            : 'opacity-0'}"
          style:left="{meterPeak * 100}%"
        ></div>
      </div>
      <div class="flex justify-between text-xs text-muted-foreground px-0.5">
        <span>-60 dB</span>
        <span>-30 dB</span>
        <span>0 dB</span>
      </div>
    </div>

    {#if isPreviewActive}
      <p class="text-xs text-muted-foreground italic text-center">
        Speak into your microphone to see audio levels
      </p>
    {/if}
  </div>

  {#if settingsStore.error}
    <Alert variant="destructive">
      <AlertCircle class="size-4" />
      <AlertDescription>{settingsStore.error}</AlertDescription>
    </Alert>
  {/if}
</div>

<style>
  /* Gradient fill for active meter — cannot express multi-stop gradient via Tailwind utilities */
  .meter-fill-active {
    background: linear-gradient(
      90deg,
      var(--chart-2) 0%,
      var(--chart-2) 70%,
      var(--chart-4) 85%,
      var(--destructive) 100%
    );
  }
</style>
