<script>
  import SettingRow from "./SettingRow.svelte";
  import * as localBridge from "../../lib/companion/client.js";
  import { companion } from "../../lib/companion/status.svelte.js";
  import { backups } from "../../lib/companion/backups.svelte.js";
  import { me, post } from "../../lib/api.js";

  let { account = {} } = $props();
  const accountId = $derived(typeof account.id === "string" ? account.id : "");
  const signedIn = $derived(Boolean(account.provider && accountId));
  const paired = $derived(companion.status.state === "connected");
  const backupStatus = $derived(backups.status);
  const data = $derived(backupStatus.data || {});
  const intervals = [1, 5, 15, 30, 60];

  let busy = $state("");
  let error = $state("");
  let connecting = $state(false);
  let requestScope = 0;
  let lastScope = "";
  const pairingAddress = $derived(companion.status.address || "");
  const pairingInstance = $derived(companion.status.instance || "");

  $effect(() => {
    const nextScope = `${accountId}\u0000${paired}\u0000${pairingAddress}\u0000${pairingInstance}`;
    backups.setScope(accountId, paired);
    if (nextScope !== lastScope) {
      lastScope = nextScope;
      requestScope += 1;
      busy = "";
      error = "";
    }
  });

  async function connect() {
    if (connecting) return;
    connecting = true;
    error = "";
    const actionScope = requestScope;
    try {
      await localBridge.connectApp();
    } catch (cause) {
      if (actionScope === requestScope) error = cause?.message || "Could not connect to LibrePaper Companion.";
    } finally {
      connecting = false;
    }
  }

  async function authorize() {
    if (!signedIn || !paired || busy) return;
    busy = "authorization";
    error = "";
    const requestedAccount = accountId;
    const requestedProvider = account.provider;
    const actionScope = requestScope;
    const address = pairingAddress;
    const instance = pairingInstance;
    const isCurrent = () => actionScope === requestScope && accountId === requestedAccount && signedIn && paired
      && pairingAddress === address && pairingInstance === instance;
    try {
      await localBridge.authorizeBackups(requestedAccount, async (userCode, pairingCurrent) => {
        const browserAccount = await me();
        if (!pairingCurrent() || !isCurrent()) {
          throw new Error("The backup authorization scope changed. Retry authorization.");
        }
        if (browserAccount?.id !== requestedAccount || browserAccount?.provider !== requestedProvider) {
          throw new Error("The signed-in browser account changed. Retry backup authorization.");
        }
        await post("/api/auth/device/approve", { user_code: userCode });
      }, isCurrent);
      if (isCurrent()) await backups.refresh();
    } catch (cause) {
      if (isCurrent()) error = cause?.message || "Backup authorization could not be completed.";
    } finally {
      if (isCurrent()) busy = "";
    }
  }

  async function changeSettings(enabled, frequency = Number(data.frequency_minutes) || 5) {
    if (!signedIn || !paired || busy) return;
    busy = "settings";
    error = "";
    const requestedAccount = accountId;
    const actionScope = requestScope;
    try {
      await localBridge.updateBackups(requestedAccount, { enabled, frequency_minutes: frequency });
      if (actionScope === requestScope) await backups.refresh();
    } catch (cause) {
      if (actionScope === requestScope) error = cause?.message || "Backup settings could not be saved.";
    } finally {
      if (actionScope === requestScope) busy = "";
    }
  }

  async function chooseFolder() {
    if (!signedIn || !paired || busy) return;
    busy = "folder";
    error = "";
    const requestedAccount = accountId;
    const actionScope = requestScope;
    try {
      await localBridge.chooseBackupFolder(requestedAccount);
      if (actionScope === requestScope) await backups.refresh();
    } catch (cause) {
      if (actionScope === requestScope) error = cause?.message || "The companion could not choose a backup folder.";
    } finally {
      if (actionScope === requestScope) busy = "";
    }
  }

  async function runNow() {
    if (!signedIn || !paired || busy || !data.enabled || data.running) return;
    busy = "run";
    error = "";
    const requestedAccount = accountId;
    const actionScope = requestScope;
    try {
      await localBridge.runBackups(requestedAccount);
      if (actionScope === requestScope) await backups.refresh();
    } catch (cause) {
      if (actionScope === requestScope) error = cause?.message || "The backup could not be started.";
    } finally {
      if (actionScope === requestScope) busy = "";
    }
  }

  const destinationSet = $derived(Boolean(data.destination_set));
  const destinationLabel = $derived(typeof data.destination === "string" ? data.destination.trim() : "");
