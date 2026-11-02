<script lang="ts">
  import type { Snippet } from 'svelte';

  interface Props {
    children?: Snippet;
  }

  let { children }: Props = $props();
</script>

<!--
  This layout uses @. to reset the layout chain.
  It does NOT import app.css - needed for transparent window background.
-->

{#if children}
  {@render children()}
{/if}

<style>
  :global(html),
  :global(body) {
    margin: 0;
    padding: 0;
    background: transparent !important;
    overflow: hidden;
  }
</style>
