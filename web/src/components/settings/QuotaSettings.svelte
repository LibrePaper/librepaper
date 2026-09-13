<script>
  import { onMount } from "svelte";
  import SettingRow from "./SettingRow.svelte";
  import {
    loadQuotaPreferences, previewQuotaPreferences, applyQuotaPreferences,
    storageBytes, draftPreferences,
  } from "../../lib/quota-preferences.js";

  let snapshot = $state(null);
  let loading = $state(true);
  let busy = $state(false);
  let error = $state("");
  let notice = $state("");
  let preview = $state(null);
  let proposed = $state(null);
  let dirty = $state(false);
  let maxCount = $state(50);
  let maxDays = $state(30);
  let profile = $state("default");
  let timezone = $state("UTC");
  let warnings = $state("75, 90");
  let alive = true;
  let generation = 0;

  const editable = $derived(snapshot?.canManage !== false && snapshot?.preferences?.version === 2);
  const disabled = $derived(busy || !editable);
  const usage = $derived(snapshot?.usage);
  const percent = $derived(usage?.hardQuota > 0 && Number.isFinite(usage?.chargedBytes)
    ? Math.round(100 * usage.chargedBytes / usage.hardQuota) : null);

  function receive(value) {
    snapshot = value;
    const prefs = value.preferences;
    maxCount = prefs?.customRetention?.maxRoutineCount ?? "";
    maxDays = prefs?.customRetention?.maxAgeMs == null ? "" : prefs.customRetention.maxAgeMs / 86400000;
    profile = prefs?.retentionProfile || "default";
    timezone = prefs?.displayTimezone || "UTC";
    warnings = (prefs?.warningThresholds || []).join(", ");
    dirty = false;
    preview = null;
    proposed = null;
  }

  async function reload() {
    const job = ++generation;
    loading = true;
    error = "";
    try {
      const result = await loadQuotaPreferences();
      if (alive && job === generation) receive(result);
    } catch (cause) {
      if (alive && job === generation) error = cause.message || "Storage settings could not be loaded.";
    } finally { if (alive && job === generation) loading = false; }
  }

  onMount(() => { void reload(); return () => { alive = false; generation++; }; });

  function changed() {
    dirty = true;
    preview = null;
    proposed = null;
    notice = "";
    error = "";
  }

  async function prepare() {
    if (disabled) return;
    busy = true;
    error = "";
    try {
      const preferences = draftPreferences(snapshot.preferences, {
        maxCount, maxDays, profile, timezone, warnings,
      });
      const result = await previewQuotaPreferences(snapshot.revision, preferences);
      if (!alive) return;
      proposed = preferences;
      preview = result;
      } catch (cause) { if (alive) error = cause.message || "The policy could not be previewed."; }
    finally { if (alive) busy = false; }
  }

  async function apply() {
    if (disabled || !preview || !proposed) return;
    busy = true;
    error = "";
    try {
      const result = await applyQuotaPreferences(snapshot.revision, proposed);
      if (!alive) return;
      await reload();
      if (alive && !error) {
        const grace = Number(result?.graceMs) === 86_400_000;
        notice = grace
          ? "Storage preferences saved. Newly eligible history has a 24-hour grace period."
          : "Storage preferences saved.";
      }
    } catch (cause) {
      if (alive) {
        error = cause.message || "The policy could not be applied. Refresh storage status and preview again.";
        preview = null;
        proposed = null;
          }
    } finally { if (alive) busy = false; }
  }
</script>
<div class="flex flex-col gap-6">
  {#if error}<p role="alert">{error}</p>{/if}
  {#if notice}<p role="status">{notice}</p>{/if}
  {#if loading}<p>Loading storage settings…</p>
  {:else if snapshot}
    <section aria-label="Storage usage" class="flex flex-col gap-2">
      <h3 class="h5">Storage</h3>
      <p>{storageBytes(usage?.chargedBytes)} of {storageBytes(usage?.hardQuota)}{percent == null ? "" : ` (${percent}%)`}</p>
      <p>Usage includes staged files and files awaiting deletion.</p>
      {#if usage?.message}<p role="status">{usage.message}</p>{/if}
      <button class="btn btn-sm preset-outlined-surface-300-700 self-start" type="button" disabled={busy} onclick={reload}>Refresh storage status</button>
    </section>
    <SettingRow id="quota-profile" title="Recovery points" stacked description="The latest version, named versions, and versions referenced by unresolved comments or suggestions stay protected. Newly eligible versions have a 24-hour grace period.">
      <select class="select" bind:value={profile} onchange={changed} aria-label="Retention profile" {disabled}>
        {#each snapshot.profiles || [] as item (item.id)}<option value={item.id}>{item.label}</option>{/each}
      </select>
      {#if profile === "manual"}<p>Automatic history cleanup is disabled. Storage limits still apply.</p>{/if}
      {#if profile === "custom"}
        <label>Maximum routine versions<input class="input setting-input" type="number" min="0" max="4096" step="1" bind:value={maxCount} oninput={changed} {disabled} /></label>
        <label>Maximum age in days<input class="input setting-input" type="number" min="0" bind:value={maxDays} oninput={changed} {disabled} /></label>
        <p>Leave either limit blank for no limit. Both entered limits apply.</p>
      {/if}
    </SettingRow>
    <SettingRow id="quota-timezone" title="Display timezone" description="Changes date labels only.">
      <input class="input setting-input" type="text" bind:value={timezone} oninput={changed} aria-label="Display timezone" {disabled} />
    </SettingRow>
    <SettingRow id="quota-warnings" title="Storage warnings" description="Increasing percentages separated by commas.">
      <input class="input setting-input" type="text" bind:value={warnings} oninput={changed} aria-label="Storage warning thresholds" {disabled} />
    </SettingRow>
    {#if preview}
      <section aria-label="Retention change preview" class="flex flex-col gap-3" aria-live="polite">
        <h3 class="h5">Review this change</h3>
        <p>Currently, {preview.affectedCount || 0} versions would be eligible for removal and {preview.protectedCount || 0} remain protected.</p>
        <p>This is an estimate. Saving the policy starts a new 24-hour grace period for newly eligible versions; protection is checked again before removal.</p>
        <button class="btn btn-sm preset-filled-primary-500" type="button" onclick={apply} {disabled}>Save preferences</button>
      </section>
    {:else}
      <button class="btn btn-sm preset-filled-primary-500 self-start" type="button" onclick={prepare} disabled={disabled || !dirty}>Preview preference changes</button>
    {/if}
  {/if}
</div>
