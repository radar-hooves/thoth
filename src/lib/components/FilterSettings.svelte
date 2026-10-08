<script lang="ts">
  /**
   * FilterSettings - Configuration UI for transcription output filtering
   *
   * Provides toggles for each filter option with a live preview showing
   * the effect of filters on sample text.
   */

  import { invoke } from '@tauri-apps/api/core';
  import { Button } from '@poodle64/ui/button';
  import { Switch } from '@poodle64/ui/switch';
  import { Textarea } from '@poodle64/ui/textarea';
  import { Label } from '@poodle64/ui/label';
  import { configStore } from '../stores/config.svelte';

  /** Filter options matching the Rust FilterOptions struct */
  interface FilterOptions {
    remove_fillers: boolean;
    normalise_whitespace: boolean;
    cleanup_punctuation: boolean;
    sentence_case: boolean;
    australian_spelling: boolean;
    spoken_numbers_to_digits: boolean;
    voice_formatting_commands: boolean;
  }

  interface Props {
    /** Initial filter options, from the loaded transcription config */
    initialOptions: FilterOptions;
    /** Callback when options change */
    onchange?: (options: FilterOptions) => void;
    /** Callback to navigate to dictionary settings */
    onOpenDictionary?: () => void;
  }

  let { initialOptions, onchange, onOpenDictionary }: Props = $props();

  /** Current filter options state - intentionally captures initialOptions once */
  // svelte-ignore state_referenced_locally
  let options = $state<FilterOptions>(initialOptions);

  /**
   * Default options, from the backend. Rust owns them (TranscriptionConfig's
   * Default impl, reached through get_default_config); this component holds no
   * copy of the values.
   */
  let defaultOptions = $state<FilterOptions | null>(null);

  /** Load the backend's filter defaults into defaultOptions */
  async function loadDefaults() {
    try {
      const defaults = await configStore.loadDefaults();
      defaultOptions = {
        remove_fillers: defaults.transcription.removeFillers,
        normalise_whitespace: defaults.transcription.normaliseWhitespace,
        cleanup_punctuation: defaults.transcription.cleanupPunctuation,
        sentence_case: defaults.transcription.sentenceCase,
        australian_spelling: defaults.transcription.australianSpelling,
        spoken_numbers_to_digits: defaults.transcription.spokenNumbersToDigits,
        voice_formatting_commands: defaults.transcription.voiceFormattingCommands,
      };
    } catch (e) {
      // Without defaults there is nothing to reset to, so the reset button
      // never appears; the toggles keep working from the loaded config.
      console.error('Failed to load filter defaults:', e);
    }
  }

  void loadDefaults();

  /** Sample text for preview */
  let sampleText = $state(
    'um, so I think that uh, this is, you know, working...what do you think ??'
  );

  /** Whether the sample text input is expanded */
  let isSampleExpanded = $state(false);

  /** Filtered preview text */
  let filteredText = $state('');

  /** Whether preview is loading */
  let isLoading = $state(false);

  /** Error message if preview fails */
  let error = $state<string | null>(null);

  /**
   * Update the preview by calling the Rust filter_transcription command
   */
  async function updatePreview() {
    isLoading = true;
    error = null;

    try {
      filteredText = await invoke<string>('filter_transcription', {
        text: sampleText,
        options,
      });
    } catch (e) {
      error = `Failed to filter text: ${e}`;
      filteredText = sampleText;
    } finally {
      isLoading = false;
    }
  }

  /**
   * Toggle a filter option
   */
  function toggleOption(key: keyof FilterOptions, checked: boolean) {
    options = { ...options, [key]: checked };
    onchange?.(options);
  }

  /**
   * Reset all options to the backend's defaults
   */
  function resetToDefaults() {
    if (!defaultOptions) return;
    options = { ...defaultOptions };
    onchange?.(options);
  }

  /** Whether any option differs from the backend's defaults */
  let hasChanges = $derived.by(() => {
    const defaults = defaultOptions;
    if (!defaults) return false;
    return filterDefinitions.some((filter) => options[filter.key] !== defaults[filter.key]);
  });

  /** Filter option definitions for rendering */
  const filterDefinitions = [
    {
      key: 'remove_fillers' as const,
      label: 'Remove filler sounds',
      description: 'Removes hesitation sounds: um, uh, er, ah',
    },
    {
      key: 'normalise_whitespace' as const,
      label: 'Normalise whitespace',
      description: 'Collapses multiple spaces and trims leading/trailing whitespace',
    },
    {
      key: 'cleanup_punctuation' as const,
      label: 'Clean up punctuation',
      description: 'Removes duplicate punctuation and fixes spacing around punctuation',
    },
    {
      key: 'sentence_case' as const,
      label: 'Apply sentence case',
      description: 'Capitalises the first letter of each sentence',
    },
    {
      key: 'australian_spelling' as const,
      label: 'Australian spelling',
      description:
        'Converts US spellings to Australian/British equivalents (color→colour, organize→organise)',
    },
    {
      key: 'spoken_numbers_to_digits' as const,
      label: 'Convert spoken numbers to digits',
      description: 'Converts number words to digits (twenty three→23, one hundred→100)',
    },
    {
      key: 'voice_formatting_commands' as const,
      label: 'Voice formatting commands',
      description: "Say 'new paragraph' or 'new line' to insert line breaks",
    },
  ];

  // Update preview when options or sample text change
  $effect(() => {
    // Track all dependencies
    void options;
    void sampleText;
    updatePreview();
  });
