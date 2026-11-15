<script lang="ts">
  import { Checkbox } from '@poodle64/ui/checkbox';
  import * as Dialog from '@poodle64/ui/dialog';
  // Imported through the barrel the migration introduced, not straight from the
  // package: the barrel is what every call site now resolves, so it is what the
  // test must exercise.
  import { EmptyState, ErrorState, LoadingState } from '#lib/components/common/index.js';

  let checked = $state(false);
  let emptyActionClicks = $state(0);
  let errorActionClicks = $state(0);
  let loadingMessage = $state('Loading dictionary…');
</script>

<Checkbox bind:checked aria-label="smoke-test-checkbox" />

<Dialog.Root>
  <Dialog.Trigger>Open dialog</Dialog.Trigger>
  <Dialog.Content>
    <Dialog.Title>Smoke test dialog</Dialog.Title>
  </Dialog.Content>
</Dialog.Root>

<EmptyState title="No transcriptions yet" description="Press the hotkey to dictate.">
  {#snippet action()}
    <button onclick={() => emptyActionClicks++}>Start recording</button>
  {/snippet}
</EmptyState>
<output data-testid="empty-action-clicks">{emptyActionClicks}</output>

<ErrorState message="Could not reach the model">
  {#snippet action()}
    <button onclick={() => errorActionClicks++}>Retry</button>
  {/snippet}
</ErrorState>
<output data-testid="error-action-clicks">{errorActionClicks}</output>

<LoadingState message={loadingMessage} />
<button onclick={() => (loadingMessage = 'Calculating storage usage…')}>Change loading message</button>
