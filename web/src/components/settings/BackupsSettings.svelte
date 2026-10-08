<script>
  import SettingRow from "./SettingRow.svelte";
  import StatusPill from "./StatusPill.svelte";
  import ConnectionRow from "./ConnectionRow.svelte";
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
  const hasStatus = $derived(signedIn && paired && backupStatus.data !== null && !backupStatus.error);
  const needsLogin = $derived(hasStatus && Boolean(data.needs_login));
  const canUseCompanion = $derived(signedIn && paired && backupStatus.data !== null && !backupStatus.error);
  const intervals = [1, 5, 15, 30, 60];

  let busy = $state("");
  let error = $state("");
  let saved = $state(false);
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
      saved = false;
    }
  });

  async function authorize() {
    if (!signedIn || !paired || !needsLogin || busy) return;
    busy = "authorization";
    saved = false;
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
    if (!canUseCompanion || busy || (needsLogin && enabled)) return;
    busy = "settings";
    saved = false;
    error = "";
    const requestedAccount = accountId;
    const actionScope = requestScope;
    try {
      await localBridge.updateBackups(requestedAccount, { enabled, frequency_minutes: frequency });
      if (actionScope === requestScope) {
        await backups.refresh();
        if (actionScope === requestScope && !backups.status.error) saved = true;
      }
    } catch (cause) {
      if (actionScope === requestScope) error = cause?.message || "Backup settings could not be saved.";
    } finally {
      if (actionScope === requestScope) busy = "";
    }
  }

  async function chooseFolder() {
    if (!canUseCompanion || busy) return;
    busy = "folder";
    saved = false;
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
    if (!canUseCompanion || needsLogin || busy || !data.enabled || data.running) return;
    busy = "run";
    saved = false;
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
  Your account's projects, copied as ZIP files to a folder on this computer by the companion. <a href="https://librepaper.org/backups.html" target="_blank" rel="noreferrer">Backup guide</a>
</p>

{#if !signedIn}
  <SettingRow id="backup-connection" title="Account connection" description="Sign in to load account backup settings.">
    <StatusPill label="Signed out" />
  </SettingRow>
{:else if !paired}
  <ConnectionRow id="backup-connection" needs="backups" />
{:else if backupStatus.error}
  <div class="setting-status" data-tone="warn" role="alert">
    <span class="setting-status-dot" aria-hidden="true"></span>
    <div class="setting-status-words"><div class="setting-title">Backup status unavailable</div><div class="setting-description">{backupStatus.error}</div></div>
    <div class="setting-control"><button type="button" class="btn btn-sm lp-control-outline" disabled={busy !== ""} onclick={() => void backups.refresh()}>Retry</button></div>
  </div>
{:else if backupStatus.loading && !backupStatus.data}
  <p class="setting-description" role="status">Loading backup status…</p>
{:else if needsLogin}
  <div class="setting-status" data-tone="warn" role="status">
    <span class="setting-status-dot" aria-hidden="true"></span>
    <div class="setting-status-words">
      <div class="setting-title">Authorize backups</div>
      <div class="setting-description">Let the companion back up this account's projects.</div>
    </div>
    <div class="setting-control"><button type="button" class="btn btn-sm lp-control-brand" disabled={busy !== "" || backupStatus.loading} onclick={() => void authorize()}>{busy === "authorization" ? "Authorizing…" : "Authorize backups"}</button></div>
  </div>
{/if}

  <SettingRow id="backup-enable" title="Automatic backups" description={needsLogin ? "Authorize to enable backups." : !destinationSet ? "Choose a folder first." : ""}>
    {#if !hasStatus}<StatusPill label="Unknown" />{/if}
    <button type="button" role="switch" class="switch backup-enable-switch" aria-label="Automatic backups" aria-checked={Boolean(data.enabled)} aria-describedby={!hasStatus ? "backup-enable-state" : undefined} data-state={!hasStatus ? "unknown" : data.enabled ? "checked" : "unchecked"} disabled={busy !== "" || !canUseCompanion || (needsLogin ? !data.enabled : !destinationSet)} onclick={() => void changeSettings(!data.enabled)}>
      <span class="switch-thumb" data-state={!hasStatus ? "unknown" : data.enabled ? "checked" : "unchecked"}></span>
    </button>
    {#if !hasStatus}<span id="backup-enable-state" class="sr-only">Unknown</span>{/if}
  </SettingRow>
  <SettingRow id="backup-destination" title="Backup folder">
    <div class="backup-folder-control setting-actions">
      <input class="input input-sm setting-input backup-path" aria-label="Selected backup folder" value={destinationLabel} readonly disabled={!canUseCompanion || busy !== ""} />
      <button type="button" class="btn btn-sm lp-control-outline" disabled={!canUseCompanion || busy !== ""} onclick={() => void chooseFolder()}>{busy === "folder" ? "Choosing…" : destinationSet ? "Change folder…" : "Choose folder…"}</button>
    </div>
  </SettingRow>
  <SettingRow id="backup-frequency" title="Frequency">
    <select class="input input-sm setting-select backup-frequency" aria-label="Backup frequency" value={hasStatus ? Number(data.frequency_minutes) || 5 : ""} disabled={busy !== "" || !destinationSet || !canUseCompanion || needsLogin} onchange={(event) => void changeSettings(Boolean(data.enabled), Number(event.currentTarget.value))}>
      {#if !hasStatus}<option value="" disabled>Unknown</option>{/if}
      {#each intervals as minutes}<option value={minutes}>{minutes} min</option>{/each}
    </select>
  </SettingRow>
  <div class="backup-status" role="status" aria-live="polite">
    {#if !hasStatus}
      <span class="backup-state">{signedIn && paired && backupStatus.loading ? "Loading…" : signedIn && paired && backupStatus.error ? "Status unavailable." : ""}</span>
    {:else if data.error}
      <span class="backup-state error">Backup error: {data.error}</span>
    {:else if data.running}
      <span class="backup-state">Backing up {Number(data.projects) || 0} projects…</span>
    {:else if data.enabled}
      <span class="backup-state">{data.last_success ? `Last backup ${new Date(Number(data.last_success) * 1000).toLocaleString()}` : "No backup yet."}</span>
    {:else}
      <span class="backup-state">Backups are off.</span>
    {/if}
    <button type="button" class="btn btn-sm lp-control-outline" disabled={busy !== "" || !destinationSet || !canUseCompanion || needsLogin || !data.enabled || data.running} onclick={() => void runNow()}>{busy === "run" ? "Starting…" : "Back up now"}</button>
  </div>
  {#if saved && !busy && !error && !backupStatus.error}<p class="setting-feedback" role="status">Saved</p>{/if}

{#if error}<p class="setting-description backup-error" role="alert">{error}</p>{/if}

<style>
  .backups-intro { max-width: 48rem; margin-block: 0 calc(var(--spacing) * 3); }
  .backups-intro a { margin-inline-start: .35rem; }
  .backup-folder-control { display: flex; align-items: center; gap: calc(var(--spacing) * 2); }
  .backup-path { width: 14rem; }
  .backup-frequency { min-width: 8rem; }
  .backup-enable-switch[data-state="unknown"] { background: var(--color-subtle); }
  .backup-enable-switch[data-state="unknown"] .switch-thumb { visibility: hidden; }
  .backup-status { display: flex; align-items: center; justify-content: space-between; gap: calc(var(--spacing) * 3); margin-top: calc(var(--spacing) * 3); }
  .backup-state { color: var(--color-text-secondary); font-size: var(--text-sm); }
  .backup-state.error, .backup-error { color: var(--color-error-text); }
  @media (max-width: 42rem) { .backup-status { align-items: flex-start; flex-direction: column; } .backup-folder-control { align-items: stretch; flex-direction: column; } .backup-path { width: 100%; } }
</style>
