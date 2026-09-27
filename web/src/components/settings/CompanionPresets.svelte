<script>
  import SettingRow from "./SettingRow.svelte";
  import * as localBridge from "../../lib/companion/client.js";

  let {
    main = "",
  } = $props();

  let companionSettings = $state(null);
  let settingsError = $state("");
  let pendingDialogAction = $state("");

  let newPresetName = $state("");
  let newPresetAdapter = $state("");
  let newPresetFormats = $state("");
  let newPresetOptions = $state("");
  let newPresetEnvironment = $state("");
  let newPresetWrapper = $state("");

  let grantPresetId = $state("");
  let grantEntrypoint = $state("");

  $effect(() => {
    void loadSettings();
  });

  function parseFormattedInput(text) {
    return text.split("\n").filter(Boolean);
  }

  function parseKeyValueInput(text) {
    const result = {};
    text.split("\n").forEach((line) => {
      const eqIndex = line.indexOf("=");
      if (eqIndex > 0) {
        const key = line.substring(0, eqIndex).trim();
        const value = line.substring(eqIndex + 1);
        result[key] = value;
      }
    });
    return result;
  }

  async function callDialogAction(action) {
    try {
      await action();
      await loadSettings();
    } finally {
      pendingDialogAction = "";
    }
  }

  async function loadSettings() {
    try {
      companionSettings = await localBridge.settings();
      if (!grantEntrypoint) grantEntrypoint = main;
    } catch (error) {
      settingsError = error?.message || "Could not load companion settings.";
    }
  }

  async function createPreset() {
    if (!newPresetName.trim() || !newPresetAdapter.trim()) return;
    pendingDialogAction = "create-preset";
    try {
      await callDialogAction(() =>
        localBridge.createPreset({
          name: newPresetName.trim(),
          adapter: newPresetAdapter.trim(),
          formats: parseFormattedInput(newPresetFormats),
          options: parseKeyValueInput(newPresetOptions),
          environment: parseKeyValueInput(newPresetEnvironment),
          wrapper: newPresetWrapper.trim() || null,
        })
      );
      newPresetName = "";
      newPresetAdapter = "";
      newPresetFormats = "";
      newPresetOptions = "";
      newPresetEnvironment = "";
      newPresetWrapper = "";
    } catch (error) {
      settingsError = error?.message || "Could not create preset.";
    }
  }

  async function removePreset(id) {
    pendingDialogAction = "delete-preset";
    try {
      await callDialogAction(() => localBridge.deletePreset(id));
    } catch (error) {
      settingsError = error?.message || "Could not delete preset.";
    }
  }

  async function grantAccess() {
    if (!grantPresetId) return;
    pendingDialogAction = "grant-preset";
    try {
      await callDialogAction(() =>
        localBridge.grantPreset(grantPresetId, grantEntrypoint.trim() || "")
      );
      grantPresetId = "";
    } catch (error) {
      settingsError = error?.message || "Could not grant access.";
    }
  }

  async function revokeAccess(id) {
    try {
      await localBridge.revokeGrant(id);
      await loadSettings();
    } catch (error) {
      settingsError = error?.message || "Could not revoke access.";
    }
  }

  async function toggleStartup(enabled) {
    pendingDialogAction = "startup";
    try {
      await callDialogAction(() => localBridge.setStartup(enabled));
    } catch (error) {
      settingsError = error?.message || "Could not change startup setting.";
    }
  }

  async function quitCompanion() {
    pendingDialogAction = "quit";
    try {
      await callDialogAction(() => localBridge.quit());
      await localBridge.retry();
    } catch (error) {
      settingsError = error?.message || "Could not quit companion.";
    }
  }
</script>

