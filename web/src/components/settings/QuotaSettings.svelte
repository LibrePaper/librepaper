<script>
  // What this account is storing on this deployment, and what a version is.
  // The account's, not the document's: it says the same thing whatever
  // happens to be open.
  import { onMount } from "svelte";
  import SettingRow from "./SettingRow.svelte";
  import { loadStorageStatus, storageBytes, trimHistory } from "../../lib/quota-preferences.js";

  let snapshot = $state(null);
  let loading = $state(true);
  let error = $state("");
  let trimErrors = $state({});
  let trimPending = $state(new Set());
  let alive = true;
  let generation = 0;

  const usage = $derived(snapshot?.usage);
  const percent = $derived(
    usage?.hardQuotaBytes > 0 && Number.isFinite(usage.chargedBytes)
      ? Math.round((100 * usage.chargedBytes) / usage.hardQuotaBytes)
      : null,
  );
  // One line under the title, whatever the status of the request: the row
  // keeps its shape while the numbers are on their way or never arrive.
  const used = $derived(
    error ? error
    : loading ? "Measuring…"
    : `${storageBytes(usage?.chargedBytes)} of ${storageBytes(usage?.hardQuotaBytes)}${percent == null ? "" : ` (${percent}%)`} used.`,
  );

  async function reload() {
    const job = ++generation;
    loading = true;
    error = "";
    trimErrors = {};
    try {
      const result = await loadStorageStatus();
      if (alive && job === generation) snapshot = result;
    } catch (cause) {
      if (alive && job === generation) error = cause.message || "Storage status could not be loaded.";
    } finally {
      if (alive && job === generation) loading = false;
    }
  }

  async function onTrimHistory(slug) {
    trimErrors = { ...trimErrors, [slug]: "" };
    trimPending.add(slug);
    trimPending = trimPending;
    try {
      await trimHistory(slug);
      void reload();
    } catch (cause) {
      if (alive) trimErrors = { ...trimErrors, [slug]: cause.message || "History could not be trimmed." };
    } finally {
      trimPending.delete(slug);
      trimPending = trimPending;
    }
  }

  onMount(() => {
    void reload();
    return () => {
      alive = false;
      generation += 1;
    };
  });
</script>

<SettingRow id="storage-account" title="Account storage" description={used}>
  <button class="btn btn-sm preset-outlined-surface-300-700" type="button" onclick={reload}>Refresh</button>
</SettingRow>

{#if snapshot && usage?.documents && usage.documents.length > 0}
  <SettingRow id="storage-documents" stacked title="Storage by document"
              description="One or more documents on your account.">
    {#each usage.documents as doc (doc.id)}
      <div class="document-storage">
        <div class="document-title">{doc.title || doc.slug}</div>
        <div class="storage-bar">
          {#if doc.figureBytes > 0}
            <div class="bar-segment preset-tonal-primary" title="Figures"
                 style:width={`${(100 * doc.figureBytes) / Math.max(1, ...usage.documents.map(d => d.figureBytes + d.archiveBytes + d.historyBytes))}%`}></div>
          {/if}
          {#if doc.archiveBytes > 0}
            <div class="bar-segment preset-tonal-secondary" title="Archives"
                 style:width={`${(100 * doc.archiveBytes) / Math.max(1, ...usage.documents.map(d => d.figureBytes + d.archiveBytes + d.historyBytes))}%`}></div>
          {/if}
          {#if doc.historyBytes > 0}
            <div class="bar-segment preset-tonal-tertiary" title="History"
                 style:width={`${(100 * doc.historyBytes) / Math.max(1, ...usage.documents.map(d => d.figureBytes + d.archiveBytes + d.historyBytes))}%`}></div>
          {/if}
        </div>
        <div class="storage-legend">
          {#if doc.figureBytes > 0}<span>Figures: {storageBytes(doc.figureBytes)}</span>{/if}
          {#if doc.archiveBytes > 0}<span>Archives: {storageBytes(doc.archiveBytes)}</span>{/if}
          {#if doc.historyBytes > 0}<span>History: {storageBytes(doc.historyBytes)}</span>{/if}
        </div>
        {#if doc.historyBytes > 0}
          <div class="trim-control">
            <button class="btn btn-sm preset-outlined-surface-300-700" type="button" disabled={trimPending.has(doc.slug)} onclick={() => onTrimHistory(doc.slug)}>Trim history</button>
            {#if trimErrors[doc.slug]}<span class="trim-error">{trimErrors[doc.slug]}</span>{/if}
          </div>
        {/if}
      </div>
    {/each}
    <p class="storage-note">Trimming keeps a document as it is now and discards its editing history. Named versions and comments stay.</p>
  </SettingRow>
{/if}

{#if snapshot}
  <SettingRow id="storage-retention" stacked title="Versions"
              description="Nothing is deleted on a schedule.">
    <p>A version is written when you name one or restore an earlier
      version, or leave a comment — never on a timer.</p>
    <p>Every version is kept until you delete the document.</p>
    <p>Editing between versions is not lost: the full editing history is kept
      separately, and the history panel can show the document as it stood at
      any moment in it, whether or not anybody named that moment.</p>
  </SettingRow>
{/if}

<style>
  .document-storage { display: flex; flex-direction: column; gap: calc(var(--spacing) * 1); padding: calc(var(--spacing) * 2); border: 1px solid var(--color-surface); border-radius: var(--radius); }
  .document-title { font-weight: 600; }
  .storage-bar { display: flex; height: 1.5rem; border-radius: var(--radius); overflow: hidden; background: var(--color-surface); }
  .bar-segment { flex: 1; min-width: 0; }
  .bar-segment.preset-tonal-primary { background: var(--color-primary-600-400); }
  .bar-segment.preset-tonal-secondary { background: var(--color-secondary-600-400); }
  .bar-segment.preset-tonal-tertiary { background: var(--color-tertiary-600-400); }
  .storage-legend { display: flex; gap: calc(var(--spacing) * 2); flex-wrap: wrap; font-size: 0.875rem; }
  .storage-legend span { display: flex; gap: calc(var(--spacing) * 1); }
  .trim-control { display: flex; gap: calc(var(--spacing) * 1); align-items: center; }
  .trim-error { color: var(--color-error-600-400); font-size: 0.875rem; }
  .storage-note { font-size: 0.875rem; color: var(--color-surface-600-400); }
</style>
