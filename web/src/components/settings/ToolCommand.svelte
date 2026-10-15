<script>
  import SettingRow from "./SettingRow.svelte";
  import * as desktopClient from "../../lib/desktop/client.js";
  import { splitArgs, joinArgs } from "../../lib/desktop/args.js";

  let { name } = $props();

  let settingsError = $state("");
  let pendingDialogAction = $state(false);
  let editingPath = $state("");
  let editingArgs = $state("");
  let savedPath = $state("");
  let savedArgs = $state([]);
  let settingsLoaded = $state(false);
  let savedFeedback = $state(false);
  let loadId = 0;

  const isConnected = Boolean(desktopClient.desktop());

  $effect(() => {
    const requestedName = name;
    const requestId = ++loadId;
    settingsLoaded = false;
    pendingDialogAction = false;
    settingsError = "";
    savedFeedback = false;
    if (isConnected) void loadSettings(requestId, requestedName);
    else {
      savedPath = "";
      savedArgs = [];
      editingPath = "";
      editingArgs = "";
    }
  });

  async function loadSettings(requestId, requestedName) {
    try {
      const integration = (await desktopClient.manage("/state"))?.settings?.integrations?.[requestedName];
      if (requestId !== loadId || requestedName !== name) return;
      if (integration) show(integration);
      settingsLoaded = Boolean(integration);
      if (!integration) settingsError = "Integration settings are unavailable from the desktop app.";
    } catch (error) {
      if (requestId === loadId && requestedName === name) {
        settingsError = error?.message || "Could not load the desktop app settings.";
      }
    }
  }

  // What the desktop app holds, and the inputs reset to it.
  function show(integration) {
    savedPath = integration.path || "";
    savedArgs = integration.args || [];
    editingPath = savedPath;
    editingArgs = joinArgs(savedArgs);
  }

  // Every change goes through the desktop app's confirmation dialog.
  async function apply(path, args) {
    if (!canEdit || pendingDialogAction) return;
    pendingDialogAction = true;
    savedFeedback = false;
    const requestId = loadId;
    try {
      await desktopClient.manage("/settings", { method: "PUT", body: { integrations: { [name]: { path, args } } } });
      if (requestId === loadId) {
        show({ path, args });
        settingsLoaded = true;
        settingsError = "";
        savedFeedback = true;
      }
    } catch (error) {
      if (requestId === loadId) {
        settingsError = error?.status === 403
          ? "You declined the change in the dialog on your computer."
          : error?.message || "Could not change the command.";
      }
    } finally {
      if (requestId === loadId) pendingDialogAction = false;
    }
  }

  const canEdit = $derived(isConnected && settingsLoaded && !pendingDialogAction);
  const unchanged = $derived(editingPath === savedPath && joinArgs(splitArgs(editingArgs)) === joinArgs(savedArgs));
  const isDefault = $derived(!savedPath && savedArgs.length === 0);
</script>

{#if settingsError}<p class="setting-description tool-command-error" role="alert">{settingsError}</p>{/if}

<div class="tool-command">
  <SettingRow id={`${name}-executable`} title="Executable" description="Executable path; leave blank to use PATH.">
    <input class="input input-sm setting-input" type="text" aria-label="Executable path" bind:value={editingPath} oninput={() => savedFeedback = false} disabled={!canEdit} />
  </SettingRow>

  <SettingRow id={`${name}-arguments`} title="Arguments" description="Added to each run, e.g. --log-level warning.">
    <input class="input input-sm setting-input" type="text" aria-label="Arguments" bind:value={editingArgs} oninput={() => savedFeedback = false} disabled={!canEdit} />
  </SettingRow>

  <div class="setting-actions tool-command-actions">
    <button type="button" class="btn btn-sm lp-control-brand" disabled={!canEdit || unchanged} onclick={() => void apply(editingPath.trim() || null, splitArgs(editingArgs))}>{pendingDialogAction ? "Saving…" : "Save"}</button>
    <button type="button" class="btn btn-sm lp-control-tonal-brand" disabled={!canEdit || isDefault} onclick={() => void apply(null, [])}>Reset to default</button>
    {#if savedFeedback}<span class="setting-feedback" role="status">Saved</span>{/if}
  </div>
  {#if pendingDialogAction}
    <p class="setting-description">Confirm the change in the dialog on this computer.</p>
  {/if}
</div>

<style>
  .tool-command-error { margin-block: calc(var(--spacing) * 2); color: var(--color-error-text); }
  .tool-command-actions { display: flex; gap: calc(var(--spacing) * 2); margin-top: calc(var(--spacing) * 3); }
</style>
