<script>
  import SettingRow from "./SettingRow.svelte";
  import * as localBridge from "../../lib/companion/client.js";
  import { companion } from "../../lib/companion/status.svelte.js";

  const local = $derived(companion.status);
  $effect(() => companion.watch());
  $effect(() => void localBridge.probe());

  let address = $state(localBridge.address());
  let addressDraft = $state(localBridge.address());
  let connecting = $state(false);
  let doctor = $state("");
  let connectionError = $state("");
  let companionSettings = $state(null);
  let settingsError = $state("");
  let pendingDialogAction = $state("");
  let settingsRequest = 0;
  let doctorRequest = 0;

  $effect(() => {
    if (!connected) return;
    connectionError = "";
  });
  const connected = $derived(local?.state === "connected");
  const settingsScope = $derived(connected ? `${address}\n${local?.address || ""}\n${JSON.stringify(local?.instance ?? null)}` : "");
  function currentSettingsScope() { return settingsScope; }
  const tone = $derived(connected ? "good" : ["denied", "incompatible"].includes(local?.state) ? "warn" : "off");
  const status = $derived(({ unknown: "Companion not connected", unreachable: "Companion not running", denied: "Local network access blocked", reachable: "Companion found", unauthorized: "Companion needs permission", connected: "Companion connected", incompatible: "Companion needs an update" })[local?.state] || "Companion not connected");

  async function pair() {
    if (connecting) return;
    connectionError = "";
    connecting = true;
    try {
      await localBridge.connectApp();
    } catch (error) {
      connectionError = error?.message || "Could not open the companion permission window.";
    }
    finally { connecting = false; }
  }

  async function doctorReport() {
    if (!connected) return;
    const scope = currentSettingsScope();
    const request = ++doctorRequest;
    try {
      const capabilities = await localBridge.capabilities({ rescan: true });
      if (request === doctorRequest && currentSettingsScope() === scope) doctor = JSON.stringify(capabilities, null, 2);
    } catch (error) {
      if (request === doctorRequest && currentSettingsScope() === scope) doctor = error?.message || "Local setup check could not reach the companion.";
    }
  }

  function saveAddress() {
    settingsRequest++;
    companionSettings = null;
    settingsError = "";
    doctor = "";
    localBridge.setAddress(addressDraft);
    address = addressDraft;
    void localBridge.retry();
  }

  async function loadSettings(scope, request) {
    try {
      const result = await localBridge.settings();
      if (request === settingsRequest && currentSettingsScope() === scope) {
        companionSettings = result;
        settingsError = "";
      }
    } catch (error) {
      if (request === settingsRequest && currentSettingsScope() === scope) {
        settingsError = error?.message || "Could not load companion settings.";
      }
    }
  }

  async function callDialogAction(action) {
    const scope = currentSettingsScope();
    const actionRequest = settingsRequest;
    try {
      await action();
      if (!scope || actionRequest !== settingsRequest || currentSettingsScope() !== scope) return;
      const request = ++settingsRequest;
      companionSettings = null;
      await loadSettings(scope, request);
    } finally {
      pendingDialogAction = "";
    }
  }

  async function toggleStartup(enabled) {
    if (!connected || !companionSettings?.standalone || companionSettings.startup == null || pendingDialogAction) return;
    pendingDialogAction = "startup";
    try {
      await callDialogAction(() => localBridge.setStartup(enabled));
    } catch (error) {
      settingsError = error?.message || "Could not change startup setting.";
    }
  }

  async function quitCompanion() {
    if (!connected || !companionSettings?.standalone || pendingDialogAction) return;
    pendingDialogAction = "quit";
    try {
      await callDialogAction(() => localBridge.quit());
      await localBridge.retry();
    } catch (error) {
      settingsError = error?.message || "Could not quit companion.";
    }
  }

  $effect(() => {
    const scope = currentSettingsScope();
    const request = ++settingsRequest;
    doctorRequest++;
    companionSettings = null;
    settingsError = "";
    doctor = "";
    if (scope) void loadSettings(scope, request);
  });
</script>

<p class="setting-description local-intro">Connect LibrePaper to apps and tools installed on this computer: coding agents, Zotero, Quarto, and local project folders.</p>

