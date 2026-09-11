<script>
  import { onMount } from "svelte";
  import SettingRow from "./SettingRow.svelte";
  import {
    loadQuotaPreferences, previewQuotaPreferences, applyQuotaPreferences,
    storageBytes, displayTimestamp, displayTiers, draftPreferences, MILESTONES,
  } from "../../lib/quota-preferences.js";

  let snapshot = $state(null);
  let loading = $state(true);
  let busy = $state(false);
  let error = $state("");
  let notice = $state("");
  let preview = $state(null);
  let proposed = $state(null);
  let confirmed = $state(false);
  let dirty = $state(false);
  let budgetKind = $state("percent");
  let budgetValue = $state(60);
  let profile = $state("balanced");
  let timezone = $state("UTC");
  let milestones = $state({});
  let warnings = $state("75, 90");
  let alive = true;
  let generation = 0;

  const editable = $derived(snapshot?.canManage !== false && snapshot?.preferences?.version === 1);
  const disabled = $derived(busy || !editable);
  const usage = $derived(snapshot?.usage);
  const percent = $derived(usage?.hardQuota > 0 && Number.isFinite(usage?.chargedBytes)
    ? Math.round(100 * usage.chargedBytes / usage.hardQuota) : null);

  function receive(value) {
    snapshot = value;
    const prefs = value.preferences;
    budgetKind = prefs?.historyBudget?.kind || "percent";
    budgetValue = budgetKind === "bytes" ? prefs?.historyBudget?.value / (1024 * 1024) : prefs?.historyBudget?.value;
    profile = prefs?.retentionProfile || "balanced";
    timezone = prefs?.displayTimezone || "UTC";
    milestones = { ...prefs?.milestonePreferences };
    warnings = (prefs?.warningThresholds || []).join(", ");
    dirty = false;
    preview = null;
    proposed = null;
    confirmed = false;
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
    confirmed = false;
    notice = "";
    error = "";
  }

  async function prepare() {
    if (disabled) return;
    busy = true;
    error = "";
    try {
      const preferences = draftPreferences(snapshot.preferences, {
        budgetKind, budgetValue, profile, timezone, milestones, warnings,
      });
      const result = await previewQuotaPreferences(snapshot.revision, preferences);
      if (!alive) return;
      proposed = preferences;
      preview = result;
      confirmed = false;
    } catch (cause) { if (alive) error = cause.message || "The policy could not be previewed."; }
    finally { if (alive) busy = false; }
  }

  async function apply() {
    if (disabled || !preview || !proposed || (preview.requiresConfirmation && !confirmed)) return;
    busy = true;
    error = "";
    try {
      await applyQuotaPreferences(snapshot.revision, preview.generation, proposed, confirmed);
      if (!alive) return;
      await reload();
      if (alive && !error) notice = "Storage preferences saved. Your edits continue to be saved promptly.";
    } catch (cause) {
      if (alive) {
        error = cause.message || "The policy could not be applied. Refresh storage status and preview again.";
        preview = null;
        proposed = null;
        confirmed = false;
      }
    } finally { if (alive) busy = false; }
  }
</script>

