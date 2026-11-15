<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { listen, emit, type UnlistenFn } from '@tauri-apps/api/event';
  import { invoke } from '@tauri-apps/api/core';
  import type { IndicatorStyle } from '#lib/stores/config.svelte.js';

  // Helper to emit logs to main window (since this window's console is separate)
  function indicatorLog(...args: unknown[]) {
    const message = args.map(a => typeof a === 'object' ? JSON.stringify(a) : String(a)).join(' ');
    console.log('[Indicator]', message);
    emit('indicator-log', { message: `[Indicator] ${message}` }).catch(() => {});
  }

  indicatorLog('===== PAGE SCRIPT EXECUTING =====');

  // State
  let visualizerState = $state<'idle' | 'recording' | 'processing'>('idle');
  let audioLevel = $state(0);
  let indicatorStyle = $state<IndicatorStyle>('cursor-dot');

  // Animation state
  let canvas: HTMLCanvasElement;
  let ctx: CanvasRenderingContext2D | null = null;
  let animationFrame: number | null = null;

  // Smoothed glow intensity
  let glowIntensity = 0;

  // Processing animation
  let processingPhase = 0;

  // Waveform history for pill style (circular buffer)
  const WAVEFORM_BARS = 32;
  let waveformHistory: number[] = new Array(WAVEFORM_BARS).fill(0);
  let waveformIndex = 0;
  let waveformUpdateCounter = 0;

  // Style dimensions
  const DOT_SIZE = 58;
  const PILL_W = 280;
  const PILL_H = 44;
  const ICON_SIZE = 34;
  const ICON_RADIUS = 9;

  // Derived dimensions based on style
  let canvasWidth = $derived(indicatorStyle === 'pill' ? PILL_W : DOT_SIZE);
  let canvasHeight = $derived(indicatorStyle === 'pill' ? PILL_H : DOT_SIZE);

  // Accent colour (Scribe's Amber)
  const ACCENT = { r: 208, g: 139, b: 62 }; // #D08B3E

  // Event listeners
  let unlisteners: UnlistenFn[] = [];

  onMount(async () => {
    indicatorLog('===== onMount CALLED =====');
    ctx = canvas.getContext('2d');
    setupCanvas();
    animate();

    // Listen for style changes from backend
    const styleUnlisten = await listen<IndicatorStyle>('indicator-style', (event) => {
      indicatorLog('Indicator style changed to:', event.payload);
      indicatorStyle = event.payload;
      // Re-setup canvas for new dimensions
      setupCanvas();
    });
    unlisteners.push(styleUnlisten);

    // Listen for the test event from backend
    const shownUnlisten = await listen('indicator-shown', (event) => {
      indicatorLog('===== RECEIVED indicator-shown TEST EVENT =====', event.payload);
    });
    unlisteners.push(shownUnlisten);

    // Listen for pipeline progress
    const progressUnlisten = await listen<{ state: string; message: string; deviceName?: string }>(
      'pipeline-progress',
      (event) => {
        const state = event.payload.state;
        indicatorLog('Pipeline state:', state);
        if (state === 'recording') {
          visualizerState = 'recording';
        } else if (
          state === 'transcribing' ||
          state === 'filtering' ||
          state === 'enhancing' ||
          state === 'outputting'
        ) {
          visualizerState = 'processing';
        } else {
          visualizerState = 'idle';
        }
        indicatorLog('Visualizer state now:', visualizerState);
      }
    );
    unlisteners.push(progressUnlisten);

    // Listen for audio levels
    indicatorLog('Setting up audio level listener...');
    try {
      const levelUnlisten = await listen<{ rms: number; peak: number }>(
        'recording-audio-level',
        (event) => {
          // Map the raw level to a visible 0-1 range. Real microphones —
          // especially line-level USB mics like the DJI MIC MINI — produce a
          // low-amplitude signal: normal speech RMS sits around 0.005-0.03 and
          // peaks around 0.03-0.10. A small linear multiplier left every bar
          // pinned at the minimum height, which is why the waveform looked
          // dead. We use the more responsive `peak`, apply a perceptual
          // (square-root) curve so quiet speech still moves the bars without
          // loud speech clipping instantly, and scale so typical speech fills
          // a healthy fraction of the bar.
          const peak = event.payload.peak;
          audioLevel = Math.min(1, Math.sqrt(peak) * 2.2);

          // Update waveform history for pill style
          waveformUpdateCounter++;
          if (waveformUpdateCounter % 2 === 0) {
            waveformHistory[waveformIndex] = audioLevel;
            waveformIndex = (waveformIndex + 1) % WAVEFORM_BARS;
          }
        }
      );
      unlisteners.push(levelUnlisten);
      indicatorLog('===== Audio level listener REGISTERED SUCCESSFULLY =====');

      // Emit ready event to signal backend we're ready to receive audio levels
      await emit('indicator-ready', {});
      indicatorLog('===== EMITTED indicator-ready EVENT =====');
    } catch (err) {
      indicatorLog('FAILED to register audio level listener:', err);
    }

    // Listen for completion
    const completeUnlisten = await listen('pipeline-complete', () => {
      visualizerState = 'idle';
    });
    unlisteners.push(completeUnlisten);
  });

  onDestroy(() => {
    if (animationFrame) {
      cancelAnimationFrame(animationFrame);
    }
    for (const unlisten of unlisteners) {
      unlisten();
    }
  });

  function setupCanvas() {
    const dpr = window.devicePixelRatio || 1;
    const w = indicatorStyle === 'pill' ? PILL_W : DOT_SIZE;
    const h = indicatorStyle === 'pill' ? PILL_H : DOT_SIZE;
    canvas.width = w * dpr;
    canvas.height = h * dpr;
    canvas.style.width = `${w}px`;
    canvas.style.height = `${h}px`;
    ctx = canvas.getContext('2d');
    ctx?.scale(dpr, dpr);
  }

  let animateCount = 0;
  function animate() {
    animateCount++;
    if (animateCount % 60 === 0) {
      indicatorLog('animate() heartbeat, state:', visualizerState, 'style:', indicatorStyle, 'audioLevel:', audioLevel.toFixed(3));
    }

    if (!ctx) {
      animationFrame = requestAnimationFrame(animate);
      return;
    }

    const w = indicatorStyle === 'pill' ? PILL_W : DOT_SIZE;
    const h = indicatorStyle === 'pill' ? PILL_H : DOT_SIZE;

    // Clear canvas
    ctx.clearRect(0, 0, w, h);

    if (indicatorStyle === 'pill') {
      drawPill(w, h);
    } else {
      drawDot(w, h);
    }

    animationFrame = requestAnimationFrame(animate);
  }

  // ─── Dot / Fixed-Float Rendering ───────────────────────────────────

  function drawDot(w: number, h: number) {
    const iconX = (w - ICON_SIZE) / 2;
    const iconY = (h - ICON_SIZE) / 2;

    if (visualizerState === 'recording') {
      const targetGlow = Math.min(1, audioLevel * 2);
      glowIntensity += (targetGlow - glowIntensity) * 0.2;
      drawDotGlow(w, h, iconX, iconY);
      drawRoundedSquare(iconX, iconY);
      drawMicIcon(w, h);
    } else if (visualizerState === 'processing') {
      processingPhase += 0.04;
      const pulse = Math.sin(processingPhase * Math.PI) * 0.5 + 0.5;
      drawRoundedSquare(iconX, iconY, 0.6 + pulse * 0.4);
      drawMicIcon(w, h, 0.6 + pulse * 0.4);
    } else {
      drawRoundedSquare(iconX, iconY);
      drawMicIcon(w, h);
    }
  }

  function drawDotGlow(w: number, h: number, iconX: number, iconY: number) {
    if (!ctx || glowIntensity < 0.05) return;

    const spread = 4 + glowIntensity * 10;
    const alpha = 0.15 + glowIntensity * 0.35;

    ctx.save();
    ctx.shadowColor = `rgba(${ACCENT.r}, ${ACCENT.g}, ${ACCENT.b}, ${alpha})`;
    ctx.shadowBlur = spread;
    ctx.shadowOffsetX = 0;
    ctx.shadowOffsetY = 0;

    ctx.beginPath();
    ctx.roundRect(iconX, iconY, ICON_SIZE, ICON_SIZE, ICON_RADIUS);
    ctx.fillStyle = `rgba(${ACCENT.r}, ${ACCENT.g}, ${ACCENT.b}, 0.01)`;
    ctx.fill();
    ctx.restore();
  }

  function drawRoundedSquare(x: number, y: number, opacity: number = 1) {
    if (!ctx) return;
    ctx.beginPath();
    ctx.roundRect(x, y, ICON_SIZE, ICON_SIZE, ICON_RADIUS);
    ctx.fillStyle = `rgba(${ACCENT.r}, ${ACCENT.g}, ${ACCENT.b}, ${opacity})`;
    ctx.fill();
  }

  function drawMicIcon(w: number, h: number, opacity: number = 1) {
    if (!ctx) return;

    const cx = w / 2;
    const cy = h / 2;

    ctx.save();
    ctx.translate(cx, cy);

    const scale = 20 / 24;
    ctx.scale(scale, scale);
    ctx.translate(0, -1);

    const white = `rgba(255, 255, 255, ${opacity})`;

    // Mic body
    ctx.beginPath();
    ctx.roundRect(-3, -8, 6, 12, 3);
    ctx.fillStyle = white;
    ctx.fill();

    // Pickup arc
    ctx.beginPath();
    ctx.arc(0, 0, 7, 0, Math.PI, false);
    ctx.strokeStyle = white;
    ctx.lineWidth = 2;
    ctx.lineCap = 'round';
    ctx.stroke();

    // Stand line
    ctx.beginPath();
    ctx.moveTo(0, 7);
    ctx.lineTo(0, 10);
    ctx.stroke();

    // Base
    ctx.beginPath();
    ctx.moveTo(-4, 10);
    ctx.lineTo(4, 10);
    ctx.stroke();

    ctx.restore();
  }

  // ─── Pill Rendering ────────────────────────────────────────────────

  function drawPill(w: number, h: number) {
    if (!ctx) return;

    const radius = h / 2;
    const micAreaWidth = 40;

    if (visualizerState === 'recording') {
      // Background pill shape
      drawPillBackground(w, h, radius, 0.85);
      // Waveform bars
      drawWaveformBars(w, h, micAreaWidth);
      // Mic icon on the left
      drawPillMicIcon(h, micAreaWidth);
    } else if (visualizerState === 'processing') {
      processingPhase += 0.04;
      const pulse = Math.sin(processingPhase * Math.PI) * 0.5 + 0.5;
      drawPillBackground(w, h, radius, 0.6 + pulse * 0.25);
      drawPillProcessingDots(w, h);
      drawPillMicIcon(h, micAreaWidth, 0.6 + pulse * 0.4);
    } else {
      drawPillBackground(w, h, radius);
      drawPillMicIcon(h, micAreaWidth);
    }
  }

  function drawPillBackground(w: number, h: number, radius: number, opacity: number = 1) {
    if (!ctx) return;
    ctx.beginPath();
    ctx.roundRect(0, 0, w, h, radius);
    ctx.fillStyle = `rgba(${ACCENT.r}, ${ACCENT.g}, ${ACCENT.b}, ${opacity})`;
    ctx.fill();
  }

  function drawPillMicIcon(h: number, areaWidth: number, opacity: number = 1) {
    if (!ctx) return;

    const cx = areaWidth / 2 + 4;
    const cy = h / 2;

    ctx.save();
    ctx.translate(cx, cy);

    const scale = 16 / 24;
    ctx.scale(scale, scale);
    ctx.translate(0, -1);

    const white = `rgba(255, 255, 255, ${opacity})`;

    // Mic body
    ctx.beginPath();
    ctx.roundRect(-3, -8, 6, 12, 3);
    ctx.fillStyle = white;
    ctx.fill();

    // Pickup arc
    ctx.beginPath();
    ctx.arc(0, 0, 7, 0, Math.PI, false);
    ctx.strokeStyle = white;
    ctx.lineWidth = 2;
    ctx.lineCap = 'round';
    ctx.stroke();

    // Stand
    ctx.beginPath();
    ctx.moveTo(0, 7);
    ctx.lineTo(0, 10);
    ctx.stroke();

    // Base
    ctx.beginPath();
    ctx.moveTo(-4, 10);
    ctx.lineTo(4, 10);
    ctx.stroke();

    ctx.restore();
  }

  function drawWaveformBars(w: number, h: number, micAreaWidth: number) {
    if (!ctx) return;

    const barRegionStart = micAreaWidth + 4;
    const barRegionEnd = w - 16;
    const barRegionWidth = barRegionEnd - barRegionStart;
    const barWidth = 3;
    const barGap = (barRegionWidth - WAVEFORM_BARS * barWidth) / (WAVEFORM_BARS - 1);
    const maxBarHeight = h - 14;
    const cy = h / 2;

    for (let i = 0; i < WAVEFORM_BARS; i++) {
      // Read from circular buffer, oldest first
      const bufIndex = (waveformIndex + i) % WAVEFORM_BARS;
      const level = waveformHistory[bufIndex] ?? 0;

      // Minimum visible height + scaled height
      const barHeight = Math.max(4, level * maxBarHeight);
      const x = barRegionStart + i * (barWidth + barGap);
      const y = cy - barHeight / 2;

      // Fade bars from left (older) to right (newer)
      const ageFactor = 0.4 + (i / WAVEFORM_BARS) * 0.6;
      const alpha = ageFactor;

      ctx.beginPath();
      ctx.roundRect(x, y, barWidth, barHeight, barWidth / 2);
      ctx.fillStyle = `rgba(255, 255, 255, ${alpha})`;
      ctx.fill();
    }
  }

  function drawPillProcessingDots(w: number, h: number) {
    if (!ctx) return;

    const cy = h / 2;
    const cx = w / 2 + 10;
    const dotRadius = 3;
    const dotSpacing = 14;

    for (let i = 0; i < 3; i++) {
      const phase = processingPhase + i * 0.7;
      const bounce = Math.sin(phase * Math.PI) * 0.5 + 0.5;
      const y = cy - bounce * 6;
      const alpha = 0.5 + bounce * 0.5;

      ctx.beginPath();
      ctx.arc(cx + (i - 1) * dotSpacing, y, dotRadius, 0, Math.PI * 2);
      ctx.fillStyle = `rgba(255, 255, 255, ${alpha})`;
      ctx.fill();
    }
  }

  async function handleStop() {
    indicatorLog('Stop button clicked');
    try {
      await invoke('pipeline_cancel');
      indicatorLog('Pipeline cancel invoked');
    } catch (err) {
      indicatorLog('Failed to cancel pipeline:', err);
    }
  }
</script>

<!-- svelte-ignore a11y_click_events_have_key_events -->
<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
  class="indicator-container"
  class:pill={indicatorStyle === 'pill'}
  onclick={handleStop}
  style:width="{canvasWidth}px"
  style:height="{canvasHeight}px"
>
  <canvas bind:this={canvas} class="visualizer-canvas"></canvas>
</div>

<style>
  :global(html),
  :global(body) {
    margin: 0;
    padding: 0;
    background: transparent !important;
    overflow: hidden;
  }

  .indicator-container {
    display: flex;
    align-items: center;
    justify-content: center;
    background: transparent;
    cursor: pointer;
  }

  .visualizer-canvas {
    display: block;
    background: transparent;
  }
</style>
