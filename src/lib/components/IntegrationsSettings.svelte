<script lang="ts">
  import { onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { writeText } from '@tauri-apps/plugin-clipboard-manager';
  import { configStore, type PublishStatus, type TelemetryStatus } from '../stores/config.svelte';
  import { toast } from 'svelte-sonner';
  import { Switch } from '@poodle64/ui/switch';
  import { Button } from '@poodle64/ui/button';
  import { Input } from '@poodle64/ui/input';
  import * as AlertDialog from '@poodle64/ui/alert-dialog';
  import Eye from '@lucide/svelte/icons/eye';
  import EyeOff from '@lucide/svelte/icons/eye-off';
  import Copy from '@lucide/svelte/icons/copy';
  import Check from '@lucide/svelte/icons/check';
  import RotateCcw from '@lucide/svelte/icons/rotate-ccw';
  import RefreshCw from '@lucide/svelte/icons/refresh-cw';

  interface IntegrationsStatus {
    apiEnabled: boolean;
    apiRunning: boolean;
    apiPort: number;
    mcpEnabled: boolean;
    hasToken: boolean;
  }

  let status = $state<IntegrationsStatus>({
    apiEnabled: false,
    apiRunning: false,
    apiPort: 8765,
    mcpEnabled: false,
    hasToken: false,
  });

  /** The live exporter and the saved values behind it; null until loaded. */
  let telemetry = $state<TelemetryStatus | null>(null);
  let endpoint = $state('');
  let headersHelper = $state('');
  let isTesting = $state(false);
  let isSavingTelemetry = $state(false);
  /** The last Test result: the crate's own class, never a URL or a header. */
  let probe = $state<{ ok: boolean; message: string } | null>(null);

  let token = $state<string | null>(null);
  let tokenRevealed = $state(false);
  let copied = $state(false);
  let showRotateDialog = $state(false);

  // Word-list publishing. The password field is write-only: nothing loads it back.
  let publishStatus = $state<PublishStatus | null>(null);
  let publishPassword = $state('');
  let isSavingPublish = $state(false);
  let isPublishingNow = $state(false);

  async function refreshStatus(): Promise<void> {
    try {
      status = await invoke<IntegrationsStatus>('get_integrations_status');
    } catch (e) {
      console.error('Failed to refresh integrations status:', e);
    }
  }

  async function loadToken(): Promise<void> {
    try {
      token = await invoke<string | null>('get_api_token');
    } catch (e) {
      console.error('Failed to load API token:', e);
    }
  }

  async function handleApiToggle(enabled: boolean): Promise<void> {
    try {
      await invoke('set_api_enabled', { enabled });
      await refreshStatus();
      if (enabled && !token) {
        await loadToken();
      }
    } catch (e) {
      toast.error('Failed to update API server', {
        description: e instanceof Error ? e.message : String(e),
      });
    }
  }

  async function handleMcpToggle(enabled: boolean): Promise<void> {
    try {
      await invoke('set_mcp_enabled', { enabled });
      await refreshStatus();
    } catch (e) {
      toast.error('Failed to update MCP server', {
        description: e instanceof Error ? e.message : String(e),
      });
    }
  }

  async function handleCopyToken(): Promise<void> {
    if (!token) return;
    try {
      await writeText(token);
      copied = true;
      setTimeout(() => {
        copied = false;
      }, 2000);
    } catch (e) {
      toast.error('Failed to copy token');
    }
  }

  async function handleRotateToken(): Promise<void> {
    showRotateDialog = false;
    try {
      token = await invoke<string>('rotate_api_token');
      tokenRevealed = false;
      await refreshStatus();
      toast.success('API token rotated', {
        description: 'The old token is now invalid. Update any connected clients.',
      });
    } catch (e) {
      toast.error('Failed to rotate API token', {
        description: e instanceof Error ? e.message : String(e),
      });
    }
  }

  async function loadTelemetry(): Promise<void> {
    try {
      const next = await invoke<TelemetryStatus>('plugin:telemetry|telemetry_get');
      telemetry = next;
      // Where the environment owns the exporter, the fields show what it set.
      endpoint = next.fromEnv ? next.endpoint : next.savedEndpoint;
      headersHelper = next.fromEnv ? next.headersHelper : next.savedHeadersHelper;
    } catch (e) {
      console.error('Failed to load telemetry settings:', e);
    }
  }

  async function handleTestTelemetry(): Promise<void> {
    isTesting = true;
    probe = null;
    try {
      await invoke('plugin:telemetry|telemetry_probe', { endpoint, headersHelper });
      probe = { ok: true, message: 'Ok' };
    } catch (e) {
      probe = { ok: false, message: e instanceof Error ? e.message : String(e) };
    } finally {
      isTesting = false;
    }
  }

  async function handleSaveTelemetry(): Promise<void> {
    isSavingTelemetry = true;
    try {
      const next = await configStore.setTelemetry(endpoint, headersHelper);
      if (next) {
        telemetry = next;
        toast.success('Telemetry endpoint saved');
      } else {
        toast.error('Failed to save telemetry settings');
      }
    } finally {
      isSavingTelemetry = false;
    }
  }

  async function refreshPublishStatus(): Promise<void> {
    try {
      publishStatus = await invoke<PublishStatus>('get_publish_status');
    } catch (e) {
      console.error('Failed to load word-list publishing status:', e);
    }
  }

  /** Enable/disable saves immediately and publishes once on the way on. */
  async function handlePublishToggle(enabled: boolean): Promise<void> {
    configStore.updatePublish('enabled', enabled);
    if (!(await configStore.save())) {
      toast.error('Failed to save word-list publishing settings');
      return;
    }
    await refreshPublishStatus();
    if (enabled) {
      await runPublishNow();
    }
  }

  function handlePublishUrlInput(event: Event): void {
    const input = event.target as HTMLInputElement;
    configStore.updatePublish('url', input.value);
  }

  function handlePublishUsernameInput(event: Event): void {
    const input = event.target as HTMLInputElement;
    configStore.updatePublish('username', input.value);
  }

  /** Persist URL + username (+ password when typed) and publish right away. */
  async function handleSavePublish(): Promise<void> {
    isSavingPublish = true;
    try {
      configStore.updatePublish('url', configStore.publish.url.trim());
      configStore.updatePublish('username', configStore.publish.username.trim());
      if (!(await configStore.save())) {
        toast.error('Failed to save word-list publishing settings');
        return;
      }
      if (publishPassword !== '') {
        try {
          await invoke('set_publish_password', { password: publishPassword });
        } catch (e) {
          toast.error('Failed to store the password', {
            description: e instanceof Error ? e.message : String(e),
          });
          return;
        }
        publishPassword = '';
      }
      await refreshPublishStatus();
      if (configStore.publish.enabled) {
        await runPublishNow();
      } else {
        toast.success('Word list publishing settings saved');
      }
    } finally {
      isSavingPublish = false;
    }
  }

  async function runPublishNow(): Promise<void> {
    isPublishingNow = true;
    try {
      publishStatus = await invoke<PublishStatus>('publish_now');
    } catch (e) {
      toast.error('Word list publishing failed', {
        description: e instanceof Error ? e.message : String(e),
      });
    } finally {
      isPublishingNow = false;
    }
  }

  const publishStatusMessage = $derived.by(() => {
    if (!publishStatus) return null;
    if (publishStatus.lastError) return { ok: false as const, text: publishStatus.lastError };
    if (publishStatus.lastPublishAt) {
      const when = new Date(publishStatus.lastPublishAt).toLocaleString();
      return { ok: true as const, text: `Last published ${when}` };
    }
    return null;
  });

  const maskedToken = $derived(token ? '••••••••••••••••••••••••••••••••' : null);
  const displayToken = $derived(tokenRevealed ? token : maskedToken);

  onMount(async () => {
    await refreshStatus();
    await loadToken();
    await loadTelemetry();
    await refreshPublishStatus();
  });
</script>

<!-- Section 1: Local Control API -->
<section class="flex flex-col">
  <div class="mb-3">
    <h2 class="text-base font-semibold text-foreground m-0">Local Control API</h2>
    <p class="text-xs text-muted-foreground m-0">
      Let local automation and AI assistants control Thoth over a loopback HTTP API.
    </p>
  </div>
  <div class="flex flex-col gap-2">
    <!-- Enable API row -->
    <div
      class="flex items-center justify-between gap-4 rounded-md border border-border bg-card p-3"
    >
      <div class="flex flex-1 flex-col gap-1">
        <span class="text-sm font-medium text-foreground">Enable API server</span>
        <span class="text-xs text-muted-foreground flex items-center gap-1.5">
          {#if status.apiRunning}
            <span
              class="inline-block h-1.5 w-1.5 rounded-full bg-chart-2 flex-shrink-0"
              aria-hidden="true"
            ></span>
            <span class="text-chart-2 font-medium">Running</span>
            <span>on http://127.0.0.1:{status.apiPort}</span>
          {:else}
            Stopped
          {/if}
        </span>
      </div>
      <Switch checked={status.apiEnabled} onCheckedChange={handleApiToggle} />
    </div>

    <!-- Token management — only shown when API is enabled and token exists -->
    {#if status.apiEnabled && status.hasToken && token}
      <div class="flex flex-col gap-2 rounded-md border border-border bg-card p-3">
        <div class="flex flex-col gap-0.5">
          <span class="text-sm font-medium text-foreground">API token</span>
          <span class="text-xs text-muted-foreground">
            Clients authenticate with this bearer token. Keep it secret.
          </span>
        </div>
        <div class="flex items-center gap-2 mt-1">
          <Input
            value={displayToken ?? ''}
            readonly
            class="font-mono text-xs flex-1 bg-muted"
            aria-label="API token"
          />
          <Button
            variant="outline"
            size="icon"
            onclick={() => (tokenRevealed = !tokenRevealed)}
            aria-label={tokenRevealed ? 'Hide token' : 'Reveal token'}
            class="flex-shrink-0"
          >
            {#if tokenRevealed}
              <EyeOff size={14} />
            {:else}
              <Eye size={14} />
            {/if}
          </Button>
          <Button
            variant="outline"
            size="icon"
            onclick={handleCopyToken}
            aria-label="Copy token"
            class="flex-shrink-0"
          >
            {#if copied}
              <Check size={14} class="text-chart-2" />
            {:else}
              <Copy size={14} />
            {/if}
          </Button>
        </div>
        <div class="flex mt-1">
          <Button
            variant="outline"
            size="sm"
            onclick={() => (showRotateDialog = true)}
            class="gap-1.5"
          >
            <RotateCcw size={13} />
            Rotate token
          </Button>
        </div>
      </div>
    {/if}
  </div>
</section>

<!-- Section 2: MCP Server -->
<section class="flex flex-col">
  <div class="mb-3">
    <h2 class="text-base font-semibold text-foreground m-0">MCP Server</h2>
    <p class="text-xs text-muted-foreground m-0">
      Expose Thoth's tools to MCP-capable assistants (Claude, etc.). Consumes the Local Control API.
    </p>
  </div>
  <div class="flex flex-col gap-2">
    <div
      class="flex items-center justify-between gap-4 rounded-md border border-border bg-card p-3"
    >
      <div class="flex flex-1 flex-col gap-1">
        <span class="text-sm font-medium text-foreground">Enable MCP server</span>
        {#if status.mcpEnabled && status.apiRunning}
          <span class="text-xs text-chart-2">
            Serving at http://127.0.0.1:{status.apiPort}/mcp
          </span>
        {:else}
          <span class="text-xs text-muted-foreground">
            Make Thoth's tools available to Claude and other MCP clients.
          </span>
        {/if}
      </div>
      <Switch checked={status.mcpEnabled} onCheckedChange={handleMcpToggle} />
    </div>

    {#if status.mcpEnabled && !status.apiEnabled}
      <p class="text-xs text-muted-foreground px-1">
        Enable the Local Control API above to start serving the MCP endpoint.
      </p>
    {/if}
  </div>
</section>

<!-- Section 3: Telemetry -->
<section class="flex flex-col">
  <div class="mb-3">
    <h2 class="text-base font-semibold text-foreground m-0">Telemetry</h2>
    <p class="text-xs text-muted-foreground m-0">
      Send Thoth's own logs and traces to a collector you run. Nothing you dictate goes with them.
    </p>
  </div>
  <div class="flex flex-col gap-2">
    <div class="flex flex-col gap-3 rounded-md border border-border bg-card p-3">
      <div class="flex flex-col gap-0.5">
        <span class="text-sm font-medium text-foreground">Endpoint</span>
        <span class="text-xs text-muted-foreground">
          The collector's OTLP/HTTP base;
          <code class="rounded bg-muted px-1 py-0.5 font-mono text-xs">/v1/logs</code>
          and
          <code class="rounded bg-muted px-1 py-0.5 font-mono text-xs">/v1/traces</code>
          are appended.
        </span>
        <Input
          type="url"
          bind:value={endpoint}
          disabled={telemetry?.fromEnv ?? false}
          placeholder="https://otlp.example"
          class="font-mono text-xs mt-1"
          aria-label="Telemetry endpoint"
        />
      </div>

      <div class="flex flex-col gap-0.5">
        <span class="text-sm font-medium text-foreground">Authorisation helper</span>
        <span class="text-xs text-muted-foreground">
          A command that prints a JSON object of headers. Thoth runs it and sends what it prints,
          so the bearer never lands in Thoth's settings.
        </span>
        <Input
          bind:value={headersHelper}
          disabled={telemetry?.fromEnv ?? false}
          placeholder="a command that prints JSON headers"
          class="font-mono text-xs mt-1"
          aria-label="Authorisation helper command"
        />
      </div>

      {#if telemetry}
        {#if telemetry.endpoint && !telemetry.started}
          <p class="text-xs text-status-error m-0" role="alert">
            An endpoint is configured but the exporter did not start. Nothing is being sent.
          </p>
        {:else if telemetry.started}
          <p class="text-xs text-muted-foreground m-0">
            Exporter started. Use Test to check the collector answers.
          </p>
        {:else}
          <p class="text-xs text-muted-foreground m-0">Off: no endpoint is set.</p>
        {/if}
      {/if}

      {#if telemetry?.fromEnv}
        <p class="text-xs text-muted-foreground m-0">Set by this machine's environment.</p>
      {/if}

      <div class="flex items-center gap-2">
        <Button
          variant="outline"
          size="sm"
          onclick={handleTestTelemetry}
          disabled={isTesting || endpoint.trim() === ''}
        >
          {isTesting ? 'Testing…' : 'Test'}
        </Button>
        {#if !telemetry?.fromEnv}
          <Button size="sm" onclick={handleSaveTelemetry} disabled={isSavingTelemetry}>
            {isSavingTelemetry ? 'Saving…' : 'Save'}
          </Button>
        {/if}
        {#if probe}
          <span
            class="text-xs flex items-center gap-1.5 {probe.ok
              ? 'text-status-success'
              : 'text-status-error'}"
            role="status"
          >
            <span
              class="inline-block h-1.5 w-1.5 rounded-full flex-shrink-0 {probe.ok
                ? 'bg-status-success'
                : 'bg-status-error'}"
              aria-hidden="true"
            ></span>
            {probe.message}
          </span>
        {/if}
      </div>
    </div>
  </div>
</section>

<!-- Section 4: Word list publishing -->
<section class="flex flex-col">
  <div class="mb-3">
    <h2 class="text-base font-semibold text-foreground m-0">Word list publishing</h2>
    <p class="text-xs text-muted-foreground m-0">
      Write your dictionary and canonical terms to a file on a WebDAV server
      (Nextcloud, ownCloud), so other tools can read the same corrections.
    </p>
  </div>
  <div class="flex flex-col gap-2">
    <div
      class="flex items-center justify-between gap-4 rounded-md border border-border bg-card p-3"
    >
      <div class="flex flex-1 flex-col gap-1">
        <span class="text-sm font-medium text-foreground">Enable word list publishing</span>
        <span class="text-xs text-muted-foreground">
          Off by default — the built-in local lists stay the whole experience until you
          turn this on.
        </span>
      </div>
      <Switch
        checked={configStore.publish.enabled}
        onCheckedChange={handlePublishToggle}
      />
    </div>

    <div class="flex flex-col gap-3 rounded-md border border-border bg-card p-3">
      <div class="flex flex-col gap-0.5">
        <span class="text-sm font-medium text-foreground">File URL</span>
        <span class="text-xs text-muted-foreground">
          The file's full WebDAV address, e.g.
          <code class="rounded bg-muted px-1 py-0.5 font-mono text-xs"
            >https://cloud.example/remote.php/dav/files/me/thoth-words.json</code
          >
        </span>
        <Input
          type="url"
          value={configStore.publish.url}
          oninput={handlePublishUrlInput}
          placeholder="https://your-nextcloud/remote.php/dav/files/you/thoth-words.json"
          class="font-mono text-xs mt-1"
          aria-label="Word list file URL"
        />
      </div>

      <div class="flex flex-col gap-0.5">
        <span class="text-sm font-medium text-foreground">Username</span>
        <Input
          value={configStore.publish.username}
          oninput={handlePublishUsernameInput}
          placeholder="you@example.com (empty for no auth)"
          class="mt-1"
          aria-label="WebDAV username"
        />
      </div>

      <div class="flex flex-col gap-0.5">
        <span class="text-sm font-medium text-foreground">Password</span>
        <span class="text-xs text-muted-foreground">
          {#if publishStatus?.hasPassword}
            Stored. Enter a new one to replace it.
          {:else}
            A Nextcloud app password (Settings → Security), not your login password.
          {/if}
        </span>
        <Input
          type="password"
          bind:value={publishPassword}
          placeholder={publishStatus?.hasPassword ? '••••••••' : 'App password'}
          class="mt-1"
          aria-label="WebDAV password"
        />
      </div>

      <div class="flex items-center gap-2">
        <Button
          size="sm"
          onclick={handleSavePublish}
          disabled={isSavingPublish || isPublishingNow}
        >
          {isSavingPublish ? 'Saving…' : 'Save'}
        </Button>
        <Button
          variant="outline"
          size="sm"
          onclick={runPublishNow}
          disabled={isPublishingNow || isSavingPublish}
        >
          <RefreshCw size={13} class={isPublishingNow ? 'animate-spin' : undefined} />
          {isPublishingNow ? 'Publishing…' : 'Publish now'}
        </Button>
        {#if publishStatusMessage}
          <span
            class="text-xs flex items-center gap-1.5 {publishStatusMessage.ok
              ? 'text-status-success'
              : 'text-status-error'}"
            role="status"
          >
            <span
              class="inline-block h-1.5 w-1.5 rounded-full flex-shrink-0 {publishStatusMessage.ok
                ? 'bg-status-success'
                : 'bg-status-error'}"
              aria-hidden="true"
            ></span>
            {publishStatusMessage.text}
          </span>
        {/if}
      </div>
    </div>
  </div>
</section>

<!-- Rotate token confirmation dialog -->
<AlertDialog.Root
  open={showRotateDialog}
  onOpenChange={(v) => {
    if (!v) showRotateDialog = false;
  }}
>
  <AlertDialog.Content>
    <AlertDialog.Header>
      <AlertDialog.Title>Rotate API token</AlertDialog.Title>
      <AlertDialog.Description>
        This generates a new token and immediately invalidates the current one. Any client using the
        old token will stop working until reconfigured.
      </AlertDialog.Description>
    </AlertDialog.Header>
    <AlertDialog.Footer>
      <AlertDialog.Cancel>Cancel</AlertDialog.Cancel>
      <AlertDialog.Action variant="destructive" onclick={handleRotateToken}>
        Rotate token
      </AlertDialog.Action>
    </AlertDialog.Footer>
  </AlertDialog.Content>
</AlertDialog.Root>