<div class="flex flex-col gap-4" aria-busy={loading || busy}>
  <p class="text-sm text-surface-600-400">Your edits are saved promptly; history keeps selected recovery points. These settings apply to the documents you own.</p>
  {#if error}<p class="setting-error" role="alert">{error}</p>{/if}
  {#if notice}<p role="status">{notice}</p>{/if}
  {#if loading}
    <p role="status">Loading storage and retention settings…</p>
  {:else if !snapshot}
    <button class="btn btn-sm preset-outlined-surface-300-700" type="button" onclick={reload}>Retry</button>
  {:else}
    {#if !editable}<p role="status">Storage preferences are read-only for this account or policy version.</p>{/if}
    <section aria-label="Storage usage" class="flex flex-col gap-2">
      <h3 class="h5">Storage use</h3>
      {#if usage?.authoritative}
        <p>{storageBytes(usage.chargedBytes)} of {storageBytes(usage.hardQuota)}{percent === null ? "" : ` (${percent}%)`}</p>
        {#if percent !== null}
          <progress class="w-full" max="100" value={Math.min(100, percent)} aria-label="Hard storage quota used" aria-valuetext={`${percent}% of the hard quota used`}></progress>
        {/if}
        <dl class="grid grid-cols-2 gap-2 text-sm">
          {#each usage.categories || [] as category (category.id)}
            <dt>{category.label}</dt><dd class="text-right">{storageBytes(category.bytes)}</dd>
          {/each}
        </dl>
      {:else}
        <p>Verified storage totals are not available yet. Reclaimed-byte estimates are experimental.</p>
        <p>Hard quota: {storageBytes(usage?.hardQuota)}</p>
      {/if}
      {#if usage?.message}<p role="status">{usage.message}</p>{/if}
      {#if snapshot.thinning?.message}<p role="status">{snapshot.thinning.message}</p>{/if}
      <button class="btn btn-sm preset-outlined-surface-300-700 self-start" type="button" disabled={busy} onclick={reload}>Refresh storage status</button>
    </section>

    <SettingRow id="quota-budget" title="History budget" stacked description="A soft target for history, within the deployment's hard quota. Protected versions may keep usage above this target. Live edits do not fail merely because it is exceeded.">
      <div class="flex flex-wrap gap-2">
        <input class="input setting-input" type="number" min="0" max={budgetKind === "percent" ? 100 : undefined} step={budgetKind === "percent" ? 1 : "any"} bind:value={budgetValue} oninput={changed} aria-label="History budget amount" {disabled} />
        <select class="select" bind:value={budgetKind} onchange={changed} aria-label="History budget units" {disabled}>
          <option value="percent">Percent of hard quota</option><option value="bytes">MiB</option>
        </select>
      </div>
      {#if snapshot.effective?.historyBudgetBytes != null}<p class="text-sm">Effective target: {storageBytes(snapshot.effective.historyBudgetBytes)}</p>{/if}
    </SettingRow>

    <SettingRow id="quota-profile" title="Recovery points" stacked description="Retention density controls which recovery points survive. It does not delay saving or guarantee a point in every time bucket.">
      <select class="select" bind:value={profile} onchange={changed} aria-label="Retention profile" {disabled}>
        {#each snapshot.profiles || [] as item (item.id)}<option value={item.id}>{item.label}</option>{/each}
      </select>
      {#if snapshot.effective?.tiers?.length}
        <table class="w-full text-sm">
          <caption class="text-left">Current effective retention, with all bucket boundaries in UTC</caption>
          <thead><tr><th class="text-left" scope="col">Checkpoint age</th><th class="text-left" scope="col">Density</th></tr></thead>
          <tbody>{#each snapshot.effective.tiers as tier}<tr><td>{tier.ageLabel}</td><td>{tier.densityLabel}</td></tr>{/each}</tbody>
        </table>
      {/if}
    </SettingRow>

    <SettingRow id="quota-timezone" title="Display timezone" description="For date and time labels only. Retention always uses UTC boundaries.">
      <input class="input setting-input" type="text" bind:value={timezone} oninput={changed} aria-label="Display timezone" placeholder="America/Toronto" {disabled} />
    </SettingRow>

    <SettingRow id="quota-milestones" title="Preferentially retain" stacked description="These source versions survive ahead of routine history, subject to hard storage and count limits. Generated outputs are transient and are not retained.">
      <fieldset class="flex flex-col gap-2" {disabled}>
        <legend class="sr-only">Milestone protection preferences</legend>
        {#each MILESTONES as [key, label]}
          <label class="flex items-center gap-2 text-sm"><input type="checkbox" class="checkbox" bind:checked={milestones[key]} onchange={changed} />{label}</label>
        {/each}
      </fieldset>
    </SettingRow>

    <SettingRow id="quota-warnings" title="Storage warnings" description="Percentages of the hard quota, in increasing order, separated by commas.">
      <input class="input setting-input" type="text" bind:value={warnings} oninput={changed} aria-label="Storage warning thresholds" {disabled} />
    </SettingRow>

    {#if snapshot.constraints?.length}
      <section aria-label="Deployment constraints"><h3 class="h5">Deployment limits</h3><ul class="list-disc pl-5 text-sm">{#each snapshot.constraints as constraint}<li>{constraint.explanation}</li>{/each}</ul></section>
    {/if}

    {#if preview}
      <section aria-label="Retention change preview" class="flex flex-col gap-3" aria-live="polite">
        <h3 class="h5">Review this change</h3>
        <p>{preview.affectedCount || 0} recovery points would become eligible for removal; {preview.protectedCount || 0} would lose protection.</p>
        {#if preview.oldestAffected}<p>Oldest affected: {displayTimestamp(preview.oldestAffected, timezone)}</p>{/if}
        <p>Estimated reclaimable storage: {storageBytes(preview.reclaimableBytes)}. Shared objects and delayed collection can change the result.</p>
        {#if preview.graceSeconds > 0}<p>Affected versions remain restorable for up to {Math.ceil(preview.graceSeconds / 60)} minutes. Hard limits may shorten this period.</p>{/if}
        {#if preview.message}<p>{preview.message}</p>{/if}
        {#each preview.explanation || [] as explanation}<p>{explanation}</p>{/each}
        {#if preview.effective?.tiers?.length}
          <table class="w-full text-sm">
            <caption class="text-left">Proposed effective retention, with bucket boundaries in UTC</caption>
            <thead><tr><th class="text-left" scope="col">Checkpoint age</th><th class="text-left" scope="col">Density</th></tr></thead>
            <tbody>{#each displayTiers(preview.effective.tiers) as tier}<tr><td>{tier.ageLabel}</td><td>{tier.densityLabel}</td></tr>{/each}</tbody>
          </table>
        {/if}
        {#if preview.requiresConfirmation}
          <label class="flex items-start gap-2"><input class="checkbox" type="checkbox" bind:checked={confirmed} disabled={busy} />I understand that applying this policy can permanently remove historical recovery points.</label>
        {/if}
        <div class="flex gap-2">
          <button class="btn btn-sm preset-filled-primary-500" type="button" onclick={apply} disabled={disabled || (preview.requiresConfirmation && !confirmed)}>Apply preferences</button>
          <button class="btn btn-sm preset-outlined-surface-300-700" type="button" disabled={busy} onclick={() => { preview = null; proposed = null; confirmed = false; }}>Cancel</button>
        </div>
      </section>
    {:else}
      <button class="btn btn-sm preset-filled-primary-500 self-start" type="button" onclick={prepare} disabled={disabled || !dirty}>{busy ? "Preparing…" : "Preview preference changes"}</button>
    {/if}
  {/if}
</div>