</script>

<div class="flex flex-col gap-6">
  <div class="flex flex-col gap-3">
    <h3 class="text-sm font-semibold">Output Filters</h3>

    <div class="flex flex-col gap-2">
      {#each filterDefinitions as filter}
        <div class="flex items-center justify-between gap-4 rounded-md border border-border bg-card p-3 flex items-center justify-between gap-4">
          <div class="flex flex-1 flex-col gap-1">
            <span class="text-sm font-medium text-foreground">{filter.label}</span>
            <span class="text-xs text-muted-foreground">{filter.description}</span>
          </div>
          <Switch
            checked={options[filter.key]}
            onCheckedChange={(checked) => toggleOption(filter.key, checked)}
          />
        </div>
      {/each}
    </div>

    {#if hasChanges}
      <div>
        <Button variant="outline" size="sm" onclick={resetToDefaults}>Reset to defaults</Button>
      </div>
    {/if}
  </div>

  <div class="flex flex-col gap-3">
    <div class="flex items-center justify-between gap-3">
      <h3 class="text-sm font-semibold">Preview</h3>
      <Button
        variant="ghost"
        size="sm"
        onclick={() => (isSampleExpanded = !isSampleExpanded)}
        aria-expanded={isSampleExpanded}
      >
        {isSampleExpanded ? 'Hide sample input' : 'Edit sample input'}
      </Button>
    </div>

    {#if isSampleExpanded}
      <div class="flex flex-col gap-1.5">
        <Label for="sample-text" class="text-xs text-muted-foreground">Sample text:</Label>
        <Textarea
          id="sample-text"
          bind:value={sampleText}
          rows={3}
          placeholder="Enter sample text to test filters..."
          class="resize-y"
        />
      </div>
    {/if}

    <div class="flex items-stretch gap-3 rounded-lg border border-border/50 bg-muted/40 p-3">
      <div class="flex min-w-0 flex-1 flex-col gap-1.5">
        <span class="text-xs font-medium uppercase tracking-wide text-muted-foreground"
          >Before:</span
        >
        <p class="m-0 break-words text-sm leading-relaxed text-muted-foreground">{sampleText}</p>
      </div>

      <div class="flex shrink-0 items-center justify-center text-muted-foreground">
        <svg
          xmlns="http://www.w3.org/2000/svg"
          width="20"
          height="20"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
          stroke-linecap="round"
          stroke-linejoin="round"
        >
          <line x1="5" y1="12" x2="19" y2="12"></line>
          <polyline points="12 5 19 12 12 19"></polyline>
        </svg>
      </div>

      <div class="flex min-w-0 flex-1 flex-col gap-1.5">
        <span class="text-xs font-medium uppercase tracking-wide text-muted-foreground">After:</span
        >
        {#if isLoading}
          <p class="m-0 text-sm italic leading-relaxed text-muted-foreground">Processing...</p>
        {:else if error}
          <p class="m-0 text-xs leading-relaxed text-destructive">{error}</p>
        {:else}
          <p class="m-0 break-words text-sm leading-relaxed">{filteredText || '(empty)'}</p>
        {/if}
      </div>
    </div>
  </div>

  <div class="flex flex-col gap-3">
    <h3 class="text-sm font-semibold">Custom Word Replacements</h3>
    <p class="rounded-md bg-card p-2 text-xs text-muted-foreground">Configure custom word replacements in the Dictionary settings.</p>
    <div>
      <Button variant="outline" onclick={() => onOpenDictionary?.()}>
        Open Dictionary Settings
      </Button>
    </div>
  </div>
</div>
