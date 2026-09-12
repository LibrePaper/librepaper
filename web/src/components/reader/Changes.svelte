<script>
  import CommentCard from "../CommentCard.svelte";

  let {
    revisions = [], comments = [], files = [], tracking = false, markup = true, showMarkup,
    canTrack = true, canReview = true, selected = "", selectedRevision = "", filters = {},
    authors = [], sessions = [], ontracking, onmarkup, onfilter, onselect,
    onaccept, onreject, onbulk, onrevisionreveal, onrevisiondecide, onrevisionundo,
    onundo, onprevious, onnext, onreveal, onresolve, ondelete, onreply, onhistory,
    onrevisionresolve, identity = "", commentingAs = "Anonymous", canModerate = false,
    went = {}, replacements = {},
  } = $props();

  let expanded = $state("");
  let checked = $state(new Set());
  let deciding = $state(new Set());
  let localFilters = $state({ status: "pending", author: "", file: "", session: "" });
  let feedback = $state("");
  let undoBatch = $state(null);

  const value = (item, ...keys) => keys.map((key) => item?.[key])
    .find((answer) => answer !== undefined && answer !== null && answer !== "") ?? "";
  const idOf = (item) => String(value(item, "id", "revisionId", "revision_id"));
  const rowId = (item) => item.__kind === "legacy" ? `legacy:${idOf(item)}` : idOf(item);
  const pathOf = (item) => value(item, "path", "file", "filePath", "sourcePath") || "Untitled";
  const authorOf = (item) => value(item, "author", "authorName", "creator") || "Unknown author";
  const sessionOf = (item) => value(item, "session", "sessionId", "revisionSession");
  const statusOf = (item) => value(item, "status", "outcome") || (item.resolved ? (item.outcome || "accepted") : "pending");
  const pending = (item) => statusOf(item) === "pending";
  const positionOf = (item) => { const n = Number(value(item, "position", "start_offset", "start", "from")); return Number.isFinite(n) ? n : Number.MAX_SAFE_INTEGER; };

  // session.list() is the stable file order. File identity wins over path so
  // renames do not reshuffle an existing review queue.
  const fileOrder = $derived.by(() => {
    const order = new Map();
    files.forEach((file, index) => {
      for (const key of [value(file, "id", "file_id"), file?.path]) if (key !== "" && key !== undefined && !order.has(String(key))) order.set(String(key), index);
    });
    return order;
  });
  const allRows = $derived([
    ...revisions.map((revision) => ({ ...revision, __kind: "revision" })),
    ...comments.filter((comment) => comment.motivation === "editing").map((comment) => ({ ...comment, __kind: "legacy" })),
  ]);
  const orderOf = (item) => fileOrder.get(String(value(item, "file_id", "fileId") || pathOf(item))) ?? Number.MAX_SAFE_INTEGER;
  const orderedRows = $derived([...allRows].sort((a, b) => orderOf(a) - orderOf(b) || pathOf(a).localeCompare(pathOf(b)) || positionOf(a) - positionOf(b) || String(value(a, "created_at", "created")).localeCompare(String(value(b, "created_at", "created"))) || rowId(a).localeCompare(rowId(b))));
  const activeFilters = $derived({ ...localFilters, ...(filters || {}) });
  const filteredRows = $derived(orderedRows.filter((item) => {
    const status = activeFilters.status || "pending";
    if (status === "pending" && !pending(item)) return false;
    if (status !== "pending" && status !== "all" && statusOf(item) !== status) return false;
    return (!activeFilters.author || authorOf(item) === activeFilters.author) && (!activeFilters.file || pathOf(item) === activeFilters.file) && (!activeFilters.session || sessionOf(item) === activeFilters.session);
  }));
  const pendingRows = $derived(filteredRows.filter(pending));
  const allPending = $derived(allRows.filter(pending).length);
  const selectedRows = $derived(filteredRows.filter((item) => checked.has(rowId(item))));
  const active = $derived(filteredRows.some((item) => rowId(item) === expanded) ? expanded : filteredRows.some((item) => rowId(item) === String(selectedRevision)) ? String(selectedRevision) : filteredRows.some((item) => rowId(item) === `legacy:${String(selected)}`) ? `legacy:${String(selected)}` : filteredRows[0] ? rowId(filteredRows[0]) : "");
  const markupVisible = $derived(showMarkup === undefined ? markup : showMarkup);
  const derivedAuthors = $derived(authors.length ? authors : [...new Set(allRows.map(authorOf))]);
  const derivedFiles = $derived(files.length ? files.map((file) => file.path || file.id).filter(Boolean) : [...new Set(allRows.map(pathOf))]);
  const derivedSessions = $derived(sessions.length ? sessions : [...new Set(allRows.map(sessionOf).filter(Boolean))]);
  const rowFor = (id) => filteredRows.find((item) => rowId(item) === id);
  const reviewAllowed = (item) => item?.__kind === "legacy" ? canModerate : canReview;

  $effect(() => {
    const valid = new Set(allRows.filter(pending).map(rowId));
    const next = new Set([...checked].filter((id) => valid.has(id)));
    if (next.size !== checked.size) checked = next;
  });
  $effect(() => { if (expanded && !filteredRows.some((item) => rowId(item) === expanded)) expanded = ""; });

  function blocker(item) {
    if (!item) return "";
    const ids = item.affectedRevisionIds || item.dependencies || [];
    if (item.conflict) return typeof item.conflict === "string" ? item.conflict : "Concurrent edits need attention.";
    if (item.dependency || ids.length) return `${typeof item.dependency === "string" ? item.dependency : "This change depends on another revision."}${ids.length ? ` (${ids.join(", ")})` : ""}`;
    return "";
  }
  function updateFilter(key, event) { const next = { ...localFilters, [key]: event.currentTarget.value }; localFilters = next; onfilter?.(next); }
  function focusRow(id) { requestAnimationFrame(() => document.getElementById(`change-${id}`)?.focus({ preventScroll: true })); }
  function activate(item, { focus = false } = {}) {
    if (!item) return;
    expanded = rowId(item); onselect?.(item);
    if (item.__kind === "legacy") onreveal?.(item); else onrevisionreveal?.(item);
    if (focus) focusRow(rowId(item));
  }
  function reveal(item) { activate(item); }
  function toggleChecked(item) { const id = rowId(item); const next = new Set(checked); next.has(id) ? next.delete(id) : next.add(id); checked = next; }
  function decisionPromise(item, action) {
    const callback = item.__kind === "legacy" ? (action === "accept" ? onaccept : onreject) : onrevisiondecide;
    if (!callback) return Promise.reject(new Error("Review action is unavailable."));
    return item.__kind === "legacy" ? callback(item) : callback(idOf(item), action);
  }
  function addBusy(id) { deciding = new Set([...deciding, id]); }
  function removeBusy(id) { deciding = new Set([...deciding].filter((current) => current !== id)); }
  function nextAfter(ids, oldIndex) { return [...ids.slice(oldIndex + 1), ...ids.slice(0, oldIndex).reverse()].map(rowFor).find((item) => item && pending(item)); }

  async function decide(item, action) {
    const id = rowId(item); const why = blocker(item);
    if (!pending(item) || !reviewAllowed(item) || deciding.has(id)) return;
    if (why) { feedback = `Cannot ${action} this change: ${why}`; activate(item); return; }
    addBusy(id); feedback = "";
    const ids = pendingRows.map(rowId); const oldIndex = ids.indexOf(id);
    try {
      const operation = decisionPromise(item, action);
      if (operation === undefined || operation === false) throw new Error("Review action is unavailable.");
      await operation;
      undoBatch = item.__kind === "revision" ? { action, ids: [id] } : null;
      const next = nextAfter(ids, oldIndex);
      if (next) activate(next, { focus: true }); else { expanded = ""; requestAnimationFrame(() => document.querySelector(".changes-panel")?.focus()); }
    } catch (error) { feedback = error?.message || `Could not ${action} this change.`; focusRow(id); }
    finally { removeBusy(id); }
  }

  async function bulk(action) {
    if (!canReview) return;
    // Capture IDs before sending anything; this is not a moving filter query.
    const captured = selectedRows.filter(pending); if (!captured.length) return;
    const excluded = captured.filter((item) => blocker(item) || !reviewAllowed(item));
    const eligible = captured.filter((item) => !blocker(item) && reviewAllowed(item));
    if (!eligible.length) { feedback = `${excluded.length} selected change${excluded.length === 1 ? "" : "s"} excluded because it needs attention.`; return; }
    const ids = eligible.map(idOf); eligible.forEach((item) => addBusy(rowId(item))); checked = new Set();
    try {
      const supplied = onbulk?.({ action, decision: action, ids: [...ids], revisions: [...eligible] });
      let report;
      if (supplied !== undefined) report = await supplied;
      else if (onbulk) throw new Error("Bulk review did not return a confirmation report.");
      else {
        const settled = await Promise.allSettled(eligible.map((item) => decisionPromise(item, action)));
        report = { succeeded: settled.flatMap((entry, index) => entry.status === "fulfilled" ? [ids[index]] : []), failed: settled.flatMap((entry, index) => entry.status === "rejected" ? [{ id: ids[index], error: entry.reason }] : []) };
      }
      if (!report) throw new Error("Bulk review did not return a confirmation report.");
      const succeeded = Array.isArray(report) ? report.filter((entry) => entry.status === "fulfilled").length : (report?.succeeded?.length ?? ids.length);
      const failed = Array.isArray(report) ? report.filter((entry) => entry.status === "rejected").length : (report?.failed?.length ?? 0);
      const omitted = excluded.length + Math.max(0, eligible.length - succeeded - failed);
      feedback = failed || omitted ? `${succeeded} ${action}ed; ${failed} failed; ${omitted} excluded or still pending.` : `${succeeded} change${succeeded === 1 ? "" : "s"} ${action}ed.`;
      const doneIds = Array.isArray(report)
        ? report.flatMap((entry, index) => entry.status === "fulfilled" ? [ids[index]] : [])
        : Array.isArray(report?.succeeded) ? report.succeeded : ids.slice(0, succeeded);
      const liveIds = doneIds.map(String).filter((doneId) => eligible.some((item) => item.__kind === "revision" && idOf(item) === doneId));
      if (liveIds.length) undoBatch = { action, ids: liveIds };
    } catch (error) { feedback = error?.message || `Could not ${action} the selected changes.`; }
    finally { eligible.forEach((item) => removeBusy(rowId(item))); }
  }
  async function undo() {
    const batch = undoBatch; if (!batch) return; undoBatch = null;
    const undoAction = onrevisionundo || onundo;
    if (!undoAction) { feedback = "Review undo is unavailable for this action."; return; }
    const settled = await Promise.allSettled(batch.ids.map((id) => {
      const operation = undoAction(id);
      return operation === undefined || operation === false
        ? Promise.reject(new Error("Review undo did not return a confirmation.")) : operation;
    }));
    const failed = settled.filter((entry) => entry.status === "rejected");
    feedback = failed.length ? `${batch.ids.length - failed.length} undone; ${failed.length} could not be undone. The affected changes remain pending.` : `Undid ${batch.action === "accept" ? "acceptance" : "rejection"}${batch.ids.length > 1 ? "s" : ""}.`;
  }
  function move(offset) { const index = filteredRows.findIndex((item) => rowId(item) === active); const item = filteredRows[index + offset] || filteredRows[index]; if (item) activate(item, { focus: true }); }
  function resolve(item) {
    const ids = item.affectedRevisionIds || item.dependencies || [idOf(item)];
    onrevisionreveal?.(item);
    if (onrevisionresolve) onrevisionresolve(ids);
    else feedback = "Source-based resolution is unavailable for this conflict; review the affected passages individually.";
  }
  function keydown(event) {
    if (event.defaultPrevented || event.isComposing || event.target.closest("input,select,textarea,[contenteditable=true],details")) return;
    const item = rowFor(active); const key = event.key.toLowerCase();
    if (event.key === "ArrowDown" || key === "j") { event.preventDefault(); onnext?.(active); move(1); }
    else if (event.key === "ArrowUp" || key === "k") { event.preventDefault(); onprevious?.(active); move(-1); }
    else if (item && key === "a" && !event.metaKey && !event.ctrlKey) { event.preventDefault(); void decide(item, "accept"); }
    else if (item && key === "r" && !event.metaKey && !event.ctrlKey) { event.preventDefault(); void decide(item, "reject"); }
  }
  function shortDiff(item) {
    const before = value(item, "before", "oldText", "deleted", "exact"); const after = value(item, "after", "newText", "inserted", "proposed");
    const result = item.__kind === "legacy" ? `Proposal: ${before || "∅"} → ${after || "∅"}` : item.kind === "delete" || item.kind === "deletion" || (before && !after) ? `− ${before}` : item.kind === "insert" || item.kind === "insertion" || (!before && after) ? `+ ${after}` : before || after ? `${before || "∅"} → ${after || "∅"}` : value(item, "summary", "title") || "Text changed";
    return result.length > 180 ? `${result.slice(0, 177)}…` : result;
  }
  function detail(item) { const explicit = value(item, "detail", "fullText", "description"); if (explicit) return explicit; const before = value(item, "before", "oldText", "deleted", "exact"); const after = value(item, "after", "newText", "inserted", "proposed"); return before || after ? `Before:\n${before || "∅"}\n\nAfter:\n${after || "∅"}` : "Text changed"; }
