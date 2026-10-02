<script>
  import SettingRow from "./SettingRow.svelte";
  import StatusPill from "./StatusPill.svelte";
  import * as localBridge from "../../lib/companion/client.js";
  import { companion } from "../../lib/companion/status.svelte.js";

  const local = $derived(companion.status);
  $effect(() => companion.watch());
  $effect(() => void localBridge.probe());

  let address = $state(localBridge.address());
  let addressDraft = $state(localBridge.address());
  let addressSaving = $state(false);
  let addressSaved = $state(false);
  let addressSavedScope = "";
  let connecting = $state(false);
  let doctor = $state("");
  let connectionError = $state("");
  let companionSettings = $state(null);
  let settingsError = $state("");
  let pendingDialogAction = $state("");
  let settingsRequest = 0;
  let doctorRequest = 0;

  $effect(() => {
    const draft = addressDraft;
    if (draft !== address) addressSaved = false;
  });

  $effect(() => {
    const scope = `${local?.state || ""}\n${local?.address || ""}\n${JSON.stringify(local?.instance ?? null)}`;
    if (addressSaved && addressSavedScope && scope !== addressSavedScope) addressSaved = false;
  });

  $effect(() => {
    if (!connected) return;
    connectionError = "";
  });
  const connected = $derived(local?.state === "connected");
  const settingsScope = $derived(connected ? `${address}\n${local?.address || ""}\n${JSON.stringify(local?.instance ?? null)}` : "");
  function currentSettingsScope() { return settingsScope; }
  const status = $derived(({ unknown: "Unknown", unreachable: "Disconnected", denied: "Access blocked", reachable: "Needs approval", unauthorized: "Needs approval", connected: "Connected", incompatible: "Update needed" })[local?.state] || "Unknown");
  const statusTone = $derived(connected ? "good" : ["denied", "incompatible"].includes(local?.state) ? "warn" : "neutral");
  const statusDescription = $derived(({ denied: "Allow local network access for this site in your browser settings.", incompatible: "Update the companion to continue.", unauthorized: "Approve access in the companion.", reachable: "Approve access in the companion.", unreachable: "Start the companion on this computer.", connected: "Local tools are ready." })[local?.state] || "Connect to the companion on this computer.");

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

  async function saveAddress() {
    let normalized;
    try {
      if (!addressDraft.trim()) normalized = localBridge.DEFAULT_ADDRESS;
      else {
        const url = new URL(addressDraft.trim());
        if (!/^https?:$/.test(url.protocol) || !url.host) throw new Error();
        normalized = url.pathname.endsWith("/") ? url.href : `${url.href}/`;
      }
    } catch {
      settingsError = "Enter a valid HTTP or HTTPS companion address.";
      addressSaved = false;
      return;
    }
    addressSaving = true;
    addressSaved = false;
    settingsRequest++;
    companionSettings = null;
    settingsError = "";
    doctor = "";
    try {
      localBridge.setAddress(addressDraft);
      address = localBridge.address();
      if (addressDraft.trim() && address !== normalized) {
        settingsError = "The companion address could not be saved.";
        return;
      }
      const savedAddress = address;
      await localBridge.retry();
      if (localBridge.address() === savedAddress) {
        addressSavedScope = `${local?.state || ""}\n${local?.address || ""}\n${JSON.stringify(local?.instance ?? null)}`;
        addressDraft = savedAddress;
        addressSaved = true;
      }
    } catch (error) {
      addressSaved = false;
      settingsError = error?.message || "Could not connect at this address.";
    } finally {
      addressDraft = address;
      addressSaving = false;
    }
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

<p class="setting-description local-intro">Connect apps and tools on this computer, including coding agents, Zotero, and Quarto.</p>

<SettingRow id="local-status" title="Local tools" description={statusDescription}>
  <div class="setting-actions">
    <StatusPill label={status} tone={statusTone} accessibleLabel={`Companion ${status.toLowerCase()}`} />
    {#if local?.state !== "connected"}
      <button type="button" class="btn btn-sm lp-control-brand" disabled={connecting} onclick={pair}>{connecting ? "Connecting…" : "Connect"}</button>
    {/if}
  </div>
</SettingRow>
{#if connectionError}<p class="setting-description local-error" role="alert">{connectionError}</p>{/if}

{#if !connected}
  <section id="local-install-help" class="local-install" aria-label="Install LibrePaper Companion">
    <h4 class="settings-subhead">Install LibrePaper Companion</h4>
    <p class="setting-description">Follow the <a href="https://librepaper.org/install.html" target="_blank" rel="noreferrer">install instructions</a> for macOS, Linux and Windows. Then run <code>librepaper</code> in a terminal and return here to connect.</p>
  </section>
{/if}

<SettingRow id="local-address" title="Companion address" description={local?.version ? `Version ${local.version}. Change only when using another port.` : "Change only when using another port."}>
  <div class="setting-actions">
    <input aria-label="Companion address" class="input input-sm setting-input" type="url" bind:value={addressDraft} disabled={addressSaving} />
    <button type="button" class="btn btn-sm lp-control-outline" disabled={addressDraft === address || addressSaving} onclick={() => void saveAddress()}>{addressSaving ? "Saving…" : "Save"}</button>
    {#if addressSaved && !addressSaving}<span id="local-address-feedback" class="setting-feedback" role="status">Saved</span>{/if}
    <button type="button" class="btn btn-sm lp-control-outline" disabled={!connected} onclick={() => void localBridge.disconnect()}>Disconnect</button>
  </div>
</SettingRow>

{#if settingsError}<p class="setting-description local-error" role="alert">{settingsError}</p>{/if}

<SettingRow id="local-startup" title="Start at login" description={!connected ? "Connect to load this setting." : companionSettings?.standalone === false ? "Available in the standalone companion app." : companionSettings?.startup == null ? (companionSettings ? "Startup preference unavailable." : "Startup preference is unknown.") : "Open the companion when you log in."}>
    {#if !companionSettings || companionSettings.startup == null}<StatusPill label={companionSettings ? "Unavailable" : "Unknown"} />{/if}
    <button type="button" role="switch" class="switch companion-startup-switch" aria-label="Start at login" aria-checked={companionSettings?.startup === true} aria-describedby={!companionSettings || companionSettings.startup == null ? "local-startup-state" : undefined} data-state={companionSettings?.startup == null ? "unknown" : companionSettings.startup ? "checked" : "unchecked"} disabled={!connected || !companionSettings?.standalone || companionSettings.startup == null || Boolean(pendingDialogAction)} onclick={() => void toggleStartup(!companionSettings.startup)}>
      <span class="switch-thumb" data-state={companionSettings?.startup == null ? "unknown" : companionSettings.startup ? "checked" : "unchecked"}></span>
    </button>
    {#if !companionSettings || companionSettings.startup == null}<span id="local-startup-state" class="sr-only">{companionSettings ? "Unavailable" : "Unknown"}</span>{/if}
    {#if pendingDialogAction === "startup"}
      <p class="setting-description">Approve the request in the dialog LibrePaper Companion opened on this computer.</p>
    {/if}
  </SettingRow>

<SettingRow title="Quit companion" description={!connected ? "Connect to use this control." : !companionSettings?.standalone ? "Available in the standalone companion app." : "Close the companion running on this computer."}>
    <button type="button" class="btn btn-sm lp-control-outline" disabled={!connected || !companionSettings?.standalone || Boolean(pendingDialogAction)} onclick={() => void quitCompanion()}>
      {pendingDialogAction === "quit" ? "Confirm on this computer…" : "Quit companion"}
    </button>
    {#if pendingDialogAction === "quit"}
      <p class="setting-description">Approve the request in the dialog LibrePaper Companion opened on this computer.</p>
    {/if}
  </SettingRow>

<SettingRow title="Check local setup" description={!connected ? "Connect to rescan available tools." : "Rescan available tools and view the report."}>
  <button type="button" class="btn btn-sm lp-control-outline" id="local-doctor" disabled={!connected} onclick={() => void doctorReport()}>Check</button>
</SettingRow>
{#if doctor}<pre class="setting-log" role="status">{doctor}</pre>{/if}

<p class="setting-description local-help">Still stuck? <a href="https://github.com/LibrePaper/librepaper/issues" target="_blank" rel="noreferrer">Report a problem</a> with the setup report attached.</p>

<style>
  .local-intro { max-width: 42rem; margin-block: 0 calc(var(--spacing) * 3); }
  .local-install { display: grid; gap: calc(var(--spacing) * 3); margin-block: calc(var(--spacing) * 4); }
  .local-error { margin-block: calc(var(--spacing) * 2); color: var(--color-error-text); }
  .companion-startup-switch { appearance: none; border: 0; padding: 0; cursor: pointer; }
  .companion-startup-switch[data-state="unknown"] { background: var(--color-subtle); }
  .companion-startup-switch[data-state="unknown"] .switch-thumb { visibility: hidden; }
  .companion-startup-switch:focus-visible { outline: 2px solid var(--color-brand); outline-offset: 2px; }
  .local-help { margin-top: calc(var(--spacing) * 3); }
  .setting-log { max-height: 18rem; overflow: auto; white-space: pre-wrap; overflow-wrap: anywhere; }
</style>
