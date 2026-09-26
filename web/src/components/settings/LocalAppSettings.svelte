<script>
  import Modal from "../Modal.svelte";
  import SettingRow from "./SettingRow.svelte";
  import * as localBridge from "../../lib/companion/client.js";
  import { companion } from "../../lib/companion/status.svelte.js";

  let {
    sourceFormat = "", main = "", mayEdit = false, onbindingid,
    localExecution = false, onlocalexecution,
  } = $props();
  const quarto = $derived(sourceFormat === "quarto");
  const projectBinding = $derived(["quarto", "typst", "markdown"].includes(sourceFormat));
  const local = $derived(companion.status);
  $effect(() => companion.watch());
  $effect(() => void localBridge.probe());

  let address = $state(localBridge.address());
  let addressDraft = $state(localBridge.address());
  let pairingCode = $state("");
  let connecting = $state(false);
  let copying = $state("");
  let doctor = $state("");
  let connectionError = $state("");
  let folderError = $state("");
  let copyError = $state(false);
  let manualPairingNeeded = $state(false);
  let choosingFolder = $state(false);
  let entrypoint = $state("");
  let detailsOpen = $state(false);
  let addressOpen = $state(false);
  let pairConfirmOpen = $state(false);

  // Companion settings
  let companionSettings = $state(null);
  let settingsError = $state("");
  let pendingDialogAction = $state("");

  // Preset form
  let newPresetName = $state("");
  let newPresetAdapter = $state("");
  let newPresetFormats = $state("");
  let newPresetOptions = $state("");
  let newPresetEnvironment = $state("");
  let newPresetWrapper = $state("");

  // Grant form
  let grantPresetId = $state("");
  let grantEntrypoint = $state("");

  $effect(() => { entrypoint = main; });
  $effect(() => {
    if (!connected) return;
    connectionError = "";
    manualPairingNeeded = false;
    copyError = false;
  });
  $effect(() => {
    if (!connected) {
      companionSettings = null;
      settingsError = "";
      return;
    }
    settingsError = "";
    loadSettings();
  });

  async function loadSettings() {
    try {
      companionSettings = await localBridge.settings();
      grantEntrypoint = main;
    } catch (error) {
      settingsError = error?.message || "Could not load companion settings.";
    }
  }

  function parseFormattedInput(text) {
    return text.split("\n").filter(Boolean);
  }

  function parseKeyValueInput(text) {
    const result = {};
    text.split("\n").forEach((line) => {
      const [key, value] = line.split("=").map((s) => s.trim());
      if (key && value !== undefined) result[key] = value;
    });
    return result;
  }

  async function callDialogAction(action) {
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), 5 * 60 * 1000);
    try {
      await action(controller.signal);
      await loadSettings();
    } finally {
      clearTimeout(timer);
      pendingDialogAction = "";
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
    } catch (error) {
      settingsError = error?.message || "Could not quit companion.";
    }
  }
  const connected = $derived(local?.state === "connected");
  const canPair = $derived(["unauthorized", "reachable"].includes(local?.state));
  const tone = $derived(connected ? "good" : canPair || ["denied", "incompatible"].includes(local?.state) ? "warn" : "off");
  const status = $derived(({ unknown: "Companion not connected", unreachable: "Companion not connected", denied: "Local network access blocked", reachable: "Companion found", unauthorized: "Companion needs permission", connected: "Companion connected", incompatible: "Companion needs an update" })[local?.state] || "Companion not connected");
  const installer = "https://github.com/LibrePaper/librepaper/releases/latest/download/librepaper-installer.sh";
  const windowsInstaller = "https://github.com/LibrePaper/librepaper/releases/latest/download/librepaper-installer.ps1";

  async function connect() {
    if (!pairingCode || connecting) return;
    pairConfirmOpen = true;
  }

  async function confirmPairing() {
    if (!pairingCode || connecting) return;
    pairConfirmOpen = false;
    connecting = true;
    try {
      await localBridge.connect(pairingCode);
      pairingCode = "";
    } catch (error) {
      doctor = error?.message || "The pairing code was not accepted. Check the code and retry.";
      detailsOpen = true;
    } finally { connecting = false; }
  }

  async function pair() {
    if (connecting) return;
    connectionError = "";
    connecting = true;
    try {
      await localBridge.connectApp();
    } catch (error) {
      connectionError = error?.message || "Could not open the companion permission window.";
      manualPairingNeeded = true;
    }
    finally { connecting = false; }
  }

  async function chooseFolder() {
    if (!mayEdit || choosingFolder) return;
    folderError = "";
    choosingFolder = true;
    try {
      const result = await localBridge.chooseFolderBinding({ entrypoint: entrypoint.trim() });
      if (result?.id) onbindingid?.(result.id);
      entrypoint = result?.entrypoint || entrypoint;
    } catch (error) { folderError = error?.message || "The companion could not choose a project folder."; }
    finally { choosingFolder = false; }
  }

  async function copy(text, label) {
    try {
      await navigator.clipboard.writeText(text);
      copyError = false;
      copying = label;
      setTimeout(() => { if (copying === label) copying = ""; }, 1500);
    } catch { copyError = true; }
  }

  async function doctorReport() {
    detailsOpen = true;
    try {
      const capabilities = await localBridge.capabilities({ rescan: true });
      doctor = JSON.stringify(capabilities, null, 2);
    } catch (error) { doctor = error?.message || "Local setup check could not reach the companion."; }
  }

  function saveAddress() {
    localBridge.setAddress(addressDraft);
    address = addressDraft;
    addressOpen = false;
    void localBridge.retry();
  }

  function openAddress() {
    addressDraft = localBridge.address();
    addressOpen = true;
  }