</script>

<!-- A focusable review context keeps shortcuts out of source and discussion fields. -->
<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<div class="panel changes-panel" role="application" tabindex="0" onkeydown={keydown} aria-label="Changes review" aria-keyshortcuts="ArrowDown ArrowUp J K A R">
  <header class="changes-header">
    <div class="changes-title-row"><h2>Changes</h2><button type="button" class="btn btn-sm {tracking ? 'preset-filled-primary-500' : 'preset-tonal-surface'}" aria-label={`Track changes: ${tracking ? "On" : "Off"}`} aria-pressed={tracking} disabled={!canTrack} title={canTrack ? "Track subsequent edits in this document" : "Editing is unavailable"} onclick={() => ontracking?.(!tracking)}>Track changes: {tracking ? "On" : "Off"}</button></div>
    <div class="changes-meta" aria-live="polite">{allPending} pending{allRows.length !== allPending ? ` · ${allRows.length} total` : ""}{pendingRows.length !== allPending ? ` · ${pendingRows.length} shown` : ""}</div>
    <label class="markup-toggle"><input type="checkbox" checked={markupVisible} disabled={!onmarkup} onchange={(event) => onmarkup?.(event.currentTarget.checked)} /> Show markup <span class="panel-muted">(off: Clean proposed text)</span></label>
    <div class="filters" aria-label="Change filters">
      <select aria-label="Change status" value={activeFilters.status} onchange={(event) => updateFilter("status", event)}><option value="pending">Pending</option><option value="accepted">Accepted</option><option value="rejected">Rejected</option><option value="all">All statuses</option></select>
      <select aria-label="Filter by author" value={activeFilters.author} onchange={(event) => updateFilter("author", event)}><option value="">All authors</option>{#each derivedAuthors as author}<option value={author}>{author}</option>{/each}</select>
      <select aria-label="Filter by file" value={activeFilters.file} onchange={(event) => updateFilter("file", event)}><option value="">All files</option>{#each derivedFiles as file}<option value={file}>{file}</option>{/each}</select>
      <select aria-label="Filter by revision session" value={activeFilters.session} onchange={(event) => updateFilter("session", event)}><option value="">All sessions</option>{#each derivedSessions as session}<option value={session}>{session}</option>{/each}</select>
    </div>
    <div class="review-controls" role="group" aria-label="Review controls">
      <button type="button" class="btn btn-sm preset-tonal-surface" onclick={() => { onprevious?.(active); move(-1); }} disabled={!filteredRows.length}>Previous</button><button type="button" class="btn btn-sm preset-tonal-surface" onclick={() => { onnext?.(active); move(1); }} disabled={!filteredRows.length}>Next</button>
      {#if rowFor(active)}{@const activeRow = rowFor(active)}<button type="button" class="btn btn-sm preset-tonal-surface" onclick={() => void decide(activeRow, "accept")} disabled={!pending(activeRow) || !reviewAllowed(activeRow) || deciding.has(active)}>Accept and next</button><button type="button" class="btn btn-sm preset-tonal-surface" onclick={() => void decide(activeRow, "reject")} disabled={!pending(activeRow) || !reviewAllowed(activeRow) || deciding.has(active)}>Reject and next</button>{/if}
      {#if selectedRows.length}<button type="button" class="btn btn-sm preset-tonal-surface" onclick={() => void bulk("accept")} disabled={!canReview}>Accept {selectedRows.length} selected</button><button type="button" class="btn btn-sm preset-tonal-surface" onclick={() => void bulk("reject")} disabled={!canReview}>Reject {selectedRows.length} selected</button>{/if}
      {#if undoBatch}<button type="button" class="btn btn-sm preset-tonal-surface" onclick={() => void undo()}>Undo {undoBatch.action === "accept" ? "acceptance" : "rejection"}{undoBatch.ids.length > 1 ? ` (${undoBatch.ids.length})` : ""}</button>{/if}
    </div>
    <p class="shortcut-hint">Shortcuts: ↑/↓ or J/K to move · A to accept · R to reject</p>
    {#if feedback}<p class="panel-status" role="status" aria-live="polite">{feedback}</p>{/if}
  </header>
  {#if !filteredRows.length}<p class="panel-muted changes-empty" role="status">{activeFilters.status === "pending" ? "No pending changes." : "No changes match these filters."}</p>{/if}
  <div class="changes-list" role="list" aria-label="Revision queue">
    {#each filteredRows as item (rowId(item))}
      {@const id = rowId(item)}{@const isOpen = active === id}{@const why = blocker(item)}
      <article class="change-row" class:active={isOpen} class:legacy={item.__kind === "legacy"} class:blocked={Boolean(why)} role="listitem">
        <div class="row-head"><input type="checkbox" aria-label={`Select change in ${pathOf(item)}`} checked={checked.has(id)} onchange={() => toggleChecked(item)} disabled={!pending(item)} /><button id={`change-${id}`} type="button" class="row-main" aria-expanded={isOpen} aria-controls={`change-detail-${id}`} onclick={() => reveal(item)}><span class="row-kind">{item.__kind === "legacy" ? "Legacy proposal" : value(item, "kind", "type") || "Change"}</span><span class="row-diff">{shortDiff(item)}</span><span class="row-context">{pathOf(item)} · {authorOf(item)}{sessionOf(item) ? ` · ${sessionOf(item)}` : ""}{statusOf(item) !== "pending" ? ` · ${statusOf(item)}` : ""}</span></button>{#if pending(item)}<button type="button" class="btn btn-sm row-action" aria-label={`Accept change in ${pathOf(item)}`} title={why || "Accept this change"} disabled={!reviewAllowed(item) || Boolean(why) || deciding.has(id)} onclick={() => void decide(item, "accept")}>{deciding.has(id) ? "Accepting…" : "Accept"}</button><button type="button" class="btn btn-sm row-action" aria-label={`Reject change in ${pathOf(item)}`} title={why || "Reject this change"} disabled={!reviewAllowed(item) || Boolean(why) || deciding.has(id)} onclick={() => void decide(item, "reject")}>{deciding.has(id) ? "Rejecting…" : "Reject"}</button>{/if}</div>
        {#if isOpen}<div id={`change-detail-${id}`} class="change-detail" role="region" aria-label={`Details for change in ${pathOf(item)}`}>
          {#if item.__kind === "legacy"}<CommentCard comment={item} {identity} {commentingAs} {canModerate} {went} replacement={replacements[item.id] ?? null} cardIdPrefix="changes-legacy" selected={String(item.id) === String(selected)} {onreveal} {onresolve} {ondelete} {onreply} {onaccept} {onreject} />{:else}<p>{detail(item)}</p>{#if value(item, "contextBefore", "beforeContext")}<p class="panel-muted">{value(item, "contextBefore", "beforeContext")} <mark>{value(item, "after", "newText", "inserted", "proposed") || value(item, "before", "oldText", "deleted", "exact")}</mark> {value(item, "contextAfter", "afterContext")}</p>{/if}{/if}
          {#if why}<div class="conflict" role="alert"><strong>Needs attention</strong><p>{why}</p>{#if (item.affectedRevisionIds || item.dependencies)?.length}<p class="panel-muted">Affected revisions: {(item.affectedRevisionIds || item.dependencies).join(", ")}</p>{/if}<button type="button" class="btn btn-sm preset-tonal-surface" onclick={() => resolve(item)}>Resolve dependency</button></div>{/if}
        </div>{#if item.replies?.length}<details class="discussion"><summary>Discussion ({item.replies.length})</summary><ul>{#each item.replies as reply (reply.id)}<li><strong>{reply.creator || "Author"}:</strong> {reply.body}</li>{/each}</ul></details>{/if}{/if}
      </article>
    {/each}
  </div>
</div>

<style>
  .changes-panel { display: flex; flex-direction: column; min-height: 0; outline: none; height: 100%; }
  .changes-header { position: sticky; top: 0; z-index: 2; flex: 0 0 auto; padding: var(--spacing); background: var(--color-surface-50-950); border-bottom: 1px solid var(--color-surface-200-800); }
  .changes-title-row, .review-controls, .filters, .row-head { display: flex; align-items: center; gap: var(--spacing); } .changes-title-row { justify-content: space-between; } h2 { margin: 0; font-size: 1rem; }
  .changes-meta, .row-context, .row-kind { color: var(--color-surface-500-400); font-size: .75rem; } .markup-toggle, .shortcut-hint { display: block; margin: .4rem 0; font-size: .8rem; } .shortcut-hint { color: var(--color-surface-500-400); }
  .filters { flex-wrap: wrap; } .filters select { min-width: 0; max-width: 100%; flex: 1 1 7rem; } .review-controls { flex-wrap: wrap; margin-top: .5rem; }
  .changes-list { flex: 1 1 auto; min-height: 0; overflow: auto; padding: var(--spacing); overscroll-behavior: contain; } .change-row { content-visibility: auto; contain-intrinsic-size: 0 5rem; border: 1px solid var(--color-surface-200-800); border-radius: .35rem; margin-bottom: .5rem; padding: .5rem; } .change-row.active { border-color: var(--color-primary-500); box-shadow: 0 0 0 1px var(--color-primary-500); } .change-row.blocked { border-left: 3px solid var(--color-warning-500); }
  .row-head { align-items: flex-start; } .row-main { min-width: 0; flex: 1; text-align: left; background: none; border: 0; padding: 0; cursor: pointer; } .row-kind, .row-diff, .row-context { display: block; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; } .row-diff { margin: .15rem 0; font-size: .85rem; } .row-action { flex: 0 0 auto; }
  .change-detail { margin: .6rem 0 0 1.5rem; max-height: 16rem; overflow: auto; white-space: pre-wrap; overflow-wrap: anywhere; } .discussion { margin: .5rem 0 0 1.5rem; font-size: .8rem; } .discussion li { margin-top: .3rem; } .conflict { margin-top: .7rem; padding: .5rem; border-left: 3px solid var(--color-warning-500); } .changes-empty { padding: 1rem; } mark { padding: 0 .15rem; }
  @media (max-width: 32rem) { .changes-header, .changes-list { padding-inline: calc(var(--spacing) * .75); } .row-head { gap: .35rem; } .row-action { padding-inline: .35rem; } .row-context { max-width: 10rem; } .review-controls .btn { flex: 1 1 auto; } .change-detail, .discussion { margin-left: .25rem; } }
</style>
