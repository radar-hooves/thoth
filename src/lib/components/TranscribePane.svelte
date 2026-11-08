<script lang="ts">
  import { open } from '@tauri-apps/plugin-dialog';
  import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow';
  import { writeText } from '@tauri-apps/plugin-clipboard-manager';
  import { pipelineStore } from '../stores/pipeline.svelte';
  import { Button } from '@poodle64/ui/button';
  import * as Card from '@poodle64/ui/card';
  import { Alert, AlertDescription } from '@poodle64/ui/alert';
  import Upload from '@lucide/svelte/icons/upload';
  import FileAudio from '@lucide/svelte/icons/file-audio';
  import Copy from '@lucide/svelte/icons/copy';
  import Check from '@lucide/svelte/icons/check';
  import X from '@lucide/svelte/icons/x';
  import Loader2 from '@lucide/svelte/icons/loader-2';

  let isDragOver = $state(false);
  let importedFileName = $state<string | null>(null);
  let copied = $state(false);


  const isProcessing = $derived(
    importedFileName !== null &&
      (pipelineStore.state === 'converting' ||
        pipelineStore.state === 'transcribing' ||
        pipelineStore.state === 'filtering' ||
        pipelineStore.state === 'enhancing')
  );

  const hasResult = $derived(
    importedFileName !== null &&
      pipelineStore.state === 'completed' &&
      pipelineStore.lastResult !== null
  );

  const hasError = $derived(
    importedFileName !== null && pipelineStore.state === 'failed' && pipelineStore.error !== null
  );

  async function handleFilePicker() {
    if (isProcessing) return;

    // No extension filter: the backend decides by content and names what it cannot decode.
    const selected = await open({ multiple: false });

    if (selected) {
      importedFileName = selected.split('/').pop() ?? selected;
      pipelineStore.transcribeFile(selected);
    }
  }

  async function handleTranscribeFile(filePath: string) {
    importedFileName = filePath.split('/').pop() ?? filePath;
    await pipelineStore.transcribeFile(filePath);
  }

  async function handleCopy() {
    const text = pipelineStore.lastResult?.text;
    if (!text) return;

    try {
      await writeText(text);
      copied = true;
      setTimeout(() => {
        copied = false;
      }, 2000);
    } catch (e) {
      console.error('Failed to copy:', e);
    }
  }

  function handleCancel() {
    pipelineStore.cancel();
    importedFileName = null;
  }

  function handleReset() {
    pipelineStore.reset();
    importedFileName = null;
  }

  $effect(() => {
    let unlisten: (() => void) | undefined;

    getCurrentWebviewWindow()
      .onDragDropEvent((event) => {
        if (event.payload.type === 'enter' || event.payload.type === 'over') {
          isDragOver = true;
        } else if (event.payload.type === 'leave') {
          isDragOver = false;
        } else if (event.payload.type === 'drop') {
          isDragOver = false;
          const paths = event.payload.paths;
          if (paths.length > 0) {
            handleTranscribeFile(paths[0]);
          }
        }
      })
      .then((fn) => {
        unlisten = fn;
      });

    return () => {
      unlisten?.();
    };
  });
</script>

<!-- Custom drag-drop dropzone: bespoke because it uses Tauri's onDragDropEvent API which cannot be replaced by a shadcn primitive -->
<div
  role="button"
  tabindex="0"
  class="flex cursor-pointer flex-col items-center justify-center rounded-lg border-2 border-dashed px-6 py-12 transition-colors {isDragOver
    ? 'border-primary bg-primary/10 border-solid'
    : 'border-border hover:border-primary hover:bg-primary/5'} {isProcessing
    ? 'cursor-default opacity-80'
    : ''}"
  onclick={handleFilePicker}
  onkeydown={(e) => {
    if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault();
      handleFilePicker();
    }
  }}
>
  {#if isProcessing}
    <span class="text-muted-foreground mb-3">
      <Loader2 size={36} class="animate-spin" />
    </span>
    <p class="text-sm">{pipelineStore.message || 'Processing...'}</p>
    {#if importedFileName}
      <p class="text-muted-foreground mt-1 text-xs">{importedFileName}</p>
    {/if}
  {:else}
    <span class="text-muted-foreground mb-3">
      <Upload size={36} />
    </span>
    <p class="text-sm">Drop audio files here or click to browse</p>
    <p class="text-muted-foreground mt-1 text-xs">
      Any audio or video file: WAV, MP3, AAC, M4A, Opus, OGG, WebM, CAF, FLAC, MP4, MOV
    </p>
  {/if}
</div>

{#if isProcessing}
  <div class="mt-2 flex justify-center">
    <Button variant="outline" size="sm" onclick={handleCancel}>
      <X class="mr-1.5 h-3.5 w-3.5" />
      Cancel
    </Button>
  </div>
{/if}

{#if hasResult}
  <Card.Root class="mt-4">
    <Card.Header class="flex flex-row items-center justify-between space-y-0 pb-2">
      <div class="text-muted-foreground flex min-w-0 items-center gap-1.5 overflow-hidden text-sm">
        <FileAudio class="h-3.5 w-3.5 flex-shrink-0" />
        <span class="truncate">{importedFileName ?? 'Transcription'}</span>
      </div>
      <div class="flex flex-shrink-0 gap-2">
        <Button size="sm" onclick={handleCopy}>
          {#if copied}
            <Check class="mr-1.5 h-3.5 w-3.5" />
            Copied
          {:else}
            <Copy class="mr-1.5 h-3.5 w-3.5" />
            Copy
          {/if}
        </Button>
        <Button variant="outline" size="sm" onclick={handleReset}>New Import</Button>
      </div>
    </Card.Header>
    <Card.Content>
      <div
        class="text-foreground max-h-72 overflow-y-auto whitespace-pre-wrap break-words text-sm leading-relaxed"
      >
        {pipelineStore.lastResult?.text}
      </div>
    </Card.Content>
  </Card.Root>
{/if}

{#if hasError}
  <Alert variant="destructive" class="mt-4">
    <AlertDescription class="flex items-center justify-between gap-3">
      <span>{pipelineStore.error}</span>
      <Button variant="ghost" size="sm" onclick={handleReset}>Dismiss</Button>
    </AlertDescription>
  </Alert>
{/if}
