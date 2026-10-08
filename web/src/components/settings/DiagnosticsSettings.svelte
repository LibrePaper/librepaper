<script>
  import SettingRow from "./SettingRow.svelte";
  import * as localBridge from "../../lib/companion/client.js";
  import { companion } from "../../lib/companion/status.svelte.js";

  let { view } = $props();
  const active = (status) => !["done", "complete", "completed", "succeeded", "failed", "error", "cancelled", "canceled", "stopped", "interrupted", "expired", "denied"].includes(String(status || "").toLowerCase());

  const local = $derived(companion.status);
  $effect(() => companion.watch());
  $effect(() => void localBridge.probe());

  let address = $state(localBridge.address());
  let addressDraft = $state(localBridge.address());
  let addressSaving = $state(false);
  let addressSaved = $state(false);
  let addressSavedScope = "";
  let doctor = $state("");
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

  const connected = $derived(local?.state === "connected");
  const settingsScope = $derived(connected ? `${address}\n${local?.address || ""}\n${JSON.stringify(local?.instance ?? null)}` : "");
  function currentSettingsScope() { return settingsScope; }

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

<p class="setting-description diagnostics-intro">Details for troubleshooting the companion on this computer.</p>


{#if !view.available}
<SettingRow id="diagnostics-address" title="Companion address" description={local?.version ? `Version ${local.version}. Change only when using another port.` : "Change only when using another port."}>
  <div class="setting-actions">
    <input aria-label="Companion address" class="input input-sm setting-input" type="url" bind:value={addressDraft} disabled={addressSaving} />
    <button type="button" class="btn btn-sm lp-control-outline" disabled={addressDraft === address || addressSaving} onclick={() => void saveAddress()}>{addressSaving ? "Saving…" : "Save"}</button>
    {#if addressSaved && !addressSaving}<span id="diagnostics-address-feedback" class="setting-feedback" role="status">Saved</span>{/if}
    <button type="button" class="btn btn-sm lp-control-outline" disabled={!connected} onclick={() => void localBridge.disconnect()}>Disconnect</button>
  </div>
</SettingRow>
{/if}

{#if settingsError}<p class="setting-description diagnostics-error" role="alert">{settingsError}</p>{/if}

{#if view.available}
  {#if view.error}
    <p class="setting-description management-error" role="alert">{view.error}</p>
  {/if}
  {#if view.loadError}
    <p class="setting-description management-error" role="alert">{view.loadError}</p>
  {/if}
  {#if view.notice}
    <p class="setting-description management-notice" role="status">{view.notice}</p>
  {/if}

  <section class="settings-subsection" aria-labelledby="diagnostics-companion-heading">
    <div class="settings-section-title"><h4 class="settings-subhead" id="diagnostics-companion-heading">Companion</h4>{#if view.state?.version}<span class="setting-description">Version {view.state.version}</span>{/if}</div>
    {#if view.state?.standalone === true}
    <SettingRow id="diagnostics-startup" title="Start at login" description="Open the companion when you log in.">
      <button type="button" role="switch" class="switch" class:checked={view.state?.settings?.startup_enabled === true} data-state={view.state?.settings?.startup_enabled == null ? "unknown" : view.state.settings.startup_enabled ? "checked" : "unchecked"} aria-label="Start at login" aria-checked={view.state?.settings?.startup_enabled === true} disabled={!view.state || typeof view.state.settings?.startup_enabled !== "boolean" || Boolean(view.pending)} onclick={() => void view.act("startup", "Startup preference saved.", "/settings", { method: "PUT", body: { startup_enabled: !view.state.settings.startup_enabled } })}>
        <span class="switch-thumb" data-state={view.state?.settings?.startup_enabled == null ? "unknown" : view.state.settings.startup_enabled ? "checked" : "unchecked"}></span>
      </button>
    </SettingRow>
    <SettingRow title="Quit companion" description="Close the companion on this computer.">
      <button class="btn btn-sm lp-control-outline" type="button" disabled={Boolean(view.pending)} onclick={() => { if (confirm("Quit LibrePaper companion? Connected sites will no longer reach local tools until it is started again.")) void view.act("quit", "Quit request sent.", "/quit", { method: "POST" }); }}>Quit companion</button>
    </SettingRow>
    {/if}
  </section>

  <section id="diagnostics-activity" class="settings-subsection" aria-labelledby="diagnostics-activity-heading">
    <div class="settings-section-title"><h4 class="settings-subhead" id="diagnostics-activity-heading">Activity</h4></div>
    {#each view.list(view.state?.jobs) as job (job.id)}
      <SettingRow title={[job.kind, job.stage].filter(Boolean).join(" · ") || "Local job"} description={`${job.status || "running"}${job.error ? ` · ${job.error}` : ""}${job.log_tail ? `\n${Array.isArray(job.log_tail) ? job.log_tail.join("\n") : job.log_tail}` : ""}`} stacked>
        {#if active(job.status)}<button class="btn btn-sm lp-control-outline" type="button" disabled={Boolean(view.pending)} onclick={() => void view.act(`job-${job.id}`, "Cancellation requested.", `/jobs/${view.id(job.id)}/cancel`, { method: "POST" })}>Cancel</button>{/if}
      </SettingRow>
    {/each}
    {#each view.list(view.state?.previews) as preview (preview.id)}
      <SettingRow title={preview.label || preview.project || "Preview"} description={`${preview.status || "active"}${preview.log_tail ? `\n${preview.log_tail}` : ""}`} stacked>
        <button class="btn btn-sm lp-control-outline" type="button" disabled={Boolean(view.pending)} onclick={() => void view.act(`preview-${preview.id}`, "Preview stopped.", `/previews/${view.id(preview.id)}`, { method: "DELETE" })}>Stop preview</button>
      </SettingRow>
    {/each}
    {#each view.list(view.state?.sessions) as session (session.id)}
      {@const sessionStatus = session.status || session.state || "active"}
      <SettingRow title={session.name || session.agent || session.id || "Agent session"} description={`${sessionStatus}${session.task || session.task_id || session.detail ? ` · ${session.task || session.task_id || session.detail}` : ""}`}>
        {#if active(sessionStatus)}<button class="btn btn-sm lp-control-outline" type="button" disabled={Boolean(view.pending)} onclick={() => void view.act(`session-${session.id}`, "Agent stop requested.", `/agents/sessions/${view.id(session.id)}/cancel`, { method: "POST" })}>Stop</button>{/if}
      </SettingRow>
    {/each}
    {#if !view.list(view.state?.jobs).length && !view.list(view.state?.previews).length && !view.list(view.state?.sessions).length}
    <p class="setting-description">Nothing running.</p>
    {/if}
  </section>
{:else}
{#if connected && companionSettings?.standalone === true}
<SettingRow id="diagnostics-startup" title="Start at login" description="Open the companion when you log in.">
    <button type="button" role="switch" class="switch companion-startup-switch" aria-label="Start at login" aria-checked={companionSettings?.startup === true} data-state={companionSettings.startup ? "checked" : "unchecked"} disabled={companionSettings.startup == null || Boolean(pendingDialogAction)} onclick={() => void toggleStartup(!companionSettings.startup)}>
      <span class="switch-thumb" data-state={companionSettings.startup ? "checked" : "unchecked"}></span>
    </button>
    {#if pendingDialogAction === "startup"}
      <p class="setting-description">Answer the request on this computer.</p>
    {/if}
  </SettingRow>

<SettingRow title="Quit companion" description="Close the companion on this computer.">
    <button type="button" class="btn btn-sm lp-control-outline" disabled={Boolean(pendingDialogAction)} onclick={() => void quitCompanion()}>
      {pendingDialogAction === "quit" ? "Confirm on this computer…" : "Quit companion"}
    </button>
    {#if pendingDialogAction === "quit"}
      <p class="setting-description">Answer the request on this computer.</p>
    {/if}
  </SettingRow>
{/if}

<SettingRow id="diagnostics-report" title="Check local setup" description={!connected ? "Connect to rescan available tools." : "Rescan available tools and view the report."}>
  <button type="button" class="btn btn-sm lp-control-outline" disabled={!connected} onclick={() => void doctorReport()}>Check</button>
</SettingRow>
{#if doctor}<pre class="setting-log" role="status">{doctor}</pre>{/if}
{/if}

<p class="setting-description diagnostics-help">Still stuck? <a href="https://github.com/LibrePaper/librepaper/issues" target="_blank" rel="noreferrer">Report a problem</a> with the setup report attached.</p>

<style>
  .diagnostics-intro { max-width: 42rem; margin-block: 0 calc(var(--spacing) * 3); }
  .diagnostics-error, .management-error { color: var(--color-error-text); }
  .management-notice { color: var(--color-success-text); }
  .companion-startup-switch { appearance: none; border: 0; padding: 0; cursor: pointer; }
  .companion-startup-switch:focus-visible { outline: 2px solid var(--color-brand); outline-offset: 2px; }
  .diagnostics-help { margin-top: calc(var(--spacing) * 3); }
  .setting-log { max-height: 18rem; overflow: auto; white-space: pre-wrap; overflow-wrap: anywhere; }
</style>
