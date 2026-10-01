<script>
  // What this account is storing on this deployment, and what a version is.
  // The account's, not the document's: it says the same thing whatever
  // happens to be open. One line per document, so an account with a thousand
  // of them is a list to filter rather than a page of cards.
  import { onMount } from "svelte";
  import Modal from "../Modal.svelte";
  import IconButton from "../IconButton.svelte";
  import SettingRow from "./SettingRow.svelte";
  import { loadStorageStatus, storageBytes, trimHistory } from "../../lib/quota-preferences.js";

  let snapshot = $state(null);
  let loading = $state(true);
  let error = $state("");
  let trimErrors = $state({});
  let alive = true;
  let generation = 0;

  let filterText = $state("");
  let sortBy = $state("largest");
  let trimOpen = $state(false);
  let trimTarget = $state(null);
  let trimPending = $state(false);

  const usage = $derived(snapshot?.usage);
  const documents = $derived(usage?.documents ?? []);
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

  const nameOf = (doc) => doc.title || doc.slug;
  const totalOf = (doc) => doc.figureBytes + doc.archiveBytes + doc.historyBytes;
  const partsOf = (doc) => [
    { key: "figure", name: "Figures", bytes: doc.figureBytes },
    { key: "versions", name: "Versions", bytes: doc.archiveBytes },
    { key: "history", name: "History", bytes: doc.historyBytes },
  ];
  const labelOf = (parts) =>
    parts.filter((p) => p.bytes > 0).map((p) => `${p.name} ${storageBytes(p.bytes)}`).join(", ") || "Empty";

  // The account bar is drawn against the quota, so what is left of it is the
  // free space; each document's bar is its own composition, always full.
  const totalParts = $derived(partsOf(documents.reduce(
    (sum, d) => ({ figureBytes: sum.figureBytes + d.figureBytes, archiveBytes: sum.archiveBytes + d.archiveBytes, historyBytes: sum.historyBytes + d.historyBytes }),
    { figureBytes: 0, archiveBytes: 0, historyBytes: 0 },
  )));
  const totalUsed = $derived(totalParts.reduce((sum, p) => sum + p.bytes, 0));
  const quotaBase = $derived(usage?.hardQuotaBytes > 0 ? Math.max(usage.hardQuotaBytes, totalUsed) : totalUsed || 1);
  const totalLabel = $derived(`${labelOf(totalParts)}, of ${storageBytes(usage?.hardQuotaBytes)}`);

  const SORTS = {
    largest: (a, b) => totalOf(b) - totalOf(a),
    history: (a, b) => b.historyBytes - a.historyBytes,
    name: (a, b) => nameOf(a).localeCompare(nameOf(b)),
  };
  const visibleDocs = $derived.by(() => {
    const needle = filterText.trim().toLowerCase();
    const docs = needle ? documents.filter((d) => nameOf(d).toLowerCase().includes(needle)) : [...documents];
    return docs.sort(SORTS[sortBy]);
  });

  function openTrimDialog(doc) {
    trimTarget = { title: nameOf(doc), slug: doc.slug, historyBytes: doc.historyBytes, archiveBytes: doc.archiveBytes };
    trimOpen = true;
  }

  async function confirmTrim() {
    const slug = trimTarget?.slug;
    if (!slug) return;
    trimPending = true;
    trimErrors = { ...trimErrors, [slug]: "" };
    try {
      const result = await trimHistory(slug);
      // The row and the account bar move at once; the server's own numbers
      // follow.
      const doc = snapshot?.usage?.documents?.find((d) => d.slug === slug);
      if (doc) {
        doc.historyBytes = 0;
        doc.archiveBytes = 0;
        doc.figureBytes = Math.max(0, doc.figureBytes - (result?.figureBytes ?? 0));
      }
      void reload();
    } catch (cause) {
      if (alive) trimErrors = { ...trimErrors, [slug]: cause.message || "History could not be trimmed." };
    } finally {
      trimPending = false;
      trimOpen = false;
    }
  }

  async function reload() {
    const job = ++generation;
    loading = true;
    error = "";
    try {
      const result = await loadStorageStatus();
      if (alive && job === generation) snapshot = result;
    } catch (cause) {
      if (alive && job === generation) error = cause.message || "Storage status could not be loaded.";
    } finally {
      if (alive && job === generation) loading = false;
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

{#snippet bar(parts, base, label, cls)}
  <div class="storage-bar {cls}" role="img" aria-label={label}>
    {#each parts as part (part.key)}
      {#if part.bytes > 0}
        <div class="bar-segment {part.key}" title={`${part.name}\n${storageBytes(part.bytes)}`}
             style:width={`${(100 * part.bytes) / base}%`}></div>
      {/if}
    {/each}
  </div>
{/snippet}

<SettingRow id="storage-account" stacked title="Account storage" description={used}>
  <div class="account-line">
    {@render bar(totalParts, quotaBase, totalLabel, "account-bar")}
    <button class="btn btn-sm lp-control-outline" type="button" onclick={reload}>Refresh</button>
  </div>
</SettingRow>

{#if documents.length > 0}
  <SettingRow id="storage-documents" stacked title="Storage by document"
              description="Trimming keeps a document as it is now and deletes everything before it: its editing history, its named versions, and figures it no longer uses. Comments stay.">
    <div class="storage-toolbar">
      <div class="legend" aria-hidden="true">
        <span><i class="swatch figure"></i>Figures</span>
        <span><i class="swatch versions"></i>Versions</span>
        <span><i class="swatch history"></i>History</span>
      </div>
      <div class="toolbar-controls">
        <input type="search" class="input input-sm" placeholder="Filter documents"
               aria-label="Filter documents by title" bind:value={filterText} />
        <select class="select select-sm" bind:value={sortBy} aria-label="Sort documents">
          <option value="largest">Largest first</option>
          <option value="history">Most history</option>
          <option value="name">Name</option>
        </select>
      </div>
    </div>

    {#if visibleDocs.length > 0}
      <div role="table" aria-label="Storage by document">
        {#each visibleDocs as doc (doc.id)}
          <div role="row" class="doc-row">
            <div role="cell" class="doc-title">
              <a class="title-text" href="/docs/{doc.slug}" title={nameOf(doc)}>{nameOf(doc)}</a>
              {#if trimErrors[doc.slug]}<div class="trim-error">{trimErrors[doc.slug]}</div>{/if}
            </div>
            <div role="cell" class="doc-storage">
              {@render bar(partsOf(doc), totalOf(doc) || 1, labelOf(partsOf(doc)), "doc-bar")}
              <span class="doc-total">{storageBytes(totalOf(doc))}</span>
            </div>
            <div role="cell">
              <IconButton icon="history" title="Trim history" label={`Trim history for ${nameOf(doc)}`}
                          size="btn-icon-sm" disabled={doc.historyBytes === 0}
                          onclick={() => openTrimDialog(doc)} />
            </div>
          </div>
        {/each}
      </div>
    {:else}
      <p class="no-match">No documents match.</p>
    {/if}
  </SettingRow>
{/if}

<Modal bind:open={trimOpen} title="Trim history?"
       confirm={{ label: trimPending ? "Trimming…" : "Trim history", tone: "error", disabled: trimPending, onclick: confirmTrim }}>
  {#if trimTarget}
    <p>This keeps “<strong>{trimTarget.title}</strong>” as it is now and permanently deletes everything before it:</p>
    <ul class="trim-list">
      <li>Its editing history ({storageBytes(trimTarget.historyBytes)})</li>
      <li>All its named versions ({storageBytes(trimTarget.archiveBytes)})</li>
      <li>Figures its current files no longer use</li>
    </ul>
    <p class="lp-text-secondary text-sm">Comments stay. This cannot be undone.</p>
  {/if}
</Modal>

<style>
  .account-line { display: flex; align-items: center; gap: calc(var(--spacing) * 3); }
  .storage-bar { display: flex; height: 10px; border-radius: 9999px; overflow: hidden; background: var(--color-subtle); }
  .account-bar { flex: 1; }
  .doc-bar { width: 140px; flex-shrink: 0; }
  .bar-segment { flex: 0 0 auto; height: 100%; }
  .figure { background: var(--color-brand); }
  .versions { background: var(--color-success-solid); }
  .history { background: var(--color-warning-solid); }

  .storage-toolbar { display: flex; justify-content: space-between; align-items: center; gap: calc(var(--spacing) * 2); flex-wrap: wrap; }
  .legend { display: flex; gap: calc(var(--spacing) * 3); font-size: 0.75rem; color: var(--color-text-secondary); }
  .legend span { display: flex; align-items: center; gap: calc(var(--spacing) * 1); }
  .swatch { width: 10px; height: 10px; border-radius: 2px; }
  .toolbar-controls { display: flex; gap: calc(var(--spacing) * 2); }
  .toolbar-controls input { width: 14rem; }
  .toolbar-controls select { width: auto; }

  .doc-row { display: grid; grid-template-columns: minmax(0, 1fr) auto auto; gap: calc(var(--spacing) * 3); align-items: center; min-height: 44px; padding: calc(var(--spacing) * 1) 0; border-bottom: 1px solid var(--color-divider); }
  .doc-row:last-child { border-bottom: 0; }
  .title-text { display: block; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; color: inherit; text-decoration: none; }
  .title-text:hover, .title-text:focus-visible { text-decoration: underline; }
  .doc-storage { display: flex; align-items: center; gap: calc(var(--spacing) * 2); }
  .doc-total { min-width: 4.5rem; text-align: right; white-space: nowrap; font-size: 0.875rem; font-variant-numeric: tabular-nums; color: var(--color-text-secondary); }
  .trim-error { color: var(--color-error-text); font-size: 0.75rem; }
  .no-match { color: var(--color-text-secondary); font-size: 0.875rem; }
  .trim-list { padding-left: 1.5rem; margin: calc(var(--spacing) * 2) 0; }

  @media (max-width: 640px) {
    .doc-bar { width: 56px; }
    .toolbar-controls { width: 100%; }
    .toolbar-controls input { flex: 1; width: auto; min-width: 0; }
  }
  @media (max-width: 420px) {
    .doc-bar { display: none; }
  }
</style>