{#if settingsError}<p class="setting-description local-error" role="alert">{settingsError}</p>{/if}

{#if companionSettings}
  <!-- Build presets section -->
  <section class="companion-section">
  <h4 class="setting-title">Build presets</h4>
  {#if companionSettings.presets && companionSettings.presets.length > 0}
    <table class="setting-table">
      <thead>
        <tr>
          <th>Name</th>
          <th>Adapter</th>
          <th>Formats</th>
          <th>Environment variables</th>
          <th></th>
        </tr>
      </thead>
      <tbody>
        {#each companionSettings.presets as preset (preset.id)}
          <tr>
            <td>{preset.display_name}</td>
            <td>{preset.base_adapter}</td>
            <td>{preset.source_formats?.join(", ") || ""}</td>
            <td>{preset.environment_keys?.join(", ") || ""}</td>
            <td>
              <button type="button" class="btn btn-sm lp-control-outline" disabled={pendingDialogAction === "delete-preset"} onclick={() => void removePreset(preset.id)}>Delete</button>
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
  {/if}

  <div class="preset-form">
    <h5 class="setting-title">New preset</h5>
    <div class="form-field">
      <label for="preset-name" class="setting-description">Name</label>
      <input id="preset-name" class="input input-sm setting-input" type="text" bind:value={newPresetName} placeholder="Preset name" />
    </div>
    <div class="form-field">
      <label for="preset-adapter" class="setting-description">Adapter</label>
      <input id="preset-adapter" class="input input-sm setting-input" type="text" bind:value={newPresetAdapter} placeholder="e.g., quarto" />
    </div>
    <div class="form-field">
      <label for="preset-formats" class="setting-description">Formats (one per line)</label>
      <textarea id="preset-formats" class="input input-sm setting-input" bind:value={newPresetFormats} placeholder="qmd&#10;html"></textarea>
    </div>
    <div class="form-field">
      <label for="preset-options" class="setting-description">Options (KEY=VALUE, one per line)</label>
      <textarea id="preset-options" class="input input-sm setting-input" bind:value={newPresetOptions} placeholder="OPTION=value"></textarea>
    </div>
    <div class="form-field">
      <label for="preset-environment" class="setting-description">Environment variables (KEY=VALUE, one per line)</label>
      <textarea id="preset-environment" class="input input-sm setting-input" bind:value={newPresetEnvironment} placeholder="VAR=value"></textarea>
    </div>
    <div class="form-field">
      <label for="preset-wrapper" class="setting-description">Wrapper (optional)</label>
      <input id="preset-wrapper" class="input input-sm setting-input" type="text" bind:value={newPresetWrapper} placeholder="Optional wrapper" />
    </div>
    <button type="button" class="btn btn-sm lp-control-brand" disabled={!newPresetName.trim() || !newPresetAdapter.trim() || pendingDialogAction} onclick={() => void createPreset()}>
      {pendingDialogAction === "create-preset" ? "Confirm on this computer…" : "Create preset"}
    </button>
    {#if pendingDialogAction === "create-preset"}
      <p class="setting-description">Approve the request in the dialog LibrePaper Companion opened on this computer.</p>
    {/if}
  </div>
</section>

<!-- Grants section -->
<section class="companion-section">
  <h4 class="setting-title">This document's preset permissions</h4>
  {#if companionSettings.grants && companionSettings.grants.length > 0}
    <div class="grants-list">
      {#each companionSettings.grants as grant (grant.id)}
        <div class="grant-item">
          <span class="setting-description">
            {companionSettings.presets?.find((p) => p.id === grant.preset)?.display_name}
            ({grant.entrypoint || "root"})
          </span>
          <button type="button" class="btn btn-sm lp-control-outline" onclick={() => void revokeAccess(grant.id)}>Revoke</button>
        </div>
      {/each}
    </div>
  {/if}

  <div class="grant-form">
    <h5 class="setting-title">Grant access</h5>
    <div class="form-field">
      <label for="grant-preset" class="setting-description">Preset</label>
      <select id="grant-preset" class="input input-sm setting-input" bind:value={grantPresetId}>
        <option value="">Choose a preset...</option>
        {#each companionSettings.presets || [] as preset (preset.id)}
          <option value={preset.id}>{preset.display_name}</option>
        {/each}
      </select>
    </div>
    <div class="form-field">
      <label for="grant-entrypoint" class="setting-description">Entrypoint</label>
      <input id="grant-entrypoint" class="input input-sm setting-input" type="text" bind:value={grantEntrypoint} placeholder="main.qmd" />
    </div>
    <button type="button" class="btn btn-sm lp-control-brand" disabled={!grantPresetId || pendingDialogAction} onclick={() => void grantAccess()}>
      {pendingDialogAction === "grant-preset" ? "Confirm on this computer…" : "Grant access"}
    </button>
    {#if pendingDialogAction === "grant-preset"}
      <p class="setting-description">Approve the request in the dialog LibrePaper Companion opened on this computer.</p>
    {/if}
  </div>
</section>

<!-- Startup and quit section (standalone only) -->
{#if companionSettings.standalone}
  <section class="companion-section">
    {#if companionSettings.startup !== null}
      <SettingRow id="companion-startup" title="Start at login" description="Open LibrePaper Companion when you log in.">
        <button type="button" role="switch" class="switch companion-startup-switch" aria-label="Start at login" aria-checked={companionSettings.startup} data-state={companionSettings.startup ? "checked" : "unchecked"} disabled={pendingDialogAction === "startup"} onclick={() => void toggleStartup(!companionSettings.startup)}>
          <span class="switch-thumb" data-state={companionSettings.startup ? "checked" : "unchecked"}></span>
        </button>
        {#if pendingDialogAction === "startup"}
          <p class="setting-description">Approve the request in the dialog LibrePaper Companion opened on this computer.</p>
        {/if}
      </SettingRow>
    {/if}
    <div class="quit-button">
      <button type="button" class="btn btn-sm lp-control-outline" disabled={pendingDialogAction === "quit"} onclick={() => void quitCompanion()}>
        {pendingDialogAction === "quit" ? "Confirm on this computer…" : "Quit companion"}
      </button>
      {#if pendingDialogAction === "quit"}
        <p class="setting-description">Approve the request in the dialog LibrePaper Companion opened on this computer.</p>
      {/if}
    </div>
  </section>
  {/if}
{/if}

<style>
  .companion-section { display: grid; gap: calc(var(--spacing) * 3); margin-block: calc(var(--spacing) * 4); }
  .preset-form { display: grid; gap: calc(var(--spacing) * 2); }
  .form-field { display: grid; gap: calc(var(--spacing) * 1); }
  .form-field label { font-size: var(--panel-meta-size); }
  .form-field textarea { min-height: 3rem; resize: vertical; }
  .grant-form { display: grid; gap: calc(var(--spacing) * 2); }
  .grants-list { display: grid; gap: calc(var(--spacing) * 2); margin-bottom: calc(var(--spacing) * 3); }
  .grant-item { display: flex; align-items: center; justify-content: space-between; padding: calc(var(--spacing) * 2); background: var(--color-subtle); border-radius: var(--radius-container); }
  .quit-button { display: grid; gap: calc(var(--spacing) * 2); }
  .companion-startup-switch { appearance: none; border: 0; padding: 0; cursor: pointer; }
  .companion-startup-switch:focus-visible { outline: 2px solid var(--color-brand); outline-offset: 2px; }
</style>
