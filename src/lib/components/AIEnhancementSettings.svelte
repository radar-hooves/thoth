<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import { onMount } from 'svelte';
  import { configStore } from '../stores/config.svelte';
  import { toast } from 'svelte-sonner';
  import { Button } from '@poodle64/ui/button';
  import * as Select from '@poodle64/ui/select';
  import * as Dialog from '@poodle64/ui/dialog';
  import { Alert, AlertDescription } from '@poodle64/ui/alert';
  import * as Form from '@poodle64/ui/form';
  import { Select as SelectPrimitive } from 'bits-ui';
  import { Input } from '@poodle64/ui/input';
  import { Textarea } from '@poodle64/ui/textarea';
  import { Switch } from '@poodle64/ui/switch';
  import { Badge } from '@poodle64/ui/badge';
  import { Label } from '@poodle64/ui/label';
  import { LoadingState } from '$components/common';
  import AlertCircle from '@lucide/svelte/icons/alert-circle';
  import Eye from '@lucide/svelte/icons/eye';
  import EyeOff from '@lucide/svelte/icons/eye-off';
  import { superForm, defaults } from 'sveltekit-superforms';
  import { zod4 } from 'sveltekit-superforms/adapters';
  import { promptSchema } from '$lib/schemas/prompt';

  interface PromptTemplate {
    id: string;
    name: string;
    template: string;
    isBuiltin: boolean;
  }

  // Ollama state
  let ollamaAvailable = $state(false);
  let ollamaModels = $state<string[]>([]);
  let isCheckingOllama = $state(false);
  let isLoadingModels = $state(false);

  // OpenAI-compat state
  let openaiCompatAvailable = $state(false);
  let openaiCompatModels = $state<string[]>([]);
  let isCheckingOpenaiCompat = $state(false);
  let isLoadingOpenaiCompatModels = $state(false);
  let showApiKey = $state(false);

  // Shared state
  let prompts = $state<PromptTemplate[]>([]);
  let isLoadingPrompts = $state(false);
  let error = $state<string | null>(null);

  let isEditing = $state(false);
  let editingPrompt = $state<PromptTemplate | null>(null);

  const promptForm = superForm(defaults(zod4(promptSchema)), {
    SPA: true,
    validators: zod4(promptSchema),
    async onUpdate({ form: f }) {
      if (!f.valid) return;
      const prompt: PromptTemplate = {
        id: editingPrompt?.id || generateId(f.data.name),
        name: f.data.name.trim(),
        template: f.data.template.trim(),
        isBuiltin: false,
      };
      try {
        await invoke('save_custom_prompt_cmd', { prompt });
        await loadPrompts();
        isEditing = false;
        editingPrompt = null;
        invoke('refresh_tray_menu').catch(() => {});
      } catch (e) {
        toast.error(e instanceof Error ? e.message : 'Failed to save prompt');
      }
    },
  });

  const { form: formData, enhance, reset } = promptForm;

  /** Active backend derived from config */
  let activeBackend = $derived(configStore.config.enhancement.backend);

  async function checkOllama(): Promise<void> {
    isCheckingOllama = true;
    error = null;
    try {
      ollamaAvailable = await invoke<boolean>('check_ollama_available');
      if (ollamaAvailable) {
        await loadModels();
      }
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to check Ollama connection';
      ollamaAvailable = false;
    } finally {
      isCheckingOllama = false;
    }
  }

  async function loadModels(): Promise<void> {
    isLoadingModels = true;
    try {
      ollamaModels = await invoke<string[]>('list_ollama_models');
    } catch (e) {
      console.error('Failed to load Ollama models:', e);
      ollamaModels = [];
    } finally {
      isLoadingModels = false;
    }
  }

  async function checkOpenaiCompat(): Promise<void> {
    isCheckingOpenaiCompat = true;
    error = null;
    try {
      openaiCompatAvailable = await invoke<boolean>('check_openai_compat_available');
      if (openaiCompatAvailable) {
        await loadOpenaiCompatModels();
      }
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to check OpenAI-compatible connection';
      openaiCompatAvailable = false;
    } finally {
      isCheckingOpenaiCompat = false;
    }
  }

  async function loadOpenaiCompatModels(): Promise<void> {
    isLoadingOpenaiCompatModels = true;
    try {
      openaiCompatModels = await invoke<string[]>('list_openai_compat_models');
    } catch (e) {
      console.error('Failed to load OpenAI-compat models:', e);
      openaiCompatModels = [];
    } finally {
      isLoadingOpenaiCompatModels = false;
    }
  }

  async function loadPrompts(): Promise<void> {
    isLoadingPrompts = true;
    try {
      prompts = await invoke<PromptTemplate[]>('get_all_prompts');
    } catch (e) {
      console.error('Failed to load prompts:', e);
      prompts = [];
    } finally {
      isLoadingPrompts = false;
    }
  }

  async function saveSettings(): Promise<void> {
    try {
      await configStore.save();
      toast.success('Settings saved');
    } catch (e) {
      console.error('Failed to save settings:', e);
      toast.error('Failed to save settings');
    }
  }

  async function handleEnabledChange(checked: boolean): Promise<void> {
    configStore.updateEnhancement('enabled', checked);
    await saveSettings();
    invoke('refresh_tray_menu').catch(() => {});
  }

  async function handleBackendChange(value: string | undefined): Promise<void> {
    if (value === undefined) return;
    configStore.updateEnhancement('backend', value);
    error = null;
    await saveSettings();
    if (value === 'openai_compat') {
      await checkOpenaiCompat();
    } else {
      await checkOllama();
    }
  }

  async function handleModelChange(value: string | undefined): Promise<void> {
    if (value === undefined) return;
    configStore.updateEnhancement('model', value);
    await saveSettings();
  }

  async function handlePromptChange(value: string | undefined): Promise<void> {
    if (value === undefined) return;
    configStore.updateEnhancement('promptId', value);
    await saveSettings();
    invoke('refresh_tray_menu').catch(() => {});
  }

  function handleUrlInput(event: Event): void {
    const input = event.target as HTMLInputElement;
    configStore.updateEnhancement('ollamaUrl', input.value);
  }

  async function handleUrlBlur(): Promise<void> {
    await saveSettings();
    await checkOllama();
  }

  function handleOpenaiCompatUrlInput(event: Event): void {
    const input = event.target as HTMLInputElement;
    configStore.updateEnhancement('openaiCompatUrl', input.value);
  }

  async function handleOpenaiCompatUrlBlur(): Promise<void> {
    await saveSettings();
    await checkOpenaiCompat();
  }

  // Tracks whether the user actually edited the API-key field this session, so a
  // no-op blur (e.g. focusing and leaving the field untouched, or a blur before
  // the key has loaded) never persists an empty value and wipes a stored key.
  let apiKeyDirty = $state(false);

  function handleApiKeyInput(event: Event): void {
    // Keep local in-memory state in sync so the field reflects what the user typed,
    // but do NOT route through updateEnhancement + generic save (that path sends
    // api_key: null for an empty field, which the backend's preservation guard now
    // blocks from clearing a stored key). Persistence happens in handleApiKeyBlur.
    const input = event.target as HTMLInputElement;
    apiKeyDirty = true;
    configStore.updateEnhancement('apiKey', input.value || null);
  }

  async function handleApiKeyBlur(): Promise<void> {
    // Only persist when the user actually edited the field. The dedicated command
    // sets/clears unconditionally (bypassing the backend preservation guard so an
    // intentional clear works), so we must not call it on an untouched blur.
    if (!apiKeyDirty) return;
    apiKeyDirty = false;
    if (!(await configStore.setEnhancementApiKey(configStore.config.enhancement.apiKey))) {
      toast.error('Failed to save API key', { description: configStore.error ?? undefined });
    }
  }

  function handleOpenaiCompatModelInput(event: Event): void {
    const input = event.target as HTMLInputElement;
    configStore.updateEnhancement('model', input.value);
  }

  function startNewPrompt(): void {
    isEditing = true;
    editingPrompt = null;
    reset({
      data: { name: '', template: 'Enhance the following text:\n\n{text}' },
      keepMessage: false,
    });
  }

  function startEditPrompt(prompt: PromptTemplate): void {
    if (prompt.isBuiltin) return;
    isEditing = true;
    editingPrompt = prompt;
    reset({
      data: { name: prompt.name, template: prompt.template },
      keepMessage: false,
    });
  }

  function cancelEdit(): void {
    isEditing = false;
    editingPrompt = null;
    reset({ keepMessage: false });
  }

  function generateId(name: string): string {
    return name
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, '-')
      .replace(/^-|-$/g, '');
  }

  async function deletePrompt(promptId: string): Promise<void> {
    try {
      await invoke('delete_custom_prompt_cmd', { promptId });
      await loadPrompts();
      if (configStore.enhancement.promptId === promptId) {
        configStore.updateEnhancement('promptId', 'fix-grammar');
        await saveSettings();
      }
      invoke('refresh_tray_menu').catch(() => {});
    } catch (e) {
      console.error('Failed to delete prompt:', e);
    }
  }

  function getSelectedPrompt(): PromptTemplate | undefined {
    return prompts.find((p) => p.id === configStore.enhancement.promptId);
  }

  async function openPromptGuide(): Promise<void> {
    try {
      await invoke('show_window', { label: 'prompt-guide' });
    } catch (e) {
      console.error('Failed to open prompt guide:', e);
    }
  }

  /** Derive model select value — falls back to empty string so Select shows placeholder */
  let modelSelectValue = $derived(configStore.config.enhancement.model ?? '');
  let promptSelectValue = $derived(configStore.config.enhancement.promptId ?? '');
  let backendSelectValue = $derived(configStore.config.enhancement.backend ?? 'ollama');

  onMount(async () => {
    await configStore.load();
    await loadPrompts();
    if (configStore.config.enhancement.backend === 'openai_compat') {
      await checkOpenaiCompat();
    } else {
      await checkOllama();
    }
  });
