<script>
  import SettingRow from "./SettingRow.svelte";
  import StatusPill from "./StatusPill.svelte";
  import * as localBridge from "../../lib/companion/client.js";
  import * as control from "../../lib/companion/control.js";
  import { companion } from "../../lib/companion/status.svelte.js";

  let { name = "quarto" } = $props();

  let settingsError = $state("");
  let pendingDialogAction = $state(false);
  let editingPath = $state("");
  let editingArgs = $state("");
  let savedPath = $state("");
  let savedArgs = $state([]);
  let settingsLoaded = $state(false);
  let savedFeedback = $state(false);
  let loadId = 0;
  let managedAvailable = $state(control.available());
  let managedState = $state(null);
  let managedScope = "";
  let managedRequest = 0;

  const local = $derived(companion.status);
  $effect(() => companion.watch());

  $effect(() => {
    const unsubscribe = control.subscribe((access) => {
      const nextScope = access?.scope || control.scope();
      if (managedScope !== nextScope || managedAvailable !== Boolean(access?.available)) {
        managedScope = nextScope;
        managedAvailable = Boolean(access?.available);
        managedState = null;
        managedRequest++;
        settingsLoaded = false;
      }
      if (managedAvailable && name !== "zotero") void loadManagedSettings(++managedRequest, name, nextScope);
      else if (managedAvailable && name === "zotero") {
        void loadManagedState(++managedRequest, nextScope);
        settingsLoaded = true;
      }
    });
    return unsubscribe;
  });

  $effect(() => {
    const connected = local?.state === "connected";
    const address = local?.address || "";
    const instance = local?.instance || "";
    const requestedName = name;
    const requestId = ++loadId;
    settingsLoaded = false;
    pendingDialogAction = false;
    settingsError = "";
    savedFeedback = false;
    if (requestedName === "zotero") settingsLoaded = true;
    else if (managedAvailable && requestedName !== "zotero") void loadManagedSettings(++managedRequest, requestedName, managedScope);
    else if (managedAvailable) settingsLoaded = true;
    else if (connected) void loadSettings(requestId, requestedName, address, instance);
    else {
      savedPath = "";
      savedArgs = [];
      editingPath = "";
      editingArgs = "";
    }
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

  async function loadSettings(requestId, requestedName, address, instance) {
    try {
      const integration = (await localBridge.settings())?.integrations?.[requestedName];
      if (requestId !== loadId || requestedName !== name || local?.state !== "connected" || (local?.address || "") !== address || (local?.instance || "") !== instance) return;
      if (integration) show(integration);
      settingsLoaded = Boolean(integration);
      if (!integration) settingsError = "Integration settings are unavailable from the companion.";
    } catch (error) {
      if (requestId === loadId && requestedName === name && local?.state === "connected" && (local?.address || "") === address && (local?.instance || "") === instance) {
        settingsError = error?.message || "Could not load companion settings.";
      }
    }
  }

  async function loadManagedState(requestId, expectedScope) {
    try {
      const result = await control.request("/state");
      if (requestId !== managedRequest || expectedScope !== control.scope() || !control.available()) return;
      managedState = result;
      settingsError = "";
    } catch (error) {
      if (requestId === managedRequest && expectedScope === control.scope()) settingsError = error?.message || "Could not load companion settings.";
    }
  }

  async function loadManagedSettings(requestId, requestedName, expectedScope) {
    await loadManagedState(requestId, expectedScope);
    if (requestId !== managedRequest || requestedName !== name || expectedScope !== control.scope() || !control.available()) return;
    const integration = managedState?.settings?.integrations?.[requestedName];
    if (integration) show(integration);
    settingsLoaded = Boolean(integration);
    if (!integration) settingsError = "Integration settings are unavailable from the companion.";
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
    if (!canEdit || pendingDialogAction) return;
    pendingDialogAction = true;
    savedFeedback = false;
    const requestId = loadId;
    try {
      let integration;
      if (managedAvailable) {
        const scope = managedScope;
        const managedId = managedRequest;
        await control.request("/settings", { method: "PUT", body: { integrations: { [name]: { path, args } } } });
        if (managedId !== managedRequest || scope !== control.scope() || !control.available()) return;
        integration = { path, args };
        managedState = { ...managedState, settings: { ...managedState?.settings, integrations: { ...managedState?.settings?.integrations, [name]: integration } } };
      } else integration = await localBridge.setIntegration(name, { path, args });
      if (requestId === loadId && isConnected) {
        show(integration);
        settingsLoaded = true;
        settingsError = "";
        savedFeedback = true;
      }
    } catch (error) {
      if (requestId === loadId && isConnected) {
        settingsError = error?.status === 403
          ? "You declined the change in the dialog on your computer."
          : error?.message || "Could not change the command.";
      }
    } finally {
      if (requestId === loadId) pendingDialogAction = false;
    }
  }

  const capability = $derived.by(() => {
    const capabilities = managedAvailable ? managedState?.tools : local?.capabilities;
    if (name === "quarto") return capabilities?.tools?.quarto;
    return capabilities?.[name] ?? null;
  });

  const isConnected = $derived(managedAvailable || local?.state === "connected");
  const canEdit = $derived(isConnected && settingsLoaded && !pendingDialogAction);
  const showFields = $derived(name !== "zotero");
  const unchanged = $derived(editingPath === savedPath && quoteAwareJoin(quoteAwareSplit(editingArgs)) === quoteAwareJoin(savedArgs));
  const isDefault = $derived(!savedPath && savedArgs.length === 0);
</script>

{#if settingsError}<p class="setting-description integration-error" role="alert">{settingsError}</p>{/if}

{#if name !== "zotero"}<SettingRow id={`${name}-status`} title="Status" description="" scope="This computer">
  {#if !isConnected}
    <StatusPill label="Not checked" tone="neutral" accessibleLabel={`Connect the local companion to configure ${name}.`} />
  {:else if name !== "zotero" && !settingsLoaded && !settingsError}
    <StatusPill label="Loading settings…" />
  {:else if capability?.available}
    <StatusPill label={capability.version ? `Available · ${capability.version}` : "Available"} tone="good" />
  {:else}
    <StatusPill label={capability ? "Unavailable" : "Availability unknown"} tone="warn" />
    {#if capability?.note}<span class="setting-description">{capability.note}</span>{/if}
  {/if}
</SettingRow>{/if}

  {#if showFields}
    <SettingRow id={`${name}-executable`} title="Executable" description="Executable path; leave blank to use PATH.">
      <input class="input input-sm setting-input" type="text" aria-label="Executable path" placeholder="Found on PATH" bind:value={editingPath} oninput={() => savedFeedback = false} disabled={!canEdit} />
    </SettingRow>

    <SettingRow id={`${name}-arguments`} title="Arguments" description="Added to each run, e.g. --log-level warning.">
      <input class="input input-sm setting-input" type="text" aria-label="Arguments" placeholder="--quiet" bind:value={editingArgs} oninput={() => savedFeedback = false} disabled={!canEdit} />
    </SettingRow>

    <div class="setting-actions integration-actions">
      <button type="button" class="btn btn-sm lp-control-brand" disabled={!canEdit || unchanged} onclick={() => void apply(editingPath.trim() || null, quoteAwareSplit(editingArgs))}>{pendingDialogAction ? "Saving…" : "Save"}</button>
      <button type="button" class="btn btn-sm lp-control-outline" disabled={!canEdit || isDefault} onclick={() => void apply(null, [])}>Reset to default</button>
      {#if savedFeedback}<span class="setting-feedback" role="status">Saved</span>{/if}
    </div>
    {#if pendingDialogAction}
      <p class="setting-description">Approve this request in Settings → Companion → Approvals.</p>
    {/if}
  {/if}
{#if name === "zotero"}
  <SettingRow id="zotero-status" title="Zotero library" description="Use your local Zotero library for citations and references.">
    <StatusPill label={!isConnected ? "Not checked" : capability?.available ? (capability.version ? `Available · ${capability.version}` : "Available") : capability ? "Unavailable" : "Availability unknown"} tone={!isConnected ? "neutral" : capability?.available ? "good" : "warn"} />
    {#if isConnected && capability?.note}<span class="setting-description">{capability.note}</span>{/if}
  </SettingRow>
  <p class="setting-description integration-note">Enable Zotero’s local API in Settings → Advanced → “Allow other applications on this computer to communicate with Zotero”.</p>
{/if}

<style>
  .integration-error { margin-block: calc(var(--spacing) * 2); color: var(--color-error-text); }
  .integration-actions { display: flex; gap: calc(var(--spacing) * 2); margin-top: calc(var(--spacing) * 3); }
  .integration-note { max-width: 42rem; margin-top: calc(var(--spacing) * 2); }
</style>
