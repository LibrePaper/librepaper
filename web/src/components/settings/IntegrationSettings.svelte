<script>
  import SettingRow from "./SettingRow.svelte";
  import * as localBridge from "../../lib/companion/client.js";
  import { companion } from "../../lib/companion/status.svelte.js";

  let { name = "quarto" } = $props();

  let settingsError = $state("");
  let pendingDialogAction = $state(false);
  let editingPath = $state("");
  let editingArgs = $state("");
  let savedPath = $state("");
  let savedArgs = $state([]);

  const local = $derived(companion.status);
  $effect(() => companion.watch());

  $effect(() => {
    if (local?.state === "connected") void loadSettings();
  });

  function quoteAwareJoin(args) {
    if (!Array.isArray(args)) return "";
    return args.map((arg) => {
      if (arg.includes(" ") || arg.includes('"') || arg.includes("'")) {
        return `"${arg.replace(/"/g, '\\"')}"`;
      }
      return arg;
    }).join(" ");
  }

  function quoteAwareSplit(text) {
    const args = [];
    let current = "";
    let inQuote = false;
    let quoteChar = "";
    let i = 0;
    while (i < text.length) {
      const ch = text[i];
      if ((ch === '"' || ch === "'") && (i === 0 || text[i - 1] !== "\\")) {
        if (inQuote && ch === quoteChar) {
          inQuote = false;
          quoteChar = "";
        } else if (!inQuote) {
          inQuote = true;
          quoteChar = ch;
        } else {
          current += ch;
        }
      } else if (ch === " " && !inQuote) {
        if (current) args.push(current);
        current = "";
      } else {
        if (ch === "\\" && i + 1 < text.length && (text[i + 1] === '"' || text[i + 1] === "'")) {
          current += text[i + 1];
          i++;
        } else {
          current += ch;
        }
      }
      i++;
    }
    if (current) args.push(current);
    return args;
  }

  async function loadSettings() {
    try {
      const integration = (await localBridge.settings())?.integrations?.[name];
      if (integration) show(integration);
    } catch (error) {
      settingsError = error?.message || "Could not load companion settings.";
    }
  }

  // What the companion holds, and the inputs reset to it.
  function show(integration) {
    savedPath = integration.path || "";
    savedArgs = integration.args || [];
    editingPath = savedPath;
    editingArgs = quoteAwareJoin(savedArgs);
  }

  // Every change goes through the companion's confirmation dialog.
  async function apply(path, args) {
    pendingDialogAction = true;
    try {
      show(await localBridge.setIntegration(name, { path, args }));
      settingsError = "";
    } catch (error) {
      settingsError = error?.status === 403
        ? "You declined the change in the dialog on your computer."
        : error?.message || "Could not change the command.";
    } finally {
      pendingDialogAction = false;
    }
  }

  const capability = $derived.by(() => {
    const capabilities = local?.capabilities;
    if (name === "quarto") return capabilities?.tools?.quarto;
    return capabilities?.[name] ?? null;
  });

  const isConnected = $derived(local?.state === "connected");
  const showFields = $derived(name !== "zotero" && isConnected);
  const unchanged = $derived(editingPath === savedPath && quoteAwareJoin(quoteAwareSplit(editingArgs)) === quoteAwareJoin(savedArgs));
  const isDefault = $derived(!savedPath && savedArgs.length === 0);
</script>

{#if settingsError}<p class="setting-description integration-error" role="alert">{settingsError}</p>{/if}

{#if !isConnected}
  <SettingRow id={`${name}-status`} title="Status" description="">
    <span class="setting-description">Connect the local companion to configure {name}.</span>
  </SettingRow>
{:else}
  <SettingRow id={`${name}-status`} title="Status" description="">
    {#if capability?.available}
      <span class="setting-description">{capability.version ? `Version ${capability.version}` : `${name} available`}</span>
    {:else}
      <span class="setting-description">{capability?.note || `${name} not found on this computer`}</span>
    {/if}
  </SettingRow>

  {#if showFields}
    <SettingRow id={`${name}-executable`} title="Executable" description="Path to the executable or folder containing it. Leave blank to use PATH.">
      <input class="input input-sm setting-input" type="text" aria-label="Executable path" placeholder="Found on PATH" bind:value={editingPath} disabled={pendingDialogAction} />
    </SettingRow>

    <SettingRow id={`${name}-arguments`} title="Arguments" description="Added to every run, e.g. --log-level warning.">
      <input class="input input-sm setting-input" type="text" aria-label="Arguments" placeholder="--quiet" bind:value={editingArgs} disabled={pendingDialogAction} />
    </SettingRow>

    <div class="integration-actions">
      <button type="button" class="btn btn-sm lp-control-brand" disabled={pendingDialogAction || unchanged} onclick={() => void apply(editingPath.trim() || null, quoteAwareSplit(editingArgs))}>Save</button>
      <button type="button" class="btn btn-sm lp-control-outline" disabled={pendingDialogAction || isDefault} onclick={() => void apply(null, [])}>Reset to default</button>
    </div>
    {#if pendingDialogAction}
      <p class="setting-description">Approve the change in the dialog LibrePaper Companion opened on this computer.</p>
    {/if}
  {/if}

  {#if name === "zotero"}
    <p class="setting-description integration-note">LibrePaper reads Zotero through its local API. Enable it in Zotero Settings > Advanced > "Allow other applications on this computer to communicate with Zotero".</p>
  {/if}
{/if}

<style>
  .integration-error { margin-block: calc(var(--spacing) * 2); color: var(--color-error-text); }
  .integration-actions { display: flex; gap: calc(var(--spacing) * 2); margin-top: calc(var(--spacing) * 3); }
  .integration-note { max-width: 42rem; margin-top: calc(var(--spacing) * 2); }
</style>
