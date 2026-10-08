<script lang="ts">
  import { onMount } from 'svelte';
  import { getVersion } from '@tauri-apps/api/app';
  import { invoke } from '@tauri-apps/api/core';
  import { writeText } from '@tauri-apps/plugin-clipboard-manager';
  import * as Dialog from '@poodle64/ui/dialog';
  import { Button } from '@poodle64/ui/button';
  import { About } from '@poodle64/ui/about';

  interface Props {
    open: boolean;
    onclose: () => void;
  }

  let { open, onclose }: Props = $props();

  let version = $state('');

  onMount(async () => {
    try {
      version = await getVersion();
    } catch {
      version = '';
    }
  });

  const links = [
    { label: 'GitHub', href: 'https://github.com/radar-hooves/thoth' },
    { label: 'MIT Licence', href: 'https://github.com/radar-hooves/thoth/blob/main/LICENCE' },
    { label: 'poodle64', href: 'https://github.com/poodle64' },
    { label: 'nephalemsec', href: 'https://github.com/nephalemsec' }
  ];

  const diagnostics = {
    Platform: navigator.platform,
    'Built with': 'Tauri · Svelte · whisper.cpp · Sherpa-ONNX · Ollama'
  };

  // The webview does not open target=_blank itself; hand http(s) links to the OS.
  function openExternal(event: MouseEvent) {
    const link = (event.target as Element).closest('a[href^="http"]');
    if (!link) return;
    event.preventDefault();
    invoke('open_url', { url: link.getAttribute('href') }).catch((err) =>
      console.error('Failed to open URL:', err)
    );
  }
</script>

<Dialog.Root
  {open}
  onOpenChange={(v) => {
    if (!v) onclose();
  }}
>
  <Dialog.Content class="max-w-md" showCloseButton={false}>
    <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
    <div onclick={openExternal}>
      <About
        name="Thoth"
        {version}
        description="Scribe to the gods. Typist to you."
        {links}
        {diagnostics}
        writeClipboard={writeText}
      >
        {#snippet brand()}
          <span class="block text-center text-7xl leading-none">𓅝</span>
        {/snippet}
      </About>
    </div>
    <Dialog.Footer class="justify-center sm:justify-center">
      <Dialog.Close>
        {#snippet child({ props })}
          <Button variant="secondary" {...props} onclick={onclose}>Close</Button>
        {/snippet}
      </Dialog.Close>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>