</script>

<p class="setting-description backups-intro">
  Save every project available to this account, including shared projects, as ZIP files on this computer. The companion runs the schedule in the background, including while browser tabs are closed. Each project keeps its latest ZIP; backups are retained when you disable this setting or delete a project. Backups do not sync changes back to LibrePaper.
  <a href="https://librepaper.org/backups.html" target="_blank" rel="noreferrer">Backup guide</a>
</p>

{#if !signedIn}
  <div class="setting-status" data-tone="off" role="status">
    <span class="setting-status-dot" aria-hidden="true"></span>
    <div class="setting-status-words">
      <div class="setting-title">Sign in to back up an account</div>
      <div class="setting-description">Account-wide backups are available after you sign in to this LibrePaper server.</div>
    </div>
  </div>
{:else if !paired}
  <div class="setting-status" data-tone="off" role="status">
    <span class="setting-status-dot" aria-hidden="true"></span>
    <div class="setting-status-words">
      <div class="setting-title">Connect LibrePaper Companion</div>
      <div class="setting-description">Start <code>librepaper</code> in a terminal, then connect. The companion runs the schedule on this computer.</div>
    </div>
    <div class="setting-control"><button type="button" class="btn btn-sm lp-control-brand" disabled={connecting} onclick={() => void connect()}>{connecting ? "Connecting…" : "Connect"}</button></div>
  </div>
{:else if data.needs_login}
  <div class="setting-status" data-tone="warn" role="status">
    <span class="setting-status-dot" aria-hidden="true"></span>
    <div class="setting-status-words">
      <div class="setting-title">Authorize backups</div>
      <div class="setting-description">Connect the companion to this browser account to let it back up your projects.</div>
    </div>
    <div class="setting-control"><button type="button" class="btn btn-sm lp-control-brand" disabled={busy !== "" || backupStatus.loading} onclick={() => void authorize()}>{busy === "authorization" ? "Authorizing…" : "Authorize backups"}</button></div>
  </div>
  <SettingRow id="backup-destination" title="Backup folder" description={destinationSet ? (destinationLabel ? `Selected folder: ${destinationLabel}` : "Selected folder on this computer.") : "Choose a folder on this computer for the ZIP files."}>
    <div class="backup-folder-control">
      <input class="input input-sm backup-path" aria-label="Selected backup folder" value={destinationLabel} placeholder="No folder selected" readonly />
      <button type="button" class="btn btn-sm lp-control-outline" disabled={busy !== ""} onclick={() => void chooseFolder()}>{busy === "folder" ? "Choosing…" : destinationSet ? "Change folder…" : "Choose folder…"}</button>
    </div>
  </SettingRow>
  {#if data.enabled}
    <SettingRow id="backup-enable" title="Automatic backups are on" description="You can turn off future backups without signing in again. A backup already in progress may finish.">
      <button type="button" role="switch" class="switch backup-enable-switch" aria-label="Automatic backups" aria-checked="true" data-state="checked" disabled={busy !== ""} onclick={() => void changeSettings(false)}>
        <span class="switch-thumb" data-state="checked"></span>
      </button>
    </SettingRow>
  {/if}
{:else if backupStatus.error}
  <div class="setting-status" data-tone="warn" role="status">
    <span class="setting-status-dot" aria-hidden="true"></span>
    <div class="setting-status-words"><div class="setting-title">Backup status unavailable</div><div class="setting-description">{backupStatus.error}</div></div>
    <div class="setting-control"><button type="button" class="btn btn-sm lp-control-outline" onclick={() => void backups.refresh()}>Retry</button></div>
  </div>
{:else if backupStatus.loading && !backupStatus.data}
  <p class="setting-description" role="status">Loading backup status…</p>
{:else}
  <SettingRow id="backup-enable" title="Automatic backups" description="Create a fresh ZIP of each project available to this account on this schedule.">
    <button type="button" role="switch" class="switch backup-enable-switch" aria-label="Automatic backups" aria-checked={Boolean(data.enabled)} data-state={data.enabled ? "checked" : "unchecked"} disabled={busy !== "" || !destinationSet} onclick={() => void changeSettings(!data.enabled)}>
      <span class="switch-thumb" data-state={data.enabled ? "checked" : "unchecked"}></span>
    </button>
  </SettingRow>
  <SettingRow id="backup-destination" title="Backup folder" description={destinationSet ? (destinationLabel ? `Selected folder: ${destinationLabel}` : "Selected folder on this computer.") : "Choose a folder on this computer for the ZIP files."}>
    <div class="backup-folder-control">
      <input class="input input-sm backup-path" aria-label="Selected backup folder" value={destinationLabel} placeholder="No folder selected" readonly />
      <button type="button" class="btn btn-sm lp-control-outline" disabled={busy !== ""} onclick={() => void chooseFolder()}>{busy === "folder" ? "Choosing…" : destinationSet ? "Change folder…" : "Choose folder…"}</button>
    </div>
  </SettingRow>
  <SettingRow id="backup-frequency" title="Frequency" description="How often the companion checks for project changes.">
    <select class="input input-sm backup-frequency" aria-label="Backup frequency" value={Number(data.frequency_minutes) || 5} disabled={busy !== "" || !destinationSet} onchange={(event) => void changeSettings(Boolean(data.enabled), Number(event.currentTarget.value))}>
      {#each intervals as minutes}<option value={minutes}>{minutes} {minutes === 1 ? "minute" : "minutes"}</option>{/each}
    </select>
  </SettingRow>
  <div class="backup-status" role="status" aria-live="polite">
    {#if data.error}
      <span class="backup-state error">Backup error: {data.error}</span>
    {:else if data.running}
      <span class="backup-state">Backing up {Number(data.projects) || 0} projects…</span>
    {:else if data.enabled}
      <span class="backup-state">{data.last_success ? `Last backup ${new Date(Number(data.last_success) * 1000).toLocaleString()}` : "Backups are enabled; the first run is pending."}</span>
    {:else}
      <span class="backup-state">Backups are off.</span>
    {/if}
    <button type="button" class="btn btn-sm lp-control-outline" disabled={busy !== "" || !destinationSet || !data.enabled || data.running} onclick={() => void runNow()}>{busy === "run" ? "Starting…" : "Back up now"}</button>
  </div>
{/if}

{#if error}<p class="setting-description backup-error" role="alert">{error}</p>{/if}

<style>
  .backups-intro { max-width: 48rem; margin-block: 0 calc(var(--spacing) * 3); }
  .backups-intro a { margin-inline-start: .35rem; }
  .backup-folder-control { display: flex; align-items: center; gap: calc(var(--spacing) * 2); }
  .backup-path { width: min(24rem, 42vw); }
  .backup-frequency { min-width: 8rem; }
  .backup-status { display: flex; align-items: center; justify-content: space-between; gap: calc(var(--spacing) * 3); margin-top: calc(var(--spacing) * 3); }
  .backup-state { color: var(--color-text-secondary); font-size: var(--text-sm); }
  .backup-state.error, .backup-error { color: var(--color-error-text); }
  @media (max-width: 42rem) { .backup-status { align-items: flex-start; flex-direction: column; } .backup-folder-control { align-items: stretch; flex-direction: column; } .backup-path { width: 100%; } }
</style>