</script>

<p class="setting-description local-intro">Connect LibrePaper to apps and tools installed on this computer, including coding agents, Zotero, Quarto, and local project folders.</p>

<div id="local-status" class="setting-status" data-tone={tone}>
  <span class="setting-status-dot" aria-hidden="true"></span>
  <div class="setting-status-words">
    <div class="setting-title" role="status">{status}</div>
    <div class="setting-description">
      {#if connected}
        Local tools are available to LibrePaper.
      {:else if local?.state === "denied"}
        Allow local-network access for this site in your browser, then retry.
      {:else if local?.state === "incompatible"}
        Install the latest companion version, then retry.
      {:else if local?.state === "unauthorized" || local?.state === "reachable"}
        The companion is running. Connect to approve access for this site.
      {:else if local?.state === "unreachable"}
        Open the companion after installing it, then retry here.
      {:else}
        Install and open the companion to use local tools.
      {/if}
    </div>
  </div>
  <div class="setting-control">
    {#if canPair}
      <button type="button" class="btn btn-sm lp-control-brand" disabled={connecting} onclick={pair}>{connecting ? "Waiting…" : "Connect companion"}</button>
    {/if}
    {#if !connected}<button type="button" class="btn btn-sm lp-control-outline" disabled={connecting} onclick={() => void localBridge.retry()}>Retry</button>{/if}
  </div>
</div>
{#if connectionError}<p class="setting-description local-error" role="alert">{connectionError}</p>{/if}
{#if copyError}<p class="setting-description local-error" role="status">Copy failed. Select and copy the command manually.</p>{/if}

{#if !connected}
  <section id="local-install-help" class="local-install" aria-label="Install LibrePaper Companion">
    <h4 class="setting-title">Install LibrePaper Companion</h4>
    <div class="install-option">
      <div class="setting-title">macOS &amp; Linux</div>
      <div class="command-line"><code>curl --proto '=https' --tlsv1.2 -LsSf {installer} | sh</code><button type="button" class="btn btn-sm lp-control-outline" onclick={() => void copy(`curl --proto '=https' --tlsv1.2 -LsSf ${installer} | sh`, "macOS & Linux")}>{copying === "macOS & Linux" ? "Copied" : "Copy"}</button></div>
    </div>
    <div class="install-option">
      <div class="setting-title">Windows</div>
      <div class="command-line"><code>powershell -ExecutionPolicy Bypass -c "irm {windowsInstaller} | iex"</code><button type="button" class="btn btn-sm lp-control-outline" onclick={() => void copy(`powershell -ExecutionPolicy Bypass -c "irm ${windowsInstaller} | iex"`, "Windows")}>{copying === "Windows" ? "Copied" : "Copy"}</button></div>
    </div>
    <p class="setting-description">After installation, open LibrePaper Companion and return here to connect.</p>
    <a class="quiet-link" href="https://github.com/LibrePaper/librepaper/blob/main/deploy/README.md" target="_blank" rel="noreferrer">Installation help</a>
  </section>
{/if}

{#if !connected && manualPairingNeeded}
  <section id="local-pairing" class="pairing">
    <h4 class="setting-title">Pair manually</h4>
    <p class="setting-description">If the companion did not open a permission prompt, enter the code shown in the local app.</p>
    <div class="pair-controls">
      <input class="input input-sm setting-input" type="text" inputmode="numeric" aria-label="Pairing code" bind:value={pairingCode} placeholder="Pairing code" />
      <button type="button" class="btn btn-sm lp-control-brand" disabled={connecting || !pairingCode} onclick={connect}>Connect</button>
    </div>
  </section>
{/if}

{#if connected && quarto && mayEdit}
  <SettingRow id="local-execution" title="Local code execution" description="Code runs on this computer with your user account’s permissions.">
    <span class="setting-description">Allow paired Quarto documents to run local code</span>
    <button type="button" role="switch" class="switch local-execution-switch" aria-label="Allow paired Quarto documents to run local code" aria-checked={localExecution} data-state={localExecution ? "checked" : "unchecked"} onclick={() => onlocalexecution?.(!localExecution)}>
      <span class="switch-thumb" data-state={localExecution ? "checked" : "unchecked"}></span>
    </button>
  </SettingRow>
{/if}

{#if connected && projectBinding}
  <SettingRow id="local-binding" title="Project folder" description="Use a folder on this computer for local builds and previews.">
    <input class="input input-sm setting-input" type="text" aria-label="Project entrypoint" placeholder={quarto ? "main.qmd" : sourceFormat === "typst" ? "main.typ" : "main.md"} bind:value={entrypoint} disabled={!mayEdit} />
    <button type="button" class="btn btn-sm lp-control-outline" disabled={!mayEdit || choosingFolder || !entrypoint.trim()} onclick={() => void chooseFolder()}>{choosingFolder ? "Choosing…" : "Choose folder…"}</button>
  </SettingRow>
  {#if folderError}<p class="setting-description local-error" role="alert">{folderError}</p>{/if}
{/if}

{#if connected && companionSettings}
  {#if settingsError}<p class="setting-description local-error" role="alert">{settingsError}</p>{/if}

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
              <td>{preset.display_name || preset.name || preset.id}</td>
              <td>{preset.base_adapter || preset.adapter || ""}</td>
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
              {companionSettings.presets?.find((p) => p.id === grant.preset)?.display_name || grant.preset}
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
            <option value={preset.id}>{preset.display_name || preset.name || preset.id}</option>
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
  {#if companionSettings.standalone !== null && companionSettings.standalone !== false}
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

<nav class="local-footer" aria-label="Local connection help">
  <button type="button" class="quiet-link" id="local-details" onclick={() => (detailsOpen = true)}>Connection details</button>
  <button type="button" class="quiet-link" id="local-doctor" onclick={() => void doctorReport()}>Check local setup</button>
  <a class="quiet-link" href="https://github.com/LibrePaper/librepaper/issues" target="_blank" rel="noreferrer">Troubleshooting</a>
</nav>

<Modal bind:open={detailsOpen} title="Connection details" wide>
  <div class="details-content">
    <SettingRow title="Status" description={status}>
      <span class="setting-description">{local?.address || address}</span>
    </SettingRow>
    {#if local?.version}<SettingRow title="Version"><span class="setting-description">{local.version}</span></SettingRow>{/if}
    {#if connected}<div class="details-actions"><button type="button" class="btn btn-sm lp-control-outline" onclick={() => void localBridge.disconnect()}>Disconnect</button></div>{/if}
    {#if local?.capabilities?.tools}
      <div class="details-tools"><div class="setting-title">Available tools</div><table class="setting-table"><thead><tr><th>Tool</th><th>Version</th></tr></thead><tbody>
        {#each Object.entries(local.capabilities.tools) as [tool, info] (tool)}<tr><td>{tool}</td><td>{info.available ? info.version || "available" : info.note || "not found"}</td></tr>{/each}
      </tbody></table></div>
    {/if}
    <div class="details-actions">
      <button type="button" class="btn btn-sm lp-control-outline" onclick={openAddress}>Custom companion address</button>
      <button type="button" class="btn btn-sm lp-control-outline" onclick={() => void doctorReport()}>Check local setup</button>
    </div>
    {#if doctor}<pre class="setting-log" role="status">{doctor}</pre>{/if}
  </div>
</Modal>

<Modal bind:open={addressOpen} title="Custom companion address" confirm={{ label: "Save address", onclick: saveAddress }}>
  <p class="setting-description">Use the default unless you started the companion at another address or port.</p>
  <label class="setting-title" for="local-address">Custom companion address</label>
  <input id="local-address" aria-label="Custom companion address" class="input input-sm setting-input" type="url" bind:value={addressDraft} />
</Modal>

<Modal bind:open={pairConfirmOpen} title="Allow local tools on this computer?" confirm={{ label: "Pair companion", onclick: confirmPairing }}>
  <p class="setting-description">Pairing lets this browser use tools made available by LibrePaper Companion. Pairing alone does not run document code; local code execution asks for permission separately.</p>
</Modal>

<style>
  .local-intro { max-width: 42rem; margin-block: 0 calc(var(--spacing) * 3); }
  .local-install { display: grid; gap: calc(var(--spacing) * 3); margin-block: calc(var(--spacing) * 4); }
  .install-option { display: grid; gap: calc(var(--spacing) * 1.5); }
  .command-line { display: flex; align-items: center; gap: calc(var(--spacing) * 2); min-width: 0; }
  .command-line code { flex: 1; min-width: 0; overflow-wrap: anywhere; padding: calc(var(--spacing) * 2); border-radius: var(--radius-container); background: var(--color-subtle); }
  .pairing { display: grid; gap: calc(var(--spacing) * 2); margin-block: calc(var(--spacing) * 4); }
  .pair-controls { display: flex; gap: calc(var(--spacing) * 2); }
  .pair-controls .setting-input { flex: 1; }
  .quiet-link { color: var(--panel-muted); font-size: var(--panel-meta-size); text-decoration: underline; text-underline-offset: 2px; background: none; border: 0; padding: 0; cursor: pointer; }
  .quiet-link:hover { color: var(--color-brand); }
  .local-footer { display: flex; flex-wrap: wrap; gap: calc(var(--spacing) * 3); margin-top: calc(var(--spacing) * 5); }
  .local-error { margin-block: calc(var(--spacing) * 2); color: var(--color-error-text); }
  .local-execution-switch { appearance: none; border: 0; padding: 0; cursor: pointer; }
  .local-execution-switch:focus-visible { outline: 2px solid var(--color-brand); outline-offset: 2px; }
  .companion-startup-switch { appearance: none; border: 0; padding: 0; cursor: pointer; }
  .companion-startup-switch:focus-visible { outline: 2px solid var(--color-brand); outline-offset: 2px; }
  .companion-section { display: grid; gap: calc(var(--spacing) * 3); margin-block: calc(var(--spacing) * 4); }
  .preset-form { display: grid; gap: calc(var(--spacing) * 2); }
  .form-field { display: grid; gap: calc(var(--spacing) * 1); }
  .form-field label { font-size: var(--panel-meta-size); }
  .form-field textarea { min-height: 3rem; resize: vertical; }
  .grant-form { display: grid; gap: calc(var(--spacing) * 2); }
  .grants-list { display: grid; gap: calc(var(--spacing) * 2); margin-bottom: calc(var(--spacing) * 3); }
  .grant-item { display: flex; align-items: center; justify-content: space-between; padding: calc(var(--spacing) * 2); background: var(--color-subtle); border-radius: var(--radius-container); }
  .quit-button { display: grid; gap: calc(var(--spacing) * 2); }
  .details-content { display: grid; gap: calc(var(--spacing) * 3); }
  .details-tools { display: grid; gap: calc(var(--spacing) * 2); }
  .details-actions { display: flex; flex-wrap: wrap; gap: calc(var(--spacing) * 2); }
  .setting-log { max-height: 18rem; overflow: auto; white-space: pre-wrap; overflow-wrap: anywhere; }
  @media (max-width: 700px) { .command-line, .pair-controls { align-items: stretch; flex-direction: column; } }
</style>
