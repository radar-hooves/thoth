<script lang="ts">
  import '../app.css';
  import { onMount } from 'svelte';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import { Toaster } from '@poodle64/ui/sonner';
  import { ModeWatcher } from 'mode-watcher';
  import type { Snippet } from 'svelte';
  import { installTauriMock } from '$lib/dev/tauri-mock';
  import { thothMockCommands } from '$lib/dev/thoth-mock-data';

  // Install the mock transport synchronously, before any child component mounts
  // and before any store calls invoke() or listen(). The DEV guard is evaluated at
  // module-eval time (synchronous), so this runs before the component body and
  // before App.svelte's onMount. Vite tree-shakes both imports and this block from
  // production builds because import.meta.env.DEV is false at build time.
  // The __TAURI_INTERNALS__ guard ensures the real Tauri runtime (which injects
  // that global before app code) is never replaced by the mock.
  if (import.meta.env.DEV && !('__TAURI_INTERNALS__' in window)) {
    installTauriMock(thothMockCommands);
  }

  interface Props {
    children?: Snippet;
  }

  let { children }: Props = $props();

  onMount(async () => {
    try {
      await getCurrentWindow().setIcon('icons/128x128@2x.png');
    } catch {
      // Silently ignore — icon may already be set via bundle in production
    }
  });
</script>

<ModeWatcher defaultMode="dark" />

{#if children}
  {@render children()}
{/if}

<Toaster />