<div id="local-status" class="setting-status" data-tone={tone}>
  <span class="setting-status-dot" aria-hidden="true"></span>
  <div class="setting-status-words">
    <div class="setting-title" role="status">{status}</div>
    <div class="setting-description">
      {#if connected}
        Local tools are available to LibrePaper.
      {:else if local?.state === "denied"}
        Your browser blocked this site from reaching the companion. Allow local network access for this site in the browser's site settings, then connect.
      {:else if local?.state === "incompatible"}
        Install the latest companion version, then connect.
      {:else if local?.state === "unauthorized" || local?.state === "reachable"}
        The companion is running. Connect to approve access for this site.
      {:else if local?.state === "unreachable"}
        Nothing answered on this computer. Start the companion with <code>librepaper</code> in a terminal, then connect.
      {:else}
        Install the companion and start it with <code>librepaper</code> to use local tools.
      {/if}
    </div>
  </div>
  <div class="setting-control">
    {#if local?.state !== "connected"}
      <button type="button" class="btn btn-sm lp-control-brand" disabled={connecting} onclick={pair}>{connecting ? "Connecting…" : "Connect"}</button>
    {/if}
  </div>
</div>
{#if connectionError}<p class="setting-description local-error" role="alert">{connectionError}</p>{/if}

{#if !connected}
  <section id="local-install-help" class="local-install" aria-label="Install LibrePaper Companion">
    <h4 class="settings-subhead">Install LibrePaper Companion</h4>
    <p class="setting-description">Follow the <a href="https://librepaper.org/install.html" target="_blank" rel="noreferrer">install instructions</a> for macOS, Linux and Windows. Then run <code>librepaper</code> in a terminal and return here to connect.</p>
  </section>
{/if}

<SettingRow id="local-address" title="Companion address" description={local?.version ? `Version ${local.version}. Change it only if you started the companion on another port.` : "Change it only if you started the companion on another port."}>
  <input aria-label="Companion address" class="input input-sm setting-input" type="url" bind:value={addressDraft} />
  <button type="button" class="btn btn-sm lp-control-outline" disabled={addressDraft === address} onclick={saveAddress}>Save</button>
  <button type="button" class="btn btn-sm lp-control-outline" disabled={!connected} onclick={() => void localBridge.disconnect()}>Disconnect</button>
</SettingRow>

{#if settingsError}<p class="setting-description local-error" role="alert">{settingsError}</p>{/if}

<SettingRow id="local-startup" title="Start at login" description={!connected ? "Connect to check whether startup controls are available." : companionSettings?.standalone === false ? "Available when the companion is installed as a standalone app." : companionSettings?.startup === null ? "Startup preference is unavailable for this companion." : "Open LibrePaper Companion when you log in."}>
    <button type="button" role="switch" class="switch companion-startup-switch" aria-label="Start at login" aria-checked={companionSettings?.startup === true} data-state={companionSettings?.startup ? "checked" : "unchecked"} disabled={!connected || !companionSettings?.standalone || companionSettings.startup == null || Boolean(pendingDialogAction)} onclick={() => void toggleStartup(!companionSettings.startup)}>
      <span class="switch-thumb" data-state={companionSettings?.startup ? "checked" : "unchecked"}></span>
    </button>
    {#if pendingDialogAction === "startup"}
      <p class="setting-description">Approve the request in the dialog LibrePaper Companion opened on this computer.</p>
    {/if}
  </SettingRow>

<div class="quit-button">
    <button type="button" class="btn btn-sm lp-control-outline" disabled={!connected || !companionSettings?.standalone || Boolean(pendingDialogAction)} onclick={() => void quitCompanion()}>
      {pendingDialogAction === "quit" ? "Confirm on this computer…" : "Quit companion"}
    </button>
    {#if connected && companionSettings && !companionSettings.standalone}
      <p class="setting-description">Available when the companion is installed as a standalone app.</p>
    {:else if !connected}
      <p class="setting-description">Connect to use this control.</p>
    {/if}
    {#if pendingDialogAction === "quit"}
      <p class="setting-description">Approve the request in the dialog LibrePaper Companion opened on this computer.</p>
    {/if}
  </div>

<SettingRow title="Check local setup" description="Rescans the tools the companion can find and shows the full report.">
  <button type="button" class="btn btn-sm lp-control-outline" id="local-doctor" disabled={!connected} onclick={() => void doctorReport()}>Check</button>
</SettingRow>
{#if doctor}<pre class="setting-log" role="status">{doctor}</pre>{/if}

<p class="setting-description local-help">Still stuck? <a href="https://github.com/LibrePaper/librepaper/issues" target="_blank" rel="noreferrer">Report a problem</a> with the setup report attached.</p>

<style>
  .local-intro { max-width: 42rem; margin-block: 0 calc(var(--spacing) * 3); }
  .local-install { display: grid; gap: calc(var(--spacing) * 3); margin-block: calc(var(--spacing) * 4); }
  .local-error { margin-block: calc(var(--spacing) * 2); color: var(--color-error-text); }
  .companion-startup-switch { appearance: none; border: 0; padding: 0; cursor: pointer; }
  .companion-startup-switch:focus-visible { outline: 2px solid var(--color-brand); outline-offset: 2px; }
  .quit-button { display: grid; gap: calc(var(--spacing) * 2); }
  .local-help { margin-top: calc(var(--spacing) * 3); }
  .setting-log { max-height: 18rem; overflow: auto; white-space: pre-wrap; overflow-wrap: anywhere; }
</style>