</script>

<div class="flex flex-col gap-6">
  {#if configStore.isLoading}
    <LoadingState message="Loading settings..." />
  {:else}
    <!-- Enable toggle -->
    <div class="flex items-center justify-between gap-4 rounded-lg border bg-card px-4 py-3">
      <div class="flex flex-col gap-0.5">
        <Label class="text-sm font-medium">Enable AI enhancement</Label>
        <p class="text-xs text-muted-foreground">
          Use a local AI model to enhance transcriptions with grammar correction, formatting, and
          more
        </p>
      </div>
      <Switch
        checked={configStore.config.enhancement.enabled}
        onCheckedChange={handleEnabledChange}
      />
    </div>

    <!-- Provider selector -->
    <div class="flex flex-col gap-3">
      <h3 class="text-sm font-semibold text-foreground">Provider</h3>

      <div class="flex items-center justify-between gap-4 rounded-lg border bg-card px-4 py-3">
        <div class="flex flex-col gap-0.5">
          <Label class="text-sm font-medium">Backend</Label>
          <p class="text-xs text-muted-foreground">
            Ollama runs on this Mac. An OpenAI-compatible server goes wherever its URL points,
            local or remote.
          </p>
        </div>
        <Select.Root
          type="single"
          value={backendSelectValue}
          onValueChange={handleBackendChange}
          items={[
            { value: 'ollama', label: 'Ollama' },
            { value: 'openai_compat', label: 'OpenAI-compatible (LM Studio, llama.cpp, vLLM…)' },
          ]}
        >
          <Select.Trigger class="w-64">
            <SelectPrimitive.Value placeholder="Select backend…" />
          </Select.Trigger>
          <Select.Content>
            <Select.Item value="ollama" label="Ollama">Ollama</Select.Item>
            <Select.Item value="openai_compat" label="OpenAI-compatible (LM Studio, llama.cpp, vLLM…)"
              >OpenAI-compatible (LM Studio, llama.cpp, vLLM…)</Select.Item
            >
          </Select.Content>
        </Select.Root>
      </div>
    </div>

    {#if activeBackend === 'openai_compat'}
      <!-- OpenAI-compatible server settings -->
      <div class="flex flex-col gap-3">
        <h3 class="text-sm font-semibold text-foreground">OpenAI-Compatible Server</h3>

        <!-- Server URL -->
        <div class="flex flex-col gap-3 rounded-lg border bg-card px-4 py-3">
          <div class="flex flex-col gap-1">
            <Label class="text-sm font-medium">Server URL</Label>
            <p class="text-xs text-muted-foreground">
              Base URL of your local OpenAI-compatible server (e.g. LM Studio, llama.cpp, vLLM)
            </p>
          </div>
          <div class="flex gap-2">
            <Input
              type="url"
              class="flex-1 font-mono text-sm"
              value={configStore.config.enhancement.openaiCompatUrl}
              oninput={handleOpenaiCompatUrlInput}
              onblur={handleOpenaiCompatUrlBlur}
              placeholder="http://localhost:1234"
            />
            <Button
              variant="outline"
              onclick={checkOpenaiCompat}
              disabled={isCheckingOpenaiCompat}
            >
              {isCheckingOpenaiCompat ? 'Testing...' : 'Test Connection'}
            </Button>
          </div>
          <div class="flex items-center gap-2">
            {#if isCheckingOpenaiCompat}
              <Badge variant="secondary">Checking…</Badge>
            {:else if openaiCompatAvailable}
              <Badge
                class="border-green-500/20 bg-green-500/10 text-green-600 dark:text-green-400"
                variant="outline"
              >
                Connected
              </Badge>
            {:else}
              <Badge variant="destructive">Not connected — is your server running?</Badge>
            {/if}
          </div>
        </div>

        <!-- API key -->
        <div class="flex flex-col gap-3 rounded-lg border bg-card px-4 py-3">
          <div class="flex flex-col gap-1">
            <Label class="text-sm font-medium">API Key</Label>
            <p class="text-xs text-muted-foreground">
              Optional. Leave blank if your server does not require authentication.
            </p>
          </div>
          <div class="flex gap-2">
            <Input
              type={showApiKey ? 'text' : 'password'}
              class="flex-1 font-mono text-sm"
              value={configStore.config.enhancement.apiKey ?? ''}
              oninput={handleApiKeyInput}
              onblur={handleApiKeyBlur}
              placeholder="(none)"
              autocomplete="off"
            />
            <Button
              variant="outline"
              size="icon"
              onclick={() => (showApiKey = !showApiKey)}
              aria-label={showApiKey ? 'Hide API key' : 'Show API key'}
            >
              {#if showApiKey}
                <EyeOff class="size-4" />
              {:else}
                <Eye class="size-4" />
              {/if}
            </Button>
          </div>
        </div>

        {#if error}
          <Alert variant="destructive">
            <AlertCircle class="size-4" />
            <AlertDescription>{error}</AlertDescription>
          </Alert>
        {/if}

        <!-- Model (text input + chip picker) -->
        <div class="flex flex-col gap-3 rounded-lg border bg-card px-4 py-3">
          <div class="flex flex-col gap-1">
            <Label class="text-sm font-medium">Model</Label>
            <p class="text-xs text-muted-foreground">
              The model name your server should use. Type it directly or pick from the list if the
              server exposes one.
            </p>
          </div>
          <Input
            type="text"
            class="font-mono text-sm"
            value={configStore.config.enhancement.model}
            oninput={handleOpenaiCompatModelInput}
            onblur={saveSettings}
            placeholder="e.g. mistral, llama3, phi3"
          />
          {#if isLoadingOpenaiCompatModels}
            <p class="text-xs text-muted-foreground">Loading models from server…</p>
          {:else if openaiCompatAvailable && openaiCompatModels.length > 0}
            <div class="flex flex-col gap-1.5">
              <p class="text-xs text-muted-foreground">Available on server:</p>
              <div class="flex flex-wrap gap-1.5">
                {#each openaiCompatModels as model}
                  <button
                    class="rounded border px-2 py-0.5 text-xs transition-colors
                      {configStore.config.enhancement.model === model
                      ? 'border-primary/40 bg-primary/10 text-primary'
                      : 'border-border bg-transparent text-muted-foreground hover:bg-muted hover:text-foreground'}"
                    onclick={() => {
                      configStore.updateEnhancement('model', model);
                      saveSettings();
                    }}
                  >
                    {model}
                  </button>
                {/each}
              </div>
            </div>
          {/if}
        </div>
      </div>
    {:else}
      <!-- Ollama connection -->
      <div class="flex flex-col gap-3">
        <h3 class="text-sm font-semibold text-foreground">Ollama Connection</h3>

        <div class="flex flex-col gap-3 rounded-lg border bg-card px-4 py-3">
          <div class="flex flex-col gap-1">
            <Label class="text-sm font-medium">Server URL</Label>
            <p class="text-xs text-muted-foreground">The URL of your local Ollama server</p>
          </div>
          <div class="flex gap-2">
            <Input
              type="url"
              class="flex-1 font-mono text-sm"
              value={configStore.config.enhancement.ollamaUrl}
              oninput={handleUrlInput}
              onblur={handleUrlBlur}
              placeholder="http://localhost:11434"
            />
            <Button variant="outline" onclick={checkOllama} disabled={isCheckingOllama}>
              {isCheckingOllama ? 'Testing...' : 'Test Connection'}
            </Button>
          </div>
          <div class="flex items-center gap-2">
            {#if isCheckingOllama}
              <Badge variant="secondary">Checking…</Badge>
            {:else if ollamaAvailable}
              <Badge
                class="border-green-500/20 bg-green-500/10 text-green-600 dark:text-green-400"
                variant="outline"
              >
                Connected
              </Badge>
            {:else}
              <Badge variant="destructive">Not connected — is Ollama running?</Badge>
            {/if}
          </div>
        </div>

        {#if error}
          <Alert variant="destructive">
            <AlertCircle class="size-4" />
            <AlertDescription>{error}</AlertDescription>
          </Alert>
        {/if}

        <!-- Model selector -->
        <div class="flex items-center justify-between gap-4 rounded-lg border bg-card px-4 py-3">
          <div class="flex flex-col gap-0.5">
            <Label class="text-sm font-medium">Model</Label>
            <p class="text-xs text-muted-foreground">The Ollama model used for text enhancement</p>
          </div>
          <Select.Root
            type="single"
            value={modelSelectValue}
            onValueChange={handleModelChange}
            disabled={!ollamaAvailable || isLoadingModels}
            items={ollamaModels.length === 0
              ? [
                  {
                    value: configStore.config.enhancement.model,
                    label: `${configStore.config.enhancement.model} (not available)`,
                  },
                ]
              : ollamaModels.map((m) => ({ value: m, label: m }))}
          >
            <Select.Trigger class="w-48">
              <SelectPrimitive.Value
                placeholder={isLoadingModels
                  ? 'Loading models…'
                  : ollamaModels.length === 0
                    ? configStore.config.enhancement.model
                    : 'Select model…'}
              />
            </Select.Trigger>
            <Select.Content>
              {#if !isLoadingModels && ollamaModels.length === 0}
                <Select.Item
                  value={configStore.config.enhancement.model}
                  label="{configStore.config.enhancement.model} (not available)"
                >
                  {configStore.config.enhancement.model} (not available)
                </Select.Item>
              {:else}
                {#each ollamaModels as model}
                  <Select.Item value={model} label={model}>{model}</Select.Item>
                {/each}
              {/if}
            </Select.Content>
          </Select.Root>
        </div>

        {#if !ollamaAvailable}
          <p class="text-xs text-muted-foreground">
            Connect to Ollama to see available models. You can download models using
            <code class="rounded bg-muted px-1 py-0.5 font-mono text-xs"
              >ollama pull &lt;model&gt;</code
            >
            in your terminal.
          </p>
        {/if}
      </div>
    {/if}

    <!-- Prompt templates -->
    <div class="flex flex-col gap-3">
      <h3 class="text-sm font-semibold text-foreground">Prompt Templates</h3>

      <!-- Active prompt -->
      <div class="flex flex-col gap-3 rounded-lg border bg-card px-4 py-3">
        <div class="flex items-center justify-between gap-4">
          <div class="flex flex-col gap-0.5">
            <Label class="text-sm font-medium">Active Prompt</Label>
            <p class="text-xs text-muted-foreground">
              The prompt template used to enhance transcriptions
            </p>
          </div>
          <Select.Root
            type="single"
            value={promptSelectValue}
            onValueChange={handlePromptChange}
            disabled={isLoadingPrompts}
            items={prompts.map((p) => ({
              value: p.id,
              label: `${p.name}${p.isBuiltin ? '' : ' (Custom)'}`,
            }))}
          >
            <Select.Trigger class="w-48">
              <SelectPrimitive.Value
                placeholder={isLoadingPrompts ? 'Loading…' : 'Select prompt…'}
              />
            </Select.Trigger>
            <Select.Content>
              {#each prompts as prompt}
                <Select.Item
                  value={prompt.id}
                  label="{prompt.name}{prompt.isBuiltin ? '' : ' (Custom)'}"
                >
                  {prompt.name}{prompt.isBuiltin ? '' : ' (Custom)'}
                </Select.Item>
              {/each}
            </Select.Content>
          </Select.Root>
        </div>
        {#if getSelectedPrompt()}
          <pre
            class="whitespace-pre-wrap break-words rounded bg-muted/50 px-3 py-2 font-mono text-xs leading-relaxed text-muted-foreground">{getSelectedPrompt()
              ?.template}</pre>
        {/if}
      </div>

      <!-- Custom prompts -->
      <div class="flex flex-col gap-3">
        <div class="flex items-center justify-between">
          <span class="text-sm font-medium text-foreground">Custom Prompts</span>
          <Button variant="outline" size="sm" onclick={startNewPrompt}>+ Add Prompt</Button>
        </div>

        <p class="text-xs text-muted-foreground">
          Create your own prompt templates for specific use cases.
          <Button variant="link" class="h-auto p-0 text-xs" onclick={openPromptGuide}>
            View Prompt Writing Guide
          </Button>
          for tips on writing effective prompts.
        </p>

        <!-- Prompt editor dialog -->
        <Dialog.Root
          bind:open={isEditing}
          onOpenChange={(open) => {
            if (!open) cancelEdit();
          }}
        >
          <Dialog.Content class="max-w-lg">
            <Dialog.Header>
              <Dialog.Title>{editingPrompt ? 'Edit Prompt' : 'New Prompt'}</Dialog.Title>
              <Dialog.Description>
                Define a custom prompt template. Use <code
                  class="rounded bg-muted px-1 py-0.5 font-mono text-xs">{'{text}'}</code
                > as the transcription placeholder.
              </Dialog.Description>
            </Dialog.Header>

            <form method="POST" use:enhance class="flex flex-col gap-4 py-2">
              <Form.Field form={promptForm} name="name">
                {#snippet children({ constraints })}
                  <Form.Control>
                    {#snippet children({ props })}
                      <Form.Label>Name</Form.Label>
                      <Input
                        {...props}
                        {...constraints}
                        type="text"
                        bind:value={$formData.name}
                        placeholder="My Custom Prompt"
                      />
                    {/snippet}
                  </Form.Control>
                  <Form.FieldErrors />
                {/snippet}
              </Form.Field>

              <Form.Field form={promptForm} name="template">
                {#snippet children({ constraints })}
                  <Form.Control>
                    {#snippet children({ props })}
                      <Form.Label>
                        Template
                        <span class="ml-1 text-xs font-normal text-muted-foreground">
                          (use {'{text}'} as placeholder)
                        </span>
                      </Form.Label>
                      <Textarea
                        {...props}
                        {...constraints}
                        bind:value={$formData.template}
                        rows={5}
                        class="resize-y font-mono text-sm"
                        placeholder="Enhance this text: {'{text}'}"
                      />
                    {/snippet}
                  </Form.Control>
                  <Form.FieldErrors />
                {/snippet}
              </Form.Field>

              <Dialog.Footer>
                <Button type="button" variant="outline" onclick={cancelEdit}>Cancel</Button>
                <Button type="submit">
                  {editingPrompt ? 'Update' : 'Create'} Prompt
                </Button>
              </Dialog.Footer>
            </form>
          </Dialog.Content>
        </Dialog.Root>

        <!-- Custom prompts list -->
        <div class="flex flex-col gap-2">
          {#each prompts.filter((p) => !p.isBuiltin) as prompt}
            <div class="flex items-center justify-between rounded-lg bg-muted/40 px-3 py-2.5">
              <span class="text-sm font-medium">{prompt.name}</span>
              <div class="flex gap-1.5">
                <Button variant="ghost" size="sm" onclick={() => startEditPrompt(prompt)}>
                  Edit
                </Button>
                <Button
                  variant="ghost"
                  size="sm"
                  class="text-destructive hover:bg-destructive/10 hover:text-destructive"
                  onclick={() => deletePrompt(prompt.id)}
                >
                  Delete
                </Button>
              </div>
            </div>
          {:else}
            <p class="py-4 text-center text-sm text-muted-foreground">
              No custom prompts yet. Click "Add Prompt" to create one.
            </p>
          {/each}
        </div>
      </div>
    </div>
  {/if}
</div>
